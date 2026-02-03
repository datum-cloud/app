use std::collections::{BTreeMap, HashMap};

use kube::api::{DeleteParams, ListParams, Patch, PatchParams, PostParams};
use kube::{Api, ResourceExt};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use n0_error::{Result, StackResultExt, StdResultExt};
use serde_json::json;
use tracing::{debug, warn};

use crate::{Advertisment, ListenNode, ProxyState, TcpProxyData};
use crate::datum_apis::connector::{
    Connector, ConnectorConnectionDetails, ConnectorConnectionDetailsPublicKey,
    ConnectorConnectionType, ConnectorSpec, PublicKeyConnectorAddress, PublicKeyDiscoveryMode,
};
use crate::datum_apis::connector_advertisement::{
    ConnectorAdvertisement, ConnectorAdvertisementLayer4, ConnectorAdvertisementLayer4Service,
    ConnectorAdvertisementSpec, Layer4ServiceAddress, Layer4ServicePort, Protocol,
};
use crate::datum_apis::connector::CONNECTOR_NAME_ANNOTATION;
use crate::datum_apis::http_proxy::{
    ConnectorReference, HTTPProxy, HTTPProxyRule, HTTPProxyRuleBackend, HTTPProxySpec,
};
use gateway_api::apis::standard::httproutes::{
    HTTPRouteRulesMatchesPath, HTTPRouteRulesMatchesPathType,
};
use crate::datum_cloud::DatumCloudClient;

const DEFAULT_PCP_NAMESPACE: &str = "default";
const DEFAULT_CONNECTOR_CLASS_NAME: &str = "datum-connect";
const CONNECTOR_SELECTOR_FIELD: &str = "status.connectionDetails.publicKey.id";
const ADVERTISEMENT_CONNECTOR_FIELD: &str = "spec.connectorRef.name";
const DISPLAY_NAME_ANNOTATION: &str = "app.kubernetes.io/name";

#[derive(Debug, Clone, PartialEq)]
pub struct TunnelSummary {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub hostnames: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct TunnelDeleteOutcome {
    pub project_id: String,
    pub connector_deleted: bool,
}

#[derive(Debug, Clone)]
pub struct TunnelService {
    datum: DatumCloudClient,
    listen: ListenNode,
    publish_tickets: bool,
}

impl TunnelService {
    pub fn new(datum: DatumCloudClient, listen: ListenNode) -> Self {
        Self {
            datum,
            listen,
            publish_tickets: publish_tickets_enabled(),
        }
    }

    pub async fn list_active(&self) -> Result<Vec<TunnelSummary>> {
        let Some(selected) = self.datum.selected_context() else {
            return Ok(Vec::new());
        };
        self.list_project(&selected.project_id).await
    }

    pub async fn get_active(&self, tunnel_id: &str) -> Result<Option<TunnelSummary>> {
        let tunnels = self.list_active().await?;
        Ok(tunnels.into_iter().find(|tunnel| tunnel.id == tunnel_id))
    }

    pub async fn create_active(&self, label: &str, endpoint: &str) -> Result<TunnelSummary> {
        let Some(selected) = self.datum.selected_context() else {
            n0_error::bail_any!("No project selected");
        };
        self.create_project(&selected.project_id, label, endpoint)
            .await
    }

    pub async fn update_active(
        &self,
        tunnel_id: &str,
        label: &str,
        endpoint: &str,
    ) -> Result<TunnelSummary> {
        let Some(selected) = self.datum.selected_context() else {
            n0_error::bail_any!("No project selected");
        };
        self.update_project(&selected.project_id, tunnel_id, label, endpoint)
            .await
    }

    pub async fn set_enabled_active(&self, tunnel_id: &str, enabled: bool) -> Result<TunnelSummary> {
        let Some(selected) = self.datum.selected_context() else {
            n0_error::bail_any!("No project selected");
        };
        self.set_enabled_project(&selected.project_id, tunnel_id, enabled)
            .await
    }

    pub async fn delete_active(&self, tunnel_id: &str) -> Result<TunnelDeleteOutcome> {
        let Some(selected) = self.datum.selected_context() else {
            n0_error::bail_any!("No project selected");
        };
        self.delete_project(&selected.project_id, tunnel_id).await
    }

