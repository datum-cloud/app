mod auth;
mod config;
pub mod datum_cloud;
mod encoding;
pub mod gateway;
mod node;
mod repo;
mod state;

pub use node::{Metrics, Node};
pub use repo::Repo;
pub use state::{ConnectionInfo, ListnerInfo, TcpProxy, TcpProxyTicket};

/// The root domain for datum connect urls to subdomain from. A proxy URL will
/// be a three-word-codename subdomain off this URL. eg: "https://vast-gold-mine.iroh.datum.net"
pub const DATUM_CONNECT_GATEWAY_DOMAIN_NAME: &'static str = "iroh.datum.net";
