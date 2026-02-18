use k8s_openapi::{api::core::v1, apimachinery::pkg::apis::meta::v1 as metav1};
use kube::CustomResource;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConnectorReference {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectorCapabilityType {
    #[serde(rename = "ConnectTCP")]
    ConnectTcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorCapabilityCommon {
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorCapabilityConnectTCP {
    #[serde(flatten)]
    pub common: ConnectorCapabilityCommon,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorCapability {
    #[serde(rename = "type")]
    pub capability_type: ConnectorCapabilityType,
    pub connect_tcp: Option<ConnectorCapabilityConnectTCP>,
}

#[derive(CustomResource, Debug, Clone, Serialize, Deserialize)]
#[kube(
    group = "networking.datumapis.com",
    version = "v1alpha1",
    kind = "Connector",
    plural = "connectors",
    namespaced,
    status = "ConnectorStatus",
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSpec {
    pub connector_class_name: String,
    pub capabilities: Option<Vec<ConnectorCapability>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PublicKeyDiscoveryMode {
    #[serde(rename = "DNS")]
    Dns,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKeyConnectorAddress {
    pub address: String,
    pub port: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorConnectionDetailsPublicKey {
    pub id: String,
    pub discovery_mode: Option<PublicKeyDiscoveryMode>,
    pub home_relay: String,
    pub addresses: Vec<PublicKeyConnectorAddress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectorConnectionType {
    #[serde(rename = "PublicKey")]
    PublicKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorConnectionDetails {
    #[serde(rename = "type")]
    pub connection_type: ConnectorConnectionType,
    pub public_key: Option<ConnectorConnectionDetailsPublicKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorCapabilityStatus {
    #[serde(rename = "type")]
    pub capability_type: ConnectorCapabilityType,
    pub conditions: Option<Vec<metav1::Condition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorStatus {
    pub capabilities: Option<Vec<ConnectorCapabilityStatus>>,
    pub conditions: Option<Vec<metav1::Condition>>,
    pub connection_details: Option<ConnectorConnectionDetails>,
    pub lease_ref: Option<v1::LocalObjectReference>,
}
