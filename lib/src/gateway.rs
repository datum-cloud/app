use std::net::SocketAddr;
use std::sync::Arc;

use hyper::StatusCode;
use iroh_proxy_utils::{
    Authority, Destination, ExtractDestination, ForwardMode, HttpRequest, ResolveDestination,
};
use n0_error::{Result, StackResultExt, StdResultExt};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, warn, warn_span};

use crate::{AdvertismentTicket, node::ConnectNode};

// const DATUM_PROXY_ID_HEADER: &str = "Datum-Proxy-Id";

pub async fn serve(
    node: ConnectNode,
    bind_addr: SocketAddr,
    cancel_token: CancellationToken,
) -> Result<()> {
    let listener = TcpListener::bind(bind_addr).await?;
    info!(?bind_addr, endpoint_id = %node.endpoint_id().fmt_short(),"TCP proxy gateway started");

    let extract_destination = ExtractDestination::Custom(Arc::new(Resolver {
        n0des: node.n0des.clone(),
    }));

    let mut conn_id = 0;
    loop {
        let (mut stream, peer_addr) = tokio::select! {
            res = listener.accept() => res?,
            _ = cancel_token.cancelled() => break,
        };
        conn_id += 1;

        tokio::spawn({
            let pool = node.pool.clone();
            let extract_destination = extract_destination.clone();
            let cancel_token = cancel_token.child_token();
            cancel_token.run_until_cancelled_owned(
                async move {
                    debug!("New connection from {}", peer_addr);
                    if let Err(err) = pool
                        .forward_http_connection(&mut stream, &extract_destination)
                        .await
                    {
                        warn!("proxy request failed: {:#}", err);
                        if let Some(status) = err.should_reply() {
                            send_error_response(&mut stream, status).await.ok();
                        }
                    } else {
                        debug!("connection closed");
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
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<Destination>> + Send + 'a>> {
        info!("resolve: {req:?}");
        // Note: This is currently a bit of a grab-all bag for development. Once we deploy this,
        // we'll only support exactly the forms that envoy sends over.
        Box::pin(async {
            // // If "Iroh-Destination" header is set, use that directly.
            // if let Some(endpoint_id) = req.headers.get(IROH_DESTINATION_HEADER) {
            //     return Some(endpoint_id.to_str().ok()?.parse().ok()?);
            // }

            // // If "Datum-Proxy-Id" header is set, use that to fetch a ticket.
            // let codename = if let Some(value) = req.headers.get(DATUM_PROXY_ID_HEADER) {
            //     value.to_str().ok()

            // If this is a regular HTTP request (not HTTP CONNECT), we try to parse the codename
            // from either the authority (which is set if this is a proxy request) or from the host
            // (for regular HTTP requests)
            // } else if let RequestKind::Http {
            // } else if let RequestKind::Http {
            //     authority_from_path,
            //     ..
            // } = &req.kind
            // {
            //     let host = authority_from_path
            //         .as_ref()
            //         .map(|x| x.host.as_str())
            //         .or_else(|| req.headers.get("host").and_then(|h| h.to_str().ok()));
            //     host.and_then(extract_subdomain)
            // // Otherwise, no destination, abort.
            // } else {
            //     None
            // };

            // We only support the subdomain extraction for now.
            let codename = req
                .headers
                .get("host")
                .and_then(|h| h.to_str().ok())
                .and_then(extract_subdomain)?;

            debug!(%codename, "extracted codename, fetching ticket...");
            let ticket = self
                .n0des
                .fetch_ticket::<AdvertismentTicket>(codename.to_string())
                .await
                .std_context("Failed to fetch ticket")
                .inspect_err(|err| warn!("{err:#}"))
                .ok()?
                .context("Ticket not found on n0des")
                .inspect_err(|err| warn!("{err:#}"))
                .ok()?
                .ticket;
            debug!(?ticket, "fetched ticket");
            Some(Destination {
                endpoint_id: ticket.endpoint,
                mode: ForwardMode::ConnectTunnel(Authority {
                    host: ticket.data.data.host.clone(),
                    port: ticket.data.data.port,
                }),
            })
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

pub(super) async fn send_error_response(
    mut writer: impl AsyncWrite + Unpin,
    status: StatusCode,
) -> Result<()> {
    let html_body = include_str!("../static/gateway_not_found.html");
    let header_section = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        html_body.len()
    );
    writer.write_all(header_section.as_bytes()).await?;
    writer.write_all(html_body.as_bytes()).await?;
    Ok(())
}
