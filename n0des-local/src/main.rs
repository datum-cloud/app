use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use iroh::protocol::Router;
use iroh::SecretKey;
use iroh_n0des::ApiSecret;
use iroh_n0des::protocol::{
    ALPN, Auth, ListTickets, N0desMessage, Ping, Pong, PublishTicket, PutMetrics, TicketData,
    UnpublishTicket,
};
use irpc::WithChannels;
use tokio::sync::Mutex;
use tracing::info;

type TicketKey = (String, String); // (ticket_kind, name)

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut rng = rand::rng();
    let endpoint_secret = SecretKey::generate(&mut rng);
    let endpoint = iroh::Endpoint::builder()
        .secret_key(endpoint_secret)
        .bind()
        .await
        .context("binding iroh endpoint")?;

    let tickets: Arc<Mutex<HashMap<TicketKey, Vec<u8>>>> = Default::default();
    let (tx, rx) = tokio::sync::mpsc::channel::<N0desMessage>(64);
    tokio::task::spawn(server_actor(rx, tickets));

    // Serve the n0des protocol over iroh via irpc.
    let handler = irpc_iroh::IrohProtocol::<iroh_n0des::protocol::N0desProtocol>::with_sender(tx);
    let router = Router::builder(endpoint.clone()).accept(ALPN, handler).spawn();

    // Create an ApiSecret ticket string that clients can put into N0DES_API_SECRET.
    let api_secret_key = SecretKey::generate(&mut rng);
    let api_secret = ApiSecret::new(api_secret_key, endpoint.addr());

    info!("local n0des started");
    info!("endpoint id: {}", router.endpoint().id().fmt_short());
    info!("export N0DES_API_SECRET='{}'", api_secret);
    info!("press ctrl+c to stop");

    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn server_actor(
    mut rx: tokio::sync::mpsc::Receiver<N0desMessage>,
    tickets: Arc<Mutex<HashMap<TicketKey, Vec<u8>>>>,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            N0desMessage::Auth(msg) => {
                let WithChannels { inner: Auth { .. }, tx, .. } = msg;
                // For local learning we accept any capabilities.
                let _ = tx.send(()).await;
            }
            N0desMessage::PutMetrics(msg) => {
                let WithChannels {
                    inner: PutMetrics { .. },
                    tx,
                    ..
                } = msg;
                let _ = tx.send(Ok(())).await;
            }
            N0desMessage::Ping(msg) => {
                let WithChannels {
                    inner: Ping { req_id },
                    tx,
                    ..
                } = msg;
                let _ = tx.send(Pong { req_id }).await;
            }

            // Ticket APIs
            N0desMessage::TicketPublish(msg) => {
                let WithChannels {
                    inner:
                        PublishTicket {
                            name,
                            ticket_kind,
                            ticket,
                            ..
                        },
                    tx,
                    ..
                } = msg;
                info!("ticket publish: kind={ticket_kind} name={name}");
                let mut guard = tickets.lock().await;
                guard.insert((ticket_kind, name), ticket);
                let _ = tx.send(Ok(())).await;
            }
            N0desMessage::TicketUnpublish(msg) => {
                let WithChannels {
                    inner:
                        UnpublishTicket {
                            name, ticket_kind, ..
                        },
                    tx,
                    ..
                } = msg;
                info!("ticket unpublish: kind={ticket_kind} name={name}");
                let mut guard = tickets.lock().await;
                let existed = guard.remove(&(ticket_kind, name)).is_some();
                let _ = tx.send(Ok(existed)).await;
            }
            N0desMessage::TicketGet(msg) => {
                let WithChannels {
                    inner:
                        iroh_n0des::protocol::GetTicket {
                            name, ticket_kind, ..
                        },
                    tx,
                    ..
                } = msg;
                info!("ticket get: kind={ticket_kind} name={name}");
                let guard = tickets.lock().await;
                let res = guard
                    .get(&(ticket_kind.clone(), name.clone()))
                    .map(|ticket_bytes| TicketData {
                        name,
                        ticket_kind,
                        ticket_bytes: ticket_bytes.clone(),
                    });
                let _ = tx.send(Ok(res)).await;
            }
            N0desMessage::TicketList(msg) => {
                let WithChannels {
                    inner:
                        ListTickets {
                            ticket_kind,
                            offset,
                            limit,
                            ..
                        },
                    tx,
                    ..
                } = msg;
                info!("ticket list: kind={ticket_kind} offset={offset} limit={limit}");
                let guard = tickets.lock().await;
                let mut all: Vec<TicketData> = guard
                    .iter()
                    .filter_map(|((kind, name), bytes)| {
                        if kind == &ticket_kind {
                            Some(TicketData {
                                name: name.clone(),
                                ticket_kind: kind.clone(),
                                ticket_bytes: bytes.clone(),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                // Stable-ish ordering for predictable pagination
                all.sort_by(|a, b| a.name.cmp(&b.name));

                let offset = offset as usize;
                let limit = limit as usize;
                let paged = all.into_iter().skip(offset).take(limit).collect();
                let _ = tx.send(Ok(paged)).await;
            }

            // N0desMessage is currently fully covered by the matches above.
        }
    }
}

