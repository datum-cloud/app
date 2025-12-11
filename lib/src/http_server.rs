use std::net::SocketAddr;

use anyhow::{Result, bail};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info, warn};

use crate::node::Node;

/// Parse HTTP request to extract Host header and determine subdomain
async fn parse_http_host(stream: &mut TcpStream) -> Result<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 8192];

    loop {
        let n = stream.read(&mut temp).await?;
        if n == 0 {
            bail!("Connection closed before complete headers");
        }

        buffer.extend_from_slice(&temp[..n]);

        // Look for end of headers
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers_end = pos + 4;
            let headers = std::str::from_utf8(&buffer[..headers_end])?;

            // Parse Host header
            for line in headers.lines() {
                if line.to_lowercase().starts_with("host:") {
                    let host = line[5..].trim().to_string();
                    return Ok((host, buffer));
                }
            }

            bail!("No Host header found");
        }

        // Prevent unbounded buffering
        if buffer.len() > 16384 {
            bail!("Headers too large");
        }
    }
}

fn extract_subdomain(host: &str) -> String {
    let host = host.split(':').next().unwrap_or(host);

    if host.parse::<std::net::IpAddr>().is_ok() {
        return String::new();
    }

    let parts: Vec<&str> = host.split('.').collect();

    if parts.len() == 2 && parts[1] == "localhost" {
        return parts[0].to_string();
    }

    if parts.len() > 2 {
        parts[0].to_string()
    } else {
        String::new()
    }
}

async fn handle_connection(mut client: TcpStream, node: Node) -> Result<()> {
    // Parse the initial request to get the Host header
    let (host, initial_data) = parse_http_host(&mut client).await?;
    let codename = extract_subdomain(&host);

    // go to my existing pool of tunnels, if it's there, open a new set of steams & proxy over
    // if not, create a new tunnel, add it to the pool, and return the streams
    let (info, (mut tunnel_write, mut tunnel_read)) = node.connect(codename).await?;
    info!(info = ?info, "connection opened");
    // Send the buffered request data through the tunnel
    tunnel_write.write_all(&initial_data).await?;

    let (mut client_read, mut client_write) = client.split();

    let client_to_tunnel = async { tokio::io::copy(&mut client_read, &mut tunnel_write).await };
    let tunnel_to_client = async { tokio::io::copy(&mut tunnel_read, &mut client_write).await };

    info!("forwarding");

    // Run both directions concurrently
    tokio::select! {
        result = client_to_tunnel => {
            if let Err(e) = result {
                debug!("Client to tunnel copy ended: {}", e);
            }
        }
        result = tunnel_to_client => {
            if let Err(e) = result {
                debug!("Tunnel to client copy ended: {}", e);
            }
        }
    }

    Ok(())
}

pub async fn serve(node: Node, port: u16) -> Result<()> {
    info!(
        endpoint_id = node.endpoint_id(),
        "HTTP proxy server starting"
    );

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    info!("HTTP proxy listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, peer_addr)) => {
                debug!("New connection from {}", peer_addr);
                let node = node.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, node).await {
                        warn!("Connection handling error: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_subdomain() {
        assert_eq!(extract_subdomain("foo.example.com"), "foo");
        assert_eq!(extract_subdomain("bar.baz.example.com"), "bar");
        assert_eq!(extract_subdomain("example.com"), "");
        assert_eq!(extract_subdomain("localhost"), "");
        assert_eq!(extract_subdomain("sub.localhost:8080"), "sub");

        // IP addresses should return empty string
        assert_eq!(extract_subdomain("192.168.1.1"), "");
        assert_eq!(extract_subdomain("127.0.0.1:8080"), "");
        assert_eq!(extract_subdomain("::1"), "");
        assert_eq!(extract_subdomain("[::1]:8080"), "");
    }
}
