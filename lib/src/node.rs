use std::{
    fmt::Debug,
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{
    Endpoint, EndpointId, SecretKey, discovery::dns::DnsDiscovery, endpoint::default_relay_mode,
    protocol::Router,
};
use iroh_relay::dns::{DnsProtocol, DnsResolver};
use iroh_n0des::ApiSecret;
use iroh_proxy_utils::{ALPN as IROH_HTTP_CONNECT_ALPN, HttpProxyRequest, HttpProxyRequestKind};
use iroh_proxy_utils::{
    downstream::{DownstreamProxy, EndpointAuthority, ProxyMode},
    upstream::{AuthError, AuthHandler, UpstreamProxy},
};
use n0_error::{AnyError, Result, StackResultExt, StdResultExt, stack_error};
use n0_future::{IterExt, StreamExt, task::AbortOnDropHandle};
use tokio::{
    net::TcpListener,
    sync::{broadcast, futures::Notified},
    task::JoinHandle,
};
use tracing::{Instrument, debug, error, error_span, info, instrument, warn};
use ttl_cache::TtlCache;

use crate::{
    Advertisment, ProxyState, Repo, SelectedContext, StateWrapper, TcpProxyData, config::Config,
    datum_cloud::DatumCloudClient, state::AdvertismentTicket,
};

const TICKET_TTL: Duration = Duration::from_secs(30);

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
    n0des: Arc<iroh_n0des::Client>,
    metrics_tx: broadcast::Sender<MetricsUpdate>,
    _metrics_task: Arc<AbortOnDropHandle<()>>,
}

impl ListenNode {
    pub async fn new(repo: Repo) -> Result<Self> {
        let n0des_api_secret = n0des_api_secret_from_env()?;
        Self::with_n0des_api_secret(repo, n0des_api_secret).await
    }

    #[instrument("listen-node", skip_all)]
    pub async fn with_n0des_api_secret(repo: Repo, n0des_api_secret: ApiSecret) -> Result<Self> {
        let config = repo.config().await?;
        let secret_key = repo.listen_key().await?;
        let endpoint = build_endpoint(secret_key, &config).await?;
        let n0des = build_n0des_client(&endpoint, n0des_api_secret).await?;

        let state = repo.load_state().await?;

        let upstream_proxy = UpstreamProxy::new(state.clone())?;

        let router = Router::builder(endpoint)
            .accept(IROH_HTTP_CONNECT_ALPN, upstream_proxy)
            .spawn();

        let (metrics_tx, _) = broadcast::channel(1);

        let metrics_update_interval = Duration::from_millis(100);
        let metrics_task = tokio::spawn(
            {
                let endpoint = router.endpoint().clone();
                let metrics_tx = metrics_tx.clone();
                async move {
                    loop {
                        let metrics = endpoint.metrics();
                        let recv_total = metrics.magicsock.recv_data_ipv4.get()
                            + metrics.magicsock.recv_data_ipv6.get()
                            + metrics.magicsock.recv_data_relay.get();
                        let send_total = metrics.magicsock.send_data.get();
                        let update = MetricsUpdate {
                            send: send_total,
                            recv: recv_total,
                        };
                        metrics_tx.send(update).ok();
                        n0_future::time::sleep(metrics_update_interval).await;
                    }
                }
            }
            .instrument(error_span!("metrics")),
        );

        let this = Self {
            n0des,
            repo,
            router,
            state,
            metrics_tx,
            _metrics_task: Arc::new(AbortOnDropHandle::new(metrics_task)),
        };
        this.announce_all().await;
        Ok(this)
    }

    pub fn state_updated(&self) -> Notified<'_> {
        self.state.updated()
    }

    pub fn state(&self) -> &StateWrapper {
        &self.state
    }

    pub fn metrics(&self) -> broadcast::Receiver<MetricsUpdate> {
        self.metrics_tx.subscribe()
    }

    pub fn proxies(&self) -> Vec<ProxyState> {
        self.state.get().proxies.iter().cloned().collect()
    }

    pub fn selected_context(&self) -> Option<SelectedContext> {
        self.state.get().selected_context.clone()
    }

    pub async fn set_selected_context(
        &self,
        selected_context: Option<SelectedContext>,
    ) -> Result<()> {
        info!(
            selected = %selected_context
                .as_ref()
                .map_or("<none>".to_string(), SelectedContext::label),
            "node: updating selected context"
        );
        self.state
            .update(&self.repo, |state| {
                state.selected_context = selected_context;
            })
            .await?;
        Ok(())
    }

    pub async fn validate_selected_context(
        &self,
        datum: &DatumCloudClient,
    ) -> Result<Option<SelectedContext>> {
        let selected = self.selected_context();
        let Some(selected) = selected else {
            return Ok(None);
        };

        let orgs = datum.orgs_and_projects().await?;
        let is_valid = orgs.iter().any(|org| {
            if org.org.resource_id != selected.org_id {
                return false;
            }
            org.projects
                .iter()
                .any(|project| project.resource_id == selected.project_id)
        });

        if is_valid {
            Ok(Some(selected))
        } else {
            self.set_selected_context(None).await?;
            Ok(None)
        }
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
        if proxy.enabled {
            self.announce_proxy(&proxy.info).await?;
        }
        Ok(())
    }

    pub async fn remove_proxy(&self, resource_id: &str) -> Result<Option<ProxyState>> {
        debug!(%resource_id, "removing proxy {resource_id}");
        let res = self
            .state
            .update(&self.repo, move |state| state.remove_proxy(resource_id))
            .await;
        debug!(%resource_id, "removed {res:?}");
        if let Err(err) = self
            .n0des
            .unpublish_ticket::<AdvertismentTicket>(resource_id.to_string())
            .await
        {
            warn!(%resource_id, "Failed to unpublish ticket from n0des: {err:#}");
        }
        res
    }

    async fn announce_proxy(&self, proxy: &Advertisment) -> Result<()> {
        let ticket = proxy.ticket(self.endpoint_id());
        let name = ticket.data.resource_id.clone();
        debug!(%name, ?proxy, "announce");
        if let Err(err) = self.n0des.publish_ticket(name, ticket).await {
            error!(?proxy, "Failed to publish ticket: {err:#}");
            Err(err).anyerr()
        } else {
            Ok(())
        }
    }

    async fn announce_all(&self) {
        let state = self.state.get();
        let count = state
            .proxies
            .iter()
            .filter(|proxy| proxy.enabled)
            .map(async |proxy| self.announce_proxy(&proxy.info).await)
            .into_unordered_stream()
            .count()
            .await;
        debug!("announced {count} proxies");
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
        self.get()
            .proxies
            .iter()
            .any(|a| a.enabled && a.info.service().host == host && a.info.service().port == port)
    }
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
            HttpProxyRequestKind::Absolute { .. } => Err(AuthError::Forbidden),
        }
    }
}

