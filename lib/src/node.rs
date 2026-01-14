use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId, SecretKey};
use n0_error::{Result, StackResultExt, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use n0_future::{IterExt, StreamExt};
use std::fmt::Debug;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::vec;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::futures::Notified;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, error_span, info, warn};

use iroh_proxy_utils::{
    ALPN as IROH_HTTP_CONNECT_ALPN, AuthError, AuthHandler, Authority, HttpRequest, RequestKind,
    TunnelClientPool, TunnelListener,
};

use crate::state::AdvertismentTicket;
use crate::{Advertisment, ProxyState, StateWrapper, TcpProxyData};
use crate::{Repo, config::Config};

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
                        send: send_total,
                        recv: recv_total,
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
    fn authorize<'a>(
        &'a self,
        _remote_id: EndpointId,
        req: &'a HttpRequest,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let authority = match &req.kind {
                RequestKind::Connect { authority } => authority,
                RequestKind::Http {
                    authority_from_path,
                    ..
                } => authority_from_path
                    .as_ref()
                    .ok_or_else(|| AuthError::BadRequest)?,
            };

            if self.tcp_proxy_exists(&authority.host, authority.port) {
                Ok(())
            } else {
                Err(AuthError::Forbidden)
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConnectNode {
    pub(crate) endpoint: Endpoint,
    pub(crate) n0des: Arc<iroh_n0des::Client>,
    pub(crate) pool: TunnelClientPool,
}

impl ConnectNode {
    pub async fn new(repo: Repo) -> Result<Self> {
        let config = repo.config().await?;
        let secret_key = repo.connect_key().await?;
        let endpoint = build_endpoint(secret_key, &config, vec![]).await?;
        let n0des = build_n0des_client(&endpoint).await?;
        let pool = TunnelClientPool::new(endpoint.clone(), Default::default());
        Ok(Self {
            endpoint,
            n0des,
            pool,
        })
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

    pub async fn connect_codename_and_bind_local(
        &self,
        codename: &str,
        bind_addr: SocketAddr,
    ) -> Result<OutboundProxyHandle> {
        let ticket = self.fetch_ticket(codename).await?;
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
        let pool = self.pool.clone();
        let authority: Authority = advertisment.clone().into();
        let task = tokio::spawn(async move {
            info!("bound local socket on {bound_addr}");
            if let Err(err) = pool.forward_from_local_listener(remote_id, authority, local_socket).await {
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