    pub async fn list_project(&self, project_id: &str) -> Result<Vec<TunnelSummary>> {
        let connector = self.find_connector(project_id).await?;
        let Some(connector) = connector else {
            return Ok(Vec::new());
        };
        let connector_name = connector.name_any();

        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let client = pcp.client();
        let proxies: Api<HTTPProxy> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let ads: Api<ConnectorAdvertisement> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        let proxy_list = proxies
            .list(&ListParams::default())
            .await
            .std_context("Failed to list HTTPProxy objects")?;

        let ad_selector = format!("{ADVERTISEMENT_CONNECTOR_FIELD}={connector_name}");
        let ad_list = ads
            .list(&ListParams::default().fields(&ad_selector))
            .await
            .std_context("Failed to list ConnectorAdvertisement objects")?;
        let enabled_by_name: HashMap<String, ConnectorAdvertisement> = ad_list
            .items
            .into_iter()
            .filter_map(|item| item.metadata.name.clone().map(|name| (name, item)))
            .collect();

        let mut tunnels = Vec::new();
        for proxy in proxy_list.items {
            let Some(name) = proxy.metadata.name.clone() else {
                continue;
            };
            let matches_connector = proxy
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(CONNECTOR_NAME_ANNOTATION))
                .map(|value| value == &connector_name)
                .unwrap_or(false);
            if !matches_connector {
                continue;
            }
            let label = proxy
                .metadata
                .annotations
                .as_ref()
                .and_then(|labels| labels.get(DISPLAY_NAME_ANNOTATION))
                .cloned()
                .unwrap_or_else(|| name.clone());
            let endpoint = normalize_endpoint(&proxy_backend_endpoint(&proxy).unwrap_or_default());
            let hostnames = proxy_hostnames(&proxy);
            let enabled = enabled_by_name.contains_key(&name);
            tunnels.push(TunnelSummary {
                id: name,
                label,
                endpoint,
                hostnames,
                enabled,
            });
        }
        Ok(tunnels)
    }

    pub async fn create_project(
        &self,
        project_id: &str,
        label: &str,
        endpoint: &str,
    ) -> Result<TunnelSummary> {
        let endpoint = normalize_endpoint(endpoint);
        let target = parse_target(&endpoint)?;
        let connector = self.ensure_connector(project_id).await?;
        let connector_name = connector.name_any();

        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let client = pcp.client();
        let proxies: Api<HTTPProxy> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let ads: Api<ConnectorAdvertisement> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        debug!(
            %project_id,
            connector = %connector_name,
            endpoint = %endpoint,
            "creating HTTPProxy"
        );
        let mut proxy = HTTPProxy {
            metadata: ObjectMeta {
                generate_name: Some("tunnel-".to_string()),
                annotations: Some(BTreeMap::from([
                    (DISPLAY_NAME_ANNOTATION.to_string(), label.to_string()),
                    (CONNECTOR_NAME_ANNOTATION.to_string(), connector_name.clone()),
                ])),
                ..Default::default()
            },
            spec: HTTPProxySpec {
                hostnames: None,
                rules: vec![proxy_rule(&endpoint, &connector_name)],
            },
            status: None,
        };
        proxy = proxies
            .create(&PostParams::default(), &proxy)
            .await
            .std_context("Failed to create HTTPProxy")
            .inspect_err(|err| {
                warn!(
                    %project_id,
                    connector = %connector_name,
                    endpoint = %endpoint,
                    "HTTPProxy create failed: {err:#}"
                );
            })?;
        let proxy_name = proxy.name_any();
        debug!(
            %project_id,
            proxy = %proxy_name,
            connector = %connector_name,
            "created HTTPProxy"
        );

        let ad_spec = advertisement_spec(&connector_name, target);
        debug!(
            %project_id,
            proxy = %proxy_name,
            connector = %connector_name,
            "creating ConnectorAdvertisement"
        );
        let ad = ConnectorAdvertisement {
            metadata: ObjectMeta {
                name: Some(proxy_name.clone()),
                ..Default::default()
            },
            spec: ad_spec,
            status: None,
        };
        ads.create(&PostParams::default(), &ad)
            .await
            .std_context("Failed to create ConnectorAdvertisement")
            .inspect_err(|err| {
                warn!(
                    %project_id,
                    proxy = %proxy_name,
                    connector = %connector_name,
                    "ConnectorAdvertisement create failed: {err:#}"
                );
            })?;
        debug!(
            %project_id,
            proxy = %proxy_name,
            connector = %connector_name,
            "created ConnectorAdvertisement"
        );

        if self.publish_tickets {
            let data = TcpProxyData::from_host_port_str(&strip_scheme(&endpoint))?;
            let info = Advertisment::with_id(proxy_name.clone(), data, Some(label.to_string()));
            let proxy_state = ProxyState {
                info,
                enabled: true,
            };
            debug!(%proxy_name, "publishing ticket for tunnel");
            if let Err(err) = self.listen.set_proxy(proxy_state).await {
                warn!(%proxy_name, "Failed to publish ticket: {err:#}");
            }
        }

        Ok(TunnelSummary {
            id: proxy_name,
            label: label.to_string(),
            endpoint,
            hostnames: proxy_hostnames(&proxy),
            enabled: true,
        })
    }

