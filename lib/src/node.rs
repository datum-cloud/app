use anyhow::anyhow;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId, SecretKey};
use n0_error::{Result, StdResultExt, anyerr};
use n0_future::task::AbortOnDropHandle;
use n0_future::try_join_all;
use quinn::{RecvStream, SendStream};
use std::fmt::Debug;
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::vec;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, trace};
use uuid::Uuid;

use iroh_proxy_utils::http_connect::{IROH_HTTP_CONNECT_ALPN, TunnelClientStreams, TunnelListener};

// use crate::datum_cloud::DatumCloudClient;
use crate::state::{ConnectionInfo, TcpProxy, TcpProxyTicket, generate_codename};
use crate::{Repo, config::Config};

#[derive(Debug, Clone)]
pub struct Node {
    id: EndpointId,
    inner: Arc<Mutex<NodeInner>>,
}

impl Node {
    pub async fn new(secret_key: SecretKey, repo: Repo) -> Result<Self> {
        // TODO(b5) - add auth string
        let inner = NodeInner::new(secret_key, repo).await?;
        let endpoint = inner.router.endpoint().clone();
        let ep_id = endpoint.id();
        let inner = Arc::new(Mutex::new(inner));

        let inner2 = inner.clone();
        n0_future::task::spawn(async move {
            // wait for online-ness in a task to avoid blocking app startup.
            // TODO(b5): we should really show some sort of "offline" indicator
            // if this doesn't resolve within a short-ish timeframe
            endpoint.online().await;
            let inner = inner2.lock().await;
            if let Err(err) = inner.announce_proxies().await {
                error!("announcing proxies: {}", err);
            }
        });

        Ok(Self { id: ep_id, inner })
    }

    pub fn endpoint_id(&self) -> String {
        self.id.fmt_short().to_string()
    }

    pub async fn proxies(&self) -> Result<Vec<TcpProxy>> {
        let inner = self.inner.lock().await;
        let state = inner.repo.load_state().await?;
        Ok(state.tcp_proxies)
    }

    pub async fn start_listening(&self, _label: String, port: String) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.listen(port).await
    }

    pub async fn stop_listening(&self, proxy: &TcpProxy) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.unlisten(proxy).await
    }

    pub async fn update_proxy(&self, proxy: &TcpProxy) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.update_proxy(proxy).await
    }

    pub async fn connect(
        &self,
        codename: String,
    ) -> Result<(ConnectionInfo, (SendStream, RecvStream))> {
        let mut inner = self.inner.lock().await;
        // are we already connected?
        if let Some(conn) = inner
            .edge_connections
            .iter()
            .find(|conn| &conn.codename == &codename)
        {
            let info = conn.info();
            let streams = conn.streams().new_streams().await?;
            return Ok((info, streams));
        }

        // resolve codename to a ticket via n0des & build a tunnel
        let ticket = inner
            .n0des
            .fetch_ticket::<TcpProxyTicket>(codename.clone())
            .await
            .std_context("fetching n0des ticket")?
            .map(|ticket| ticket.ticket);

        let Some(ticket) = ticket else {
            return Err(n0_error::anyerr!("codename not found"));
        };

        let conn = inner.connect(codename, ticket).await?;
        let info = conn.info();
        let streams = conn.streams().new_streams().await?;
        Ok((info, streams))
    }

    pub async fn connect_ticket(
        &self,
        ticket: TcpProxyTicket,
    ) -> Result<(ConnectionInfo, (SendStream, RecvStream))> {
        let codename = generate_codename(ticket.id);
        let mut inner = self.inner.lock().await;
        let conn = inner.connect(codename, ticket).await?;
        let info = conn.info();
        let streams = conn.streams().new_streams().await?;
        Ok((info, streams))
    }

    pub async fn wrap_connection_tcp(
        &self,
        codename: Option<String>,
        ticket: Option<TcpProxyTicket>,
        listen: &str,
    ) -> Result<JoinHandle<()>> {
        let addr = SocketAddr::from_str(listen).std_context("invalid socket address")?;
        let mut inner = self.inner.lock().await;
        let (codename, ticket) = match (codename, ticket) {
            (Some(codename), None) => {
                let ticket = inner
                    .n0des
                    .fetch_ticket::<TcpProxyTicket>(codename.clone())
                    .await
                    .std_context("fetching n0des ticket")?
                    .map(|ticket| ticket.ticket)
                    .ok_or(anyerr!("ticket not found"))?;
                (codename, ticket)
            }
            (None, Some(ticket)) => {
                let codename = generate_codename(ticket.id);
                (codename, ticket)
            }
            (_, _) => return Err(anyerr!("invalid arguments")),
        };

        let conn = inner.connect(codename, ticket).await?;
        conn.streams().wrap_tcp(vec![addr]).await
    }

    pub async fn metrics(&self) -> Result<tokio::sync::broadcast::Receiver<Metrics>> {
        let sub = self.inner.lock().await.metrics_events.subscribe();
        Ok(sub)
    }
}

