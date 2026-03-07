use std::sync::Arc;

use arc_swap::ArcSwap;
use chrono::{Duration, Utc};
use n0_error::{Result, StackResultExt, StdResultExt};
use n0_future::{BufferedStreamExt, TryStreamExt, task::AbortOnDropHandle};
use rand::Rng;
use tokio::sync::watch;
use tracing::warn;

use crate::datum_apis::user_invitation::RoleReference;
use crate::{ProjectControlPlaneClient, Repo, SelectedContext};

pub use self::{
    auth::{AuthClient, AuthState, LoginState, MaybeAuth, UserProfile},
    env::ApiEnv,
};

mod auth;
mod env;

#[derive(derive_more::Debug, Clone)]
pub struct DatumCloudClient {
    env: ApiEnv,
    auth: AuthClient,
    http: reqwest::Client,
    session: SessionStateWrapper,
    _session_task: Option<Arc<AbortOnDropHandle<()>>>,
}

impl DatumCloudClient {
    pub async fn with_repo(env: ApiEnv, repo: Repo) -> Result<Self> {
        let auth = AuthClient::with_repo(env, repo.clone()).await?;
        let session = SessionStateWrapper::from_repo(Some(repo)).await?;
        let http = reqwest::Client::builder().build().anyerr()?;
        let mut client = Self {
            env,
            auth,
            http,
            session,
            _session_task: None,
        };
        client.start_session_sync();
        Ok(client)
    }

    pub async fn new(env: ApiEnv) -> Result<Self> {
        let auth = AuthClient::new(env).await?;
        let session = SessionStateWrapper::empty();
        let http = reqwest::Client::builder().build().anyerr()?;
        let mut client = Self {
            env,
            auth,
            http,
            session,
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
                        let ns = ref_
                            .namespace
                            .as_deref()
                            .unwrap_or("milo-system");
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
        let orgs = self.orgs().await?;
        let stream = n0_future::stream::iter(orgs.into_iter().map(async |org| {
            let projects = self.projects(&org.resource_id).await?;
            n0_error::Ok(OrganizationWithProjects { org, projects })
        }));
        let list: Vec<OrganizationWithProjects> =
            stream.buffered_unordered(16).try_collect().await?;
        self.session.set_orgs_projects(list.clone());
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
                let r#type = org.get("type")?.as_str()?;
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
        parse_orgs(&json).context("Failed to parse reply")
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

    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        tracing::debug!("POST {url}");

        let auth_state = self.auth.load_refreshed().await?;
        let auth = auth_state.get()?;

        let res = self
            .http
            .post(url)
            .header(
                "Authorization",
                format!("Bearer {}", auth.tokens.access_token.secret()),
            )
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .inspect_err(|e| warn!(%url, "Failed to POST: {e:#}"))
            .with_std_context(|_| format!("Failed to POST {url}"))?;
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

        // Refresh access token if they are close to expiring.
        let auth_state = self.auth.load_refreshed().await?;
        let auth = auth_state.get()?;

        let res = self
            .http
            .get(url)
            .header(
                "Authorization",
                format!("Bearer {}", auth.tokens.access_token.secret()),
            )
            .send()
            .await
            .inspect_err(|e| warn!(%url, "Failed to fetch: {e:#}"))
            .with_std_context(|_| format!("Failed to fetch {url}"))?;
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

    fn set_orgs_projects(&self, orgs_projects: Vec<OrganizationWithProjects>) {
        self.orgs_projects.store(Arc::new(orgs_projects.clone()));
        let _ = self.orgs_projects_tx.send(orgs_projects);
    }
}

#[derive(Debug, Clone)]
pub struct Organization {
    pub resource_id: String,
    pub display_name: String,
    pub r#type: String,
}

#[derive(Debug, Clone)]
pub struct OrganizationWithProjects {
    pub org: Organization,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone)]
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
