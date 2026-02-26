use gateway_api::apis::standard::{
    gateways::GatewayStatusAddresses,
    httproutes::{HTTPRouteRulesFilters, HTTPRouteRulesMatches},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::CustomResource;
use serde::{Deserialize, Serialize};

pub type Hostname = String;
pub type SectionName = String;
pub type GatewayStatusAddress = GatewayStatusAddresses;
pub type HTTPRouteMatch = HTTPRouteRulesMatches;
pub type HTTPRouteFilter = HTTPRouteRulesFilters;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorReference {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HTTPProxyRuleBackend {
    pub endpoint: String,
    pub connector: Option<ConnectorReference>,
    pub filters: Option<Vec<HTTPRouteFilter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HTTPProxyRule {
    pub name: Option<SectionName>,
    pub matches: Vec<HTTPRouteMatch>,
    pub filters: Option<Vec<HTTPRouteFilter>>,
    pub backends: Option<Vec<HTTPProxyRuleBackend>>,
}

#[derive(CustomResource, Debug, Clone, Serialize, Deserialize)]
#[kube(
    group = "networking.datumapis.com",
    version = "v1alpha",
    kind = "HTTPProxy",
    plural = "httpproxies",
    namespaced,
    status = "HTTPProxyStatus",
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct HTTPProxySpec {
    pub hostnames: Option<Vec<Hostname>>,
    pub rules: Vec<HTTPProxyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HTTPProxyStatus {
    pub addresses: Option<Vec<GatewayStatusAddress>>,
    pub hostnames: Option<Vec<Hostname>>,
    pub conditions: Option<Vec<metav1::Condition>>,
}

pub const HTTP_PROXY_CONDITION_ACCEPTED: &str = "Accepted";
pub const HTTP_PROXY_CONDITION_PROGRAMMED: &str = "Programmed";
pub const HTTP_PROXY_CONDITION_HOSTNAMES_VERIFIED: &str = "HostnamesVerified";
pub const HTTP_PROXY_CONDITION_HOSTNAMES_IN_USE: &str = "HostnamesInUse";

pub const HTTP_PROXY_REASON_ACCEPTED: &str = "Accepted";
pub const HTTP_PROXY_REASON_PROGRAMMED: &str = "Programmed";
pub const HTTP_PROXY_REASON_CONFLICT: &str = "Conflict";
pub const HTTP_PROXY_REASON_PENDING: &str = "Pending";
pub const HTTP_PROXY_REASON_HOSTNAMES_VERIFIED: &str = "HostnamesVerified";
pub const HTTP_PROXY_REASON_UNVERIFIED_HOSTNAMES_PRESENT: &str = "UnverifiedHostnamesPresent";
pub const HTTP_PROXY_REASON_HOSTNAME_IN_USE: &str = "HostnameInUse";
