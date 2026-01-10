use anyhow::anyhow;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use n0_error::{Result, StdResultExt, anyerr};
use n0_future::task::AbortOnDropHandle;
use n0_future::try_join_all;
use quinn::{RecvStream, SendStream};
use std::env::VarError;
use std::fmt::Debug;
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::vec;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use iroh_proxy_utils::http_connect::{IROH_HTTP_CONNECT_ALPN, TunnelClientStreams, TunnelListener};

use crate::datum_cloud::DatumCloudClient;
use crate::state::{ConnectionInfo, TcpProxy, TcpProxyTicket, generate_codename};
use crate::{Repo, config::Config};

#[derive(Debug, Clone)]
pub struct Node {
    id: EndpointId,
    inner: Arc<Mutex<NodeInner>>,
    repo: Arc<Repo>,
}

impl Node {
    pub async fn new(secret_key: SecretKey, repo: Repo) -> Result<Self> {
        Self::new_with_announce(secret_key, repo, true).await
    }

    /// Create a Node intended only for dialing tunnels (e.g. the gateway).
    /// This does **not** announce/publish any locally persisted proxies to n0des.
    pub async fn new_connect_only(secret_key: SecretKey, repo: Repo) -> Result<Self> {
        Self::new_with_announce(secret_key, repo, false).await
    }

    async fn new_with_announce(
        secret_key: SecretKey,
        repo: Repo,
        announce_on_startup: bool,
    ) -> Result<Self> {
        // TODO(b5) - add auth string
        let repo = Arc::new(repo);
        let inner = NodeInner::new(secret_key, repo.clone()).await?;
        let endpoint = inner.router.endpoint().clone();
        let ep_id = endpoint.id();
        let inner = Arc::new(Mutex::new(inner));

        if announce_on_startup {
            let inner2 = inner.clone();
            n0_future::task::spawn(async move {
                // wait for online-ness in a task to avoid blocking app startup.
                // TODO(b5): we should really show some sort of "offline" indicator
                // if this doesn't resolve within a short-ish timeframe
                endpoint.online().await;
                // Snapshot the pieces we need without holding the mutex during network IO.
                let (repo, n0des, endpoint_addr) = {
                    let inner = inner2.lock().await;
                    (inner.repo.clone(), inner.n0des.clone(), inner.router.endpoint().addr())
                };

                let Some(n0des) = n0des else {
                    // Local-only mode: nothing to publish.
                    return;
                };

                let state = match repo.load_state().await {
                    Ok(state) => state,
                    Err(err) => {
                        error!("announcing proxies: loading state: {}", err);
                        return;
                    }
                };

                if let Err(err) = try_join_all(
                    state
                        .tcp_proxies
                        .iter()
                        .filter(|proxy| proxy.enabled)
                        .map(|proxy| async {
                            announce_proxy(n0des.as_ref(), endpoint_addr.clone(), proxy).await
                        }),
                )
                .await
                {
                    error!("announcing proxies: {}", err);
                }
            });
        }

        Ok(Self { id: ep_id, inner, repo })
    }

    pub fn endpoint_id(&self) -> String {
        self.id.fmt_short().to_string()
    }

    pub async fn proxies(&self) -> Result<Vec<TcpProxy>> {
        // Avoid taking the NodeInner mutex for a read-only state load.
        // That mutex can be held while doing n0des publishing, which would otherwise stall the UI.
        let state = self.repo.load_state().await?;
        Ok(state.tcp_proxies)
    }

