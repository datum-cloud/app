use data_encoding::BASE64;
use iroh::SecretKey;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<ConnectorKeyBinding>,
}

const CONNECTOR_KEY_BINDING_DOMAIN: &str = "datum-connector-binding/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorKeyBinding {
    pub signature: String,
}

impl ConnectorKeyBinding {
    pub fn message(connector_uid: &str, endpoint_id: &str) -> String {
        format!("{CONNECTOR_KEY_BINDING_DOMAIN}\n{connector_uid}\n{endpoint_id}")
    }

    pub fn sign(secret_key: &SecretKey, connector_uid: &str, endpoint_id: &str) -> Self {
        let message = Self::message(connector_uid, endpoint_id);
        let signature = secret_key.sign(message.as_bytes());
        Self {
            signature: BASE64.encode(&signature.to_bytes()),
        }
    }
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

impl ConnectorConnectionDetails {
    pub fn bind(mut self, secret_key: &SecretKey, connector_uid: &str) -> Self {
        if let Some(public_key) = self.public_key.as_mut() {
            public_key.binding = Some(ConnectorKeyBinding::sign(
                secret_key,
                connector_uid,
                &public_key.id,
            ));
        }
        self
    }
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

#[cfg(test)]
mod tests {
    use iroh::Signature;

    use super::*;

    const TEST_SECRET_KEY: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];
    const TEST_CONNECTOR_UID: &str = "6b0f3c1e-2a4d-4e8b-9c7f-1d2e3f4a5b6c";

    fn decode_signature(binding: &ConnectorKeyBinding) -> Signature {
        let bytes: [u8; Signature::LENGTH] = BASE64
            .decode(binding.signature.as_bytes())
            .unwrap()
            .try_into()
            .unwrap();
        Signature::from_bytes(&bytes)
    }

    #[test]
    fn binding_message_format() {
        assert_eq!(
            ConnectorKeyBinding::message("uid-1", "endpoint-1"),
            "datum-connector-binding/v1\nuid-1\nendpoint-1"
        );
    }

    #[test]
    fn binding_verifies_with_endpoint_key() {
        let secret_key = SecretKey::from_bytes(&TEST_SECRET_KEY);
        let endpoint_id = secret_key.public().to_string();
        let binding = ConnectorKeyBinding::sign(&secret_key, TEST_CONNECTOR_UID, &endpoint_id);
        let signature = decode_signature(&binding);
        let message = ConnectorKeyBinding::message(TEST_CONNECTOR_UID, &endpoint_id);
        secret_key
            .public()
            .verify(message.as_bytes(), &signature)
            .unwrap();

        let other =
            ConnectorKeyBinding::message("00000000-0000-0000-0000-000000000000", &endpoint_id);
        assert!(
            secret_key
                .public()
                .verify(other.as_bytes(), &signature)
                .is_err()
        );
    }

    #[test]
    fn binding_test_vector() {
        let secret_key = SecretKey::from_bytes(&TEST_SECRET_KEY);
        let endpoint_id = secret_key.public().to_string();
        let binding = ConnectorKeyBinding::sign(&secret_key, TEST_CONNECTOR_UID, &endpoint_id);
        assert_eq!(
            endpoint_id,
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
        assert_eq!(
            binding.signature,
            "pmdY3eHUNbxquZQw9swz5kxpfWoe1AnFU484kzICxgVRoO0Yn3NcyTJMc1QNzh+DDSU9UPI8V0T+U5Zk/cfuAg=="
        );
    }

    #[test]
    fn bind_signs_public_key_id() {
        let secret_key = SecretKey::from_bytes(&TEST_SECRET_KEY);
        let endpoint_id = secret_key.public().to_string();
        let details = ConnectorConnectionDetails {
            connection_type: ConnectorConnectionType::PublicKey,
            public_key: Some(ConnectorConnectionDetailsPublicKey {
                id: endpoint_id.clone(),
                discovery_mode: Some(PublicKeyDiscoveryMode::Dns),
                home_relay: "https://relay.example".to_string(),
                addresses: vec![],
                binding: None,
            }),
        };
        let unbound = serde_json::to_value(&details).unwrap();
        assert!(unbound["publicKey"].get("binding").is_none());

        let bound = serde_json::to_value(details.bind(&secret_key, TEST_CONNECTOR_UID)).unwrap();
        let expected = ConnectorKeyBinding::sign(&secret_key, TEST_CONNECTOR_UID, &endpoint_id);
        assert_eq!(
            bound["publicKey"]["binding"]["signature"],
            serde_json::Value::String(expected.signature)
        );
    }
}
