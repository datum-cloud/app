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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApiEnv {
    Staging,
    Production,
    /// Custom endpoint resolved at runtime (e.g. from datumctl's session config).
    /// `auth_provider` is best-effort; embedded OAuth flows will use it but datumctl-backed
    /// clients never trigger those.
    Custom {
        api_url: String,
        web_url: String,
        auth_provider: AuthProvider,
        oauth_storage_key: String,
    },
}

impl ApiEnv {
    /// Uses `DATUM_API_ENV`: `staging` → Staging, anything else (including unset) → Production.
    pub fn from_env() -> Self {
        match env::var("DATUM_API_ENV").as_deref() {
            Ok("staging") => ApiEnv::Staging,
            _ => ApiEnv::Production,
        }
    }

    /// Map a known API URL to a built-in variant, falling back to a `Custom` env with a
    /// stubbed auth provider. The stubbed provider is only valid for clients that never
    /// trigger embedded OIDC (i.e. datumctl-backed clients).
    pub fn from_api_url(api_url: &str) -> Self {
        let trimmed = api_url.trim_end_matches('/');
        if trimmed == PROD_API_URL {
            ApiEnv::Production
        } else if trimmed == STAGING_API_URL {
            ApiEnv::Staging
        } else {
            ApiEnv::Custom {
                api_url: trimmed.to_string(),
                web_url: trimmed.to_string(),
                auth_provider: AuthProvider {
                    issuer_url: trimmed.to_string(),
                    client_id: String::new(),
                    client_secret: None,
                },
                oauth_storage_key: "custom".to_string(),
            }
        }
    }

    /// Storage key for per-env OAuth state (e.g. "staging", "production").
    pub fn oauth_storage_key(&self) -> &str {
        match self {
            ApiEnv::Staging => "staging",
            ApiEnv::Production => "production",
            ApiEnv::Custom {
                oauth_storage_key, ..
            } => oauth_storage_key,
        }
    }

    pub fn api_url(&self) -> &str {
        match self {
            ApiEnv::Staging => STAGING_API_URL,
            ApiEnv::Production => PROD_API_URL,
            ApiEnv::Custom { api_url, .. } => api_url,
        }
    }

    pub fn web_url(&self) -> &str {
        match self {
            ApiEnv::Staging => STAGING_WEB_URL,
            ApiEnv::Production => PROD_WEB_URL,
            ApiEnv::Custom { web_url, .. } => web_url,
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
            ApiEnv::Custom { auth_provider, .. } => auth_provider.clone(),
        }
    }
}

impl Default for ApiEnv {
    fn default() -> Self {
        Self::from_env()
    }
}