    pub async fn start_listening(&self, label: String, port: String) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.listen(label, port).await
    }

    pub async fn stop_listening(&self, proxy: &TcpProxy) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.unlisten(proxy).await
    }

    /// Permanently delete a proxy from local state (best-effort unpublish from n0des).
    pub async fn delete_proxy(&self, proxy: &TcpProxy) -> Result<()> {
        info!(
            proxy_id = %proxy.id,
            codename = %proxy.codename,
            "delete_proxy: deleting from local state"
        );
        // Ensure our endpoint is online before attempting n0des RPC calls.
        // (Avoid holding the mutex while awaiting.)
        let endpoint = { self.inner.lock().await.router.endpoint().clone() };
        endpoint.online().await;

        // Snapshot n0des client without holding the mutex during network IO.
        let n0des = { self.inner.lock().await.n0des.clone() };

        // Remove from local persistent state first.
        let mut state = self.repo.load_state().await?;
        let before = state.tcp_proxies.len();
        state.tcp_proxies.retain(|p| p.id != proxy.id);
        let after = state.tcp_proxies.len();
        info!(before, after, "delete_proxy: updated local state");
        self.repo.write_state(&state).await?;

        // Best-effort: unpublish from n0des so the codename stops resolving.
        if let Some(n0des) = n0des {
            match tokio::time::timeout(
                Duration::from_secs(2),
                n0des.unpublish_ticket::<TcpProxyTicket>(proxy.codename.clone()),
            )
            .await
            {
                Ok(Ok(_removed)) => {}
                Ok(Err(err)) => warn!("unpublishing ticket from n0des failed: {err}"),
                Err(_) => warn!("unpublishing ticket from n0des timed out"),
            }
        }

        Ok(())
    }

    pub async fn set_proxy_enabled(&self, proxy_id: uuid::Uuid, enabled: bool) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.set_proxy_enabled(proxy_id, enabled).await
    }

    pub async fn update_proxy(&self, proxy: &TcpProxy) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.update_proxy(proxy).await
    }

    pub async fn connect(
        &self,
        codename: String,
    ) -> Result<(ConnectionInfo, (SendStream, RecvStream))> {
        // Ensure our endpoint is online before trying any n0des RPC or dial attempts.
        // (Avoid holding the mutex while awaiting.)
        let endpoint = { self.inner.lock().await.router.endpoint().clone() };
        endpoint.online().await;

        // Snapshot n0des client without holding the mutex across network IO.
        let n0des = { self.inner.lock().await.n0des.clone() };

        // For codename-based routing, n0des is required. Without it, a previously-established cached
        // connection could remain usable even after the tunnel is disabled/deleted (because we would
        // have no authoritative source of truth to consult). This is especially important for the gateway.
        let n0des = n0des.ok_or(anyerr!(
            "N0DES_API_SECRET is not set; cannot resolve codenames via n0des"
        ))?;

        // Always consult n0des first (when configured). This makes enable/disable semantics work:
        // when a tunnel is disabled it is unpublished from n0des, and the gateway should stop
        // routing to it even if it has a previously-established cached connection.
        // Keep this short; n0des-local should be fast and we don't want to hang the gateway
        // on every request. (Gateway has a larger overall timeout.)
        let ticket = tokio::time::timeout(
            Duration::from_secs(3),
            n0des.fetch_ticket::<TcpProxyTicket>(codename.clone()),
        )
        .await
        .map_err(|_| anyerr!("timed out fetching n0des ticket"))?
        .std_context("fetching n0des ticket")?
        .map(|ticket| ticket.ticket);

        let Some(ticket) = ticket else {
            // If this codename is not published, drop any cached connection and treat it as not found.
            let mut inner = self.inner.lock().await;
            if let Some(idx) = inner
                .edge_connections
                .iter()
                .position(|c| c.codename == codename)
            {
                let conn = inner.edge_connections.remove(idx);
                conn.streams.close();
            }
            return Err(anyerr!("codename not found"));
        };

        // If already connected, try to open a new stream set. If the cached tunnel has gone stale
        // (or the published ticket now points somewhere else), drop it and fall through to a fresh dial.
        if let Some((info, streams)) = {
            let mut inner = self.inner.lock().await;
            let idx = inner
                .edge_connections
                .iter()
                .position(|conn| conn.codename == codename);
            if let Some(idx) = idx {
                let mut mismatch = false;
                let remote = inner.edge_connections[idx].streams().remote_id();
                if remote != ticket.endpoint.id {
                    mismatch = true;
                    warn!(
                        codename = %codename,
                        cached = %remote.fmt_short(),
                        published = %ticket.endpoint.id.fmt_short(),
                        "connect: cached tunnel points at a different endpoint than published ticket; redialing"
                    );
                }

                if mismatch {
                    let conn = inner.edge_connections.remove(idx);
                    conn.streams.close();
                    None
                } else {
                    let info = inner.edge_connections[idx].info();
                    match tokio::time::timeout(
                        Duration::from_secs(3),
                        inner.edge_connections[idx].streams().new_streams(),
                    )
                    .await
                    {
                        Ok(Ok(streams)) => Some((info, streams)),
                        Ok(Err(err)) => {
                            warn!(
                                codename = %codename,
                                "connect: cached tunnel is stale (opening bidi stream failed: {err}); redialing"
                            );
                            let conn = inner.edge_connections.remove(idx);
                            conn.streams.close();
                            None
                        }
                        Err(_) => {
                            warn!(
                                codename = %codename,
                                "connect: cached tunnel is stale (timed out opening tunnel streams); redialing"
                            );
                            let conn = inner.edge_connections.remove(idx);
                            conn.streams.close();
                            None
                        }
                    }
                }
            } else {
                None
            }
        } {
            return Ok((info, streams));
        }

        // If we got here, we need to dial. This requires a ticket (unless the caller already had
        // an existing connection, which we handled above).
        info!(
            codename = %codename,
            endpoint = %ticket.endpoint.id.fmt_short(),
            "connect: dialing tunnel endpoint"
        );
        // Dial the endpoint + open streams (timeouts keep gateway from hanging).
        let mut inner = self.inner.lock().await;
        let conn = tokio::time::timeout(Duration::from_secs(3), inner.connect(codename, ticket))
            .await
            .map_err(|_| anyerr!("timed out connecting to tunnel endpoint"))??;
        let info = conn.info();
        let streams = tokio::time::timeout(Duration::from_secs(3), conn.streams().new_streams())
            .await
            .map_err(|_| anyerr!("timed out opening tunnel streams"))??;
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
                let n0des = inner.n0des.as_deref().ok_or(anyerr!(
                    "N0DES_API_SECRET is not set; cannot resolve codenames via n0des"
                ))?;
                let ticket = n0des
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
    repo: Arc<Repo>,
    router: Router,
    n0des: Option<Arc<iroh_n0des::Client>>,
    // TODO(b5) - use datum client
    #[allow(dead_code)]
    datum: DatumCloudClient,
    /// direct connections to another iroh endpoint, skipping the datum network
    edge_connections: Vec<Connection>,
    metrics_events: tokio::sync::broadcast::Sender<Metrics>,
    _metrics_task: AbortOnDropHandle<()>,
}

