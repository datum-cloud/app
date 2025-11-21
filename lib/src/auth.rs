use std::{path::PathBuf, pin::Pin};

use anyhow::Context;
use iroh::EndpointId;
use iroh_proxy_utils::{
    error::AuthError,
    http_connect::{AuthHandler, Request},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum Protocol {
    Tcp,
    Udp,
    Sctp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum IntOrString {
    Int(u32),
    String(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkPolicyPort {
    /// protocol represents the protocol (TCP, UDP, or SCTP) which traffic must match.
    /// If not specified, this field defaults to TCP.
    protocol: Option<Protocol>,

    /// port represents the port on the given protocol. This can either be a numerical or named
    /// port on an instance. If this field is not provided, this matches all port names and
    // numbers.
    /// If present, only traffic on the specified protocol AND port will be matched.
    port: Option<IntOrString>,

    /// endPort indicates that the range of ports from port to endPort if set, inclusive,
    /// should be allowed by the policy. This field cannot be defined if the port field
    /// is not defined or if the port field is defined as a named (string) port.
    /// The endPort must be equal or greater than port.
    #[serde(rename = "endPort")]
    end_port: Option<i32>,
}

impl Default for NetworkPolicyPort {
    fn default() -> Self {
        Self {
            protocol: Some(Protocol::Tcp),
            port: Some(IntOrString::String("*".to_string())),
            end_port: None,
        }
    }
}

impl NetworkPolicyPort {
    fn allows(&self, port: u32) -> bool {
        match self.port {
            Some(ref pos) => match pos {
                IntOrString::String(_s) => {
                    todo!("finish support for string ports in config");
                }
                IntOrString::Int(i) => *i == port,
            },
            None => true,
        }
    }
}

/// NetworkPolicyPeer describes a peer to allow traffic to/from. Only certain combinations of
/// fields are allowed
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkPolicyPeer {
    /// ipBlock defines policy on a particular IPBlock. If this field is set then
    /// neither of the other fields can be.
    #[serde(rename = "ipBlock")]
    ip_block: Option<IpBlock>,
    /// endpoint specifies the iroh endpoint identifier that will be allowed
    #[serde(rename = "endpoint")]
    endpoint: Option<EndpointId>,
}

impl NetworkPolicyPeer {
    fn allows(&self, _id: &EndpointId) -> bool {
        return true;
    }
}

/// IpBlock describes a particular CIDR (Ex. "192.168.1.0/24","2001:db8::/64")
/// that is allowed to the targets matched by a network policy. The except entry
/// describes CIDRs that should not be included within this rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IpBlock {
    /// cidr is a string representing the IPBlock
    /// Valid examples are "192.168.1.0/24" or "2001:db8::/64"
    cidr: String,

    // except is a slice of CIDRs that should not be included within an IPBlock
    // Valid examples are "192.168.1.0/24" or "2001:db8::/64"
    // Except values will be rejected if they are outside the cidr range
    //
    // +listType=atomic
    except: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    /// ports is a list of ports which should be made accessible on the instances selected for
    /// this rule. Each item in this list is combined using a logical OR. If this field is
    /// empty or missing, this rule matches all ports (traffic not restricted by port).
    /// If this field is present and contains at least one item, then this rule allows
    /// traffic only if the traffic matches at least one port in the list.
    ///
    /// +listType=atomic
    ports: Option<Vec<NetworkPolicyPort>>,

    /// from is a list of sources which should be able to access the instances selected for this rule.
    /// Items in this list are combined using a logical OR operation. If this field is
    /// empty or missing, this rule matches all sources (traffic not restricted by
    /// source). If this field is present and contains at least one item, this rule
    /// allows traffic only if the traffic matches at least one item in the from list.
    ///
    /// +listType=atomic
    from: Option<Vec<NetworkPolicyPeer>>,
}

impl AuthHandler for Auth {
    fn authorize<'a>(
        &'a self,
        req: &'a Request,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AuthError>> + Send + 'a>> {
        let res = self.allows_req(req);
        Box::pin(async move {
            let res: Result<bool, AuthError> = Ok(res);
            res
        })
    }
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            ports: None,
            from: None,
        }
    }
}

impl Auth {
    pub async fn from_file(path: PathBuf) -> anyhow::Result<Self> {
        let config = tokio::fs::read_to_string(path)
            .await
            .context("reading auth file")?;
        let config = serde_yml::from_str(&config).context("parsing auth file")?;
        Ok(config)
    }

    pub async fn write(&self, path: PathBuf) -> anyhow::Result<()> {
        let data = serde_yml::to_string(self)?;
        tokio::fs::write(path, data).await?;
        Ok(())
    }

    fn allows_req<'a>(&self, req: &'a Request) -> bool {
        match req {
            Request::Connect(connect_req) => {
                // if !self.allows_endpoint(connect_req.endpoint_addr) {
                //     return false;
                // }
                if !self.allows_port(connect_req.port) {
                    return false;
                }
                false
            }
            Request::Http(_http_req) => {
                // TODO - finish
                // if !self.allows_port(http_req.path) {
                //     return Ok(false)
                // }
                return true;
            }
        }
    }

    fn allows_port(&self, port: u16) -> bool {
        if let Some(ref ports) = self.ports {
            return ports.iter().any(|policy| policy.allows(port as u32));
        }
        false
    }

    #[allow(unused)]
    fn allows_endpoint(&self, id: &EndpointId) -> bool {
        if let Some(ref peers) = self.from {
            return peers.iter().any(|peer_policy| peer_policy.allows(id));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use iroh_proxy_utils::http_connect::{AuthHandler, Request};
    use serde::Deserialize;

    use crate::auth::Auth;

    #[tokio::test]
    async fn test_auth_smoke() {
        let no_auth = Auth::default();
        let request = b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n";
        let req = Request::parse(request).unwrap();
        let got = no_auth.authorize(&req).await.unwrap();

        assert_eq!(got, false);
    }

    #[derive(Deserialize)]
    struct Fixture {
        request: String,
        allow: Option<bool>,
        error: Option<String>,
    }

    #[tokio::test]
    async fn test_auth_fixtures() {
        let auth = include_str!("../tests/auth/01_tcp_443.config.yml");
        let fixtures = include_str!("../tests/auth/01_tcp_443.fixtures.json");
        let fixtures: Vec<Fixture> = serde_json::from_str(fixtures).unwrap();
        let auth: Auth = serde_yml::from_str(auth).unwrap();
        for fixture in fixtures {
            let req = Request::parse(fixture.request.as_bytes()).unwrap();
            match auth.authorize(&req).await {
                Ok(allow) => assert_eq!(fixture.allow, Some(allow)),
                Err(e) => {
                    assert_eq!(fixture.error, Some(format!("{:?}", e)))
                }
            }
        }
    }
}
