use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use iroh::EndpointId;
use iroh_proxy_utils::{HttpRequest, IROH_DESTINATION_HEADER, RequestKind, ResolveDestination};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, warn, warn_span};

use crate::{AdvertismentTicket, node::ConnectNode};

const DATUM_PROXY_ID_HEADER: &str = "Datum-Proxy-Id";

pub async fn serve(
    node: ConnectNode,
    bind_addr: SocketAddr,
    cancel_token: CancellationToken,
) -> Result<()> {
    let listener = TcpListener::bind(bind_addr).await?;
    info!(?bind_addr, endpoint_id = %node.endpoint_id().fmt_short(),"TCP proxy gateway started");

    let parse_destination = Resolver {
        n0des: node.n0des.clone(),
    };

    let html_body = include_str!("../static/gateway_not_found.html");

    let mut conn_id = 0;
    loop {
        let (stream, peer_addr) = tokio::select! {
            res = listener.accept() => res?,
            _ = cancel_token.cancelled() => break,
        };
        conn_id += 1;

        tokio::spawn({
            let pool = node.pool.clone();
            let parse_destination = parse_destination.clone();
            let cancel_token = cancel_token.child_token();
            cancel_token.run_until_cancelled_owned(
                async move {
                    debug!("New connection from {}", peer_addr);
                    match pool
                        .forward_http_connection(stream, &parse_destination)
                        .await
                    {
                        Ok(()) => {
                            debug!("connection closed");
                        }
                        Err(mut err) => {
                            warn!("proxy request failed: {} {:#}", err.status, err.source);
                            if let Err(err) = err.finalize_with_body(html_body.as_bytes()).await {
                                warn!("failed to send error response to client: {err:#}");
                            }
                        }
                    }
                }
                .instrument(warn_span!("conn", %conn_id)),
            )
        });
    }
    Ok(())
}

#[derive(Clone)]
struct Resolver {
    n0des: Arc<iroh_n0des::Client>,
}

impl ResolveDestination for Resolver {
    fn resolve_destination<'a>(
        &'a self,
        req: &'a HttpRequest,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<EndpointId>> + Send + 'a>> {
        // Note: This is currently a bit of a grab-all bag for development. Once we deploy this,
        // we'll only support exactly the forms that envoy sends over.
        Box::pin(async {
            // If "Iroh-Destination" header is set, use that directly.
            if let Some(endpoint_id) = req.headers.get(IROH_DESTINATION_HEADER) {
                return Some(endpoint_id.to_str().ok()?.parse().ok()?);
            }

            // If "Datum-Proxy-Id" header is set, use that to fetch a ticket.
            let codename = if let Some(value) = req.headers.get(DATUM_PROXY_ID_HEADER) {
                value.to_str().ok()

            // If this is a regular HTTP request (not HTTP CONNECT), we try to parse the codename
            // from either the authority (which is set if this is a proxy request) or from the host
            // (for regular HTTP requests)
            } else if let RequestKind::Http {
                authority_from_path,
                ..
            } = &req.kind
            {
                let host = authority_from_path
                    .as_ref()
                    .map(|x| x.host.as_str())
                    .or_else(|| req.headers.get("host").and_then(|h| h.to_str().ok()));
                host.and_then(extract_subdomain)

            // Otherwise, no destination, abort.
            } else {
                None
            };

            let codename = codename?;
            let ticket = self
                .n0des
                .fetch_ticket::<AdvertismentTicket>(codename.to_string())
                .await
                .inspect_err(|err| warn!("Failed to fetch ticket: {err:#}"))
                .ok()??;
            Some(ticket.ticket.endpoint)
        })
    }
}

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
