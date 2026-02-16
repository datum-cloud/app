use std::{path::PathBuf, str::FromStr, sync::Arc};

use arc_swap::{ArcSwap, Guard};
use iroh::EndpointId;
use iroh_proxy_utils::Authority;
use iroh_tickets::{ParseError, Ticket};
use n0_error::{Result, StackResultExt, StdResultExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, futures::Notified};

use crate::{DATUM_CONNECT_GATEWAY_DOMAIN_NAME, Repo};

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct State {
    pub proxies: Vec<ProxyState>,
}

impl State {
    pub fn set_proxy(&mut self, proxy: ProxyState) {
        if let Some(existing) = self
            .proxies
            .iter_mut()
            .find(|p| p.info.resource_id == proxy.info.resource_id)
        {
            *existing = proxy;
        } else {
            self.proxies.push(proxy);
        }
    }

    pub fn remove_proxy(&mut self, resouce_id: &str) -> Option<ProxyState> {
        if let Some(idx) = self
            .proxies
            .iter()
            .position(|p| p.info.resource_id == resouce_id)
        {
            Some(self.proxies.remove(idx))
        } else {
            None
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct SelectedContext {
    pub org_id: String,
    pub org_name: String,
    pub project_id: String,
    pub project_name: String,
}

impl SelectedContext {
    pub fn label(&self) -> String {
        format!("{} / {}", self.org_name, self.project_name)
    }
}

#[derive(Debug, Clone)]
pub struct StateWrapper {
    inner: Arc<ArcSwap<State>>,
    notify: Arc<Notify>,
}

impl StateWrapper {
    pub fn new(state: State) -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(state))),
            notify: Default::default(),
        }
    }

    pub fn get(&self) -> Guard<Arc<State>> {
        self.inner.load()
    }

    pub fn get_cloned(&self) -> Arc<State> {
        self.inner.load_full()
    }

    pub fn updated(&self) -> Notified<'_> {
        self.notify.notified()
    }

    pub async fn update<R>(
        &self,
        repo: &Repo,
        f: impl FnOnce(&mut State) -> R,
    ) -> n0_error::Result<R> {
        let mut inner = (*self.inner.load_full()).clone();
        let res = f(&mut inner);
        let inner = Arc::new(inner);
        self.inner.store(inner.clone());
        repo.write_state(&inner).await?;
        self.notify.notify_waiters();
        Ok(res)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct ProxyState {
    pub info: Advertisment,
    pub enabled: bool,
}

impl ProxyState {
    pub fn new(info: Advertisment) -> Self {
        Self {
            info,
            enabled: true,
        }
    }

    pub fn id(&self) -> &str {
        &self.info.resource_id
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct Advertisment {
    pub resource_id: String,
    pub label: Option<String>,
    pub data: TcpProxyData,
}

impl Advertisment {
    pub fn new(data: TcpProxyData, label: Option<String>) -> Self {
        let resource_id = format!("proxy-{}", rand_str(12));
        Self {
            resource_id,
            data,
            label,
        }
    }

    pub fn with_id(resource_id: String, data: TcpProxyData, label: Option<String>) -> Self {
        Self {
            resource_id,
            data,
            label,
        }
    }

    pub fn id(&self) -> &str {
        &self.resource_id
    }

    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or_else(|| self.id())
    }

    pub fn codename(&self) -> String {
        self.resource_id.clone()
    }

    pub fn service(&self) -> &TcpProxyData {
        &self.data
    }

    pub fn domain(&self) -> String {
        format!("{}.{}", self.id(), DATUM_CONNECT_GATEWAY_DOMAIN_NAME)
    }

    // TODO: Change to HTTPS
    pub fn datum_url(&self) -> String {
        format!("http://{}.{}", self.id(), DATUM_CONNECT_GATEWAY_DOMAIN_NAME)
    }

    // TODO: Not everything is HTTP
    pub fn local_url(&self) -> String {
        format!("http://{}", self.service().address())
    }

    pub fn datum_resource_url(&self) -> String {
        format!("datum://{}", self.id())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct TcpProxyData {
    pub host: String,
    pub port: u16,
}

impl From<TcpProxyData> for Authority {
    fn from(value: TcpProxyData) -> Self {
        Self {
            host: value.host,
            port: value.port,
        }
    }
}

impl TcpProxyData {
    pub fn from_host_port_str(s: &str) -> Result<Self> {
        let (host, port) = Self::parse_host_port(s)?;
        Ok(Self { host, port })
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    fn parse_host_port(s: &str) -> Result<(String, u16)> {
        let (host, port) = s.rsplit_once(":").context("missing port")?;
        let port: u16 = port.parse().std_context("invalid port")?;
        Ok((host.to_string(), port))
    }
}

impl State {
    pub(crate) async fn from_file(path: PathBuf) -> Result<Self> {
        let data = tokio::fs::read(path).await?;
        let state: State = serde_yml::from_slice(&data).anyerr()?;
        Ok(state)
    }

    pub(crate) async fn write_to_file(&self, path: PathBuf) -> Result<()> {
        let data = serde_yml::to_string(&self).anyerr()?;
        tokio::fs::write(&path, &data).await?;
        Ok(())
    }
}

impl Advertisment {
    pub fn ticket(&self, endpoint: EndpointId) -> AdvertismentTicket {
        AdvertismentTicket {
            data: self.clone(),
            endpoint,
        }
    }
}

fn rand_str(len: usize) -> String {
    rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .take(len)
        .map(char::from)
        .collect()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdvertismentTicket {
    pub data: Advertisment,
    pub endpoint: EndpointId,
}

impl AdvertismentTicket {
    pub fn service(&self) -> &TcpProxyData {
        &self.data.data
    }
}

impl FromStr for AdvertismentTicket {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        iroh_tickets::Ticket::deserialize(s)
    }
}

impl Ticket for AdvertismentTicket {
    const KIND: &'static str = "datum";

    fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(&self).expect("serialize should work")
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, iroh_tickets::ParseError> {
        let ticket: Self = postcard::from_bytes(bytes)?;
        Ok(ticket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tcp_proxy_data_from_host_port() {
        let data = TcpProxyData::from_host_port_str("example.test:443").unwrap();
        assert_eq!(data.host, "example.test");
        assert_eq!(data.port, 443);
    }

    #[test]
    fn parse_tcp_proxy_data_rejects_missing_port() {
        let err = TcpProxyData::from_host_port_str("example.test").unwrap_err();
        assert!(err.to_string().contains("missing port"));
    }

    #[test]
    fn parse_tcp_proxy_data_rejects_invalid_port() {
        let err = TcpProxyData::from_host_port_str("example.test:abc").unwrap_err();
        assert!(err.to_string().contains("invalid port"));
    }

    // #[test]
    // fn test_tcp_proxy_has_codename() {
    //     let proxy = TcpProxy::new("127.0.0.1".to_string(), 8080);
    //     let codename = &proxy.codename;

    //     let parts: Vec<&str> = codename.split('-').collect();
    //     assert_eq!(parts.len(), 3);
    // }
}
