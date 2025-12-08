// use std::str::FromStr;

// use iroh::EndpointId;
// use iroh_tickets::{ParseError, Ticket};
// use n0_error::e;
// use serde::{Deserialize, Serialize};
// // use url::Url;

// use crate::auth::Protocol;

// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub struct ProxyTicket {
//     /// Authentication secret
//     pub secret: String,
//     /// Iroh endpoint addressing info
//     pub endpoint_id: EndpointId,
//     /// Target protocol (tcp/udp)
//     pub protocol: Protocol,
//     /// Target host
//     pub host: String,
//     /// Target port
//     pub port: u16,
// }

// impl FromStr for ProxyTicket {
//     type Err = ParseError;

//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         Ticket::deserialize(s)
//     }
// }

// impl Ticket for ProxyTicket {
//     const KIND: &'static str = "datum";

//     fn to_bytes(&self) -> Vec<u8> {
//         postcard::to_stdvec(self).expect("postcard serialization should not fail")
//     }

//     fn from_bytes(bytes: &[u8]) -> Result<Self, ParseError> {
//         postcard::from_bytes(bytes).map_err(|source| e!(ParseError::Postcard { source }))
//     }
// }

// /// Contains all information needed to establish a proxy connection. It can
// /// serialize to both a ticket an an iroh:// URI
// #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
// pub struct ProxyUri {
//     /// Authentication secret
//     pub secret: String,
//     /// Iroh endpoint addressing info
//     pub endpoint_id: EndpointId,
//     /// Target protocol (tcp/udp)
//     pub protocol: Protocol,
//     /// Target host
//     pub host: String,
//     /// Target port
//     pub port: u16,
// }

// impl From<ProxyTicket> for ProxyUri {
//     fn from(ticket: ProxyTicket) -> Self {
//         Self {
//             secret: ticket.secret,
//             endpoint_id: ticket.endpoint_id,
//             protocol: ticket.protocol,
//             host: ticket.host,
//             port: ticket.port,
//         }
//     }
// }

// impl ProxyUri {
//     pub fn new(
//         secret: String,
//         endpoint_id: EndpointId,
//         protocol: Protocol,
//         host: String,
//         port: u16,
//     ) -> Self {
//         Self {
//             secret,
//             endpoint_id,
//             protocol,
//             host,
//             port,
//         }
//     }

//     /// Convert to iroh:// URL for Envoy Backend configuration
//     pub fn to_envoy_url(&self, secret_name: &str, secret_key: &str) -> String {
//         format!(
//             "iroh://secret/{}/{}/{}/{}/{}",
//             secret_name,
//             self.protocol.as_str(),
//             self.host,
//             self.port
//         )
//     }

//     /// Parse from an iroh:// URL (for envoy-iroh-sidecar to resolve)
//     ///
//     /// The secret_resolver is called with (secret_name, secret_key) and should
//     /// return the ProxyTarget ticket stored in that k8s secret
//     pub fn from_envoy_url(
//         url_str: &str,
//         secret_resolver: impl FnOnce(&str, &str) -> Result<String, Box<dyn std::error::Error>>,
//     ) -> Result<Self, Box<dyn std::error::Error>> {
//         // Resolve the ticket from k8s secret
//         let ticket_str = secret_resolver(secret_name, secret_key)?;

//         // Deserialize the ticket
//         let mut proxy_target = ProxyTicket::from_str(&ticket_str)?;

//         // Validate that URL params match ticket params
//         let url_protocol = Protocol::from_str(protocol_str)?;
//         if proxy_target.protocol != url_protocol {
//             return Err(format!(
//                 "Protocol mismatch: URL has {:?}, ticket has {:?}",
//                 url_protocol, proxy_target.protocol
//             )
//             .into());
//         }

//         if proxy_target.host != host {
//             return Err(format!(
//                 "Host mismatch: URL has {}, ticket has {}",
//                 host, proxy_target.host
//             )
//             .into());
//         }

//         if proxy_target.port != port {
//             return Err(format!(
//                 "Port mismatch: URL has {}, ticket has {}",
//                 port, proxy_target.port
//             )
//             .into());
//         }

//         Ok(proxy_target.into())
//     }
// }

// impl std::fmt::Display for ProxyUri {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.to_string())
//     }
// }

// impl FromStr for ProxyUri {
//     type Err = ParseError;

//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         let url = Url::parse(s)?;

//         if url.scheme() != "iroh" {
//             return Err("URL must have 'iroh' scheme".into());
//         }

//         if url.host_str() != Some("secret") {
//             return Err("URL must have 'secret' host".into());
//         }

//         // Parse path: /secret/<name>/<key>/<proto>/<host>/<port>
//         let path_segments: Vec<&str> = url.path().trim_start_matches('/').split('/').collect();

//         if path_segments.len() != 5 {
//             return Err(format!(
//                 "Expected 5 path segments, got {}: {:?}",
//                 path_segments.len(),
//                 path_segments
//             )
//             .into());
//         }

//         let secret_name = path_segments[0];
//         let secret_key = path_segments[1];
//         let protocol_str = path_segments[2];
//         let host = path_segments[3];
//         let port: u16 = path_segments[4].parse()?;

//         Self {
//             secret: secret_name,
//         }
//     }
// }

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use iroh::EndpointId;

//     #[test]
//     fn test_ticket_roundtrip() {
//         let endpoint_id = EndpointId::from_bytes(&[1u8; 32]).unwrap();

//         let target = ProxyUri::new(
//             "my-secret-token".to_string(),
//             endpoint_id,
//             Protocol::Tcp,
//             "localhost".to_string(),
//             8080,
//         );

//         // Serialize to ticket string
//         let ticket_str = target.to_string();
//         assert!(ticket_str.starts_with("proxy"));

//         // Deserialize back
//         let parsed: ProxyUri = ticket_str.parse().unwrap();
//         assert_eq!(parsed, target);
//     }

//     #[test]
//     fn test_envoy_url() {
//         let endpoint_id = EndpointId::from_bytes(&[1u8; 32]).unwrap();

//         let target = ProxyUri::new(
//             "my-secret".to_string(),
//             endpoint_id,
//             Protocol::Tcp,
//             "localhost".to_string(),
//             1313,
//         );

//         let url = target.to_envoy_url("my-proxy", "token");
//         assert_eq!(url, "iroh://secret/my-proxy/token/tcp/localhost/1313");
//     }
// }
