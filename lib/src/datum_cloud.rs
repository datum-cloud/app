use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use arc_swap::ArcSwap;
use chrono::{Duration, Utc};
use n0_error::{Result, StackResultExt, StdResultExt};
use n0_future::{BufferedStreamExt, TryStreamExt, task::AbortOnDropHandle};
use rand::Rng;
use tokio::sync::{Mutex, watch};
use tracing::warn;

use crate::datum_apis::user_invitation::RoleReference;
use crate::http_user_agent::datum_http_user_agent;
use crate::{ProjectControlPlaneClient, Repo, SelectedContext};

pub use self::{
    auth::{AuthClient, AuthState, LoginState, MaybeAuth, NotLoggedIn, Unauthorized, UserProfile},
    env::ApiEnv,
};

mod auth;
mod env;

const ORGS_PROJECTS_DEDUP_WINDOW: StdDuration = StdDuration::from_secs(2);

#[derive(derive_more::Debug, Clone)]
pub struct DatumCloudClient {
    env: ApiEnv,
    auth: AuthClient,
    http: reqwest::Client,
    session: SessionStateWrapper,
    orgs_projects_fetch_gate: Arc<Mutex<Option<Instant>>>,
    _session_task: Option<Arc<AbortOnDropHandle<()>>>,
}

impl DatumCloudClient {
    pub async fn with_repo(env: ApiEnv, repo: Repo) -> Result<Self> {
        let auth = AuthClient::with_repo(env, repo.clone()).await?;
        let session = SessionStateWrapper::from_repo(Some(repo)).await?;
        let http = reqwest::Client::builder()
            .user_agent(datum_http_user_agent())
            .build()
            .anyerr()?;
        let mut client = Self {
            env,
            auth,
            http,
            session,
            orgs_projects_fetch_gate: Arc::new(Mutex::new(None)),
            _session_task: None,
        };
        client.start_session_sync();
        Ok(client)
    }

    pub async fn new(env: ApiEnv) -> Result<Self> {
        let auth = AuthClient::new(env).await?;
        let session = SessionStateWrapper::empty();
        let http = reqwest::Client::builder()
            .user_agent(datum_http_user_agent())
            .build()
            .anyerr()?;
        let mut client = Self {
            env,
            auth,
            http,
            session,
            orgs_projects_fetch_gate: Arc::new(Mutex::new(None)),
            _session_task: None,
        };
        client.start_session_sync();
        Ok(client)
    }

    pub fn login_state(&self) -> LoginState {
        self.auth.login_state()
    }

