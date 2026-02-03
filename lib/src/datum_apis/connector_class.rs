use kube::CustomResource;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Debug, Clone, Serialize, Deserialize)]
#[kube(
    group = "networking.datumapis.com",
    version = "v1alpha1",
    kind = "ConnectorClass",
    plural = "connectorclasses",
    namespaced,
    status = "ConnectorClassStatus",
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorClassSpec {
    pub controller_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorClassStatus {}