impl NodeInner {
    async fn new(secret_key: SecretKey, repo: Arc<Repo>) -> anyhow::Result<Self> {
        let config = repo.config().await?;
        let endpoint =
            build_endpoint(secret_key, &config, vec![IROH_HTTP_CONNECT_ALPN.to_vec()]).await?;

        let auth = repo.auth().await?;
        let tunnel_listener = TunnelListener::new(auth)?;

        // If N0DES_API_SECRET isn't configured, we can still run in "local-only"
        // mode (no publishing/fetching tickets via n0des).
        let n0des = match std::env::var("N0DES_API_SECRET") {
            Ok(_) => Some(
                iroh_n0des::Client::builder(&endpoint)
                    .api_secret_from_env()?
                    .build()
                    .await
                    .std_context("constructing n0des client")?,
            ),
            Err(VarError::NotPresent) => {
                info!("N0DES_API_SECRET not set; running without n0des integration");
                None
            }
            Err(VarError::NotUnicode(_)) => {
                return Err(anyhow!("N0DES_API_SECRET is not valid unicode"))
            }
        };

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

        let datum = DatumCloudClient::new(None);
        let inner = NodeInner {
            repo,
            router,
            datum,
            n0des: n0des.map(Arc::new),
            edge_connections: Vec::new(),
            metrics_events,
            _metrics_task: AbortOnDropHandle::new(metrics_task),
        };

        Ok(inner)
    }

    async fn announce_proxies(&self) -> Result<()> {
        let Some(n0des) = self.n0des.as_deref() else {
            // Local-only mode: nothing to publish.
            return Ok(());
        };
        let endpoint = self.router.endpoint();
        endpoint.online().await;
        let endpoint_addr = endpoint.addr();
        let state = self.repo.load_state().await?;
        try_join_all(
            state
                .tcp_proxies
                .iter()
                .filter(|proxy| proxy.enabled)
                .map(|proxy| async { announce_proxy(n0des, endpoint_addr.clone(), proxy).await }),
        )
        .await?;

        Ok(())
    }

