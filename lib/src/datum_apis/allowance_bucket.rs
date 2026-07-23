use kube::CustomResource;
use serde::{Deserialize, Serialize};

/// Reference to the quota consumer (e.g. a Project) tracked by an AllowanceBucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_group: Option<String>,
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// Milo AllowanceBucket — aggregates grants and tracks consumption for a
/// (consumer, resourceType) pair. Status fields are written by the quota system.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize)]
#[kube(
    group = "quota.miloapis.com",
    version = "v1alpha1",
    kind = "AllowanceBucket",
    plural = "allowancebuckets",
    namespaced,
    status = "AllowanceBucketStatus",
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceBucketSpec {
    pub consumer_ref: ConsumerRef,
    pub resource_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceBucketStatus {
    #[serde(default)]
    pub limit: i64,
    #[serde(default)]
    pub allocated: i64,
    #[serde(default)]
    pub available: i64,
}
