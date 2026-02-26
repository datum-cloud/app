use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::CustomResource;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrafficProtectionPolicyMode {
    Observe,
    Enforce,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPolicyTargetReferenceWithSectionName {
    pub group: String,
    pub kind: String,
    pub name: String,
    pub section_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrafficProtectionPolicyRuleSetType {
    OWASPCoreRuleSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParanoiaLevels {
    pub blocking: Option<i32>,
    pub detection: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OWASPScoreThresholds {
    pub inbound: Option<i32>,
    pub outbound: Option<i32>,
}

pub type OWASPIDRange = String;
pub type OWASPTag = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OWASPRuleExclusions {
    pub tags: Option<Vec<OWASPTag>>,
    pub ids: Option<Vec<i32>>,
    pub id_ranges: Option<Vec<OWASPIDRange>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OWASPCRS {
    pub paranoia_levels: Option<ParanoiaLevels>,
    pub score_thresholds: Option<OWASPScoreThresholds>,
    pub rule_exclusions: Option<OWASPRuleExclusions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficProtectionPolicyRuleSet {
    #[serde(rename = "type")]
    pub rule_set_type: TrafficProtectionPolicyRuleSetType,
    pub owasp_core_rule_set: Option<OWASPCRS>,
}

#[derive(CustomResource, Debug, Clone, Serialize, Deserialize)]
#[kube(
    group = "networking.datumapis.com",
    version = "v1alpha",
    kind = "TrafficProtectionPolicy",
    plural = "trafficprotectionpolicies",
    namespaced,
    status = "TrafficProtectionPolicyStatus",
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct TrafficProtectionPolicySpec {
    pub target_refs: Vec<LocalPolicyTargetReferenceWithSectionName>,
    pub mode: Option<TrafficProtectionPolicyMode>,
    pub sampling_percentage: Option<i32>,
    pub rule_sets: Option<Vec<TrafficProtectionPolicyRuleSet>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyAncestorRef {
    pub name: String,
    pub group: Option<String>,
    pub kind: Option<String>,
    pub namespace: Option<String>,
    pub port: Option<i32>,
    pub section_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyAncestorStatus {
    pub ancestor_ref: PolicyAncestorRef,
    pub controller_name: String,
    pub conditions: Option<Vec<metav1::Condition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficProtectionPolicyStatus {
    pub ancestors: Option<Vec<PolicyAncestorStatus>>,
}