    pub async fn update_project(
        &self,
        project_id: &str,
        tunnel_id: &str,
        label: &str,
        endpoint: &str,
    ) -> Result<TunnelSummary> {
        let endpoint = normalize_endpoint(endpoint);
        let target = parse_target(&endpoint)?;
        let connector = self.ensure_connector(project_id).await?;
        let connector_name = connector.name_any();

        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let client = pcp.client();
        let proxies: Api<HTTPProxy> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let ads: Api<ConnectorAdvertisement> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        let existing = proxies
            .get(tunnel_id)
            .await
            .std_context("Failed to fetch HTTPProxy")?;
        let hostnames = existing.spec.hostnames.clone().unwrap_or_default();

        let patch = json!({
            "metadata": {
                "annotations": {
                    DISPLAY_NAME_ANNOTATION: label,
                    CONNECTOR_NAME_ANNOTATION: connector_name,
                }
            },
            "spec": {
                "hostnames": hostnames,
                "rules": [proxy_rule(&endpoint, &connector_name)],
            }
        });
        proxies
            .patch(tunnel_id, &PatchParams::default(), &Patch::Merge(&patch))
            .await
            .std_context("Failed to update HTTPProxy")?;

        if let Ok(existing_ad) = ads.get_opt(tunnel_id).await {
            if existing_ad.is_some() {
                let ad_patch = json!({
                    "spec": advertisement_spec(&connector_name, target)
                });
                ads.patch(tunnel_id, &PatchParams::default(), &Patch::Merge(&ad_patch))
                    .await
                    .std_context("Failed to update ConnectorAdvertisement")?;
            }
        }

        let enabled = ads
            .get_opt(tunnel_id)
            .await
            .std_context("Failed to load ConnectorAdvertisement")?
            .is_some();

        Ok(TunnelSummary {
            id: tunnel_id.to_string(),
            label: label.to_string(),
            endpoint,
            hostnames: proxy_hostnames(&existing),
            enabled,
        })
    }

