use std::{collections::HashMap, path::PathBuf};

use iroh_tickets::endpoint::EndpointTicket;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::Protocol;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    me: Option<User>,
    org: Option<Organization>,
    project: Option<Project>,
    connector: Connector,
    advertisements: Vec<Advertisement>,
    proxies: Vec<HttpProxy>,
}

impl State {
    pub(crate) async fn from_file(path: PathBuf) -> anyhow::Result<Self> {
        let data = tokio::fs::read(path).await?;
        let state: State = serde_yml::from_slice(&data)?;
        Ok(state)
    }

    pub(crate) async fn write_to_file(&self, path: PathBuf) -> anyhow::Result<()> {
        let data = serde_yml::to_string(&path)?;
        tokio::fs::write(&path, &data).await?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct User {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Organization {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Project {}

const DATUM_CONNECTOR_CLASS: &'static str = "datum-connect";

/// Datum Connectors introduce a method to model outbound connectivity as
/// first class resources in the control plane. A Connector resource lives in a
/// project and captures what kinds of network access it can facilitate, while
/// ConnectorAdvertisement resources describe the networks and endpoints that
/// are reachable through that connector.
///
/// There is one connector for each datum connect app instance.
///
/// Other platform components, such as proxies, can then target backends via
/// these connectors, allowing the control plane to reason about and route
/// traffic to remote or otherwise inaccessible environments.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Connector {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Advertisement {
    layer3: Option<AdvertisementLayer3>,
    layer4: Option<AdvertisementLayer4>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdvertisementLayer3 {
    name: Option<String>,
    cidrs: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdvertisementLayer4 {
    name: Option<String>,
    services: Vec<Layer4ServiceAddress>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Layer4ServiceAddress {
    // TODO - make this an IPv4, IPv6, or a DNS address type
    address: String,
    ports: Vec<Port>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Port {
    name: Option<String>,
    port: u32,
    protocol: Protocol,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpProxy {
    backends: Vec<HttpProxyBackend>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpProxyBackend {
    // TODO - change from string -> url
    endpoint: String,
    connector: ConnectorRef,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectorRef {
    name: String,
    selector: Option<Selector>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Selector {
    match_labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionInfo {
    pub id: Uuid,
    pub label: String,
    pub addr: String,
    pub ticket: Option<EndpointTicket>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListnerInfo {
    pub id: Uuid,
    pub label: String,
    pub ticket: EndpointTicket,
}
