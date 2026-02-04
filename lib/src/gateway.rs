use std::{io, net::SocketAddr, str::FromStr};

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
    downstream::{Deny, DownstreamProxy, ErrorResponder, HttpProxyOpts, ProxyMode, RequestHandler},
};
use n0_error::Result;
use tokio::net::TcpListener;
use tracing::info;

use crate::build_endpoint;

pub async fn bind_and_serve(
    secret_key: SecretKey,
    config: crate::config::GatewayConfig,
    tcp_bind_addr: SocketAddr,
) -> Result<()> {
    let listener = TcpListener::bind(tcp_bind_addr).await?;
    let endpoint = build_endpoint(secret_key, &config.common).await?;
    serve(endpoint, listener).await
}

pub async fn serve(endpoint: Endpoint, listener: TcpListener) -> Result<()> {
    let tcp_bind_addr = listener.local_addr()?;
    info!(
        ?tcp_bind_addr,
        endpoint_id = %endpoint.id().fmt_short(),
        "TCP proxy gateway started"
    );

    let proxy = DownstreamProxy::new(endpoint, Default::default());
    let mode =
        ProxyMode::Http(HttpProxyOpts::new(HeaderResolver).error_responder(ErrorResponseWriter));
    proxy.forward_tcp_listener(listener, mode).await
}

const HEADER_NODE_ID: &str = "x-iroh-endpoint-id";
const HEADER_TARGET_HOST: &str = "x-datum-target-host";
const HEADER_TARGET_PORT: &str = "x-datum-target-port";

const DATUM_HEADERS: [&str; 3] = [HEADER_NODE_ID, HEADER_TARGET_HOST, HEADER_TARGET_PORT];

struct HeaderResolver;

impl RequestHandler for HeaderResolver {
    async fn handle_request(
        &self,
        src_addr: SocketAddr,
        req: &mut HttpRequest,
    ) -> Result<EndpointId, Deny> {
        match req.classify()? {
            HttpRequestKind::Tunnel => {
                let endpoint_id = endpoint_id_from_headers(&req.headers)?;
                // TODO: This exposes the client's IP addr to the upstream proxy. Not sure if that is desired or not.
                // If not, just remove the next line.
                req.set_forwarded_for(src_addr)
                    .remove_headers(DATUM_HEADERS);
                Ok(endpoint_id)
            }
            HttpRequestKind::Origin | HttpRequestKind::Http1Absolute => {
                let endpoint_id = endpoint_id_from_headers(&req.headers)?;
                let host = header_value(&req.headers, HEADER_TARGET_HOST)?;
                let port = header_value(&req.headers, HEADER_TARGET_PORT)?
                    .parse::<u16>()
                    .map_err(|_| Deny::bad_request("invalid x-datum-target-port header"))?;
                // Rewrite the request target.
                req.set_absolute_http_authority(Authority::new(host.to_string(), port))?
                    // TODO: This exposes the client's IP addr to the upstream proxy. Not sure if that is desired or not.
                    // If not, just remove the next line.
                    .set_forwarded_for(src_addr)
                    .remove_headers(DATUM_HEADERS);
                Ok(endpoint_id)
            }
        }
    }
}

fn endpoint_id_from_headers(headers: &HeaderMap<HeaderValue>) -> Result<EndpointId, Deny> {
    let s = header_value(headers, HEADER_NODE_ID)?;
    EndpointId::from_str(s).map_err(|_| Deny::bad_request("invalid x-iroh-endpoint-id value"))
}

fn header_value<'a>(headers: &'a HeaderMap<HeaderValue>, name: &str) -> Result<&'a str, Deny> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Deny::bad_request(format!("Missing header {name}")))
}

#[derive(Template)]
#[template(path = "gateway_error.html")]
struct GatewayErrorTemplate<'a> {
    title: &'a str,
    body: &'a str,
}

struct ErrorResponseWriter;

impl ErrorResponder for ErrorResponseWriter {
    async fn error_response<'a>(
        &'a self,
        status: StatusCode,
    ) -> hyper::Response<BoxBody<Bytes, io::Error>> {
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
            body: &body,
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