    pub async fn set_enabled_project(
        &self,
        project_id: &str,
        tunnel_id: &str,
        enabled: bool,
    ) -> Result<TunnelSummary> {
        let connector = self.ensure_connector(project_id).await?;
        let connector_name = connector.name_any();

        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let client = pcp.client();
        let proxies: Api<HTTPProxy> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let ads: Api<ConnectorAdvertisement> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        let proxy = proxies
            .get(tunnel_id)
            .await
            .std_context("Failed to fetch HTTPProxy")?;
        let endpoint = normalize_endpoint(&proxy_backend_endpoint(&proxy).unwrap_or_default());
        let hostnames = proxy.spec.hostnames.clone().unwrap_or_default();
        let label = proxy
            .metadata
            .annotations
            .as_ref()
            .and_then(|labels| labels.get(DISPLAY_NAME_ANNOTATION))
            .cloned()
            .unwrap_or_else(|| tunnel_id.to_string());

        if enabled {
            let target = parse_target(&endpoint)?;
            let ad_spec = advertisement_spec(&connector_name, target);
            match ads
                .get_opt(tunnel_id)
                .await
                .std_context("Failed to load ConnectorAdvertisement")?
            {
                Some(_) => {
                    let ad_patch = json!({ "spec": ad_spec });
                    ads.patch(tunnel_id, &PatchParams::default(), &Patch::Merge(&ad_patch))
                        .await
                        .std_context("Failed to update ConnectorAdvertisement")?;
                }
                None => {
                    let ad = ConnectorAdvertisement {
                        metadata: ObjectMeta {
                            name: Some(tunnel_id.to_string()),
                            ..Default::default()
                        },
                        spec: ad_spec,
                        status: None,
                    };
                    ads.create(&PostParams::default(), &ad)
                        .await
                        .std_context("Failed to create ConnectorAdvertisement")?;
                }
            }
        } else if ads
            .get_opt(tunnel_id)
            .await
            .std_context("Failed to load ConnectorAdvertisement")?
            .is_some()
        {
            ads.delete(tunnel_id, &DeleteParams::default())
                .await
                .std_context("Failed to delete ConnectorAdvertisement")?;
        }

        Ok(TunnelSummary {
            id: tunnel_id.to_string(),
            label,
            endpoint,
            hostnames: proxy_hostnames(&proxy),
            enabled,
        })
    }

    pub async fn delete_project(
        &self,
        project_id: &str,
        tunnel_id: &str,
    ) -> Result<TunnelDeleteOutcome> {
        let connector = self.find_connector(project_id).await?;
        let Some(connector) = connector else {
            return Ok(TunnelDeleteOutcome {
                project_id: project_id.to_string(),
                connector_deleted: false,
            });
        };
        let connector_name = connector.name_any();

        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let client = pcp.client();
        let proxies: Api<HTTPProxy> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let ads: Api<ConnectorAdvertisement> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let connectors: Api<Connector> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        if proxies
            .get_opt(tunnel_id)
            .await
            .std_context("Failed to load HTTPProxy")?
            .is_some()
        {
            proxies
                .delete(tunnel_id, &DeleteParams::default())
                .await
                .std_context("Failed to delete HTTPProxy")?;
        }

        if ads
            .get_opt(tunnel_id)
            .await
            .std_context("Failed to load ConnectorAdvertisement")?
            .is_some()
        {
            ads.delete(tunnel_id, &DeleteParams::default())
                .await
                .std_context("Failed to delete ConnectorAdvertisement")?;
        }

        if self.publish_tickets {
            debug!(%tunnel_id, "unpublishing ticket for tunnel");
            if let Err(err) = self.listen.remove_proxy(tunnel_id).await {
                warn!(%tunnel_id, "Failed to unpublish ticket: {err:#}");
            }
        }

        let remaining = proxies
            .list(&ListParams::default())
            .await
            .std_context("Failed to list remaining HTTPProxy objects")?;
        let mut connector_deleted = false;
        let mut remaining_for_connector = remaining
            .items
            .into_iter()
            .filter(|proxy| {
                proxy
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|annotations| annotations.get(CONNECTOR_NAME_ANNOTATION))
                    .map(|value| value == &connector_name)
                    .unwrap_or(false)
            })
            .peekable();
        if remaining_for_connector.peek().is_none() {
            let ad_selector = format!("{ADVERTISEMENT_CONNECTOR_FIELD}={connector_name}");
            let ads_list = ads
                .list(&ListParams::default().fields(&ad_selector))
                .await
                .std_context("Failed to list remaining ConnectorAdvertisements")?;
            for ad in ads_list.items {
                if let Some(name) = ad.metadata.name.clone() {
                    if let Err(err) = ads.delete(&name, &DeleteParams::default()).await {
                        warn!(%name, "Failed to delete connector advertisement: {err:#}");
                    }
                }
            }

            if connectors
                .get_opt(&connector_name)
                .await
                .std_context("Failed to load Connector")?
                .is_some()
            {
                connectors
                    .delete(&connector_name, &DeleteParams::default())
                    .await
                    .std_context("Failed to delete Connector")?;
                connector_deleted = true;
            }
        }

