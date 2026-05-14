use std::collections::{BTreeMap, HashMap};

use iroh_proxy_utils::Authority;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams, PostParams};
use kube::{Api, ResourceExt};
use n0_error::{Result, StackResultExt, StdResultExt};
use serde_json::json;
use tracing::{debug, warn};

use crate::datum_apis::connector::{
    Connector, ConnectorConnectionDetails, ConnectorConnectionDetailsPublicKey,
    ConnectorConnectionType, ConnectorSpec, PublicKeyConnectorAddress, PublicKeyDiscoveryMode,
};
use crate::datum_apis::connector_advertisement::{
    ConnectorAdvertisement, ConnectorAdvertisementLayer4, ConnectorAdvertisementLayer4Service,
    ConnectorAdvertisementSpec, Layer4ServiceAddress, Layer4ServicePort, Protocol,
};
use crate::datum_apis::http_proxy::{
    ConnectorReference, HTTP_PROXY_CONDITION_ACCEPTED, HTTP_PROXY_CONDITION_PROGRAMMED, HTTPProxy,
    HTTPProxyRule, HTTPProxyRuleBackend, HTTPProxySpec,
};
use crate::datum_apis::traffic_protection_policy::{
    LocalPolicyTargetReferenceWithSectionName, OWASPCRS, ParanoiaLevels, TrafficProtectionPolicy,
    TrafficProtectionPolicyMode, TrafficProtectionPolicyRuleSet,
    TrafficProtectionPolicyRuleSetType, TrafficProtectionPolicySpec,
};
use crate::datum_cloud::DatumCloudClient;
use crate::{Advertisment, ListenNode, ProxyState, TcpProxyData};
use gateway_api::apis::standard::httproutes::{
    HTTPRouteRulesFiltersRequestRedirectScheme, HTTPRouteRulesFiltersType,
    HTTPRouteRulesMatchesHeaders, HTTPRouteRulesMatchesHeadersType, HTTPRouteRulesMatchesPath,
    HTTPRouteRulesMatchesPathType,
};

const DEFAULT_PCP_NAMESPACE: &str = "default";
const DEFAULT_CONNECTOR_CLASS_NAME: &str = "iroh-quic-tunnel";
const CONNECTOR_SELECTOR_FIELD: &str = "status.connectionDetails.publicKey.id";
const ADVERTISEMENT_CONNECTOR_FIELD: &str = "spec.connectorRef.name";
const DISPLAY_NAME_ANNOTATION: &str = "app.kubernetes.io/name";

/// Returns true if any rule in the HTTPProxy has a backend that references the given connector by name.
fn proxy_uses_connector(proxy: &HTTPProxy, connector_name: &str) -> bool {
    proxy
        .spec
        .rules
        .iter()
        .flat_map(|rule| rule.backends.iter().flatten())
        .any(|backend| {
            backend
                .connector
                .as_ref()
                .map(|c| c.name == connector_name)
                .unwrap_or(false)
        })
}

#[derive(Debug, Clone, PartialEq)]
pub struct TunnelSummary {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub hostnames: Vec<String>,
    pub enabled: bool,
    pub accepted: bool,
    pub programmed: bool,
}

impl TunnelSummary {
    // TODO(Frando): this should all be cleared up and use more common types instead of
    // converting around wildly.
    pub fn origin_authority(&self) -> Option<Authority> {
        TcpProxyData::from_host_port_str(&strip_scheme(&self.endpoint))
            .ok()
            .map(Authority::from)
    }
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
    create_traffic_protection_policies: bool,
}

// TODO(zachsmith1): Use connectors + ConnectorAdvertisements across all projects to
// decide which local proxies should be allowed, instead of only syncing the
// selected project's tunnel list.
fn proxy_state_from_summary(
    tunnel_id: &str,
    endpoint: &str,
    label: &str,
    enabled: bool,
) -> Result<ProxyState> {
    let data = TcpProxyData::from_host_port_str(&strip_scheme(endpoint))?;
    let info = Advertisment::with_id(tunnel_id.to_string(), data, Some(label.to_string()));
    Ok(ProxyState { info, enabled })
}

