use std::env;

use serde::{Deserialize, Serialize};

use super::auth::AuthProvider;

const STAGING_API_URL: &str = "https://api.staging.env.datum.net";
const STAGING_ISSUER_URL: &str = "https://auth.staging.env.datum.net";
const STAGING_CLIENT_ID: &str = "360628090294044442";
const STAGING_WEB_URL: &str = "https://cloud.staging.env.datum.net";

const PROD_API_URL: &str = "https://api.datum.net";
const PROD_ISSUER_URL: &str = "https://auth.datum.net";
const PROD_CLIENT_ID: &str = "360628348109527815";
const PROD_WEB_URL: &str = "https://cloud.datum.net";

/// Environment for Datum API and auth. Use [`ApiEnv::from_env()`] or `ApiEnv::default()` to respect `DATUM_API_ENV`.
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum ApiEnv {
    Staging,
    Production,
}

impl ApiEnv {
    /// Uses `DATUM_API_ENV`: `staging` → Staging, anything else (including unset) → Production.
    pub fn from_env() -> Self {
        match env::var("DATUM_API_ENV").as_deref() {
            Ok("staging") => ApiEnv::Staging,
            _ => ApiEnv::Production,
        }
    }

    /// Storage key for per-env OAuth state (e.g. "staging", "production").
    pub fn oauth_storage_key(&self) -> &'static str {
        match self {
            ApiEnv::Staging => "staging",
            ApiEnv::Production => "production",
        }
    }

    pub fn api_url(&self) -> &'static str {
        match self {
            ApiEnv::Staging => STAGING_API_URL,
            ApiEnv::Production => PROD_API_URL,
        }
    }

    pub fn web_url(&self) -> &'static str {
        match self {
            ApiEnv::Staging => STAGING_WEB_URL,
            ApiEnv::Production => PROD_WEB_URL,
        }
    }

    pub fn auth_provider(&self) -> AuthProvider {
        match self {
            ApiEnv::Staging => AuthProvider {
                issuer_url: STAGING_ISSUER_URL.to_string(),
                client_id: STAGING_CLIENT_ID.to_string(),
                client_secret: None,
            },
            ApiEnv::Production => AuthProvider {
                issuer_url: PROD_ISSUER_URL.to_string(),
                client_id: PROD_CLIENT_ID.to_string(),
                client_secret: None,
            },
        }
    }
}

impl Default for ApiEnv {
    fn default() -> Self {
        Self::from_env()
    }
}
