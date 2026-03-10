use kube::CustomResource;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleReference {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

#[derive(CustomResource, Debug, Clone, Serialize, Deserialize)]
#[kube(
    group = "iam.miloapis.com",
    version = "v1alpha1",
    kind = "UserInvitation",
    plural = "userinvitations",
    namespaced,
    schema = "disabled"
)]
#[serde(rename_all = "camelCase")]
pub struct UserInvitationSpec {
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<RoleReference>>,
}