    pub async fn listen(&mut self, label: String, addr: String) -> Result<()> {
        let mut state = self.repo.load_state().await?;
        self.router.endpoint().online().await;

        info!("creating proxy for address {}", addr.clone());
        let (host, port) = parse_host_port(&addr)?;
        let label = match label.trim() {
            "" => None,
            s => Some(s.to_string()),
        };
        let proxy = TcpProxy::new(host, port, label);

        // TODO - validate we don't already have a listener for this host/port combo

        state.tcp_proxies.push(proxy);
        self.repo.write_state(&state).await?;

        // Best-effort publishing: do not block tunnel creation on n0des being slow/unavailable.
        if let Some(n0des) = self.n0des.as_deref() {
            let endpoint_addr = self.router.endpoint().addr();
            let proxy = state.tcp_proxies.last().expect("just pushed proxy");
            match tokio::time::timeout(Duration::from_secs(2), announce_proxy(n0des, endpoint_addr, proxy)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => warn!("publishing ticket to n0des failed: {err}"),
                Err(_) => warn!("publishing ticket to n0des timed out"),
            }
        } else {
            info!(
                "N0DES_API_SECRET not set; created proxy locally but did not publish ticket"
            );
        }

        Ok(())
    }

    pub async fn unlisten(&mut self, info: &TcpProxy) -> Result<()> {
        let mut state = self.repo.load_state().await?;
        if let Some(proxy) = state.tcp_proxies.iter_mut().find(|p| p.id == info.id) {
            proxy.enabled = false;
        }
        // Best-effort: unpublish from n0des so the codename stops resolving.
        if let Some(n0des) = self.n0des.as_deref() {
            let _ = n0des
                .unpublish_ticket::<TcpProxyTicket>(info.codename.clone())
                .await;
        }
        self.repo.write_state(&state).await?;
        Ok(())
    }

    pub async fn update_proxy(&mut self, updated_proxy: &TcpProxy) -> Result<()> {
        let mut state = self.repo.load_state().await?;

        // Find and update the proxy with matching ID
        if let Some(proxy) = state.tcp_proxies.iter_mut().find(|p| p.id == updated_proxy.id) {
            proxy.label = updated_proxy.label.clone();
            proxy.host = updated_proxy.host.clone();
            proxy.port = updated_proxy.port;
            proxy.enabled = updated_proxy.enabled;
            // Note: codename and id should not change
        }

        self.repo.write_state(&state).await?;
        Ok(())
    }

    pub async fn set_proxy_enabled(&mut self, proxy_id: uuid::Uuid, enabled: bool) -> Result<()> {
        let mut state = self.repo.load_state().await?;
        let Some(idx) = state.tcp_proxies.iter().position(|p| p.id == proxy_id) else {
            return Ok(());
        };

        if state.tcp_proxies[idx].enabled == enabled {
            return Ok(());
        }

        state.tcp_proxies[idx].enabled = enabled;
        let codename = state.tcp_proxies[idx].codename.clone();
        let proxy_snapshot = state.tcp_proxies[idx].clone();

        // Persist the state change first so the UI toggle always reflects the user's intent,
        // even if n0des is slow/unavailable.
        self.repo.write_state(&state).await?;

        // Best-effort publish/unpublish to n0des. Do not fail the toggle if this errors.
        if let Some(n0des) = self.n0des.as_deref() {
            if enabled {
                match tokio::time::timeout(
                    Duration::from_secs(2),
                    announce_proxy(n0des, self.router.endpoint().addr(), &proxy_snapshot),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => warn!("publishing ticket to n0des failed: {err}"),
                    Err(_) => warn!("publishing ticket to n0des timed out"),
                }
            } else {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    n0des.unpublish_ticket::<TcpProxyTicket>(codename),
                )
                .await;
            }
        }

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
    local_endpoint: EndpointAddr,
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
