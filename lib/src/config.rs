use std::{
    fs,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    path::PathBuf,
};

use n0_error::{Result, StackResultExt, StdResultExt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMode {
    #[default]
    Default,
    Dns,
    Hybrid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GatewayMode {
    #[default]
    Reverse,
    Forward,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Config {
    /// The IPv4 address that the endpoint will listen on.
    ///
    /// If None, defaults to a random free port, but it can be useful to specify a fixed
    /// port, e.g. to configure a firewall rule.
    pub ipv4_addr: Option<SocketAddrV4>,

    /// The IPv6 address that the endpoint will listen on.
    ///
    /// If None, defaults to a random free port, but it can be useful to specify a fixed
    /// port, e.g. to configure a firewall rule.
    pub ipv6_addr: Option<SocketAddrV6>,

    /// How the gateway resolves endpoint connection details.
    #[serde(default)]
    pub discovery_mode: DiscoveryMode,

    /// DNS origin domain used for _iroh.<z32-endpoint-id>.<origin> lookups.
    ///
    /// Required when discovery_mode is `dns` or `hybrid`.
    #[serde(default)]
    pub dns_origin: Option<String>,

    /// Optional DNS resolver address for discovery lookups.
    ///
    /// Useful for local development (e.g. 127.0.0.1:53535).
    #[serde(default)]
    pub dns_resolver: Option<SocketAddr>,

}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GatewayConfig {
    #[serde(flatten)]
    pub common: Config,
    /// Gateway operating mode for HTTP proxying.
    #[serde(default)]
    pub gateway_mode: GatewayMode,
}

impl Config {
    pub async fn from_file(path: PathBuf) -> Result<Self> {
        let config = tokio::fs::read_to_string(path)
            .await
            .context("reading config file")?;
        let config = serde_yml::from_str(&config).std_context("parsing config file")?;
        Ok(config)
    }

    pub async fn write(&self, path: PathBuf) -> Result<()> {
        let data = serde_yml::to_string(self).anyerr()?;
        fs::write(path, data)?;
        Ok(())
    }
}

impl GatewayConfig {
    pub async fn from_file(path: PathBuf) -> Result<Self> {
        let config = tokio::fs::read_to_string(path)
            .await
            .context("reading config file")?;
        let config = serde_yml::from_str(&config).std_context("parsing config file")?;
        Ok(config)
    }

    pub async fn write(&self, path: PathBuf) -> Result<()> {
        let data = serde_yml::to_string(self).anyerr()?;
        fs::write(path, data)?;
        Ok(())
    }
}