#[derive(Debug)]
struct NodeInner {
    repo: Repo,
    router: Router,
    n0des: iroh_n0des::Client,
    // TODO(b5) - use datum client
    // datum: DatumCloudClient,
    /// direct connections to another iroh endpoint, skipping the datum network
    edge_connections: Vec<Connection>,
    metrics_events: tokio::sync::broadcast::Sender<Metrics>,
    _metrics_task: AbortOnDropHandle<()>,
}

impl NodeInner {
    async fn new(secret_key: SecretKey, repo: Repo) -> anyhow::Result<Self> {
        let config = repo.config().await?;
        let endpoint =
            build_endpoint(secret_key, &config, vec![IROH_HTTP_CONNECT_ALPN.to_vec()]).await?;

        let auth = repo.auth().await?;
        let tunnel_listener = TunnelListener::new(auth)?;

        let n0des = iroh_n0des::Client::builder(&endpoint)
            .api_secret_from_env()
            // TODO(b5) - remove expect
            .expect("failed to read api secret from env")
            .build()
            .await
            .std_context("construction n0des client")?;

        let (tx, _) = tokio::sync::broadcast::channel(32);
        let metrics = endpoint.metrics().clone();
        let metrics_events = tx.clone();
        let metrics_task = n0_future::task::spawn(async move {
            loop {
                let recv = metrics.magicsock.recv_data_ipv4.get()
                    + metrics.magicsock.recv_data_ipv6.get()
                    + metrics.magicsock.recv_data_relay.get();
                let send = metrics.magicsock.send_data.get();
                if let Err(err) = tx.send(Metrics { send, recv }) {
                    trace!("send metrics on channel error: {:?}", err);
                }
                n0_future::time::sleep(n0_future::time::Duration::from_millis(100)).await;
            }
        });

        let router = Router::builder(endpoint)
            .accept(IROH_HTTP_CONNECT_ALPN, tunnel_listener)
            .spawn();

        // let datum = DatumCloudClient::new(None);
        let inner = NodeInner {
            repo,
            router,
            // datum,
            n0des,
            edge_connections: Vec::new(),
            metrics_events,
            _metrics_task: AbortOnDropHandle::new(metrics_task),
        };

        Ok(inner)
    }

    async fn announce_proxies(&self) -> Result<()> {
        let endpoint = self.router.endpoint();
        endpoint.online().await;
        let endpoint_id = endpoint.id();
        let state = self.repo.load_state().await?;
        try_join_all(
            state
                .tcp_proxies
                .iter()
                .map(|proxy| async { announce_proxy(&self.n0des, endpoint_id, proxy).await }),
        )
        .await?;

        Ok(())
    }

