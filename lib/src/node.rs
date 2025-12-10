use anyhow::anyhow;
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_n0des::Client;
use iroh_tickets::endpoint::EndpointTicket;
use n0_error::{Result, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use n0_future::try_join_all;
use std::fmt::Debug;
use std::vec;
use std::{net::ToSocketAddrs, sync::Arc};
use tokio::sync::Mutex;
use tracing::{debug, info, trace};
use uuid::Uuid;

use iroh_proxy_utils::http_connect::{
    AuthHandler, HttpConnectEntranceHandle, HttpConnectListenerHandle, IROH_HTTP_CONNECT_ALPN,
};

use crate::datum_cloud::DatumCloudClient;
use crate::state::{ConnectionInfo, ListnerInfo, TcpProxy, TcpProxyTicket};
use crate::{Repo, auth::Auth, config::Config};

#[derive(Debug, Clone)]
pub struct Node {
    id: EndpointId,
    inner: Arc<Mutex<NodeInner>>,
}

impl Node {
    pub async fn new(secret_key: SecretKey, repo: Repo) -> Result<Self> {
        // TODO(b5) - add auth string
        let inner = NodeInner::new(secret_key, repo).await?;
        let ep_id = inner.endpoint.id();

        Ok(Self {
            id: ep_id,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    pub fn endpoint_id(&self) -> String {
        self.id.fmt_short().to_string()
    }

    pub async fn proxies(&self) -> Result<Vec<TcpProxy>> {
        let inner = self.inner.lock().await;
        let proxies = inner
            .tcp_listeners
            .iter()
            .map(|proxy| proxy.info.clone())
            .collect();
        Ok(proxies)
    }

    pub async fn start_listening(&self, _label: String, port: String) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.listen(port).await
    }

    pub async fn stop_listening(&self, proxy: &TcpProxy) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.unlisten(proxy).await
    }

    pub async fn connect_codename(
        &self,
        codename: String,
        listen_addr: String,
    ) -> Result<ConnectionInfo> {
        let mut inner = self.inner.lock().await;
        if let Some(conn) = inner
            .edge_connections
            .iter()
            .find(|c| c.codename == codename)
        {
            return Ok(conn.info());
        }
        println!("need to connect");
        inner.connect_codename(codename, listen_addr).await
    }

    pub async fn connect(
        &self,
        codename: String,
        addrs: String,
        ticket: Option<EndpointTicket>,
    ) -> Result<ConnectionInfo> {
        let mut inner = self.inner.lock().await;
        inner.connect(codename, addrs, ticket).await
    }

    pub async fn metrics(&self) -> Result<tokio::sync::broadcast::Receiver<Metrics>> {
        let sub = self.inner.lock().await.metrics_events.subscribe();
        Ok(sub)
    }
}

#[derive(Debug)]
struct NodeInner {
    repo: Repo,
    endpoint: Endpoint,
    n0des: iroh_n0des::Client,
    auth: Auth,
    datum: DatumCloudClient,
    /// the main TCP iroh endpoint listener that accepts connections
    tcp_listeners: Vec<ProxyListener>,
    /// direct connections to another iroh endpoint, skipping the datum network
    edge_connections: Vec<Connection>,
    metrics_events: tokio::sync::broadcast::Sender<Metrics>,
    _metrics_task: AbortOnDropHandle<()>,
}

impl NodeInner {
    async fn new(secret_key: SecretKey, repo: Repo) -> anyhow::Result<Self> {
        let config = repo.config().await?;
        let auth = repo.auth().await?;
        let endpoint =
            create_endpoint(secret_key, &config, vec![IROH_HTTP_CONNECT_ALPN.to_vec()]).await?;

        let n0des = iroh_n0des::Client::builder(&endpoint)
            .api_secret_from_env()
            // TODO(b5) - remove expect
            .expect("failed to read api secret from env")
            .build()
            .await
            .std_context("construction n0des client")?;

        let datum = DatumCloudClient::new(None);
        let tcp_listeners = load_proxies(&endpoint, &repo, &n0des).await?;

        let (tx, _) = tokio::sync::broadcast::channel(32);
        let metrics_events = tx.clone();
        let metrics = endpoint.metrics().clone();
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

        let inner = NodeInner {
            repo,
            auth,
            endpoint,
            datum,
            n0des,
            tcp_listeners,
            edge_connections: Vec::new(),
            metrics_events,
            _metrics_task: AbortOnDropHandle::new(metrics_task),
        };

        Ok(inner)
    }

    pub async fn listen(&mut self, addr: String) -> Result<()> {
        let mut state = self.repo.load_state().await?;
        self.endpoint.online().await;

        info!("creating proxy for address {}", addr.clone());
        let proxy = TcpProxy::new(addr);

        // TODO - validate we don't already have a listener for that port

        let listener =
            open_and_publish_tcp_proxy_listeners(&self.endpoint, &self.auth, &proxy, &self.n0des)
                .await?;

        self.tcp_listeners.push(listener);
        self.repo.write_state(&state).await?;

        Ok(())
    }

    pub async fn unlisten(&mut self, info: &TcpProxy) -> Result<()> {
        self.tcp_listeners.retain(|proxy| proxy.info.id != info.id);
        let mut state = self.repo.load_state().await?;
        state.tcp_proxies = self
            .tcp_listeners
            .iter()
            .map(|proxy| proxy.info.clone())
            .collect();
        self.repo.write_state(&state).await?;
        Ok(())
    }

    pub async fn connections(&self) -> Vec<ConnectionInfo> {
        self.edge_connections.iter().map(|l| l.info()).collect()
    }

    pub async fn connect_codename(
        &mut self,
        codename: String,
        addr: String,
    ) -> Result<ConnectionInfo> {
        let res = self
            .edge_connections
            .iter()
            .find(|conn| conn.codename == codename);
        if let Some(res) = res {
            return Ok(res.info());
        }

        println!("getting ticket");
        info!("getting ticket");
        let ticket = self
            .n0des
            .fetch_ticket::<TcpProxyTicket>(codename.clone())
            .await
            .std_context("fetching n0des ticket")?;
        let Some(ticket) = ticket else {
            let err = n0_error::AnyError::from_anyhow(anyhow!("codename not found"));
            return Err(err);
        };
        info!("have ticket {:?}", ticket);

        let ticket_addr = EndpointAddr::new(ticket.ticket.endpoint);
        let ticket = EndpointTicket::new(ticket_addr);

        self.connect(codename, addr, Some(ticket)).await
    }

    pub async fn connect(
        &mut self,
        codename: String,
        addrs: String,
        ticket: Option<EndpointTicket>,
    ) -> Result<ConnectionInfo> {
        let addr_string = addrs.clone();
        let addrs = addrs
            .to_socket_addrs()
            .std_context(format!("invalid host string {}", addrs))?;

        let endpoint = self.endpoint.clone();
        let handle = HttpConnectEntranceHandle::connect(endpoint, addrs).await?;
        let conn = Connection {
            id: Uuid::new_v4(),
            codename,
            addr: addr_string,
            target: ticket,
            _handle: handle,
        };
        let info = conn.info();
        self.edge_connections.push(conn);
        Ok(info)
    }

    pub async fn disconnect(&mut self, conn: &ConnectionInfo) -> anyhow::Result<()> {
        let mut found = false;
        debug!("disconnect tcp. id: {:?}", conn.id);
        self.edge_connections.retain(|h| {
            if h.id == conn.id {
                h._handle.close();
                found = true;
                false
            } else {
                true
            }
        });
        match found {
            true => Ok(()),
            false => Err(anyhow!("TCP connection not found")),
        }
    }
}

#[derive(Debug)]
pub struct ProxyListener {
    pub info: TcpProxy,
    handle: HttpConnectListenerHandle,
}

async fn load_proxies(
    endpoint: &Endpoint,
    repo: &Repo,
    n0des: &iroh_n0des::Client,
) -> Result<Vec<ProxyListener>> {
    endpoint.online().await;
    let auth = repo.auth().await?;
    let state = repo.load_state().await?;
    let listeners = try_join_all(state.tcp_proxies.iter().map(|proxy| async {
        open_and_publish_tcp_proxy_listeners(endpoint, &auth, proxy, n0des).await
    }))
    .await?;
    Ok(listeners)
}

async fn open_and_publish_tcp_proxy_listeners(
    endpoint: &Endpoint,
    auth: &Auth,
    proxy: &TcpProxy,
    n0des: &iroh_n0des::Client,
) -> Result<ProxyListener> {
    let auth: Arc<Box<dyn AuthHandler>> = Arc::new(Box::new(auth.clone()));
    let handle = HttpConnectListenerHandle::listen(endpoint.clone(), Some(auth)).await?;

    let ticket = proxy.ticket(endpoint.id());
    n0des
        .publish_ticket(proxy.codename.clone(), ticket)
        .await
        .std_context("publishing ticket to n0des")?;

    debug!("published ticket to n0des. codename: {}", proxy.codename);

    Ok(ProxyListener {
        info: proxy.clone(),
        handle,
    })
}

#[derive(Debug, Default, Clone)]
pub struct Metrics {
    pub send: u64,
    pub recv: u64,
}

#[derive(Debug)]
pub struct Connection {
    id: Uuid,
    codename: String,
    addr: String,
    // TODO - currently this ticket isn't being used. It should be pushed
    // into the HttpConnectEntranceHandle as a param that always directs
    // the tunnel at the same endpoint
    target: Option<EndpointTicket>,
    _handle: HttpConnectEntranceHandle,
}

impl Connection {
    fn ticket(&self) -> &Option<EndpointTicket> {
        &self.target
    }

    fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            id: self.id,
            codename: self.codename.clone(),
            addr: self.addr.clone(),
            ticket: self.ticket().clone(),
        }
    }
}

#[derive(Debug)]
struct Listener {
    id: Uuid,
    label: String,
    handle: HttpConnectListenerHandle,
}

impl Listener {
    fn ticket(&self) -> EndpointTicket {
        let addr = self.handle.receiving().addr().clone();
        EndpointTicket::new(addr)
    }

    fn info(&self) -> ListnerInfo {
        ListnerInfo {
            id: self.id,
            label: self.label.clone(),
            ticket: self.ticket(),
        }
    }
}

/// Create a new iroh endpoint.
async fn create_endpoint(
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
