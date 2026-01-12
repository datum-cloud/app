use std::net::SocketAddr;

use anyhow::Result;
use iroh::EndpointId;
use n0_error::StackResultExt;
use quinn::SendStream;
use tokio::io::{self, AsyncRead, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{Instrument, debug, error, info, trace, warn, warn_span};

use crate::TcpProxyData;
use crate::node::OutboundDialer;

use self::parse::{PartialHttpRequest, extract_subdomain};

mod parse;

const DATUM_PROXY_ID_HEADER: &str = "X-Datum-Proxy-Id";

async fn resolve_target(
    req: &PartialHttpRequest,
    node: &OutboundDialer,
) -> n0_error::Result<(EndpointId, TcpProxyData)> {
    let codename = if let Some(value) = req.headers.get(DATUM_PROXY_ID_HEADER) {
        value.as_str()
    } else {
        extract_subdomain(&req.host).context("No codename found")?
    };
    let ticket = node
        .fetch_ticket(codename)
        .await
        .context("Failed to resolve codename")?;
    Ok((ticket.endpoint, ticket.service().clone()))
}

async fn handle_tcp_connection(mut client: TcpStream, node: OutboundDialer) -> Result<()> {
    // Parse the initial request to get the Host header and/or X-Connector header
    let header_names = [DATUM_PROXY_ID_HEADER];
    let req = PartialHttpRequest::read(&mut client, header_names).await?;
    trace!(?req, "parsed request");

    let (endpoint_id, target_authority) = match resolve_target(&req, &node).await {
        Ok(res) => res,
        Err(err) => {
            warn!("Failed to resolve destination from HTTP request: {err:#}");
            send_404_response(&mut client).await?;
            return Ok(());
        }
    };

    let mut streams = node.connect(endpoint_id, &target_authority).await?;
    info!(remote=%streams.endpoint_id.fmt_short(), "iroh connection opened");

    let (mut client_read, mut client_write) = client.split();

    // Run both directions concurrently
    tokio::join!(
        async {
            let res = send_all(&mut streams.send, &req.initial_data, &mut client_read).await;
            debug!("Client to tunnel copy ended: {res:?}");
        },
        async {
            let res = tokio::io::copy(&mut streams.recv, &mut client_write).await;
            debug!("Tunnel to client copy ended: {res:?}");
        },
    );

    debug!("connection finished");

    Ok(())
}

async fn send_all(
    send: &mut SendStream,
    initial_data: &[u8],
    reader: &mut (impl AsyncRead + Unpin),
) -> io::Result<u64> {
    send.write_all(&initial_data).await?;
    let res = tokio::io::copy(reader, send).await;
    send.finish()?;
    res.map(|n| n + initial_data.len() as u64)
}

/// Send an HTTP 404 Not Found response to the client, assumes TCP stream has
/// made an HTTP request.
async fn send_404_response(stream: &mut TcpStream) -> Result<()> {
    let html_body = include_str!("../static/gateway_not_found.html");

    let response = format!(
        "HTTP/1.1 404 Not Found\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        html_body.len(),
        html_body
    );

    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;

    Ok(())
}

pub async fn serve(node: OutboundDialer, bind_addr: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(bind_addr).await?;
    info!(?bind_addr, endpoint_id = %node.endpoint_id().fmt_short(),"TCP proxy gateway started");

    let mut conn_id = 0;
    loop {
        match listener.accept().await {
            Ok((stream, peer_addr)) => {
                conn_id += 1;
                let span = warn_span!("conn", id = conn_id);
                let node = node.clone();
                tokio::spawn(
                    async move {
                        debug!("New connection from {}", peer_addr);
                        if let Err(e) = handle_tcp_connection(stream, node).await {
                            warn!("Connection handling error: {}", e);
                        }
                    }
                    .instrument(span),
                );
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}
