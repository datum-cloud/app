use std::{str::FromStr, sync::Arc};

use iroh::Endpoint;
use iroh_services::{ApiSecret, Client, ClientHost, caps::NetDiagnosticsCap};
use n0_error::{Result, StackResultExt, StdResultExt};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::Repo;

const IROH_SERVICES_API_KEY: &str = "IROH_SERVICES_API_KEY";

/// Persisted diagnostics preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSettings {
    /// Whether network diagnostics collection is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Whether the user has seen the opt-in prompt.
    #[serde(default)]
    pub prompted: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for DiagnosticsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            prompted: false,
        }
    }
}

/// Read the iroh-services API key from the environment, falling back to a
/// build-time value. Returns `None` when neither is set.
pub(crate) fn iroh_services_api_key_from_env() -> Result<Option<ApiSecret>> {
    let key_str = match std::env::var(IROH_SERVICES_API_KEY) {
        Ok(s) => s,
        Err(_) => match option_env!("BUILD_IROH_SERVICES_API_KEY") {
            None => return Ok(None),
            Some(s) => s.to_string(),
        },
    };
    let api_secret =
        ApiSecret::from_str(&key_str).context("Failed to parse iroh-services API key from env")?;
    Ok(Some(api_secret))
}

/// State returned after diagnostics are started, kept alive to maintain
/// the ClientHost connection.
#[derive(Debug, Clone)]
pub struct DiagnosticsHandle {
    _client: Arc<Client>,
}

/// Start net diagnostics: connect to iroh-services, grant the
/// `NetDiagnosticsCap::GetAny` capability, and return a [`ClientHost`] that
/// must be registered on the Router so the service can dial back in.
///
/// Returns `(ClientHost, DiagnosticsHandle)` on success. The caller must
/// `.accept(CLIENT_HOST_ALPN, host)` on its Router builder.
pub async fn start_diagnostics(
    endpoint: &Endpoint,
    api_secret: ApiSecret,
) -> Result<(ClientHost, DiagnosticsHandle)> {
    let remote_id = api_secret.addr().id;
    info!(remote=%remote_id.fmt_short(), "connecting to iroh-services for net diagnostics");

    let client = Client::builder(endpoint)
        .api_secret(api_secret)?
        .build()
        .await
        .std_context("Failed to connect to iroh-services for diagnostics")?;

    // Grant the capability in a background task so we don't block startup
    // if the remote is temporarily unavailable.
    let grant_client = client.clone();
    tokio::spawn(async move {
        if let Err(err) = grant_client
            .grant_capability(remote_id, vec![NetDiagnosticsCap::GetAny])
            .await
        {
            warn!("Failed to grant net diagnostics capability: {err:#}");
        } else {
            info!("Granted NetDiagnosticsCap::GetAny to iroh-services");
        }
    });

    let host = ClientHost::new(endpoint);
    let handle = DiagnosticsHandle {
        _client: Arc::new(client),
    };
    Ok((host, handle))
}

impl Repo {
    const DIAGNOSTICS_SETTINGS_FILE: &str = "diagnostics_settings.yml";

    /// Load diagnostics settings from disk, returning defaults if the file
    /// does not exist.
    pub async fn diagnostics_settings(&self) -> Result<DiagnosticsSettings> {
        let path = self.path().join(Self::DIAGNOSTICS_SETTINGS_FILE);
        if !path.exists() {
            return Ok(DiagnosticsSettings::default());
        }
        let data = tokio::fs::read_to_string(&path)
            .await
            .context("failed to read diagnostics settings")?;
        let settings: DiagnosticsSettings =
            serde_yml::from_str(&data).std_context("failed to parse diagnostics settings")?;
        Ok(settings)
    }

    /// Persist diagnostics settings to disk.
    pub async fn write_diagnostics_settings(&self, settings: &DiagnosticsSettings) -> Result<()> {
        let path = self.path().join(Self::DIAGNOSTICS_SETTINGS_FILE);
        let data = serde_yml::to_string(settings).anyerr()?;
        tokio::fs::write(&path, data)
            .await
            .context("failed to write diagnostics settings")?;
        Ok(())
    }
}