#[derive(derive_more::Debug, Clone)]
pub struct TicketClient {
    n0des: Arc<iroh_n0des::Client>,
    #[debug(skip)]
    cache: Arc<Mutex<TtlCache<String, AdvertismentTicket>>>,
}

#[stack_error(derive)]
pub enum FetchTicketError {
    NotFound,
    FailedToFetch(#[error(source)] AnyError),
}

impl TicketClient {
    pub(crate) fn new(n0des: Arc<iroh_n0des::Client>) -> Self {
        Self {
            n0des,
            cache: Arc::new(Mutex::new(TtlCache::new(1024))),
        }
    }

    pub async fn get(&self, codename: &str) -> Result<AdvertismentTicket, FetchTicketError> {
        if let Some(ticket) = self.cache.lock().expect("poisoned").get(codename) {
            debug!(%codename, "ticket is cached");
            Ok(ticket.clone())
        } else {
            debug!(%codename, "fetch ticket from n0des");
            let ticket = self
                .n0des
                .fetch_ticket::<AdvertismentTicket>(codename.to_string())
                .await
                .map_err(|err| FetchTicketError::FailedToFetch(AnyError::from_std(err)))?
                .map(|ticket| ticket.ticket)
                .ok_or(FetchTicketError::NotFound)?;
            self.cache.lock().expect("poisoned").insert(
                codename.to_string(),
                ticket.clone(),
                TICKET_TTL,
            );
            Ok(ticket)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectNode {
    pub(crate) endpoint: Endpoint,
    pub(crate) proxy: DownstreamProxy,
    pub tickets: TicketClient,
}

impl ConnectNode {
    pub async fn new(repo: Repo) -> Result<Self> {
        let n0des_api_secret = n0des_api_secret_from_env()?;
        Self::with_n0des_api_secret(repo, n0des_api_secret).await
    }

    #[instrument("connect-node", skip_all)]
    pub async fn with_n0des_api_secret(repo: Repo, n0des_api_secret: ApiSecret) -> Result<Self> {
        let config = repo.config().await?;
        let secret_key = repo.connect_key().await?;
        let endpoint = build_endpoint(secret_key, &config).await?;
        let n0des = build_n0des_client(&endpoint, n0des_api_secret).await?;
        let tickets = TicketClient::new(n0des);
        let pool = DownstreamProxy::new(endpoint.clone(), Default::default());
        Ok(Self {
            endpoint,
            tickets,
            proxy: pool,
        })
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub async fn connect_codename_and_bind_local(
        &self,
        codename: &str,
        bind_addr: SocketAddr,
    ) -> Result<OutboundProxyHandle> {
        let ticket = self.tickets.get(codename).await?;
        self.connect_and_bind_local(ticket.endpoint, ticket.service(), bind_addr)
            .await
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
        crate::config::DiscoveryMode::Default
        | crate::config::DiscoveryMode::Hybrid => Endpoint::builder().secret_key(secret_key),
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

pub(crate) fn n0des_api_secret_from_env() -> Result<ApiSecret> {
    let api_secret_str = match std::env::var("N0DES_API_SECRET") {
        Ok(s) => s,
        Err(_) => match option_env!("BUILD_N0DES_API_SECRET") {
            None => n0_error::bail_any!("Missing env varable N0DES_API_SECRET"),
            Some(s) => s.to_string(),
        },
    };
    let api_secret = ApiSecret::from_str(&api_secret_str)
        .context("Failed to parse n0des API secret from env variable N0DES_API_SECRET")?;
    Ok(api_secret)
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
    info!(remote=%remote_id.fmt_short(), "connected to n0des endpoint");
    Ok(Arc::new(client))
}