    pub fn api_url(&self) -> &'static str {
        self.env.api_url()
    }

    pub fn web_url(&self) -> &'static str {
        self.env.web_url()
    }

    pub fn auth(&self) -> &AuthClient {
        &self.auth
    }

    pub fn auth_update_watch(&self) -> watch::Receiver<u64> {
        self.auth.auth_update_watch()
    }

    pub fn auth_state(&self) -> Arc<MaybeAuth> {
        self.auth.load()
    }

    pub fn selected_context(&self) -> Option<SelectedContext> {
        self.session.selected_context()
    }

    pub fn selected_context_watch(&self) -> watch::Receiver<Option<SelectedContext>> {
        self.session.selected_context_watch()
    }

    pub async fn set_selected_context(
        &self,
        selected_context: Option<SelectedContext>,
    ) -> Result<()> {
        self.session.set_selected_context(selected_context).await
    }

    fn project_control_plane_url(&self, project_id: &str) -> String {
        format!(
            "{}/apis/resourcemanager.miloapis.com/v1alpha1/projects/{project_id}/control-plane",
            self.api_url()
        )
    }

    pub async fn project_control_plane_client(
        &self,
        project_id: &str,
    ) -> Result<ProjectControlPlaneClient> {
        let auth_state = self.auth().load_refreshed().await?;
        let auth = auth_state.get()?;
        self.project_control_plane_client_with_token(project_id, auth.tokens.access_token.secret())
    }

    pub async fn project_control_plane_client_active(
        &self,
    ) -> Result<Option<ProjectControlPlaneClient>> {
        let Some(selected) = self.selected_context() else {
            return Ok(None);
        };
        Ok(Some(
            self.project_control_plane_client(&selected.project_id)
                .await?,
        ))
    }

    /// Lists IAM roles in a namespace (org-scoped control plane).
    /// Uses the namespaced roles API; defaults to `datum-cloud` to match the web app.
    pub async fn list_roles(
        &self,
        org_id: &str,
        namespace: Option<&str>,
    ) -> Result<Vec<RoleSummary>> {
        let ns = namespace.unwrap_or("datum-cloud");
        let json = self
            .fetch(
                Scope::Org(org_id.to_string()),
                Api::Iam(IamResource::NamespacedRoles(ns.to_string())),
            )
            .await?;

        parse_role_list(&json).context("Failed to parse roles list")
    }

    /// Creates a UserInvitation at org scope (org-scoped API).
    /// Payload matches web app adapter: expirationDate 24h, organizationRef, state Pending, role namespace default milo-system.
    pub async fn create_user_invitation_org(
        &self,
        org_id: &str,
        email: &str,
        given_name: Option<&str>,
        family_name: Option<&str>,
        roles: Option<Vec<RoleReference>>,
    ) -> Result<()> {
        let namespace = format!("organization-{org_id}");
        let url = self.url(
            Scope::Org(org_id.to_string()),
            Api::Iam(IamResource::NamespacedUserInvitations(namespace)),
        );
        let name = invitation_name(org_id);
        let expiration_date = (Utc::now() + Duration::hours(24)).to_rfc3339();
        let roles_payload: Vec<serde_json::Value> = roles
            .as_ref()
            .map(|r| {
                r.iter()
                    .map(|ref_| {
                        let ns = ref_.namespace.as_deref().unwrap_or("milo-system");
                        serde_json::json!({ "name": ref_.name, "namespace": ns })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let body = serde_json::json!({
            "apiVersion": "iam.miloapis.com/v1alpha1",
            "kind": "UserInvitation",
            "metadata": {
                "name": name,
            },
            "spec": {
                "email": email.trim(),
                "expirationDate": expiration_date,
                "organizationRef": { "name": org_id },
                "givenName": given_name.and_then(|s| {
                    let t = s.trim();
                    if t.is_empty() { None } else { Some(t) }
                }),
                "familyName": family_name.and_then(|s| {
                    let t = s.trim();
                    if t.is_empty() { None } else { Some(t) }
                }),
                "roles": roles_payload,
                "state": "Pending"
            }
        });
        self.post_json(&url, &body).await?;
        Ok(())
    }

    pub fn orgs_projects_cache(&self) -> Vec<OrganizationWithProjects> {
        self.session.orgs_projects()
    }

    pub fn orgs_projects_watch(&self) -> watch::Receiver<Vec<OrganizationWithProjects>> {
        self.session.orgs_projects_watch()
    }

    pub async fn orgs_and_projects(&self) -> Result<Vec<OrganizationWithProjects>> {
        // Serialize callers and collapse startup bursts.
        let mut last_fetch = self.orgs_projects_fetch_gate.lock().await;
        let cached = self.session.orgs_projects();
        if !cached.is_empty()
            && last_fetch
                .as_ref()
                .is_some_and(|instant| instant.elapsed() < ORGS_PROJECTS_DEDUP_WINDOW)
        {
            return Ok(cached);
        }

        let orgs = self.orgs().await?;

        let stream = n0_future::stream::iter(orgs.into_iter().map(async |org| {
            let projects = self.projects(&org.resource_id).await?;
            n0_error::Ok(OrganizationWithProjects { org, projects })
        }));
        let mut list: Vec<OrganizationWithProjects> =
            stream.buffered_unordered(16).try_collect().await?;
        for org in &mut list {
            org.projects
                .sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
        }
        list.sort_by(|a, b| a.org.resource_id.cmp(&b.org.resource_id));
        let _ = self.session.set_orgs_projects(list.clone());
        *last_fetch = Some(Instant::now());
        Ok(list)
    }

    pub async fn orgs(&self) -> Result<Vec<Organization>> {
        fn parse_orgs(json: &serde_json::Value) -> Option<Vec<Organization>> {
            let items = json.as_object()?.get("items")?.as_array()?;
            let parsed = items.iter().filter_map(|item| {
                let item = item.as_object()?;
                let org = item
                    .get("status")?
                    .as_object()?
                    .get("organization")?
                    .as_object()?;
                let name = org.get("displayName")?.as_str()?;
                // `type` is deprecated (ignored when the UnifiedOrganizations feature
                // gate is enabled) and absent from most memberships; treat as optional.
                let r#type = org.get("type").and_then(|v| v.as_str()).unwrap_or_default();
                let spec = item.get("spec")?.as_object()?;
                let resource_id = spec
                    .get("organizationRef")?
                    .as_object()?
                    .get("name")?
                    .as_str()?;
                Some(Organization {
                    resource_id: resource_id.to_string(),
                    display_name: name.to_string(),
                    r#type: r#type.to_string(),
                })
            });
            Some(parsed.collect())
        }

        let json = self
            .fetch(
                Scope::user(&self.auth.load().get()?.profile),
                Api::ResourceManager(ResourceManager::OrganizationMemberships),
            )
            .await?;
        let mut orgs: Vec<Organization> = parse_orgs(&json).context("Failed to parse reply")?;
        // A user can hold multiple memberships in the same org; keep one entry per org.
        let mut seen = std::collections::HashSet::new();
        orgs.retain(|org| seen.insert(org.resource_id.clone()));
        Ok(orgs)
    }

    pub async fn projects(&self, org_id: &str) -> Result<Vec<Project>> {
        fn parse_projects(json: &serde_json::Value) -> Option<Vec<Project>> {
            let items = json.as_object()?.get("items")?.as_array()?;
            let parsed = items.iter().filter_map(|item| {
                let item = item.as_object()?;
                let metadata = item.get("metadata")?.as_object()?;
                let resource_id = metadata.get("name")?.as_str()?;
                let display_name = metadata
                    .get("annotations")?
                    .as_object()?
                    .get("kubernetes.io/description")?
                    .as_str()?;
                // let uid = metadata.get("uid")?.as_str()?;
                Some(Project {
                    resource_id: resource_id.to_string(),
                    display_name: display_name.to_string(),
                })
            });
            Some(parsed.collect())
        }

        let json = self
            .fetch(
                Scope::Org(org_id.to_string()),
                Api::ResourceManager(ResourceManager::Projects),
            )
            .await?;
        parse_projects(&json).context("Failed to parse reply")
    }

    fn url(&self, scope: Scope, api: Api) -> String {
        let base = self.env.api_url();
        format!("{base}{scope}{api}")
    }

    async fn fetch(&self, scope: Scope, api: Api) -> Result<serde_json::Value> {
        let url = self.url(scope, api);
        self.fetch_direct(&url).await
    }

    async fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<()> {
        tracing::debug!("POST {url}");
        let res = self
            .request_with_auth_retry(|token| {
                self.http
                    .post(url)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .json(body)
            })
            .await?;
        let status = res.status();
        if !status.is_success() {
            let text = match res.text().await {
                Ok(text) => text,
                Err(err) => err.to_string(),
            };
            warn!(%url, "Request failed: {status} {text}");
            n0_error::bail_any!("Request failed with status {status}");
        }
        Ok(())
    }

    async fn fetch_direct(&self, url: &str) -> Result<serde_json::Value> {
        tracing::debug!("GET {url}");
        let res = self
            .request_with_auth_retry(|token| {
                self.http
                    .get(url)
                    .header("Authorization", format!("Bearer {token}"))
            })
            .await?;
        let status = res.status();
        if !status.is_success() {
            let text = match res.text().await {
                Ok(text) => text,
                Err(err) => err.to_string(),
            };
            warn!(%url, "Request failed: {status} {text}");
            n0_error::bail_any!("Request failed with status {status}");
        }

        let json: serde_json::Value = res
            .json()
            .await
            .std_context("Failed to parse response text as JSON")?;
        Ok(json)
    }

    /// Send an authenticated request and, on 401/403, force a token refresh and retry once.
    /// If the second attempt still returns 401/403, clear the local auth state and return
    /// [`Unauthorized`] so the UI redirects to login.
    ///
    /// The closure builds the request (sans `.send()`) given the current bearer token, so we
    /// can rebuild it after a refresh without the caller having to reconstruct headers/body.
    async fn request_with_auth_retry<F>(&self, build: F) -> Result<reqwest::Response>
    where
        F: Fn(&str) -> reqwest::RequestBuilder,
    {
        let auth_state = self.auth.load_refreshed().await?;
        let auth = auth_state.get()?;
        let res = build(auth.tokens.access_token.secret())
            .send()
            .await
            .inspect_err(|e| warn!("Request failed: {e:#}"))
            .std_context("HTTP request failed")?;
        if !is_auth_failure(res.status()) {
            return Ok(res);
        }

        warn!(
            status = %res.status(),
            "Server rejected token; attempting forced refresh"
        );
        if let Err(err) = self.auth.force_refresh().await {
            warn!("Forced auth refresh failed: {err:#}");
            return Err(Unauthorized.into());
        }
        let auth_state = self.auth.load();
        let Ok(auth) = auth_state.get() else {
            return Err(Unauthorized.into());
        };
        let retry = build(auth.tokens.access_token.secret())
            .send()
            .await
            .inspect_err(|e| warn!("Retried request failed: {e:#}"))
            .std_context("HTTP request retry failed")?;
        if is_auth_failure(retry.status()) {
            warn!(
                status = %retry.status(),
                "Server still rejected token after refresh; logging out"
            );
            if let Err(err) = self.auth.logout().await {
                warn!("Failed to clear auth state after persistent 401/403: {err:#}");
            }
            return Err(Unauthorized.into());
        }
        Ok(retry)
    }

    fn project_control_plane_client_with_token(
        &self,
        project_id: &str,
        access_token: &str,
    ) -> Result<ProjectControlPlaneClient> {
        let server_url = self.project_control_plane_url(project_id);
        ProjectControlPlaneClient::new(
            project_id.to_string(),
            server_url,
            access_token.to_string(),
            self.clone(),
        )
    }

    pub async fn refresh_orgs_projects_and_validate_context(&self) -> Result<()> {
        let list = self.orgs_and_projects().await?;
        let selected = self.selected_context();
        let Some(selected) = selected else {
            return Ok(());
        };

        let is_valid = list.iter().any(|org| {
            if org.org.resource_id != selected.org_id {
                return false;
            }
            org.projects
                .iter()
                .any(|project| project.resource_id == selected.project_id)
        });

        if !is_valid {
            self.set_selected_context(None).await?;
        } else {
            self.set_selected_context(Some(selected)).await?;
        }
        Ok(())
    }

    fn start_session_sync(&mut self) {
        if self._session_task.is_some() {
            return;
        }
        let client = self.clone();
        let mut login_rx = self.auth.login_state_watch();
        let mut auth_update_rx = self.auth.auth_update_watch();
        let task = tokio::spawn(async move {
            if *login_rx.borrow() != LoginState::Missing {
                let _ = client.refresh_orgs_projects_and_validate_context().await;
            }
            loop {
                tokio::select! {
                    res = login_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                    res = auth_update_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                }
                if *login_rx.borrow() != LoginState::Missing {
                    let _ = client.refresh_orgs_projects_and_validate_context().await;
                }
            }
        });
        self._session_task = Some(Arc::new(AbortOnDropHandle::new(task)));
    }
}

#[derive(Debug, Clone, Default)]
struct SessionStateWrapper {
    selected_context: Arc<ArcSwap<Option<SelectedContext>>>,
    selected_context_tx: watch::Sender<Option<SelectedContext>>,
    orgs_projects: Arc<ArcSwap<Vec<OrganizationWithProjects>>>,
    orgs_projects_tx: watch::Sender<Vec<OrganizationWithProjects>>,
    repo: Option<Repo>,
}

impl SessionStateWrapper {
    fn empty() -> Self {
        let (selected_context_tx, _) = watch::channel(None);
        let (orgs_projects_tx, _) = watch::channel(Vec::new());
        Self {
            selected_context: Arc::new(ArcSwap::from_pointee(None)),
            selected_context_tx,
            orgs_projects: Arc::new(ArcSwap::from_pointee(Vec::new())),
            orgs_projects_tx,
            repo: None,
        }
    }

    async fn from_repo(repo: Option<Repo>) -> Result<Self> {
        let selected = if let Some(repo) = repo.as_ref() {
            repo.read_selected_context().await?
        } else {
            None
        };
        let (selected_context_tx, _) = watch::channel(selected.clone());
        let (orgs_projects_tx, _) = watch::channel(Vec::new());
        Ok(Self {
            selected_context: Arc::new(ArcSwap::from_pointee(selected)),
            selected_context_tx,
            orgs_projects: Arc::new(ArcSwap::from_pointee(Vec::new())),
            orgs_projects_tx,
            repo,
        })
    }

    fn selected_context(&self) -> Option<SelectedContext> {
        self.selected_context.load_full().as_ref().clone()
    }

    fn selected_context_watch(&self) -> watch::Receiver<Option<SelectedContext>> {
        self.selected_context_tx.subscribe()
    }

    async fn set_selected_context(&self, selected_context: Option<SelectedContext>) -> Result<()> {
        let current = self.selected_context.load_full();
        if current.as_ref().as_ref() != selected_context.as_ref() {
            if let Some(repo) = self.repo.as_ref() {
                repo.write_selected_context(selected_context.as_ref())
                    .await?;
            }
            self.selected_context
                .store(Arc::new(selected_context.clone()));
        }
        let _ = self.selected_context_tx.send(selected_context);
        Ok(())
    }

    fn orgs_projects(&self) -> Vec<OrganizationWithProjects> {
        self.orgs_projects.load_full().as_ref().clone()
    }

    fn orgs_projects_watch(&self) -> watch::Receiver<Vec<OrganizationWithProjects>> {
        self.orgs_projects_tx.subscribe()
    }

    fn set_orgs_projects(&self, orgs_projects: Vec<OrganizationWithProjects>) -> bool {
        let current = self.orgs_projects.load_full();
        if current.as_ref().as_slice() == orgs_projects.as_slice() {
            return false;
        }
        self.orgs_projects.store(Arc::new(orgs_projects.clone()));
        let _ = self.orgs_projects_tx.send(orgs_projects);
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    pub resource_id: String,
    pub display_name: String,
    pub r#type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationWithProjects {
    pub org: Organization,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub resource_id: String,
    pub display_name: String,
}

/// Summary of an IAM Role for listing (e.g. in invite dialog).
#[derive(Debug, Clone)]
pub struct RoleSummary {
    pub name: String,
    pub namespace: Option<String>,
    /// Human-readable label; from annotations["kubernetes.io/display-name"] ?? displayName ?? name.
    pub display_name: Option<String>,
    /// Human-readable description of the role's permissions (e.g. from spec or annotations).
    pub description: Option<String>,
    /// Sort order for display; from annotations["taxonomy.miloapis.com/sort-order"], default 999.
    pub sort_order: u32,
}

/// Returns true if the role should be shown: either it has no status (include by default) or
/// its status has Ready condition True. Excludes only when Ready is explicitly not True.
fn role_status_is_success(item: &serde_json::Value) -> bool {
    let status = match item.get("status") {
        Some(s) => s,
        None => return true, // no status -> include
    };
    let conditions = match status.get("conditions").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return true, // no conditions -> include
    };
    let ready_false = conditions.iter().any(|c| {
        let c = c.as_object();
        let type_ = c.and_then(|o| o.get("type")).and_then(|v| v.as_str());
        let status = c.and_then(|o| o.get("status")).and_then(|v| v.as_str());
        type_.map_or(false, |t| t == "Ready") && status.map_or(false, |s| s == "False")
    });
    !ready_false // include unless Ready is explicitly False
}

fn parse_role_list(json: &serde_json::Value) -> Result<Vec<RoleSummary>> {
    let items = json
        .get("items")
        .and_then(|v| v.as_array())
        .context("Missing or invalid items")?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if !role_status_is_success(item) {
            continue;
        }
        let meta = item.get("metadata").context("Missing metadata")?;
        let name = meta
            .get("name")
            .and_then(|v| v.as_str())
            .context("Missing metadata.name")?
            .to_string();
        let namespace = meta
            .get("namespace")
            .and_then(|v| v.as_str())
            .map(String::from);
        let display_name = meta
            .get("annotations")
            .and_then(|a| a.get("kubernetes.io/display-name"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                item.get("displayName")
                    .or_else(|| item.get("spec").and_then(|s| s.get("displayName")))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
        let description = item
            .get("spec")
            .and_then(|s| s.get("description"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                meta.get("annotations")
                    .and_then(|a| a.get("kubernetes.io/description"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
        let sort_order = meta
            .get("annotations")
            .and_then(|a| a.get("taxonomy.miloapis.com/sort-order"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(999);
        out.push(RoleSummary {
            name,
            namespace,
            display_name,
            description,
            sort_order,
        });
    }
    out.sort_by_key(|r| r.sort_order);
    Ok(out)
}

#[derive(Debug, Clone, derive_more::Display)]
enum Scope {
    #[display("/apis/iam.miloapis.com/v1alpha1/users/{_0}")]
    User(String),
    #[display("/apis/resourcemanager.miloapis.com/v1alpha1/organizations/{_0}")]
    Org(String),
}

impl Scope {
    fn user(profile: &UserProfile) -> Self {
        Self::User(profile.user_id.to_string())
    }
}

#[derive(Debug, Clone, derive_more::Display)]
enum Api {
    #[display("/control-plane/apis/resourcemanager.miloapis.com/v1alpha1{_0}")]
    ResourceManager(ResourceManager),
    #[display("/control-plane/apis/iam.miloapis.com/v1alpha1{_0}")]
    Iam(IamResource),
}

#[derive(Debug, Clone, derive_more::Display)]
enum ResourceManager {
    #[display("/organizationmemberships")]
    OrganizationMemberships,
    #[display("/projects")]
    Projects,
}

#[derive(Debug, Clone, derive_more::Display)]
enum IamResource {
    /// Namespaced list: /namespaces/{namespace}/roles (matches web app listIamMiloapisComV1Alpha1NamespacedRole)
    #[display("/namespaces/{_0}/roles")]
    NamespacedRoles(String),
    /// Namespaced create: /namespaces/organization-{orgId}/userinvitations (matches web app createIamMiloapisComV1Alpha1NamespacedUserInvitation)
    #[display("/namespaces/{_0}/userinvitations")]
    NamespacedUserInvitations(String),
}

/// Produces invitation resource name matching web app: `{organizationId}-{random8}`.
fn invitation_name(org_id: &str) -> String {
    let suffix: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(8)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();
    format!("{org_id}-{suffix}")
}

/// True if the response status indicates the bearer token is no longer accepted.
fn is_auth_failure(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN
}

#[cfg(test)]
mod auth_failure_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn classifies_401_and_403_as_auth_failures() {
        assert!(is_auth_failure(reqwest::StatusCode::UNAUTHORIZED));
        assert!(is_auth_failure(reqwest::StatusCode::FORBIDDEN));
    }

    #[test]
    fn does_not_classify_other_statuses_as_auth_failures() {
        assert!(!is_auth_failure(reqwest::StatusCode::OK));
        assert!(!is_auth_failure(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_auth_failure(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!is_auth_failure(reqwest::StatusCode::BAD_REQUEST));
        // 407 Proxy Authentication Required is distinct from end-user auth failures;
        // we intentionally do not treat it as a bearer-token rejection.
        assert!(!is_auth_failure(
            reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED
        ));
    }

    #[test]
    fn unauthorized_error_displays_user_friendly_message() {
        let err: n0_error::AnyError = Unauthorized.into();
        let msg = format!("{err}");
        assert!(!msg.is_empty(), "Unauthorized should have a Display impl");
        // The roundtrip downcast must work so callers can switch on auth failures.
        assert!(err.downcast_ref::<Unauthorized>().is_some());
    }

    /// Models the retry behavior we expect from [`DatumCloudClient::request_with_auth_retry`]:
    /// hit a 401 once, ask for a refreshed token, retry the same request with the new
    /// token, and observe a 200. We exercise the pattern at the HTTP layer against a
    /// local hyper server so the contract is pinned independent of the wider client.
    async fn run_with_auth_retry(
        client: &reqwest::Client,
        url: &str,
        tokens: Arc<TokenStash>,
        outcome_log: Arc<Mutex<Vec<&'static str>>>,
    ) -> reqwest::Response {
        let send = |bearer: &str| {
            client
                .get(url)
                .header("Authorization", format!("Bearer {bearer}"))
        };

        let res = send(&tokens.current()).send().await.expect("first request");
        if !is_auth_failure(res.status()) {
            outcome_log.lock().unwrap().push("first-ok");
            return res;
        }
        outcome_log.lock().unwrap().push("first-401");

        tokens.rotate();
        outcome_log.lock().unwrap().push("refreshed");

        let retry = send(&tokens.current()).send().await.expect("retry request");
        if is_auth_failure(retry.status()) {
            outcome_log.lock().unwrap().push("retry-401-logout");
        } else {
            outcome_log.lock().unwrap().push("retry-ok");
        }
        retry
    }

    struct TokenStash {
        tokens: Mutex<Vec<String>>,
    }
    impl TokenStash {
        fn new(initial: &str) -> Arc<Self> {
            Arc::new(Self {
                tokens: Mutex::new(vec![initial.into()]),
            })
        }
        fn current(&self) -> String {
            self.tokens.lock().unwrap().last().cloned().unwrap()
        }
        fn rotate(&self) {
            let mut tokens = self.tokens.lock().unwrap();
            let next = format!("fresh-{}", tokens.len());
            tokens.push(next);
        }
    }

    async fn spawn_server<H>(handler: H) -> (String, tokio::task::JoinHandle<()>)
    where
        H: Fn(Request<hyper::body::Incoming>) -> Response<Full<Bytes>>
            + Send
            + Sync
            + Clone
            + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");
        let handle = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let svc = service_fn(move |req| {
                        let handler = handler.clone();
                        async move { Ok::<_, std::convert::Infallible>(handler(req)) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });
        (url, handle)
    }

    fn auth_header(req: &Request<hyper::body::Incoming>) -> Option<String> {
        req.headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }

    #[tokio::test]
    async fn retry_succeeds_after_401_then_200() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_handler = calls.clone();
        let (url, handle) = spawn_server(move |req| {
            let n = calls_handler.fetch_add(1, Ordering::SeqCst);
            let bearer = auth_header(&req).unwrap_or_default();
            if n == 0 {
                assert_eq!(bearer, "Bearer t0", "first request uses initial token");
                Response::builder()
                    .status(401)
                    .body(Full::new(Bytes::from("unauthorized")))
                    .unwrap()
            } else {
                assert_eq!(
                    bearer, "Bearer fresh-1",
                    "retry uses the refreshed token from the stash"
                );
                Response::builder()
                    .status(200)
                    .body(Full::new(Bytes::from("ok")))
                    .unwrap()
            }
        })
        .await;

        let client = reqwest::Client::new();
        let tokens = TokenStash::new("t0");
        let log = Arc::new(Mutex::new(Vec::new()));
        let res = run_with_auth_retry(&client, &url, tokens, log.clone()).await;
        handle.abort();

        assert!(res.status().is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            &*log.lock().unwrap(),
            &["first-401", "refreshed", "retry-ok"]
        );
    }

    #[tokio::test]
    async fn retry_still_401_triggers_logout_path() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_handler = calls.clone();
        let (url, handle) = spawn_server(move |_req| {
            calls_handler.fetch_add(1, Ordering::SeqCst);
            Response::builder()
                .status(401)
                .body(Full::new(Bytes::from("still nope")))
                .unwrap()
        })
        .await;

        let client = reqwest::Client::new();
        let tokens = TokenStash::new("t0");
        let log = Arc::new(Mutex::new(Vec::new()));
        let res = run_with_auth_retry(&client, &url, tokens, log.clone()).await;
        handle.abort();

        // After two 401s we surface the failure and the caller is expected to clear auth.
        assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            &*log.lock().unwrap(),
            &["first-401", "refreshed", "retry-401-logout"]
        );
    }
}
