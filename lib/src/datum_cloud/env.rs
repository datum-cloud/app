use serde::{Deserialize, Serialize};

use super::auth::AuthProvider;

const STAGING_API_URL: &str = "https://api.staging.env.datum.net";
const STAGING_ISSUER_URL: &str = "https://auth.staging.env.datum.net";
const STAGING_CLIENT_ID: &str = "351641555150375458";

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum ApiEnv {
    Staging,
}

impl ApiEnv {
    pub fn api_url(&self) -> &'static str {
        match self {
            ApiEnv::Staging => STAGING_API_URL,
        }
    }

    pub fn auth_provider(&self) -> AuthProvider {
        match self {
            ApiEnv::Staging => AuthProvider {
                issuer_url: STAGING_ISSUER_URL.to_string(),
                client_id: STAGING_CLIENT_ID.to_string(),
                client_secret: None,
            },
        }
    }
}
