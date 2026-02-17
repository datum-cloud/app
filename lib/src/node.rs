use std::{fmt::Debug, net::SocketAddr, str::FromStr, sync::Arc};

use iroh::{
    Endpoint, EndpointId, SecretKey, discovery::dns::DnsDiscovery, endpoint::default_relay_mode,
    protocol::Router,
};
use iroh_n0des::ApiSecret;
use iroh_proxy_utils::upstream::Metrics;
use iroh_proxy_utils::{
    ALPN as IROH_HTTP_CONNECT_ALPN, Authority, HttpProxyRequest, HttpProxyRequestKind,
};
use iroh_proxy_utils::{
    downstream::{DownstreamProxy, EndpointAuthority, ProxyMode},
    upstream::{AuthError, AuthHandler, UpstreamProxy},
};
use iroh_relay::dns::{DnsProtocol, DnsResolver};
use n0_error::{Result, StackResultExt, StdResultExt};
use tokio::{net::TcpListener, sync::futures::Notified, task::JoinHandle};
use tracing::{Instrument, debug, error_span, info, instrument, warn};

use crate::{ProxyState, Repo, StateWrapper, TcpProxyData, config::Config};

#[derive(Debug, Clone)]
pub struct Node {
    pub listen: ListenNode,
    pub connect: ConnectNode,
}

