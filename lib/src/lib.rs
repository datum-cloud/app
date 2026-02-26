mod auth;
pub mod config;
pub mod datum_apis;
pub mod datum_cloud;
pub mod gateway;
pub mod heartbeat;
mod node;
pub mod project_control_plane;
mod repo;
mod state;
pub mod tunnels;
pub mod update;

pub use config::{Config, DiscoveryMode, GatewayConfig};
pub use heartbeat::HeartbeatAgent;
pub use node::*;
pub use project_control_plane::ProjectControlPlaneClient;
pub use repo::Repo;
pub use state::*;
pub use tunnels::{TunnelDeleteOutcome, TunnelService, TunnelSummary};
pub use update::{UpdateChecker, UpdateInfo, UpdateSettings};

/// The root domain for datum connect urls to subdomain from. A proxy URL will
/// be a three-word-codename subdomain off this URL. eg: "https://vast-gold-mine.iroh.datum.net"
pub const DATUM_CONNECT_GATEWAY_DOMAIN_NAME: &str = "iroh.datum.net";

#[cfg(test)]
mod tests;
