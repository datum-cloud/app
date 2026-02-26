use std::net::SocketAddr;

use n0_error::Result;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional},
    net::{TcpListener, TcpStream},
};
use tracing::{info, warn};

use crate::TunnelDevArgs;

const MAX_CONNECT_RESPONSE: usize = 16 * 1024;

pub async fn serve(args: TunnelDevArgs) -> Result<()> {
    if args.target_protocol != "tcp" {
        n0_error::bail_any!("target-protocol must be tcp for now");
    }

    let listener = TcpListener::bind(args.listen).await?;
    info!(
        listen = %args.listen,
        gateway = %args.gateway,
        target = %format!("{}:{}", args.target_host, args.target_port),
        "tunnel-dev listening"
    );

    loop {
        let (mut inbound, peer) = listener.accept().await?;
        let gateway = args.gateway;
        let node_id = args.node_id.clone();
        let target_host = args.target_host.clone();
        let target_port = args.target_port;
        let target_protocol = args.target_protocol.clone();

        tokio::spawn(async move {
            if let Err(err) = handle_connection(
                &mut inbound,
                gateway,
                &node_id,
                &target_host,
                target_port,
                &target_protocol,
            )
            .await
            {
                warn!(%peer, "tunnel-dev connection failed: {err:#}");
            }
        });
    }
}

async fn handle_connection(
    inbound: &mut TcpStream,
    gateway: SocketAddr,
    node_id: &str,
    target_host: &str,
    target_port: u16,
    _target_protocol: &str,
) -> Result<()> {
    let mut outbound = TcpStream::connect(gateway).await?;
    let authority = format!("{target_host}:{target_port}");
    let connect_req = format!(
        "CONNECT {authority} HTTP/1.1\r\n\
Host: {authority}\r\n\
x-iroh-endpoint-id: {node_id}\r\n\
\r\n"
    );
    outbound.write_all(connect_req.as_bytes()).await?;
    read_connect_response(&mut outbound).await?;
    copy_bidirectional(inbound, &mut outbound).await?;
    Ok(())
}

async fn read_connect_response(stream: &mut TcpStream) -> Result<()> {
    let mut buf = Vec::new();
    let mut scratch = [0u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut scratch).await?;
        if read == 0 {
            n0_error::bail_any!("gateway closed before CONNECT response");
        }
        buf.extend_from_slice(&scratch[..read]);
        if buf.len() > MAX_CONNECT_RESPONSE {
            n0_error::bail_any!("CONNECT response headers too large");
        }
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
    };

    let header = std::str::from_utf8(&buf[..header_end])
        .map_err(|_| n0_error::anyerr!("CONNECT response was not valid UTF-8"))?;
    let status_line = header.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") && !status_line.starts_with("HTTP/1.1 200") {
        n0_error::bail_any!("CONNECT failed: {status_line}");
    }
    Ok(())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| pos + 4)
}
