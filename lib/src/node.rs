use std::{fmt::Debug, net::SocketAddr, str::FromStr, sync::Arc, time::Duration};

use iroh::{
    Endpoint, EndpointId, SecretKey, discovery::dns::DnsDiscovery, endpoint::default_relay_mode,
    protocol::Router,
};
use iroh_base::RelayUrl;
use iroh_n0des::ApiSecret;
use iroh_proxy_utils::upstream::UpstreamMetrics;
use iroh_proxy_utils::{
    ALPN as IROH_HTTP_CONNECT_ALPN, Authority, HttpProxyRequest, HttpProxyRequestKind,
};
use iroh_proxy_utils::{
    downstream::{DownstreamProxy, EndpointAuthority, ProxyMode},
    upstream::{AuthError, AuthHandler, UpstreamProxy},
};
use iroh_relay::dns::{DnsProtocol, DnsResolver};
use iroh_relay::{RelayConfig, RelayMap};
use n0_error::{Result, StackResultExt, StdResultExt};
use tokio::{
    net::TcpListener,
    sync::futures::Notified,
    task::{JoinHandle, JoinSet},
};
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
    metrics: Arc<UpstreamMetrics>,
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

    pub fn metrics(&self) -> &Arc<UpstreamMetrics> {
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
    let relay_mode = relay_mode_from_env_or_build().await?;
    let mut builder = match common.discovery_mode {
        crate::config::DiscoveryMode::Dns => {
            Endpoint::empty_builder(relay_mode).secret_key(secret_key)
        }
        crate::config::DiscoveryMode::Default | crate::config::DiscoveryMode::Hybrid => {
            Endpoint::builder()
                .relay_mode(relay_mode)
                .secret_key(secret_key)
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

const DATUM_CONNECT_RELAY_URLS: &str = "DATUM_CONNECT_RELAY_URLS";
const BUILD_DATUM_CONNECT_RELAY_URLS: &str = "BUILD_DATUM_CONNECT_RELAY_URLS";
const STARTUP_RELAY_SELECTION_MAX: usize = 5;
const STARTUP_RELAY_PROBE_TIMEOUT: Duration = Duration::from_millis(800);

async fn relay_mode_from_env_or_build() -> Result<iroh::endpoint::RelayMode> {
    if let Ok(raw_urls) = std::env::var(DATUM_CONNECT_RELAY_URLS) {
        match parse_relay_urls(&raw_urls) {
            Ok(relays) => {
                let relays =
                    select_best_relays_for_startup(relays, STARTUP_RELAY_SELECTION_MAX).await;
                info!(
                    source = %DATUM_CONNECT_RELAY_URLS,
                    count = relays.len(),
                    "using custom iroh relay list from environment"
                );
                return Ok(iroh::endpoint::RelayMode::Custom(relays_to_map(relays)));
            }
            Err(err) => {
                warn!("invalid relay urls in {DATUM_CONNECT_RELAY_URLS}: {err:#}");
            }
        }
    }

    if let Some(raw_urls) = option_env!("BUILD_DATUM_CONNECT_RELAY_URLS") {
        match parse_relay_urls(raw_urls) {
            Ok(relays) => {
                let relays =
                    select_best_relays_for_startup(relays, STARTUP_RELAY_SELECTION_MAX).await;
                info!(
                    source = %BUILD_DATUM_CONNECT_RELAY_URLS,
                    count = relays.len(),
                    "using custom iroh relay list from build environment"
                );
                return Ok(iroh::endpoint::RelayMode::Custom(relays_to_map(relays)));
            }
            Err(err) => {
                warn!("invalid relay urls in {BUILD_DATUM_CONNECT_RELAY_URLS}: {err:#}");
            }
        }
    }

    Ok(default_relay_mode())
}

fn parse_relay_urls(raw: &str) -> Result<Vec<RelayUrl>> {
    let relays: Vec<RelayUrl> = raw
        .split(|c: char| c == ',' || c == ';' || c.is_ascii_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_relay_url)
        .map(|url| RelayUrl::from_str(&url))
        .collect::<std::result::Result<Vec<_>, _>>()
        .std_context(
            "Failed parsing relay URL list. Expected comma/space/newline separated URLs",
        )?;

    if relays.is_empty() {
        n0_error::bail_any!("Relay URL list was provided but empty after parsing");
    }

    let mut deduped = Vec::with_capacity(relays.len());
    for relay in relays {
        if !deduped.iter().any(|seen: &RelayUrl| seen == &relay) {
            deduped.push(relay);
        }
    }
    Ok(deduped)
}

fn normalize_relay_url(raw: &str) -> String {
    if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    }
}

async fn select_best_relays_for_startup(relays: Vec<RelayUrl>, max_relays: usize) -> Vec<RelayUrl> {
    let total_candidates = relays.len();
    if relays.len() <= max_relays {
        return relays;
    }

    let client = match reqwest::Client::builder()
        .timeout(STARTUP_RELAY_PROBE_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            warn!("relay probe setup failed, using first {max_relays} relays: {err:#}");
            return relays.into_iter().take(max_relays).collect();
        }
    };

    let mut joinset = JoinSet::new();
    for relay in relays.iter().cloned() {
        let client = client.clone();
        joinset.spawn(async move {
            let latency = probe_relay_latency(&client, &relay).await;
            (relay, latency)
        });
    }

    let mut successful = Vec::new();
    let mut failed = Vec::new();
    while let Some(joined) = joinset.join_next().await {
        match joined {
            Ok((relay, Ok(latency))) => successful.push((relay, latency)),
            Ok((relay, Err(reason))) => failed.push((relay, reason)),
            Err(err) => {
                debug!("relay probe task join error: {err:#}");
            }
        }
    }

    successful.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.as_str().cmp(b.0.as_str())));
    let mut selected: Vec<RelayUrl> = successful
        .iter()
        .take(max_relays)
        .map(|(relay, _)| relay.clone())
        .collect();

    if selected.len() < max_relays {
        failed.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        for (relay, _) in &failed {
            if selected.len() == max_relays {
                break;
            }
            if !selected.iter().any(|r| r == relay) {
                selected.push(relay.clone());
            }
        }
    }

    if selected.len() < max_relays {
        for relay in relays {
            if selected.len() == max_relays {
                break;
            }
            if !selected.iter().any(|r| r == &relay) {
                selected.push(relay);
            }
        }
    }

    if !failed.is_empty() {
        let failure_samples: Vec<String> = failed
            .iter()
            .take(5)
            .map(|(relay, reason)| format!("{relay} -> {reason}"))
            .collect();
        warn!(
            failed = failed.len(),
            samples = ?failure_samples,
            "relay ping probe failures observed"
        );
    }
    info!(
        total = total_candidates,
        successful = successful.len(),
        selected = selected.len(),
        selected_relays = ?selected,
        "selected startup relay shortlist"
    );
    selected
}