        Ok(TunnelDeleteOutcome {
            project_id: project_id.to_string(),
            connector_deleted,
        })
    }

    async fn find_connector(&self, project_id: &str) -> Result<Option<Connector>> {
        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let client = pcp.client();
        let connectors: Api<Connector> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
        let endpoint_id = self.listen.endpoint_id().to_string();
        let selector = format!("{CONNECTOR_SELECTOR_FIELD}={endpoint_id}");
        let list = connectors
            .list(&ListParams::default().fields(&selector))
            .await
            .std_context("Failed to list connectors")?;
        if list.items.is_empty() {
            let fallback = connectors
                .list(&ListParams::default())
                .await
                .std_context("Failed to list connectors for fallback")?;
            if fallback.items.len() != 1 {
                if !fallback.items.is_empty() {
                    warn!(
                        %project_id,
                        count = fallback.items.len(),
                        "Multiple connectors found without status match"
                    );
                }
                return Ok(None);
            }
            let mut connector = fallback.items.into_iter().next().unwrap();
            let needs_patch = connector
                .status
                .as_ref()
                .and_then(|status| status.connection_details.as_ref())
                .and_then(|details| details.public_key.as_ref())
                .map(|details| details.id.as_str() != endpoint_id.as_str())
                .unwrap_or(true);
            if needs_patch {
                if let Some(details) = build_connection_details(&self.listen) {
                    let details_value = serde_json::to_value(details)
                        .std_context("Failed to serialize connection details")?;
                    let patch = json!({ "status": { "connectionDetails": details_value } });
                    if let Err(err) = connectors
                        .patch_status(
                            &connector.name_any(),
                            &PatchParams::default(),
                            &Patch::Merge(&patch),
                        )
                        .await
                    {
                        warn!(
                            connector = %connector.name_any(),
                            "Failed to patch connector status: {err:#}"
                        );
                    } else {
                        connector = connectors
                            .get(&connector.name_any())
                            .await
                            .std_context("Failed to reload connector after patch")?;
                    }
                }
            }
            return Ok(Some(connector));
        }
        if list.items.len() > 1 {
            debug!(
                %selector,
                count = list.items.len(),
                "Multiple connectors found for endpoint, using first"
            );
        }
        Ok(list.items.into_iter().next())
    }

    async fn ensure_connector(&self, project_id: &str) -> Result<Connector> {
        if let Some(connector) = self.find_connector(project_id).await? {
            return Ok(connector);
        }

        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let client = pcp.client();
        let connectors: Api<Connector> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        let mut connector = Connector {
            metadata: ObjectMeta {
                generate_name: Some("datum-connect-".to_string()),
                ..Default::default()
            },
            spec: ConnectorSpec {
                connector_class_name: DEFAULT_CONNECTOR_CLASS_NAME.to_string(),
                capabilities: None,
            },
            status: None,
        };
        connector = connectors
            .create(&PostParams::default(), &connector)
            .await
            .std_context("Failed to create Connector")?;

        if let Some(details) = build_connection_details(&self.listen) {
            let details_value =
                serde_json::to_value(details).std_context("Failed to serialize connection details")?;
            let patch = json!({ "status": { "connectionDetails": details_value } });
            if let Err(err) = connectors
                .patch_status(
                    &connector.name_any(),
                    &PatchParams::default(),
                    &Patch::Merge(&patch),
                )
                .await
            {
                warn!(connector = %connector.name_any(), "Failed to patch connector status: {err:#}");
            }
        } else {
            warn!(connector = %connector.name_any(), "Missing connection details for connector status");
        }

        Ok(connector)
    }
}

#[derive(Debug, Clone)]
struct ParsedTarget {
    address: String,
    port: u16,
}