impl Node {
    pub async fn new(repo: Repo) -> Result<Self> {
        let listen = ListenNode::new(repo.clone()).await?;
        let connect = ConnectNode::new(repo).await?;
        Ok(Self { listen, connect })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MetricsUpdate {
    pub send: u64,
    pub recv: u64,
}

#[derive(Debug, Clone)]
pub struct ListenNode {
    router: Router,
    state: StateWrapper,
    repo: Repo,
    metrics: Arc<Metrics>,
    _n0des: Option<Arc<iroh_n0des::Client>>,
}

impl ListenNode {
    pub async fn new(repo: Repo) -> Result<Self> {
        let n0des_api_secret = n0des_api_secret_from_env()?;
        Self::with_n0des_api_secret(repo, n0des_api_secret).await
    }

    #[instrument("listen-node", skip_all)]
    pub async fn with_n0des_api_secret(
        repo: Repo,
        n0des_api_secret: Option<ApiSecret>,
    ) -> Result<Self> {
        let config = repo.config().await?;
        let secret_key = repo.listen_key().await?;
        let endpoint = build_endpoint(secret_key, &config).await?;
        let n0des = build_n0des_client_opt(&endpoint, n0des_api_secret).await;
        let state = repo.load_state().await?;

        let upstream_proxy = UpstreamProxy::new(state.clone())?;
        let metrics = upstream_proxy.metrics();

        let router = Router::builder(endpoint)
            .accept(IROH_HTTP_CONNECT_ALPN, upstream_proxy)
            .spawn();

        let this = Self {
            repo,
            router,
            state,
            metrics,
            _n0des: n0des,
        };
        Ok(this)
    }

    pub fn state_updated(&self) -> Notified<'_> {
        self.state.updated()
    }

    pub fn state(&self) -> &StateWrapper {
        &self.state
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    pub fn proxies(&self) -> Vec<ProxyState> {
        self.state.get().proxies.to_vec()
    }

    pub fn proxy_by_id(&self, id: &str) -> Option<ProxyState> {
        self.state
            .get()
            .proxies
            .iter()
            .find(|p| p.id() == id)
            .cloned()
    }

    pub async fn set_proxy(&self, proxy: ProxyState) -> Result<()> {
        self.state
            .update(&self.repo, |state| state.set_proxy(proxy.clone()))
            .await?;
        Ok(())
    }

    pub async fn set_proxy_state(&self, proxy: ProxyState) -> Result<()> {
        self.state
            .update(&self.repo, |state| state.set_proxy(proxy))
            .await?;
        Ok(())
    }

    pub async fn remove_proxy(&self, resource_id: &str) -> Result<Option<ProxyState>> {
        debug!(%resource_id, "removing proxy {resource_id}");
        let res = self
            .state
            .update(&self.repo, move |state| state.remove_proxy(resource_id))
            .await;
        debug!(%resource_id, "removed {res:?}");
        res
    }

    pub async fn remove_proxy_state(&self, resource_id: &str) -> Result<Option<ProxyState>> {
        debug!(%resource_id, "removing proxy state {resource_id}");
        let res = self
            .state
            .update(&self.repo, move |state| state.remove_proxy(resource_id))
            .await;
        debug!(%resource_id, "removed {res:?}");
        res
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id()
    }
}

impl StateWrapper {
    fn tcp_proxy_exists(&self, host: &str, port: u16) -> bool {
        // Strip scheme from incoming host (e.g., "http://127.0.0.1" -> "127.0.0.1")
        // The gateway may send the host with scheme, but local state stores without
        let normalized_host = strip_host_scheme(host);
        let exists = self.get().proxies.iter().any(|a| {
            a.enabled && a.info.service().host == normalized_host && a.info.service().port == port
        });
        if !exists {
            debug!(
                requested_host = host,
                normalized_host, port, "tcp_proxy_exists: no matching proxy found"
            );
        }
        exists
    }
}

/// Strip scheme prefix from host (e.g., "http://127.0.0.1" -> "127.0.0.1")
fn strip_host_scheme(host: &str) -> &str {
    host.strip_prefix("http://")
        .or_else(|| host.strip_prefix("https://"))
        .unwrap_or(host)
}

impl AuthHandler for StateWrapper {
    async fn authorize<'a>(
        &'a self,
        _remote_id: EndpointId,
        req: &'a HttpProxyRequest,
    ) -> Result<(), AuthError> {
        match &req.kind {
            HttpProxyRequestKind::Tunnel { target } => {
                if self.tcp_proxy_exists(&target.host, target.port) {
                    Ok(())
                } else {
                    Err(AuthError::Forbidden)
                }
            }
            HttpProxyRequestKind::Absolute { target, .. } => {
                // Parse host:port from absolute URL (e.g., "http://localhost:5173/path")
                if let Ok(authority) = Authority::from_absolute_uri(&target) {
                    if self.tcp_proxy_exists(&authority.host, authority.port) {
                        Ok(())
                    } else {
                        Err(AuthError::Forbidden)
                    }
                } else {
                    debug!(%target, "failed to parse host:port from absolute URL");
                    Err(AuthError::Forbidden)
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectNode {
    endpoint: Endpoint,
    proxy: DownstreamProxy,
    _n0des: Option<Arc<iroh_n0des::Client>>,
}

impl ConnectNode {
    pub async fn new(repo: Repo) -> Result<Self> {
        let n0des_api_secret = n0des_api_secret_from_env()?;
        Self::with_n0des_api_secret(repo, n0des_api_secret).await
    }

    #[instrument("connect-node", skip_all)]
    pub async fn with_n0des_api_secret(
        repo: Repo,
        n0des_api_secret: Option<ApiSecret>,
    ) -> Result<Self> {
        let config = repo.config().await?;
        let secret_key = repo.connect_key().await?;
        let endpoint = build_endpoint(secret_key, &config).await?;
        let n0des = build_n0des_client_opt(&endpoint, n0des_api_secret).await;
        let pool = DownstreamProxy::new(endpoint.clone(), Default::default());
        Ok(Self {
            endpoint,
            _n0des: n0des,
            proxy: pool,
        })
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub async fn connect_and_bind_local(
        &self,
        remote_id: EndpointId,
        advertisment: &TcpProxyData,
        bind_addr: SocketAddr,
    ) -> Result<OutboundProxyHandle> {
        let local_socket = TcpListener::bind(bind_addr).await?;
        let bound_addr = local_socket.local_addr()?;

        let upstream = EndpointAuthority::new(remote_id, advertisment.clone().into());
        let mode = ProxyMode::Tcp(upstream);

        let proxy = self.proxy.clone();
        let task = tokio::spawn(async move {
            info!("bound local socket on {bound_addr}");
            if let Err(err) = proxy.forward_tcp_listener(local_socket, mode).await {
                warn!("Forwarding local socket failed: {err:#}");
            }
        }.instrument(error_span!("forward-tcp", remote_id=%remote_id.fmt_short(), authority=%advertisment.address())));
        Ok(OutboundProxyHandle {
            remote_id,
            task,
            bound_addr: bind_addr,
            advertisment: advertisment.clone(),
        })
    }
}

pub struct OutboundProxyHandle {
    task: JoinHandle<()>,
    bound_addr: SocketAddr,
    remote_id: EndpointId,
    advertisment: TcpProxyData,
}

impl OutboundProxyHandle {
    pub fn abort(&self) {
        self.task.abort();
    }

    pub fn remote_id(&self) -> EndpointId {
        self.remote_id
    }

    pub fn bound_addr(&self) -> SocketAddr {
        self.bound_addr
    }

    pub fn advertisment(&self) -> &TcpProxyData {
        &self.advertisment
    }
}

/// Build a new iroh endpoint, applying all relevant details from Configuration
/// to the base endpoint setup
pub(crate) async fn build_endpoint(secret_key: SecretKey, common: &Config) -> Result<Endpoint> {
    let mut builder = match common.discovery_mode {
        crate::config::DiscoveryMode::Dns => {
            Endpoint::empty_builder(default_relay_mode()).secret_key(secret_key)
        }
        crate::config::DiscoveryMode::Default | crate::config::DiscoveryMode::Hybrid => {
            Endpoint::builder().secret_key(secret_key)
        }
    };
    if let Some(addr) = common.ipv4_addr {
        builder = builder.bind_addr_v4(addr);
    }
    if let Some(addr) = common.ipv6_addr {
        builder = builder.bind_addr_v6(addr);
    }
    match common.discovery_mode {
        crate::config::DiscoveryMode::Default => {}
        crate::config::DiscoveryMode::Dns | crate::config::DiscoveryMode::Hybrid => {
            let origin = match &common.dns_origin {
                Some(origin) => origin.clone(),
                None => n0_error::bail_any!(
                    "dns_origin is required when discovery_mode is set to dns or hybrid"
                ),
            };
            if let Some(resolver_addr) = common.dns_resolver {
                let resolver = DnsResolver::builder()
                    .with_nameserver(resolver_addr, DnsProtocol::Udp)
                    .build();
                builder = builder.dns_resolver(resolver);
            }
            builder = builder.discovery(DnsDiscovery::builder(origin));
        }
    }
    let endpoint = builder.bind().await?;
    info!(id = %endpoint.id(), "iroh endpoint bound");
    Ok(endpoint)
}

pub(crate) fn n0des_api_secret_from_env() -> Result<Option<ApiSecret>> {
    let api_secret_str = match std::env::var("N0DES_API_SECRET") {
        Ok(s) => s,
        Err(_) => match option_env!("BUILD_N0DES_API_SECRET") {
            None => return Ok(None),
            Some(s) => s.to_string(),
        },
    };
    let api_secret = ApiSecret::from_str(&api_secret_str)
        .context("Failed to parse n0des API secret from env variable N0DES_API_SECRET")?;
    Ok(Some(api_secret))
}

pub(crate) async fn build_n0des_client_opt(
    endpoint: &Endpoint,
    api_secret: Option<ApiSecret>,
) -> Option<Arc<iroh_n0des::Client>> {
    match api_secret {
        None => {
            info!("Disabling metrics collection: N0DES_API_SECRET is not set");
            None
        }
        Some(n0des_api_secret) => match build_n0des_client(endpoint, n0des_api_secret).await {
            Ok(client) => Some(client),
            Err(err) => {
                warn!("Disabling metrics collection: Failed to connect to n0des: {err:#}");
                None
            }
        },
    }
}

pub(crate) async fn build_n0des_client(
    endpoint: &Endpoint,
    api_secret: ApiSecret,
) -> Result<Arc<iroh_n0des::Client>> {
    let remote_id = api_secret.remote.id;
    debug!(remote=%remote_id.fmt_short(), "connecting to n0des endpoint");
    let client = iroh_n0des::Client::builder(endpoint)
        .api_secret(api_secret)?
        .build()
        .await
        .std_context("Failed to connect to n0des endpoint")?;
    info!(remote=%remote_id.fmt_short(), "Connected to n0des endpoint for metrics collection");
    Ok(Arc::new(client))
}
