use std::{io, net::SocketAddr, str::FromStr, sync::Arc};

use askama::Template;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{
    StatusCode,
    body::Bytes,
    http::{self, HeaderMap, HeaderValue},
};
use iroh::{Endpoint, EndpointId, SecretKey};
use iroh_proxy_utils::{
    Authority, HttpRequest, HttpRequestKind,
    downstream::{
        Deny, DownstreamProxy, ErrorResponder, HttpProxyOpts, ProxyMode, RequestHandler, SrcAddr,
    },
};
use n0_error::Result;
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tracing::info;

mod metrics;

use self::metrics::{GatewayMetrics, MetricsHttpState, serve_metrics_http, shared_gateway_metrics};
use crate::build_endpoint;

pub async fn bind_and_serve(
    secret_key: SecretKey,
    config: crate::config::GatewayConfig,
    tcp_bind_addr: SocketAddr,
    metrics_bind_addr: Option<SocketAddr>,
) -> Result<()> {
    let listener = TcpListener::bind(tcp_bind_addr).await?;
    let endpoint = build_endpoint(secret_key, &config.common).await?;
    serve_with_metrics(endpoint, listener, metrics_bind_addr).await
}

pub async fn serve(endpoint: Endpoint, listener: TcpListener) -> Result<()> {
    serve_with_metrics(endpoint, listener, None).await
}

pub async fn serve_with_metrics(
    endpoint: Endpoint,
    listener: TcpListener,
    metrics_bind_addr: Option<SocketAddr>,
) -> Result<()> {
    let tcp_bind_addr = listener.local_addr()?;
    info!(
        ?tcp_bind_addr,
        endpoint_id = %endpoint.id().fmt_short(),
        "TCP proxy gateway started"
    );

    // Use one shared metrics instance so both TCP and UDS listeners contribute
    // to the same /metrics output in this process.
    let metrics = shared_gateway_metrics();
    if let Some(metrics_bind_addr) = metrics_bind_addr {
        let state = MetricsHttpState::new(endpoint.clone(), metrics.clone());
        tokio::spawn(async move {
            if let Err(err) = serve_metrics_http(metrics_bind_addr, state).await {
                tracing::warn!(%err, "gateway metrics server failed");
            }
        });
    }

    let resolver_endpoint = endpoint.clone();
    let error_endpoint = endpoint.clone();
    let proxy = DownstreamProxy::new(endpoint, Default::default());
    let mode = ProxyMode::Http(
        HttpProxyOpts::new(HeaderResolver::new(resolver_endpoint, metrics.clone()))
            .error_responder(ErrorResponseWriter::new(error_endpoint, metrics)),
    );
    proxy.forward_tcp_listener(listener, mode).await
}

/// Serves the gateway on a Unix Domain Socket.
#[cfg(unix)]
pub async fn serve_uds(endpoint: Endpoint, listener: UnixListener) -> Result<()> {
    let uds_path = listener
        .local_addr()
        .ok()
        .and_then(|a| a.as_pathname().map(|p| p.to_path_buf()));
    info!(
        ?uds_path,
        endpoint_id = %endpoint.id().fmt_short(),
        "UDS proxy gateway started"
    );

    let metrics = shared_gateway_metrics();
    let resolver_endpoint = endpoint.clone();
    let error_endpoint = endpoint.clone();
    let proxy = DownstreamProxy::new(endpoint, Default::default());
    let mode = ProxyMode::Http(
        HttpProxyOpts::new(HeaderResolver::new(resolver_endpoint, metrics.clone()))
            .error_responder(ErrorResponseWriter::new(error_endpoint, metrics)),
    );
    proxy.forward_uds_listener(listener, mode).await
}

/// Binds the gateway to a Unix Domain Socket at `path` and serves.
#[cfg(unix)]
pub async fn bind_and_serve_uds(
    secret_key: SecretKey,
    config: crate::config::GatewayConfig,
    path: impl AsRef<std::path::Path>,
) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    let endpoint = build_endpoint(secret_key, &config.common).await?;
    serve_uds(endpoint, listener).await
}

const HEADER_NODE_ID: &str = "x-iroh-endpoint-id";
const HEADER_TARGET_HOST: &str = "x-datum-target-host";
const HEADER_TARGET_PORT: &str = "x-datum-target-port";

const DATUM_HEADERS: [&str; 3] = [HEADER_NODE_ID, HEADER_TARGET_HOST, HEADER_TARGET_PORT];

struct HeaderResolver {
    endpoint: Endpoint,
    metrics: Arc<GatewayMetrics>,
}

