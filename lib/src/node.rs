use anyhow::anyhow;
use iroh::{Endpoint, EndpointId, SecretKey};
use iroh_tickets::endpoint::EndpointTicket;
use n0_error::{Result, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use std::fmt::Debug;
use std::{net::ToSocketAddrs, sync::Arc};
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

use iroh_proxy_utils::http_connect::{
    AuthHandler, HttpConnectEntranceHandle, HttpConnectListenerHandle, IROH_HTTP_CONNECT_ALPN,
};

use crate::datum_cloud::DatumCloudClient;
use crate::state::{ConnectionInfo, HttpProxy, ListnerInfo, Project, State};
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

    pub async fn start_listening(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.listen().await
    }

    pub async fn stop_listening(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.unlisten().await
    }

    pub async fn connect(
        &self,
        label: String,
        addrs: String,
        ticket: Option<EndpointTicket>,
    ) -> Result<()> {
        let inner = self.inner.lock().await;
        inner.connect(label, addrs, ticket).await
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
    tcp_listener: Option<HttpConnectListenerHandle>,
    /// direct connections to another iroh endpoint, skipping the datum network
    edge_connections: Mutex<Vec<Connection>>,
    metrics_events: tokio::sync::broadcast::Sender<Metrics>,
    metrics_task: AbortOnDropHandle<()>,
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
                    warn!("send metrics on channel error: {:?}", err);
                }
                n0_future::time::sleep(n0_future::time::Duration::from_secs(2)).await;
            }
        });

        let inner = NodeInner {
            repo,
            auth,
            endpoint,
            datum,
            n0des,
            tcp_listener: None,
            edge_connections: Mutex::new(Vec::new()),
            metrics_events,
            metrics_task: AbortOnDropHandle::new(metrics_task),
        };

        Ok(inner)
    }

    // TODO - this used to take a local port argument, pretty sure we need
    // to restore that
    pub async fn listen(&mut self) -> Result<()> {
        self.endpoint.online().await;
        let auth: Arc<Box<dyn AuthHandler>> = Arc::new(Box::new(self.auth.clone()));
        let listener = HttpConnectListenerHandle::listen(self.endpoint.clone(), Some(auth)).await?;
        self.tcp_listener = Some(listener);
        Ok(())
    }

    pub async fn unlisten(&mut self) -> Result<()> {
        match &self.tcp_listener {
            Some(listener) => {
                listener.close();
                self.tcp_listener = None;
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub async fn connections(&self) -> Vec<ConnectionInfo> {
        self.edge_connections
            .lock()
            .await
            .iter()
            .map(|l| l.info())
            .collect()
    }

    pub async fn connect(
        &self,
        label: String,
        addrs: String,
        ticket: Option<EndpointTicket>,
    ) -> Result<()> {
        let addr_string = addrs.clone();
        let addrs = addrs
            .to_socket_addrs()
            .std_context(format!("invalid host string {}", addrs))?;

        let endpoint = self.endpoint.clone();
        let handle = HttpConnectEntranceHandle::connect(endpoint, addrs).await?;
        let conn = Connection {
            id: Uuid::new_v4(),
            label,
            addr: addr_string,
            target: ticket,
            handle,
        };
        let mut tcp_connections = self.edge_connections.lock().await;
        tcp_connections.push(conn);
        Ok(())
    }

    pub async fn disconnect(&self, conn: &ConnectionInfo) -> anyhow::Result<()> {
        let mut conns = self.edge_connections.lock().await;
        let mut found = false;
        debug!("disconnect tcp. id: {:?}", conn.id);
        conns.retain(|h| {
            if h.id == conn.id {
                h.handle.close();
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

#[derive(Debug, Default, Clone)]
pub struct Metrics {
    pub send: u64,
    pub recv: u64,
}

#[derive(Debug)]
pub struct Connection {
    id: Uuid,
    label: String,
    addr: String,
    // TODO - currently this ticket isn't being used. It should be pushed
    // into the HttpConnectEntranceHandle as a param that always directs
    // the tunnel at the same endpoint
    target: Option<EndpointTicket>,
    handle: HttpConnectEntranceHandle,
}

impl Connection {
    fn ticket(&self) -> &Option<EndpointTicket> {
        &self.target
    }

    fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            id: self.id,
            label: self.label.clone(),
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
