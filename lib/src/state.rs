use std::{collections::HashMap, path::PathBuf, str::FromStr};

use iroh::EndpointId;
use iroh_tickets::{ParseError, Ticket, endpoint::EndpointTicket};
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
    // this is a shim we're using for now while we stand up datum control plane mgmt
    pub tcp_proxies: Vec<TcpProxy>,
}

impl State {
    pub(crate) async fn from_file(path: PathBuf) -> anyhow::Result<Self> {
        let data = tokio::fs::read(path).await?;
        let state: State = serde_yml::from_slice(&data)?;
        Ok(state)
    }

    pub(crate) async fn write_to_file(&self, path: PathBuf) -> anyhow::Result<()> {
        let data = serde_yml::to_string(&self)?;
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
    pub codename: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListnerInfo {
    pub id: Uuid,
    pub label: String,
    pub ticket: EndpointTicket,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TcpProxy {
    pub id: Uuid,
    pub codename: String,
    pub host: String,
    pub port: u16,
}

impl TcpProxy {
    pub fn new(host: String, port: u16) -> Self {
        let id = Uuid::new_v4();
        let codename = generate_codename(id);
        TcpProxy {
            id,
            codename,
            host,
            port,
        }
    }

    pub fn ticket(&self, endpoint: EndpointId) -> TcpProxyTicket {
        TcpProxyTicket {
            id: self.id,
            endpoint,
            host: self.host.clone(),
            port: self.port,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TcpProxyTicket {
    pub id: Uuid,
    pub endpoint: EndpointId,
    pub host: String,
    pub port: u16,
}

impl FromStr for TcpProxyTicket {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        iroh_tickets::Ticket::deserialize(s)
    }
}

impl Ticket for TcpProxyTicket {
    const KIND: &'static str = "datum";

    fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(&self).expect("serialize should work")
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, iroh_tickets::ParseError> {
        let ticket: Self = postcard::from_bytes(bytes)?;
        Ok(ticket)
    }
}

pub(crate) fn generate_codename(id: Uuid) -> String {
    const ADJECTIVES: &[&str] = &[
        "amber", "bold", "calm", "dark", "eager", "fair", "gentle", "happy", "icy", "jolly",
        "kind", "light", "merry", "noble", "proud", "quiet", "rapid", "smart", "tall", "warm",
        "wise", "young", "zealous", "bright", "clever", "deep", "fast", "grand", "keen", "loud",
        "mild", "neat", "odd", "pale", "quick", "rich", "safe", "tame", "vast", "wild", "brave",
        "clean", "crisp", "dull", "free", "glad", "cool", "fresh", "pure", "sharp",
    ];

    const COLORS: &[&str] = &[
        "azure", "beige", "coral", "cream", "cyan", "ebony", "gold", "gray", "green", "indigo",
        "ivory", "jade", "khaki", "lemon", "lime", "navy", "olive", "orange", "peach", "pearl",
        "pink", "plum", "rose", "ruby", "rust", "sand", "silver", "snow", "tan", "teal", "violet",
        "white", "amber", "bronze", "brown", "cherry", "copper", "crimson", "yellow", "maroon",
        "mint", "scarlet", "slate", "steel", "taupe", "blue", "red", "purple", "black",
    ];

    const NOUNS: &[&str] = &[
        "anchor", "bridge", "canyon", "delta", "echo", "forest", "glacier", "harbor", "island",
        "jungle", "lagoon", "meadow", "nebula", "ocean", "peak", "river", "storm", "thunder",
        "valley", "wave", "zenith", "aurora", "breeze", "cloud", "dawn", "ember", "fjord", "grove",
        "hawk", "inlet", "knight", "lotus", "moon", "nova", "oak", "pine", "quartz", "ridge",
        "star", "tiger", "vortex", "whale", "axis", "beacon", "comet", "dune", "eagle", "flare",
        "gem", "stream",
    ];

    let bytes = id.as_bytes();

    // Convert first 6 bytes into three 16-bit values
    // This gives us values in range 0-65535
    let val1 = u16::from_be_bytes([bytes[0], bytes[1]]);
    let val2 = u16::from_be_bytes([bytes[2], bytes[3]]);
    let val3 = u16::from_be_bytes([bytes[4], bytes[5]]);

    let idx1 = (val1 as usize) % ADJECTIVES.len();
    let idx2 = (val2 as usize) % COLORS.len();
    let idx3 = (val3 as usize) % NOUNS.len();

    format!("{}-{}-{}", ADJECTIVES[idx1], COLORS[idx2], NOUNS[idx3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_codename() {
        // Test with a known UUID
        let id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let codename = generate_codename(id);

        // Should always produce the same codename for the same UUID
        assert_eq!(codename, generate_codename(id));

        // Should have format: word-word-word
        let parts: Vec<&str> = codename.split('-').collect();
        assert_eq!(parts.len(), 3, "Codename should have 3 parts");

        // Each part should be non-empty
        assert!(!parts[0].is_empty());
        assert!(!parts[1].is_empty());
        assert!(!parts[2].is_empty());
    }

    #[test]
    fn test_generate_codename_different_uuids() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        let codename1 = generate_codename(id1);
        let codename2 = generate_codename(id2);

        assert_ne!(codename1, codename2);
    }

    #[test]
    fn test_tcp_proxy_has_codename() {
        let proxy = TcpProxy::new("127.0.0.1".to_string(), 8080);
        let codename = &proxy.codename;

        let parts: Vec<&str> = codename.split('-').collect();
        assert_eq!(parts.len(), 3);
    }
}