    pub async fn listen(&mut self, addr: String) -> Result<()> {
        let mut state = self.repo.load_state().await?;
        self.router.endpoint().online().await;

        info!("creating proxy for address {}", addr.clone());
        let (host, port) = parse_host_port(&addr)?;
        let proxy = TcpProxy::new(host, port);

        // TODO - validate we don't already have a listener for this host/port combo

        announce_proxy(&self.n0des, self.router.endpoint().id(), &proxy).await?;

        state.tcp_proxies.push(proxy);
        self.repo.write_state(&state).await?;

        Ok(())
    }

    pub async fn unlisten(&mut self, info: &TcpProxy) -> Result<()> {
        let mut state = self.repo.load_state().await?;
        state.tcp_proxies.retain(|proxy| proxy.id != info.id);
        self.repo.write_state(&state).await?;
        Ok(())
    }

    pub async fn update_proxy(&mut self, updated_proxy: &TcpProxy) -> Result<()> {
        let mut state = self.repo.load_state().await?;

        // Find and update the proxy with matching ID
        if let Some(proxy) = state
            .tcp_proxies
            .iter_mut()
            .find(|p| p.id == updated_proxy.id)
        {
            proxy.host = updated_proxy.host.clone();
            proxy.port = updated_proxy.port;
            // Note: codename and id should not change
        }

        self.repo.write_state(&state).await?;
        Ok(())
    }

    pub async fn connections(&self) -> Vec<ConnectionInfo> {
        self.edge_connections.iter().map(|l| l.info()).collect()
    }

    pub async fn connect(
        &mut self,
        codename: String,
        ticket: TcpProxyTicket,
    ) -> Result<&Connection> {
        let streams = TunnelClientStreams::new(
            &self.router.endpoint(),
            ticket.endpoint,
            ticket.host.clone(),
            ticket.port,
        )
        .await?;
        let conn = Connection {
            id: ticket.id,
            codename,
            host: ticket.host.clone(),
            port: ticket.port,
            streams,
        };
        self.edge_connections.push(conn);
        let conn = self.edge_connections.last().expect("just added");
        Ok(conn)
    }

    pub async fn disconnect(&mut self, conn: &ConnectionInfo) -> anyhow::Result<()> {
        let mut found = false;
        debug!("disconnect tcp. id: {:?}", conn.id);
        let index = self.edge_connections.iter().position(|c| c.id == conn.id);
        if let Some(index) = index {
            let conn = self.edge_connections.remove(index);
            conn.streams.close();
            found = true;
        }
        match found {
            true => Ok(()),
            false => Err(anyhow!("Tunnel connection not found")),
        }
    }
}

async fn announce_proxy(
    n0des: &iroh_n0des::Client,
    local_endpoint: EndpointId,
    proxy: &TcpProxy,
) -> Result<()> {
    let ticket = proxy.ticket(local_endpoint);
    n0des
        .publish_ticket(proxy.codename.clone(), ticket)
        .await
        .std_context("publishing ticket to n0des")
}

#[derive(Debug, Default, Clone)]
pub struct Metrics {
    pub send: u64,
    pub recv: u64,
}

#[derive(Debug)]
pub struct Connection {
    /// the id of the tunnel listener
    id: Uuid,
    /// the codename of the tunne, a three-word-combo derived from the id
    codename: String,
    /// the host on the exit of the tunnel
    host: String,
    /// port on on the exit of the tunnel
    port: u16,
    /// The actual streams of the connection
    streams: TunnelClientStreams,
}

impl Connection {
    fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            id: self.id,
            codename: self.codename.clone(),
            host: self.host.clone(),
            port: self.port,
        }
    }

    pub(crate) fn streams(&self) -> &TunnelClientStreams {
        &self.streams
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

fn parse_host_port(s: &str) -> Result<(String, u16)> {
    // ToSocketAddrs handles all the parsing for us
    let mut addrs = s.to_socket_addrs()?;

    // Get the first resolved address
    let addr = addrs.next().ok_or(anyerr!("Failed to resolve address"))?;

    // Extract host and port
    // Note: this gives us the resolved IP, not the original hostname
    Ok((addr.ip().to_string(), addr.port()))
}
