use anyhow::anyhow;
use iroh::{Endpoint, PublicKey, SecretKey};
use iroh_tickets::endpoint::EndpointTicket;
use n0_error::{Result, StdResultExt};
use std::{net::ToSocketAddrs, sync::Arc};
use tokio::sync::Mutex;
use tracing::debug;
use uuid::Uuid;

use iroh_proxy_utils::http_connect::{
    AuthHandler, HttpConnectEntranceHandle, HttpConnectListenerHandle, IROH_HTTP_CONNECT_ALPN,
};

use crate::{auth::Auth, config::Config};

#[derive(Debug, Clone)]
pub struct Node {
    inner: Arc<NodeInner>,
}

#[derive(Debug)]
struct NodeInner {
    ep_id: PublicKey,
    endpoint: Endpoint,
    auth: Auth,
    listeners: Mutex<Vec<Listener>>,
    connections: Mutex<Vec<Connection>>,
}

impl Node {
    pub async fn new(secret_key: SecretKey, config: &Config, auth: Auth) -> Result<Self> {
        let endpoint =
            create_endpoint(secret_key, config, vec![IROH_HTTP_CONNECT_ALPN.to_vec()]).await?;

        // wait for the endpoint to figure out its address before making a ticket
        endpoint.online().await;
        let node_addr = endpoint.addr();

        Ok(Self {
            inner: Arc::new(NodeInner {
                auth,
                ep_id: node_addr.id,
                endpoint,
                listeners: Mutex::new(Vec::new()),
                connections: Mutex::new(Vec::new()),
            }),
        })
    }

    pub fn endpoint_id(&self) -> String {
        self.inner.ep_id.to_string()
    }

    pub async fn listeners(&self) -> Vec<ListnerInfo> {
        self.inner
            .listeners
            .lock()
            .await
            .iter()
            .map(|l| l.info())
            .collect()
    }

    pub async fn connections(&self) -> Vec<ConnectionInfo> {
        self.inner
            .connections
            .lock()
            .await
            .iter()
            .map(|l| l.info())
            .collect()
    }

    // TODO - this used to take a local port argument, pretty sure we need
    // to restore that
    pub async fn listen(&self, label: String) -> Result<EndpointTicket> {
        let auth: Arc<Box<dyn AuthHandler>> = Arc::new(Box::new(self.inner.auth.clone()));
        let handle =
            HttpConnectListenerHandle::listen(self.inner.endpoint.clone(), Some(auth)).await?;
        let id = Uuid::new_v4();
        let listener = Listener { id, label, handle };
        let ticket = listener.ticket();
        let mut tcp_listeners = self.inner.listeners.lock().await;
        tcp_listeners.push(listener);
        Ok(ticket)
    }

    pub async fn unlisten(&self, lstn: &ListnerInfo) -> anyhow::Result<()> {
        let mut lstrs = self.inner.listeners.lock().await;
        let mut found = false;
        debug!("unlisten tcp. id: {:?}", lstn.id);
        lstrs.retain(|l| {
            if l.id == lstn.id {
                l.handle.close();
                found = true;
                false
            } else {
                true
            }
        });
        match found {
            true => Ok(()),
            false => Err(anyhow!("TCP listener not found")),
        }
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

        let endpoint = self.inner.endpoint.clone();
        let handle = HttpConnectEntranceHandle::connect(endpoint, addrs).await?;
        let conn = Connection {
            id: Uuid::new_v4(),
            label,
            addr: addr_string,
            target: ticket,
            handle,
        };
        let mut tcp_connections = self.inner.connections.lock().await;
        tcp_connections.push(conn);
        Ok(())
    }

    pub async fn disconnect(&self, conn: &ConnectionInfo) -> anyhow::Result<()> {
        let mut conns = self.inner.connections.lock().await;
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

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionInfo {
    pub id: Uuid,
    pub label: String,
    pub addr: String,
    pub ticket: Option<EndpointTicket>,
}

#[derive(Debug)]
struct Connection {
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

#[derive(Debug, Clone, PartialEq)]
pub struct ListnerInfo {
    pub id: Uuid,
    pub label: String,
    pub ticket: EndpointTicket,
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
