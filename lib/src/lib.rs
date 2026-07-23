mod auth;
pub mod config;
pub mod datum_apis;
pub mod datum_cloud;
pub mod diagnostics;
pub mod heartbeat;
mod http_user_agent;
mod node;
pub mod project_control_plane;
mod repo;
mod state;
pub mod tunnel_activity;
pub mod tunnels;
pub mod update;

pub use config::{Config, DiscoveryMode};
pub use diagnostics::DiagnosticsSettings;
pub use heartbeat::HeartbeatAgent;
pub use http_user_agent::datum_http_user_agent;
pub use node::*;
pub use project_control_plane::{
    ProjectControlPlaneClient, error_looks_like_quota, is_kube_auth_failure,
    is_kube_quota_exceeded, message_looks_like_quota,
};
pub use repo::Repo;
pub use state::*;
pub use tunnel_activity::{TunnelActivityEntry, TunnelActivityTracker};
pub use tunnels::{
    CONNECTOR_ADVERTISEMENT_QUOTA_RESOURCE_TYPE, HTTPPROXY_QUOTA_RESOURCE_TYPE, TunnelCreateQuota,
    TunnelDeleteOutcome, TunnelService, TunnelSummary, tunnel_create_quota_from_buckets,
};
pub use update::{UpdateChannel, UpdateChecker, UpdateInfo, UpdateSettings};

/// The root domain for datum connect urls to subdomain from. A proxy URL will
/// be a three-word-codename subdomain off this URL. eg: "https://vast-gold-mine.iroh.datum.net"
pub const DATUM_CONNECT_GATEWAY_DOMAIN_NAME: &str = "iroh.datum.net";
