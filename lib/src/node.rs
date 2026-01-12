use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId, SecretKey};
use n0_error::{Result, StackResultExt, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use n0_future::{IterExt, StreamExt};
use quinn::{RecvStream, SendStream};
use std::fmt::Debug;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::vec;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, error, warn};

use iroh_proxy_utils::http_connect::{
    AuthHandler, IROH_HTTP_CONNECT_ALPN, TunnelClientStreams, TunnelListener,
};

use crate::state::AdvertismentTicket;
use crate::{Advertisment, ProxyState, StateWrapper, TcpProxyData};
use crate::{Repo, config::Config};

#[derive(Debug, Clone)]
pub struct Node {
    pub inbound: InboundListener,
    pub outbound: OutboundDialer,
}

impl Node {
    pub async fn new(repo: Repo) -> Result<Self> {
        let inbound = InboundListener::new(repo.clone()).await?;
        let outbound = OutboundDialer::new(repo).await?;
        Ok(Self { inbound, outbound })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MetricsUpdate {
    pub send_total: u64,
    pub recv_total: u64,
}

#[derive(Debug, Clone)]
pub struct InboundListener {
    router: Router,
    state: StateWrapper,
    repo: Repo,
    n0des: Arc<iroh_n0des::Client>,
    metrics_tx: broadcast::Sender<MetricsUpdate>,
    _metrics_task: Arc<AbortOnDropHandle<()>>,
}

impl InboundListener {
    pub async fn new(repo: Repo) -> Result<Self> {
        let config = repo.config().await?;
        let secret_key = repo.listen_key().await?;
        let endpoint =
            build_endpoint(secret_key, &config, vec![IROH_HTTP_CONNECT_ALPN.to_vec()]).await?;
        let n0des = build_n0des_client(&endpoint).await?;

        let state = repo.load_state().await?;

        let tunnel_listener = TunnelListener::new(state.clone())?;

        let router = Router::builder(endpoint)
            .accept(IROH_HTTP_CONNECT_ALPN, tunnel_listener)
            .spawn();

        let (metrics_tx, _) = broadcast::channel(1);

        let metrics_update_interval = Duration::from_millis(100);
        let metrics_task = tokio::spawn({
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
                        send_total,
                        recv_total,
                    };
                    metrics_tx.send(update).ok();
                    n0_future::time::sleep(metrics_update_interval).await;
                }
            }
        });

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

    pub fn state(&self) -> &StateWrapper {
        &self.state
    }

    pub fn metrics(&self) -> broadcast::Receiver<MetricsUpdate> {
        self.metrics_tx.subscribe()
    }

    pub fn proxies(&self) -> Vec<ProxyState> {
        self.state.get().proxies.iter().cloned().collect()
    }

    pub async fn set_proxy(&self, proxy: ProxyState) -> Result<()> {
        self.state
            .update(&self.repo, |state| state.set_proxy(proxy.clone()))
            .await?;
        if proxy.enabled {
            self.announce_proxy(&proxy.info).await;
        }
        Ok(())
    }

    pub async fn remove_proxy(&self, resource_id: &str) -> Result<Option<ProxyState>> {
        let res = self
            .state
            .update(&self.repo, move |state| state.remove_proxy(resource_id))
            .await;
        if let Err(err) = self
            .n0des
            .unpublish_ticket::<AdvertismentTicket>(resource_id.to_string())
            .await
        {
            warn!(%resource_id, "Failed to unpublish ticket from n0des: {err:#}");
        }
        res
    }

    async fn announce_proxy(&self, proxy: &Advertisment) {
        let ticket = proxy.ticket(self.endpoint_id());
        let name = ticket.data.resource_id.clone();
        debug!(%name, ?proxy, "announce");
        if let Err(err) = self.n0des.publish_ticket(name, ticket).await {
            error!(?proxy, "Failed to publish ticket: {err:#}");
        }
    }

    async fn announce_all(&self) {
        let state = self.state.get();
        state
            .proxies
            .iter()
            .filter(|proxy| proxy.enabled)
            .map(async |proxy| self.announce_proxy(&proxy.info))
            .into_unordered_stream()
            .count()
            .await;
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id()
    }
}

impl StateWrapper {
    fn is_tcp_allowed(&self, host: &str, port: u16) -> bool {
        self.get()
            .proxies
            .iter()
            .any(|a| a.enabled && a.info.service().host == host && a.info.service().port == port)
    }
}