fn condition_is_true(
    conditions: Option<&[k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition]>,
    kind: &str,
) -> bool {
    conditions
        .unwrap_or_default()
        .iter()
        .find(|condition| condition.type_ == kind)
        .map(|condition| condition.status == "True")
        .unwrap_or(false)
}

impl TunnelService {
    pub fn new(datum: DatumCloudClient, listen: ListenNode) -> Self {
        Self {
            datum,
            listen,
            publish_tickets: publish_tickets_enabled(),
            create_traffic_protection_policies: create_traffic_protection_policies_enabled(),
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

    pub async fn set_enabled_active(
        &self,
        tunnel_id: &str,
        enabled: bool,
    ) -> Result<TunnelSummary> {
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
        let ad_selector = format!("{ADVERTISEMENT_CONNECTOR_FIELD}={connector_name}");
        let (proxy_items, ad_items) = pcp
            .with_auth_retry(|client| {
                let ad_selector = ad_selector.clone();
                async move {
                    let proxies: Api<HTTPProxy> =
                        Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
                    let ads: Api<ConnectorAdvertisement> =
                        Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
                    let proxy_list = proxies.list(&ListParams::default()).await?;
                    let ad_list = ads
                        .list(&ListParams::default().fields(&ad_selector))
                        .await?;
                    Ok((proxy_list.items, ad_list.items))
                }
            })
            .await?;
        let enabled_by_name: HashMap<String, ConnectorAdvertisement> = ad_items
            .into_iter()
            .filter_map(|item| item.metadata.name.clone().map(|name| (name, item)))
            .collect();

        let mut tunnels = Vec::new();
        for proxy in proxy_items {
            let Some(name) = proxy.metadata.name.clone() else {
                continue;
            };
            if !proxy_uses_connector(&proxy, &connector_name) {
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
            let accepted = condition_is_true(
                proxy
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_ACCEPTED,
            );
            let programmed = condition_is_true(
                proxy
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_PROGRAMMED,
            );
            let enabled = enabled_by_name.contains_key(&name);
            tunnels.push(TunnelSummary {
                id: name,
                label,
                endpoint,
                hostnames,
                enabled,
                accepted,
                programmed,
            });
        }
        if !self.publish_tickets {
            let current_ids: std::collections::HashSet<&str> =
                tunnels.iter().map(|t| t.id.as_str()).collect();

            // Sync state for each tunnel returned by the server.
            for tunnel in &tunnels {
                if let Ok(proxy_state) = proxy_state_from_summary(
                    &tunnel.id,
                    &tunnel.endpoint,
                    &tunnel.label,
                    tunnel.enabled,
                ) && let Err(err) = self.listen.set_proxy_state(proxy_state).await
                {
                    warn!(tunnel_id = %tunnel.id, "Failed to store proxy state: {err:#}");
                }
            }

            // Remove stale local entries that share host:port with a current tunnel
            // but have a different resource_id. These accumulate when a tunnel is
            // deleted and recreated with the same endpoint (new ID). Without this,
            // the stale enabled entry causes tcp_proxy_exists to return true even
            // when the current tunnel is disabled, allowing traffic through.
            //
            // Scoped to same-endpoint matches so we don't touch entries belonging
            // to other projects with different endpoints.
            for tunnel in &tunnels {
                let Ok(data) = TcpProxyData::from_host_port_str(&strip_scheme(&tunnel.endpoint))
                else {
                    continue;
                };
                let stale_ids: Vec<String> = self
                    .listen
                    .proxies()
                    .into_iter()
                    .filter(|p| {
                        !current_ids.contains(p.id())
                            && p.info.service().host == data.host
                            && p.info.service().port == data.port
                    })
                    .map(|p| p.id().to_string())
                    .collect();
                for id in stale_ids {
                    if let Err(err) = self.listen.remove_proxy_state(&id).await {
                        warn!(tunnel_id = %id, "Failed to remove stale proxy state: {err:#}");
                    }
                }
            }
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
        let create_traffic_protection_policies = self.create_traffic_protection_policies;
        let project_id_owned = project_id.to_string();
        let connector_name_owned = connector_name.clone();
        let label_owned = label.to_string();
        let endpoint_owned = endpoint.clone();
        let proxy = pcp
            .with_auth(|client| async move {
                let proxies: Api<HTTPProxy> =
                    Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
                let ads: Api<ConnectorAdvertisement> =
                    Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);

                debug!(
                    project_id = %project_id_owned,
                    connector = %connector_name_owned,
                    endpoint = %endpoint_owned,
                    "creating HTTPProxy"
                );
                let proxy = HTTPProxy {
                    metadata: ObjectMeta {
                        generate_name: Some("tunnel-".to_string()),
                        annotations: Some(BTreeMap::from([(
                            DISPLAY_NAME_ANNOTATION.to_string(),
                            label_owned.clone(),
                        )])),
                        ..Default::default()
                    },
                    spec: HTTPProxySpec {
                        hostnames: None,
                        rules: vec![
                            https_redirect_rule(),
                            proxy_rule(&endpoint_owned, &connector_name_owned),
                        ],
                    },
                    status: None,
                };
                let proxy = proxies
                    .create(&PostParams::default(), &proxy)
                    .await
                    .inspect_err(|err| {
                        warn!(
                            project_id = %project_id_owned,
                            connector = %connector_name_owned,
                            endpoint = %endpoint_owned,
                            "HTTPProxy create failed: {err:#}"
                        );
                    })?;
                let proxy_name = proxy.name_any();
                debug!(
                    project_id = %project_id_owned,
                    proxy = %proxy_name,
                    connector = %connector_name_owned,
                    "created HTTPProxy"
                );

                let ad_spec = advertisement_spec(&connector_name_owned, target);
                debug!(
                    project_id = %project_id_owned,
                    proxy = %proxy_name,
                    connector = %connector_name_owned,
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
                    .inspect_err(|err| {
                        warn!(
                            project_id = %project_id_owned,
                            proxy = %proxy_name,
                            connector = %connector_name_owned,
                            "ConnectorAdvertisement create failed: {err:#}"
                        );
                    })?;
                debug!(
                    project_id = %project_id_owned,
                    proxy = %proxy_name,
                    connector = %connector_name_owned,
                    "created ConnectorAdvertisement"
                );

                if create_traffic_protection_policies {
                    let tpps: Api<TrafficProtectionPolicy> =
                        Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
                    debug!(
                        project_id = %project_id_owned,
                        proxy = %proxy_name,
                        "creating TrafficProtectionPolicy"
                    );
                    let tpp = TrafficProtectionPolicy {
                        metadata: ObjectMeta {
                            name: Some(proxy_name.clone()),
                            ..Default::default()
                        },
                        spec: TrafficProtectionPolicySpec {
                            target_refs: vec![LocalPolicyTargetReferenceWithSectionName {
                                group: "gateway.networking.k8s.io".to_string(),
                                kind: "Gateway".to_string(),
                                name: proxy_name.clone(),
                                section_name: None,
                            }],
                            mode: Some(TrafficProtectionPolicyMode::Enforce),
                            sampling_percentage: None,
                            rule_sets: Some(vec![TrafficProtectionPolicyRuleSet {
                                rule_set_type: TrafficProtectionPolicyRuleSetType::OWASPCoreRuleSet,
                                owasp_core_rule_set: Some(OWASPCRS {
                                    paranoia_levels: Some(ParanoiaLevels {
                                        blocking: Some(1),
                                        detection: Some(1),
                                    }),
                                    score_thresholds: None,
                                    rule_exclusions: None,
                                }),
                            }]),
                        },
                        status: None,
                    };
                    tpps.create(&PostParams::default(), &tpp)
                        .await
                        .inspect_err(|err| {
                            warn!(
                                project_id = %project_id_owned,
                                proxy = %proxy_name,
                                "TrafficProtectionPolicy create failed: {err:#}"
                            );
                        })?;
                    debug!(
                        project_id = %project_id_owned,
                        proxy = %proxy_name,
                        "created TrafficProtectionPolicy"
                    );
                } else {
                    debug!(
                        project_id = %project_id_owned,
                        proxy = %proxy_name,
                        "skipping TrafficProtectionPolicy creation (env disabled)"
                    );
                }

                Ok(proxy)
            })
            .await?;
        let proxy_name = proxy.name_any();

        let proxy_state = proxy_state_from_summary(&proxy_name, &endpoint, label, true)?;
        if self.publish_tickets {
            debug!(%proxy_name, "publishing ticket for tunnel");
            if let Err(err) = self.listen.set_proxy(proxy_state).await {
                warn!(%proxy_name, "Failed to publish ticket: {err:#}");
            }
        } else if let Err(err) = self.listen.set_proxy_state(proxy_state).await {
            warn!(%proxy_name, "Failed to store proxy state: {err:#}");
        }

        Ok(TunnelSummary {
            id: proxy_name,
            label: label.to_string(),
            endpoint,
            hostnames: proxy_hostnames(&proxy),
            enabled: true,
            accepted: condition_is_true(
                proxy
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_ACCEPTED,
            ),
            programmed: condition_is_true(
                proxy
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_PROGRAMMED,
            ),
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
        let tunnel_id_owned = tunnel_id.to_string();
        let label_owned = label.to_string();
        let endpoint_owned = endpoint.clone();
        let connector_name_owned = connector_name.clone();
        let (existing, enabled) = pcp
            .with_auth(|client| async move {
                let proxies: Api<HTTPProxy> =
                    Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
                let ads: Api<ConnectorAdvertisement> =
                    Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

                let existing = proxies.get(&tunnel_id_owned).await?;
                let hostnames = existing.spec.hostnames.clone().unwrap_or_default();

                let patch = json!({
                    "metadata": {
                        "annotations": {
                            DISPLAY_NAME_ANNOTATION: label_owned,
                        }
                    },
                    "spec": {
                        "hostnames": hostnames,
                        "rules": [https_redirect_rule(), proxy_rule(&endpoint_owned, &connector_name_owned)],
                    }
                });
                proxies
                    .patch(&tunnel_id_owned, &PatchParams::default(), &Patch::Merge(&patch))
                    .await?;

                if let Some(_existing_ad) = ads.get_opt(&tunnel_id_owned).await? {
                    let ad_patch = json!({
                        "spec": advertisement_spec(&connector_name_owned, target)
                    });
                    ads.patch(
                        &tunnel_id_owned,
                        &PatchParams::default(),
                        &Patch::Merge(&ad_patch),
                    )
                    .await?;
                }

                let enabled = ads.get_opt(&tunnel_id_owned).await?.is_some();
                Ok((existing, enabled))
            })
            .await?;

        let summary = TunnelSummary {
            id: tunnel_id.to_string(),
            label: label.to_string(),
            endpoint,
            hostnames: proxy_hostnames(&existing),
            enabled,
            accepted: condition_is_true(
                existing
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_ACCEPTED,
            ),
            programmed: condition_is_true(
                existing
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_PROGRAMMED,
            ),
        };

        if !self.publish_tickets
            && let Ok(proxy_state) = proxy_state_from_summary(
                &summary.id,
                &summary.endpoint,
                &summary.label,
                summary.enabled,
            )
            && let Err(err) = self.listen.set_proxy_state(proxy_state).await
        {
            warn!(tunnel_id = %summary.id, "Failed to store proxy state: {err:#}");
        }

        Ok(summary)
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

        // Fetch the existing proxy first so we can derive endpoint/label up front and
        // do the (fallible, non-kube) target parsing outside the auth-handling closure.
        let tunnel_id_for_get = tunnel_id.to_string();
        let proxy = pcp
            .with_auth_retry(move |client| {
                let tunnel_id = tunnel_id_for_get.clone();
                async move {
                    let proxies: Api<HTTPProxy> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
                    proxies.get(&tunnel_id).await
                }
            })
            .await?;
        let endpoint = normalize_endpoint(&proxy_backend_endpoint(&proxy).unwrap_or_default());
        let label = proxy
            .metadata
            .annotations
            .as_ref()
            .and_then(|labels| labels.get(DISPLAY_NAME_ANNOTATION))
            .cloned()
            .unwrap_or_else(|| tunnel_id.to_string());

        let tunnel_id_owned = tunnel_id.to_string();
        let connector_name_owned = connector_name.clone();
        let endpoint_for_closure = endpoint.clone();
        pcp.with_auth(|client| async move {
            let ads: Api<ConnectorAdvertisement> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
            if enabled {
                // parse_target is infallible at this point: we already used `endpoint` for
                // proxy_backend_endpoint, and we re-validate below for safety.
                let target = match parse_target(&endpoint_for_closure) {
                    Ok(t) => t,
                    Err(err) => {
                        // Surface as a kube Service error so it round-trips through the
                        // helper; the caller still sees a wrapped n0_error.
                        return Err(kube::Error::Service(Box::new(std::io::Error::other(
                            err.to_string(),
                        ))));
                    }
                };
                let ad_spec = advertisement_spec(&connector_name_owned, target);
                match ads.get_opt(&tunnel_id_owned).await? {
                    Some(_) => {
                        let ad_patch = json!({ "spec": ad_spec });
                        ads.patch(
                            &tunnel_id_owned,
                            &PatchParams::default(),
                            &Patch::Merge(&ad_patch),
                        )
                        .await?;
                    }
                    None => {
                        let ad = ConnectorAdvertisement {
                            metadata: ObjectMeta {
                                name: Some(tunnel_id_owned.clone()),
                                ..Default::default()
                            },
                            spec: ad_spec,
                            status: None,
                        };
                        ads.create(&PostParams::default(), &ad).await?;
                    }
                }
            } else if ads.get_opt(&tunnel_id_owned).await?.is_some() {
                ads.delete(&tunnel_id_owned, &DeleteParams::default())
                    .await?;
            }
            Ok(())
        })
        .await?;

        let summary = TunnelSummary {
            id: tunnel_id.to_string(),
            label,
            endpoint,
            hostnames: proxy_hostnames(&proxy),
            enabled,
            accepted: condition_is_true(
                proxy
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_ACCEPTED,
            ),
            programmed: condition_is_true(
                proxy
                    .status
                    .as_ref()
                    .and_then(|status| status.conditions.as_deref()),
                HTTP_PROXY_CONDITION_PROGRAMMED,
            ),
        };

        if !self.publish_tickets
            && let Ok(proxy_state) = proxy_state_from_summary(
                &summary.id,
                &summary.endpoint,
                &summary.label,
                summary.enabled,
            )
            && let Err(err) = self.listen.set_proxy_state(proxy_state).await
        {
            warn!(tunnel_id = %summary.id, "Failed to store proxy state: {err:#}");
        }

        Ok(summary)
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

        let tunnel_id_owned = tunnel_id.to_string();
        let connector_name_owned = connector_name.clone();
        let connector_deleted = pcp
            .with_auth(|client| async move {
                let proxies: Api<HTTPProxy> =
                    Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
                let ads: Api<ConnectorAdvertisement> =
                    Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
                let connectors: Api<Connector> =
                    Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);

                if proxies.get_opt(&tunnel_id_owned).await?.is_some() {
                    proxies
                        .delete(&tunnel_id_owned, &DeleteParams::default())
                        .await?;
                }

                if ads.get_opt(&tunnel_id_owned).await?.is_some() {
                    ads.delete(&tunnel_id_owned, &DeleteParams::default())
                        .await?;
                }

                let tpps: Api<TrafficProtectionPolicy> =
                    Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
                if tpps.get_opt(&tunnel_id_owned).await?.is_some() {
                    tpps.delete(&tunnel_id_owned, &DeleteParams::default())
                        .await?;
                }

                let remaining = proxies.list(&ListParams::default()).await?;
                let mut connector_deleted = false;
                let mut remaining_for_connector = remaining
                    .items
                    .into_iter()
                    .filter(|proxy| proxy_uses_connector(proxy, &connector_name_owned))
                    .peekable();
                if remaining_for_connector.peek().is_none() {
                    let ad_selector =
                        format!("{ADVERTISEMENT_CONNECTOR_FIELD}={connector_name_owned}");
                    let ads_list = ads
                        .list(&ListParams::default().fields(&ad_selector))
                        .await?;
                    for ad in ads_list.items {
                        if let Some(name) = ad.metadata.name.clone()
                            && let Err(err) = ads.delete(&name, &DeleteParams::default()).await
                        {
                            warn!(%name, "Failed to delete connector advertisement: {err:#}");
                        }
                    }

                    if connectors.get_opt(&connector_name_owned).await?.is_some() {
                        connectors
                            .delete(&connector_name_owned, &DeleteParams::default())
                            .await?;
                        connector_deleted = true;
                    }
                }

                Ok(connector_deleted)
            })
            .await?;

        if self.publish_tickets {
            debug!(%tunnel_id, "unpublishing ticket for tunnel");
            if let Err(err) = self.listen.remove_proxy(tunnel_id).await {
                warn!(%tunnel_id, "Failed to unpublish ticket: {err:#}");
            }
        } else if let Err(err) = self.listen.remove_proxy_state(tunnel_id).await {
            warn!(%tunnel_id, "Failed to remove proxy state: {err:#}");
        }

        Ok(TunnelDeleteOutcome {
            project_id: project_id.to_string(),
            connector_deleted,
        })
    }

    async fn find_connector(&self, project_id: &str) -> Result<Option<Connector>> {
        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let endpoint_id = self.listen.endpoint_id().to_string();
        let selector = format!("{CONNECTOR_SELECTOR_FIELD}={endpoint_id}");
        let project_id_owned = project_id.to_string();

        let connection_details = build_connection_details(&self.listen);
        let device_annotations_value = device_annotations();

        let connector = pcp
            .with_auth_retry(|client| {
                let selector = selector.clone();
                let endpoint_id = endpoint_id.clone();
                let project_id = project_id_owned.clone();
                let connection_details = connection_details.clone();
                let device_annotations_value = device_annotations_value.clone();
                async move {
                    let connectors: Api<Connector> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
                    let list = connectors
                        .list(&ListParams::default().fields(&selector))
                        .await?;
                    let mut connector = if list.items.is_empty() {
                        let fallback = connectors.list(&ListParams::default()).await?;
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
                        if needs_patch && let Some(details) = connection_details.as_ref() {
                            let details_value = match serde_json::to_value(details) {
                                Ok(v) => v,
                                Err(err) => {
                                    return Err(kube::Error::SerdeError(err));
                                }
                            };
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
                                connector = connectors.get(&connector.name_any()).await?;
                            }
                        }
                        connector
                    } else {
                        if list.items.len() > 1 {
                            debug!(
                                %selector,
                                count = list.items.len(),
                                "Multiple connectors found for endpoint, using first"
                            );
                        }
                        list.items.into_iter().next().unwrap()
                    };
                    patch_device_annotations(
                        &connectors,
                        &mut connector,
                        &device_annotations_value,
                    )
                    .await;
                    Ok(Some(connector))
                }
            })
            .await?;
        Ok(connector)
    }

    async fn ensure_connector(&self, project_id: &str) -> Result<Connector> {
        if let Some(connector) = self.find_connector(project_id).await? {
            return Ok(connector);
        }

        let pcp = self.datum.project_control_plane_client(project_id).await?;
        let connection_details = build_connection_details(&self.listen);
        let device_annotations_value = device_annotations();

        let connector = pcp
            .with_auth(|client| async move {
                let connectors: Api<Connector> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

                let connector = Connector {
                    metadata: ObjectMeta {
                        generate_name: Some("datum-connect-".to_string()),
                        annotations: Some(device_annotations_value),
                        ..Default::default()
                    },
                    spec: ConnectorSpec {
                        connector_class_name: DEFAULT_CONNECTOR_CLASS_NAME.to_string(),
                        capabilities: None,
                    },
                    status: None,
                };
                let connector = connectors
                    .create(&PostParams::default(), &connector)
                    .await?;

                if let Some(details) = connection_details.as_ref() {
                    let details_value =
                        serde_json::to_value(details).map_err(kube::Error::SerdeError)?;
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
                    }
                } else {
                    warn!(
                        connector = %connector.name_any(),
                        "Missing connection details for connector status"
                    );
                }

                Ok(connector)
            })
            .await?;

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
    if let Ok(url) = url::Url::parse(endpoint)
        && let Some(host) = url.host_str()
        && let Some(port) = url.port()
    {
        return format!("{host}:{port}");
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

/// Rule that matches requests with x-forwarded-proto: http and redirects to HTTPS (301).
/// Evaluated first so HTTP traffic is upgraded before hitting the backend rule.
fn https_redirect_rule() -> HTTPProxyRule {
    use gateway_api::apis::standard::httproutes::{
        HTTPRouteRulesFilters, HTTPRouteRulesFiltersRequestRedirect,
    };
    HTTPProxyRule {
        name: None,
        matches: vec![crate::datum_apis::http_proxy::HTTPRouteMatch {
            path: Some(HTTPRouteRulesMatchesPath {
                r#type: Some(HTTPRouteRulesMatchesPathType::PathPrefix),
                value: Some("/".to_string()),
            }),
            headers: Some(vec![HTTPRouteRulesMatchesHeaders {
                name: "x-forwarded-proto".to_string(),
                r#type: Some(HTTPRouteRulesMatchesHeadersType::Exact),
                value: "http".to_string(),
            }]),
            ..Default::default()
        }],
        filters: Some(vec![HTTPRouteRulesFilters {
            request_redirect: Some(HTTPRouteRulesFiltersRequestRedirect {
                scheme: Some(HTTPRouteRulesFiltersRequestRedirectScheme::Https),
                status_code: Some(301),
                hostname: None,
                path: None,
                port: None,
            }),
            r#type: HTTPRouteRulesFiltersType::RequestRedirect,
            extension_ref: None,
            request_header_modifier: None,
            request_mirror: None,
            response_header_modifier: None,
            url_rewrite: None,
        }]),
        backends: None,
    }
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
        .iter()
        .find_map(|rule| rule.backends.as_ref().and_then(|b| b.first()))
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

fn friendly_device_name() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("scutil")
            .arg("--get")
            .arg("ComputerName")
            .output()
        {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
        }
    }
    let hostname = gethostname::gethostname().to_string_lossy().into_owned();
    hostname
        .strip_suffix(".local")
        .unwrap_or(&hostname)
        .to_string()
}

const DEVICE_NAME_ANNOTATION: &str = "datum.net/device-name";
const DEVICE_OS_ANNOTATION: &str = "datum.net/device-os";

fn device_annotations() -> BTreeMap<String, String> {
    BTreeMap::from([
        (DEVICE_NAME_ANNOTATION.to_string(), friendly_device_name()),
        (
            DEVICE_OS_ANNOTATION.to_string(),
            std::env::consts::OS.to_string(),
        ),
    ])
}

async fn patch_device_annotations(
    api: &Api<Connector>,
    connector: &mut Connector,
    expected: &BTreeMap<String, String>,
) {
    let current = connector.metadata.annotations.as_ref();
    let needs_patch = expected.iter().any(|(k, v)| {
        current
            .and_then(|a| a.get(k))
            .map(|cv| cv != v)
            .unwrap_or(true)
    });
    if !needs_patch {
        return;
    }
    let patch = json!({ "metadata": { "annotations": expected } });
    match api
        .patch(
            &connector.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await
    {
        Ok(patched) => *connector = patched,
        Err(err) => {
            warn!(
                connector = %connector.name_any(),
                "Failed to patch device annotations: {err:#}"
            );
        }
    }
}

fn publish_tickets_enabled() -> bool {
    std::env::var("DATUM_CONNECT_PUBLISH_TICKETS")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn create_traffic_protection_policies_enabled() -> bool {
    std::env::var("DATUM_CONNECT_CREATE_TRAFFIC_PROTECTION_POLICIES")
        .ok()
        .or_else(|| {
            option_env!("BUILD_DATUM_CONNECT_CREATE_TRAFFIC_PROTECTION_POLICIES")
                .map(str::to_string)
        })
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}
