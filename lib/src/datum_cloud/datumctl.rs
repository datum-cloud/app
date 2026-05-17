//! Bridge for sourcing auth + selected project from a colocated `datumctl` install.
//!
//! `datum-connect` doesn't run its own OAuth flow when this bridge is used; it reads the
//! datumctl config file for the active session and selected context, and shells out to
//! `datumctl auth get-token` for access tokens.

use std::path::PathBuf;
use std::process::Stdio;

use data_encoding::BASE64URL_NOPAD;
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::SelectedContext;
use crate::datum_cloud::auth::UserProfile;

const DATUMCTL_BIN: &str = "datumctl";
const CONFIG_DIR: &str = ".datumctl";
const CONFIG_FILE: &str = "config";

/// Subset of the datumctl YAML config we depend on. Unknown fields are ignored so schema
/// additions on the datumctl side don't break us.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DatumctlConfig {
    #[serde(default, rename = "active-session")]
    pub active_session: Option<String>,
    #[serde(default, rename = "current-context")]
    pub current_context: Option<String>,
    #[serde(default)]
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub contexts: Vec<Context>,
    #[serde(default)]
    pub cache: Cache,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Session {
    pub name: String,
    pub endpoint: Endpoint,
    #[serde(default, rename = "user-email")]
    pub user_email: Option<String>,
    #[serde(default, rename = "user-name")]
    pub user_name: Option<String>,
    #[serde(default, rename = "user-key")]
    pub user_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Endpoint {
    pub server: String,
    #[serde(default, rename = "auth-hostname")]
    pub auth_hostname: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Context {
    pub name: String,
    #[serde(default, rename = "organization-id")]
    pub organization_id: Option<String>,
    #[serde(default, rename = "project-id")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Cache {
    #[serde(default)]
    pub organizations: Vec<CachedOrg>,
    #[serde(default)]
    pub projects: Vec<CachedProject>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedOrg {
    pub id: String,
    #[serde(default, rename = "display-name")]
    pub display_name: Option<String>,
    #[serde(default, rename = "type")]
    pub org_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedProject {
    pub id: String,
    #[serde(default, rename = "display-name")]
    pub display_name: Option<String>,
    #[serde(default, rename = "org-id")]
    pub org_id: Option<String>,
}

/// Everything `datum-connect` needs to talk to the Datum API: an active session, the
/// resolved selected context, and (optionally) a project override.
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub session_name: String,
    pub api_url: String,
    pub profile: UserProfile,
    pub selected_context: SelectedContext,
}

fn config_path() -> Result<PathBuf> {
    let home = dirs_next::home_dir()
        .std_context("Could not determine home directory for datumctl config")?;
    Ok(home.join(CONFIG_DIR).join(CONFIG_FILE))
}

/// Read and parse the datumctl config file. Fails with a friendly message if the file is
/// missing (user hasn't run `datumctl auth login` yet).
pub async fn read_config() -> Result<DatumctlConfig> {
    let path = config_path()?;
    let data = match tokio::fs::read_to_string(&path).await {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            n0_error::bail_any!(
                "datumctl config not found at {}. Run `datumctl auth login` first.",
                path.display()
            );
        }
        Err(err) => {
            return Err(err)
                .with_std_context(|_| format!("Failed to read datumctl config at {}", path.display()));
        }
    };
    let config: DatumctlConfig = serde_yml::from_str(&data)
        .with_std_context(|_| format!("Failed to parse datumctl config at {}", path.display()))?;
    Ok(config)
}

/// Resolve the active session and selected context from the datumctl config.
///
/// `project_override` lets the caller pin a specific project (e.g. `datum-connect tunnel
/// --project=...`). When set, the org is looked up via the cache; if the project isn't in
/// the cache we trust the user and pass through with the project_id alone.
pub async fn resolve(project_override: Option<String>) -> Result<ResolvedSession> {
    let config = read_config().await?;
    let mut resolved = resolve_from_config(&config, project_override)?;
    // datumctl's session.user-key holds the user's email, but the IAM API at
    // /apis/iam.miloapis.com/v1alpha1/users/{user_id} requires the OIDC subject.
    // Pull a fresh access token and read its `sub` claim.
    let token = fetch_access_token(Some(&resolved.session_name)).await?;
    resolved.profile.user_id = decode_jwt_sub(&token)
        .std_context("Failed to extract user ID from datumctl access token")?;
    Ok(resolved)
}

pub fn resolve_from_config(
    config: &DatumctlConfig,
    project_override: Option<String>,
) -> Result<ResolvedSession> {
    let current_name = config
        .current_context
        .as_deref()
        .filter(|s| !s.is_empty())
        .std_context(
            "No current context set in datumctl. Run `datumctl project switch` to pick one.",
        )?;
    let context = config
        .contexts
        .iter()
        .find(|c| c.name == current_name)
        .with_std_context(|_| {
            format!(
                "datumctl current-context '{current_name}' not present in contexts list. \
                 Re-run `datumctl project switch`."
            )
        })?;

    let session_name = context
        .session
        .as_deref()
        .or(config.active_session.as_deref())
        .filter(|s| !s.is_empty())
        .std_context("datumctl current context is not bound to a session. Run `datumctl auth login`.")?;
    let session = config
        .sessions
        .iter()
        .find(|s| s.name == session_name)
        .with_std_context(|_| {
            format!("datumctl session '{session_name}' is referenced but missing. Re-run `datumctl auth login`.")
        })?;

    let org_id = context
        .organization_id
        .clone()
        .filter(|s| !s.is_empty())
        .std_context("Selected context has no organization. Use `datumctl project switch` to pick a project.")?;

    let project_id = match project_override {
        Some(pid) => pid,
        None => context
            .project_id
            .clone()
            .filter(|s| !s.is_empty())
            .std_context(
                "Selected context has no project. Use `datumctl project switch` to pick one, \
                 or pass `--project`.",
            )?,
    };

    let org = config.cache.organizations.iter().find(|o| o.id == org_id);
    let project = config.cache.projects.iter().find(|p| p.id == project_id);

    let selected_context = SelectedContext {
        org_id: org_id.clone(),
        org_name: org
            .and_then(|o| o.display_name.clone())
            .unwrap_or_else(|| org_id.clone()),
        project_id: project_id.clone(),
        project_name: project
            .and_then(|p| p.display_name.clone())
            .unwrap_or_else(|| project_id.clone()),
        org_type: org
            .and_then(|o| o.org_type.clone())
            .unwrap_or_else(|| "personal".to_string()),
    };

    let profile = UserProfile {
        user_id: session.user_key.clone().unwrap_or_default(),
        email: session.user_email.clone().unwrap_or_default(),
        first_name: session.user_name.clone(),
        last_name: None,
        avatar_url: None,
        registration_approval: None,
    };

    Ok(ResolvedSession {
        session_name: session.name.clone(),
        api_url: session.endpoint.server.trim_end_matches('/').to_string(),
        profile,
        selected_context,
    })
}

/// Shell out to `datumctl auth get-token [--session=NAME]` and return the trimmed token.
/// Hard fails if the binary is missing or exits non-zero.
pub async fn fetch_access_token(session_name: Option<&str>) -> Result<String> {
    let mut cmd = Command::new(DATUMCTL_BIN);
    cmd.arg("auth").arg("get-token");
    if let Some(name) = session_name {
        cmd.arg("--session").arg(name);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd.output().await.map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            n0_error::anyerr!(
                "`datumctl` binary not found on PATH. Install datumctl to use datum-connect."
            )
        } else {
            n0_error::anyerr!("Failed to spawn datumctl: {err}")
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            n0_error::bail_any!(
                "datumctl auth get-token exited with status {}. Try `datumctl auth login`.",
                output.status
            );
        } else {
            n0_error::bail_any!("datumctl auth get-token failed: {stderr}");
        }
    }

    let token = String::from_utf8(output.stdout)
        .std_context("datumctl returned non-UTF8 token output")?
        .trim()
        .to_string();
    if token.is_empty() {
        n0_error::bail_any!("datumctl returned an empty access token");
    }
    Ok(token)
}

/// Extract the `sub` claim from a JWT access token without verifying its signature.
/// We rely on datumctl to have already validated the token; we only need the subject
/// to build IAM API paths like `/apis/iam.miloapis.com/v1alpha1/users/{sub}`.
fn decode_jwt_sub(token: &str) -> Result<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        n0_error::bail_any!(
            "access token is not a JWT (expected 3 dot-separated segments, got {})",
            parts.len()
        );
    }
    let payload_bytes = BASE64URL_NOPAD
        .decode(parts[1].as_bytes())
        .std_context("Failed to base64url-decode JWT payload")?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .std_context("Failed to parse JWT payload as JSON")?;
    payload
        .get("sub")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .std_context("JWT payload is missing `sub` claim")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_jwt(payload_json: &str) -> String {
        let header = BASE64URL_NOPAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = BASE64URL_NOPAD.encode(payload_json.as_bytes());
        let sig = BASE64URL_NOPAD.encode(b"sig");
        format!("{header}.{payload}.{sig}")
    }

    #[test]
    fn decode_jwt_sub_reads_subject_claim() {
        let token = make_jwt(r#"{"sub":"user-abc123","email":"a@b.com"}"#);
        assert_eq!(decode_jwt_sub(&token).unwrap(), "user-abc123");
    }

    #[test]
    fn decode_jwt_sub_rejects_non_jwt() {
        assert!(decode_jwt_sub("not-a-jwt").is_err());
        assert!(decode_jwt_sub("a.b").is_err());
    }

    #[test]
    fn decode_jwt_sub_rejects_missing_sub() {
        let token = make_jwt(r#"{"email":"a@b.com"}"#);
        assert!(decode_jwt_sub(&token).is_err());
    }
}