impl AuthHandler for StateWrapper {
    fn authorize<'a>(
        &'a self,
        req: &'a iroh_proxy_utils::http_connect::Request,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<bool, iroh_proxy_utils::error::AuthError>> + Send + 'a>,
    > {
        Box::pin(async move {
            match req {
                iroh_proxy_utils::http_connect::Request::Connect(req) => {
                    Ok(self.is_tcp_allowed(&req.host, req.port))
                }
                iroh_proxy_utils::http_connect::Request::Http(_req) => Ok(false),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct OutboundDialer {
    endpoint: Endpoint,
    n0des: Arc<iroh_n0des::Client>,
}

impl OutboundDialer {
    pub async fn new(repo: Repo) -> Result<Self> {
        let config = repo.config().await?;
        let secret_key = repo.connect_key().await?;
        let endpoint = build_endpoint(secret_key, &config, vec![]).await?;
        let n0des = build_n0des_client(&endpoint).await?;
        Ok(Self { endpoint, n0des })
    }

    pub async fn fetch_ticket(&self, codename: &str) -> Result<AdvertismentTicket> {
        let ticket = self
            .n0des
            .fetch_ticket::<AdvertismentTicket>(codename.to_string())
            .await
            .std_context("fetching n0des ticket")?
            .map(|ticket| ticket.ticket)
            .context("ticket not found")?;
        Ok(ticket)
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub async fn connect_codename(&self, codename: &str) -> Result<OutboundProxyStreams> {
        let ticket = self.fetch_ticket(codename).await?;
        self.connect(ticket.endpoint, ticket.service()).await
    }

    pub async fn connect_codename_and_bind_local(
        &self,
        codename: &str,
        bind_addr: SocketAddr,
    ) -> Result<OutboundProxyHandle> {
        let ticket = self.fetch_ticket(codename).await?;
        self.connect_and_bind_local(ticket.endpoint, ticket.service(), bind_addr)
            .await
    }

    pub async fn connect(
        &self,
        endpoint: EndpointId,
        advertisment: &TcpProxyData,
    ) -> Result<OutboundProxyStreams> {
        let conn = TunnelClientStreams::new(
            &self.endpoint,
            endpoint,
            advertisment.host.clone(),
            advertisment.port,
        )
        .await?;
        let (send, recv) = conn.new_streams().await?;
        Ok(OutboundProxyStreams {
            endpoint_id: endpoint,
            send,
            recv,
        })
    }

    pub async fn connect_and_bind_local(
        &self,
        remote_id: EndpointId,
        advertisment: &TcpProxyData,
        bind_addr: SocketAddr,
    ) -> Result<OutboundProxyHandle> {
        let conn = TunnelClientStreams::new(
            &self.endpoint,
            remote_id,
            advertisment.host.clone(),
            advertisment.port,
        )
        .await?;
        let handle = conn.wrap_tcp([bind_addr]).await?;
        Ok(OutboundProxyHandle {
            remote_id,
            task: handle,
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

pub enum BindAddr {
    AsAdvertised,
    Override(SocketAddr),
}

impl BindAddr {
    pub fn or_default(&self, addr: SocketAddr) -> SocketAddr {
        match self {
            BindAddr::AsAdvertised => addr,
            BindAddr::Override(addr) => *addr,
        }
    }
}

pub struct OutboundProxyStreams {
    pub endpoint_id: EndpointId,
    pub send: SendStream,
    pub recv: RecvStream,
}

/// Build a new iroh endpoint, applying all relevant details from Configuration
/// to the base endpoint setup
async fn build_endpoint(
    secret_key: SecretKey,
    common: &Config,
    alpns: Vec<Vec<u8>>,
) -> Result<Endpoint> {
    let mut builder = Endpoint::builder().secret_key(secret_key).alpns(alpns);
    if let Some(addr) = common.ipv4_addr {
        builder = builder.bind_addr_v4(addr);
    }
    if let Some(addr) = common.ipv6_addr {
        builder = builder.bind_addr_v6(addr);
    }
    let endpoint = builder.bind().await?;
    Ok(endpoint)
}

async fn build_n0des_client(endpoint: &Endpoint) -> Result<Arc<iroh_n0des::Client>> {
    let client = iroh_n0des::Client::builder(endpoint)
        .api_secret_from_env()
        .context("failed to read api secret from env")?
        .build()
        .await
        .std_context("construction n0des client")?;
    Ok(Arc::new(client))
}