impl RequestHandler for HeaderResolver {
    async fn handle_request(
        &self,
        src_addr: SrcAddr,
        req: &mut HttpRequest,
    ) -> Result<EndpointId, Deny> {
        let is_tcp = matches!(src_addr, SrcAddr::Tcp(_));
        match src_addr {
            SrcAddr::Tcp(_) => self.metrics.inc_tcp_requests(),
            #[cfg(unix)]
            SrcAddr::Unix(_) => self.metrics.inc_uds_requests(),
        }
        match req.classify()? {
            HttpRequestKind::Tunnel => {
                self.metrics.inc_tunnel_requests();
                self.metrics
                    .inc_tunnel_reuse_attempt(has_existing_peer_conn(&self.endpoint));
                if is_tcp {
                    self.metrics.inc_tunnel_tcp_requests();
                } else {
                    #[cfg(unix)]
                    self.metrics.inc_tunnel_uds_requests();
                }
                let endpoint_id = self.endpoint_id_from_headers(&req.headers)?;
                req.remove_headers(DATUM_HEADERS);
                Ok(endpoint_id)
            }
            HttpRequestKind::Origin | HttpRequestKind::Http1Absolute => {
                self.metrics.inc_origin_requests();
                self.metrics
                    .inc_origin_reuse_attempt(has_existing_peer_conn(&self.endpoint));
                if is_tcp {
                    self.metrics.inc_origin_tcp_requests();
                } else {
                    #[cfg(unix)]
                    self.metrics.inc_origin_uds_requests();
                }
                let endpoint_id = self.endpoint_id_from_headers(&req.headers)?;
                let host = self.header_value(&req.headers, HEADER_TARGET_HOST)?;
                let port = self
                    .header_value(&req.headers, HEADER_TARGET_PORT)?
                    .parse::<u16>()
                    .map_err(|_| {
                        self.metrics.inc_denied_invalid_target_port();
                        Deny::bad_request("invalid x-datum-target-port header")
                    })?;
                // Rewrite the request target.
                req.set_absolute_http_authority(Authority::new(host.to_string(), port))?
                    .remove_headers(DATUM_HEADERS);
                Ok(endpoint_id)
            }
        }
    }
}

impl HeaderResolver {
    fn new(endpoint: Endpoint, metrics: Arc<GatewayMetrics>) -> Self {
        Self { endpoint, metrics }
    }

    fn endpoint_id_from_headers(
        &self,
        headers: &HeaderMap<HeaderValue>,
    ) -> Result<EndpointId, Deny> {
        let s = self.header_value(headers, HEADER_NODE_ID)?;
        EndpointId::from_str(s).map_err(|_| {
            self.metrics.inc_denied_invalid_endpoint();
            Deny::bad_request("invalid x-iroh-endpoint-id value")
        })
    }

    fn header_value<'a>(
        &self,
        headers: &'a HeaderMap<HeaderValue>,
        name: &str,
    ) -> Result<&'a str, Deny> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                self.metrics.inc_denied_missing_header_name(name);
                Deny::bad_request(format!("Missing header {name}"))
            })
    }
}

#[derive(Template)]
#[template(path = "gateway_error.html")]
struct GatewayErrorTemplate<'a> {
    title: &'a str,
    body: &'a str,
}

struct ErrorResponseWriter {
    endpoint: Endpoint,
    metrics: Arc<GatewayMetrics>,
}

impl ErrorResponder for ErrorResponseWriter {
    async fn error_response(
        &self,
        status: StatusCode,
    ) -> hyper::Response<BoxBody<Bytes, io::Error>> {
        self.metrics.inc_status_code(status);
        if status.is_server_error() {
            self.metrics
                .inc_5xx_failure_by_peer_conn_state(has_existing_peer_conn(&self.endpoint));
        }
        let title = format!(
            "{} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or_default()
        );
        let body = match status {
            StatusCode::BAD_REQUEST => {
                "The request could not be understood by the gateway. Please try again."
            }
            StatusCode::UNAUTHORIZED => {
                "You are not logged in or your session has expired. Please sign in and try again."
            }
            StatusCode::FORBIDDEN => "Access to this resource is not allowed through the gateway.",
            StatusCode::NOT_FOUND => "The requested page could not be found through the gateway.",
            StatusCode::INTERNAL_SERVER_ERROR => {
                "The gateway encountered an internal error. Please try again later."
            }
            StatusCode::BAD_GATEWAY => {
                "The gateway could not get a valid response from the upstream service."
            }
            StatusCode::SERVICE_UNAVAILABLE => {
                "The service is temporarily unavailable. Please try again shortly."
            }
            StatusCode::GATEWAY_TIMEOUT => "The upstream service took too long to respond.",
            _ => "The service experienced an unexpected error.",
        };
        let html = GatewayErrorTemplate {
            body,
            title: &title,
        }
        .render()
        .unwrap_or(title);
        hyper::Response::builder()
            .status(status)
            .header(http::header::CONTENT_LENGTH, html.len().to_string())
            .body(
                Full::new(Bytes::from(html))
                    .map_err(|err| match err {})
                    .boxed(),
            )
            .expect("infallible")
    }
}

impl ErrorResponseWriter {
    fn new(endpoint: Endpoint, metrics: Arc<GatewayMetrics>) -> Self {
        Self { endpoint, metrics }
    }
}

fn has_existing_peer_conn(endpoint: &Endpoint) -> bool {
    let endpoint_metrics = endpoint.metrics();
    let direct_current = endpoint_metrics
        .magicsock
        .num_direct_conns_added
        .get()
        .saturating_sub(endpoint_metrics.magicsock.num_direct_conns_removed.get());
    let relay_current = endpoint_metrics
        .magicsock
        .num_relay_conns_added
        .get()
        .saturating_sub(endpoint_metrics.magicsock.num_relay_conns_removed.get());
    direct_current + relay_current > 0
}