async fn probe_relay_latency(
    client: &reqwest::Client,
    relay: &RelayUrl,
) -> std::result::Result<Duration, String> {
    let host = relay
        .host_str()
        .ok_or_else(|| "missing host in relay url".to_string())?
        // RelayUrl canonicalizes with trailing dot, which can fail strict TLS hostname checks.
        .trim_end_matches('.');
    let mut https_url = reqwest::Url::parse(&format!("https://{host}/ping"))
        .map_err(|err| format!("url parse: {err}"))?;
    https_url.set_query(None);
    debug!(
        relay = %relay,
        url = %https_url,
        timeout_ms = STARTUP_RELAY_PROBE_TIMEOUT.as_millis(),
        "starting relay ping probe"
    );
    let start = tokio::time::Instant::now();
    match client.get(https_url.clone()).send().await {
        Ok(resp) if resp.status().is_success() => {
            let elapsed = start.elapsed();
            debug!(
                relay = %relay,
                url = %https_url,
                status = %resp.status(),
                elapsed_ms = elapsed.as_millis(),
                "relay ping probe succeeded"
            );
            Ok(elapsed)
        }
        Ok(resp) => {
            debug!(
                relay = %relay,
                url = %https_url,
                status = %resp.status(),
                elapsed_ms = start.elapsed().as_millis(),
                "relay ping probe got non-success response"
            );
            Err(format!("status {}", resp.status()))
        }
        Err(err) => {
            debug!(
                relay = %relay,
                url = %https_url,
                elapsed_ms = start.elapsed().as_millis(),
                "relay ping probe request failed: {err:#}"
            );
            Err(format!("{err:#}"))
        }
    }
}

fn relays_to_map(relays: Vec<RelayUrl>) -> RelayMap {
    RelayMap::from_iter(relays.into_iter().map(RelayConfig::from))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_relay_urls_accepts_bare_hostnames() {
        let input = "iroh-relay.us-east-1.datumconnect.net,iroh-relay.us-west-1.datumconnect.net";
        let parsed = parse_relay_urls(input).expect("should parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].scheme(), "https");
        assert_eq!(
            parsed[0].host_str(),
            Some("iroh-relay.us-east-1.datumconnect.net.")
        );
        assert_eq!(
            parsed[1].host_str(),
            Some("iroh-relay.us-west-1.datumconnect.net.")
        );
    }

    #[test]
    fn parse_relay_urls_dedupes_and_skips_empty_tokens() {
        let input = "  relay-a.example.com, relay-a.example.com;;relay-b.example.com  ";
        let parsed = parse_relay_urls(input).expect("should parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].host_str(), Some("relay-a.example.com."));
        assert_eq!(parsed[1].host_str(), Some("relay-b.example.com."));
    }
}
