use crate::node::Node;
use anyhow::Result;
use axum::{
    Router,
    extract::{Host, State},
    response::IntoResponse,
    routing::get,
};
use reqwest::Request;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub node: Node,
}

/// Extract the leftmost subdomain from a host string
/// * "foo.bar.example.com" -> "foo"
/// * "example.com" -> ""
/// * "192.168.1.1" -> "" (IP addresses have no subdomain)
/// * "sub.localhost" -> "sub" (special case for localhost)
fn extract_subdomain(host: &str) -> String {
    // Remove port if present
    let host = host.split(':').next().unwrap_or(host);

    // skip raw IPs
    if host.parse::<std::net::IpAddr>().is_ok() {
        return String::new();
    }

    // Split by dots and take the first part
    let parts: Vec<&str> = host.split('.').collect();

    // Special case: localhost with subdomain (e.g., "sub.localhost")
    if parts.len() == 2 && parts[1] == "localhost" {
        return parts[0].to_string();
    }

    // If there are more than 2 parts, the first one is the subdomain
    // This assumes domain.tld format (e.g., example.com)
    if parts.len() > 2 {
        parts[0].to_string()
    } else {
        String::new()
    }
}

async fn health_handler() -> impl IntoResponse {
    "OK"
}

async fn root_handler(Host(host): Host, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let subdomain = extract_subdomain(&host);

    // Access the node from shared state
    let node = &state.node;
    let addr = "127.0.0.1:8888".to_string();
    let info = node
        .connect_codename(subdomain, addr.clone())
        .await
        .unwrap();

    let url = format!("http://{}", addr);

    let res = reqwest::get(url).await.unwrap();
    let body = res.bytes().await.unwrap();
    body.to_vec()
}

/// Create and configure the HTTP server
pub async fn serve(node: Node, port: u16) -> Result<()> {
    info!("HTTP server starting");
    let state = Arc::new(AppState { node });

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("HTTP server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
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
