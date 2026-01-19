use std::collections::BTreeMap;

use iroh::protocol::Router;
use iroh::{Endpoint, SecretKey};
use iroh_n0des::ApiSecret;
use iroh_n0des::protocol::{
    ALPN, GetTicket, ListTickets, N0desMessage, Ping, Pong, PublishTicket, TicketData,
    UnpublishTicket,
};
use irpc::WithChannels;
use n0_error::Result;
use tracing::info;

pub async fn bind_and_start() -> Result<(ApiSecret, Router)> {
    let endpoint = Endpoint::bind().await?;
    start(endpoint)
}

pub fn start(endpoint: Endpoint) -> Result<(ApiSecret, Router)> {
    let (tx, rx) = tokio::sync::mpsc::channel::<N0desMessage>(64);
    tokio::task::spawn(server_actor(rx));

    // Serve the n0des protocol over iroh via irpc.
    let handler = irpc_iroh::IrohProtocol::<iroh_n0des::protocol::N0desProtocol>::with_sender(tx);
    let router = Router::builder(endpoint.clone())
        .accept(ALPN, handler)
        .spawn();

    // Create an ApiSecret ticket string that clients can put into N0DES_API_SECRET.
    let api_secret_key = SecretKey::generate(&mut rand::rng());
    let api_secret = ApiSecret::new(api_secret_key, endpoint.addr());

    Ok((api_secret, router))
}

async fn server_actor(mut rx: tokio::sync::mpsc::Receiver<N0desMessage>) {
    let mut tickets = BTreeMap::new();
    while let Some(msg) = rx.recv().await {
        match msg {
            N0desMessage::Auth(WithChannels { tx, .. }) => {
                tx.send(()).await.ok();
            }
            N0desMessage::PutMetrics(WithChannels { tx, .. }) => {
                tx.send(Ok(())).await.ok();
            }
            N0desMessage::Ping(WithChannels { inner, tx, .. }) => {
                let Ping { req_id } = inner;
                tx.send(Pong { req_id }).await.ok();
            }
            N0desMessage::TicketPublish(WithChannels { inner, tx, .. }) => {
                let PublishTicket {
                    name,
                    ticket_kind,
                    ticket,
                    ..
                } = inner;
                tickets.insert((ticket_kind, name), ticket);
                tx.send(Ok(())).await.ok();
            }
            N0desMessage::TicketUnpublish(WithChannels { inner, tx, .. }) => {
                let UnpublishTicket {
                    name, ticket_kind, ..
                } = inner;
                info!("ticket unpublish: kind={ticket_kind} name={name}");
                let existed = tickets.remove(&(ticket_kind, name)).is_some();
                tx.send(Ok(existed)).await.ok();
            }
            N0desMessage::TicketGet(WithChannels { inner, tx, .. }) => {
                let GetTicket {
                    name, ticket_kind, ..
                } = inner;
                info!("ticket get: kind={ticket_kind} name={name}");
                let res = tickets
                    .get(&(ticket_kind.clone(), name.clone()))
                    .map(|ticket_bytes| TicketData {
                        name,
                        ticket_kind,
                        ticket_bytes: ticket_bytes.clone(),
                    });
                tx.send(Ok(res)).await.ok();
            }
            N0desMessage::TicketList(WithChannels { inner, tx, .. }) => {
                let ListTickets {
                    ticket_kind,
                    offset,
                    limit,
                    ..
                } = inner;
                info!("ticket list: kind={ticket_kind} offset={offset} limit={limit}");
                let res = tickets
                    .iter()
                    .filter(|((kind, _name), _data)| kind == &ticket_kind)
                    .map(|((kind, name), bytes)| TicketData {
                        name: name.clone(),
                        ticket_kind: kind.clone(),
                        ticket_bytes: bytes.clone(),
                    })
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect();
                tx.send(Ok(res)).await.ok();
            }
        }
    }
}
