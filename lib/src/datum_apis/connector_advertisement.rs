use kube::CustomResource;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use serde::{Deserialize, Serialize};

use crate::datum_apis::connector::LocalConnectorReference;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer4ServiceAddress(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Protocol {
    #[serde(rename = "TCP")]
    Tcp,
    #[serde(rename = "UDP")]
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer4ServicePort {
    pub name: String,
    pub port: i32,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAdvertisementLayer4Service {
    pub address: Layer4ServiceAddress,
    pub ports: Vec<Layer4ServicePort>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAdvertisementLayer4 {
    pub name: String,
    pub services: Vec<ConnectorAdvertisementLayer4Service>,
}

#[derive(CustomResource, Debug, Clone, Serialize, Deserialize)]
#[kube(
    group = "networking.datumapis.com",
    version = "v1alpha1",
    kind = "ConnectorAdvertisement",
    plural = "connectoradvertisements",
    namespaced,
    status = "ConnectorAdvertisementStatus",
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAdvertisementSpec {
    pub connector_ref: LocalConnectorReference,
    pub layer4: Option<Vec<ConnectorAdvertisementLayer4>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAdvertisementStatus {
    pub conditions: Option<Vec<metav1::Condition>>,
}

pub const CONNECTOR_ADVERTISEMENT_CONDITION_ACCEPTED: &str = "Accepted";
pub const CONNECTOR_ADVERTISEMENT_REASON_ACCEPTED: &str = "Accepted";
pub const CONNECTOR_ADVERTISEMENT_REASON_PENDING: &str = "Pending";
pub const CONNECTOR_ADVERTISEMENT_REASON_CONNECTOR_NOT_FOUND: &str = "ConnectorNotFound";