fn parse_target(target: &str) -> Result<ParsedTarget> {
    let target = target.trim();
    if let Ok(url) = url::Url::parse(target) {
        let host = url.host_str().context("missing host")?;
        let port = url.port().context("missing port")?;
        return Ok(ParsedTarget {
            address: host.to_string(),
            port,
        });
    }

    let (host, port_str) = if target.starts_with('[') {
        let end = target.find(']').context("invalid IPv6 address")?;
        let host = &target[1..end];
        let port = target
            .get(end + 1..)
            .and_then(|rest| rest.strip_prefix(':'))
            .context("missing port")?;
        (host, port)
    } else {
        let (host, port) = target.rsplit_once(':').context("missing port")?;
        (host, port)
    };
    let port: u16 = port_str.parse().std_context("invalid port")?;
    Ok(ParsedTarget {
        address: host.to_string(),
        port,
    })
}

fn build_connection_details(listen: &ListenNode) -> Option<ConnectorConnectionDetails> {
    let endpoint = listen.endpoint();
    let endpoint_addr = endpoint.addr();
    let home_relay = endpoint_addr.relay_urls().next()?.to_string();
    let addresses: Vec<PublicKeyConnectorAddress> = endpoint_addr
        .ip_addrs()
        .map(|addr| PublicKeyConnectorAddress {
            address: addr.ip().to_string(),
            port: addr.port() as i32,
        })
        .collect();

    Some(ConnectorConnectionDetails {
        connection_type: ConnectorConnectionType::PublicKey,
        public_key: Some(ConnectorConnectionDetailsPublicKey {
            id: endpoint.id().to_string(),
            discovery_mode: Some(PublicKeyDiscoveryMode::Dns),
            home_relay,
            addresses,
        }),
    })
}

fn normalize_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return endpoint.to_string();
    }
    if endpoint.contains("://") {
        return endpoint.to_string();
    }
    format!("http://{endpoint}")
}

fn strip_scheme(endpoint: &str) -> String {
    if let Ok(url) = url::Url::parse(endpoint) {
        if let Some(host) = url.host_str() {
            if let Some(port) = url.port() {
                return format!("{host}:{port}");
            }
        }
    }
    endpoint.to_string()
}

fn proxy_hostnames(proxy: &HTTPProxy) -> Vec<String> {
    proxy
        .status
        .as_ref()
        .and_then(|status| status.hostnames.clone())
        .or_else(|| proxy.spec.hostnames.clone())
        .unwrap_or_default()
}

fn proxy_rule(endpoint: &str, connector_name: &str) -> HTTPProxyRule {
    HTTPProxyRule {
        name: None,
        matches: vec![default_match()],
        filters: None,
        backends: Some(vec![HTTPProxyRuleBackend {
            endpoint: endpoint.to_string(),
            connector: Some(ConnectorReference {
                name: connector_name.to_string(),
            }),
            filters: None,
        }]),
    }
}

fn proxy_backend_endpoint(proxy: &HTTPProxy) -> Option<String> {
    proxy
        .spec
        .rules
        .first()
        .and_then(|rule| rule.backends.as_ref())
        .and_then(|backends| backends.first())
        .map(|backend| backend.endpoint.clone())
}

fn advertisement_spec(connector_name: &str, target: ParsedTarget) -> ConnectorAdvertisementSpec {
    let port_name = format!("tcp-{}", target.port);
    ConnectorAdvertisementSpec {
        connector_ref: crate::datum_apis::connector::LocalConnectorReference {
            name: connector_name.to_string(),
        },
        layer4: Some(vec![ConnectorAdvertisementLayer4 {
            name: "default".to_string(),
            services: vec![ConnectorAdvertisementLayer4Service {
                address: Layer4ServiceAddress(target.address),
                ports: vec![Layer4ServicePort {
                    name: port_name,
                    port: target.port as i32,
                    protocol: Protocol::Tcp,
                }],
            }],
        }]),
    }
}

fn default_match() -> crate::datum_apis::http_proxy::HTTPRouteMatch {
    crate::datum_apis::http_proxy::HTTPRouteMatch {
        path: Some(HTTPRouteRulesMatchesPath {
            r#type: Some(HTTPRouteRulesMatchesPathType::PathPrefix),
            value: Some("/".to_string()),
        }),
        ..Default::default()
    }
}

fn publish_tickets_enabled() -> bool {
    std::env::var("DATUM_CONNECT_PUBLISH_TICKETS")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}
