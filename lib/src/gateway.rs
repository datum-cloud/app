use std::{io, net::SocketAddr, sync::Arc};

use askama::Template;
use hyper::StatusCode;
use iroh::{Endpoint, SecretKey};
use iroh_proxy_utils::{
    HttpOriginRequest, HttpResponse,
    downstream::{
        DownstreamProxy, EndpointAuthority, ExtractError, HttpProxyOpts, ProxyMode,
        ReverseProxyResolver, WriteErrorResponse,
    },
};
use n0_error::Result;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use tracing::{debug, info};

use crate::{
    FetchTicketError, TicketClient, build_endpoint, build_n0des_client, n0des_api_secret_from_env,
};

pub async fn bind_and_serve(
    secret_key: SecretKey,
    config: crate::config::Config,
    tcp_bind_addr: SocketAddr,
) -> Result<()> {
    let listener = TcpListener::bind(tcp_bind_addr).await?;
    let endpoint = build_endpoint(secret_key, &config).await?;
    let n0des_api_secret = n0des_api_secret_from_env()?;
    let n0des = build_n0des_client(&endpoint, n0des_api_secret).await?;
    serve(endpoint, n0des, listener).await
}

pub async fn serve(
    endpoint: Endpoint,
    n0des: Arc<iroh_n0des::Client>,
    listener: TcpListener,
) -> Result<()> {
    let tcp_bind_addr = listener.local_addr()?;
    info!(?tcp_bind_addr, endpoint_id = %endpoint.id().fmt_short(),"TCP proxy gateway started");

    let proxy = DownstreamProxy::new(endpoint, Default::default());
    let tickets = TicketClient::new(n0des);
    let resolver = Resolver { tickets };
    let opts = HttpProxyOpts::default()
        // Right now the gatewy functions as a reverse proxy, i.e. incoming requests are regular origin-form HTTP
        // requests, and we resolve the destination from the host header's subdomain.
        // Once envoy takes over this part, we will use [`HttpProxyOpts::forward`] instead, i.e. accept CONNECT
        // requests only.
        .reverse(resolver)
        .error_response_writer(ErrorResponseWriter);
    let mode = ProxyMode::Http(opts);

    proxy.forward_tcp_listener(listener, mode).await
}

#[derive(Clone)]
struct Resolver {
    tickets: TicketClient,
}

/// When operating in reverse-proxy mode we accept origin-form http requests,
/// and need to resolve their full destination.
///
/// This is the currently-deployed version, which uses the subdomain.
impl ReverseProxyResolver for Resolver {
    async fn destination<'a>(
        &'a self,
        req: &'a HttpOriginRequest,
    ) -> Result<EndpointAuthority, ExtractError> {
        let host = req.headers.get("host").ok_or(ExtractError::BadRequest)?;
        let host = host.to_str().map_err(|_| ExtractError::BadRequest)?;
        let codename = extract_subdomain(host).ok_or(ExtractError::NotFound)?;

        debug!(%codename, "extracted codename, resolving ticket...");
        let ticket = self.tickets.get(codename).await.map_err(|err| {
            debug!(%codename, "failed to resolve ticket: {err:#}");
            match err {
                FetchTicketError::NotFound => ExtractError::NotFound,
                FetchTicketError::FailedToFetch(_) => ExtractError::ServiceUnavailable,
            }
        })?;
        debug!(?ticket, "resolved ticket");
        Ok(EndpointAuthority {
            endpoint_id: ticket.endpoint,
            authority: ticket.data.data.into(),
        })
    }
}

// /// When operating in forward-proxy mode, i.e. when accepting CONNECT requests or requests
// /// with absolute-form targets, we only need to resolve an endpoint id from the request,
// /// because the upstream authority is already part of the original request.
// impl ForwardProxyResolver for Resolver {
//     async fn destination<'a>(
//         &'a self,
//         req: &'a HttpProxyRequest,
//     ) -> Result<EndpointId, ExtractError> {
//         todo!()
//     }
// }

pub(super) fn extract_subdomain(host: &str) -> Option<&str> {
    let host = host
        .rsplit_once(':')
        .map(|(host, _port)| host)
        .unwrap_or(host);
    if host.parse::<std::net::IpAddr>().is_ok() {
        None
    } else {
        host.split_once(".").map(|(first, _rest)| first)
    }
}

#[derive(Template)]
#[template(path = "gateway_error.html")]
struct GatewayErrorTemplate<'a> {
    title: &'a str,
    body: &'a str,
}

struct ErrorResponseWriter;

impl WriteErrorResponse for ErrorResponseWriter {
    async fn write_error_response<'a>(
        &'a self,
        res: &'a HttpResponse,
        writer: &'a mut (dyn AsyncWrite + Send + Unpin),
    ) -> io::Result<()> {
        let title = format!("{} {}", res.status.as_u16(), res.reason());
        let body = match res.status {
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
        writer.write_all(res.status_line().as_bytes()).await?;
        let headers = format!(
            "Content-Type: text/html\r\nContent-Length: {}\r\n\r\n",
            html.len()
        );
        writer.write_all(headers.as_bytes()).await?;
        writer.write_all(html.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::extract_subdomain;

    #[test]
    fn extract_subdomain_from_host_with_port() {
        assert_eq!(extract_subdomain("alpha.example.test:8080"), Some("alpha"));
    }

    #[test]
    fn extract_subdomain_from_host_without_port() {
        assert_eq!(extract_subdomain("beta.example.test"), Some("beta"));
    }

    #[test]
    fn extract_subdomain_rejects_ip() {
        assert_eq!(extract_subdomain("127.0.0.1:8080"), None);
    }

    #[test]
    fn extract_subdomain_returns_none_without_dot() {
        assert_eq!(extract_subdomain("localhost:8080"), None);
    }

    #[test]
    fn extract_subdomain_rejects_ipv6_literal() {
        assert_eq!(extract_subdomain("[::1]:8080"), None);
    }
}
