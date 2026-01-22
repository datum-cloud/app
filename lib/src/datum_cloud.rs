use std::sync::Arc;

use n0_error::{Result, StackResultExt, StdResultExt};
use n0_future::{BufferedStreamExt, TryStreamExt};
use tracing::warn;

use crate::Repo;

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
}

impl DatumCloudClient {
    pub async fn with_repo(env: ApiEnv, repo: Repo) -> Result<Self> {
        let auth = AuthClient::with_repo(env, repo).await?;
        let http = reqwest::Client::builder().build().anyerr()?;
        Ok(Self { env, auth, http })
    }

    pub async fn new(env: ApiEnv) -> Result<Self> {
        let auth = AuthClient::new(env).await?;
        let http = reqwest::Client::builder().build().anyerr()?;
        Ok(Self { env, auth, http })
    }

    pub fn login_state(&self) -> LoginState {
        self.auth.login_state()
    }

    pub fn api_url(&self) -> &'static str {
        self.env.api_url()
    }

    pub fn auth(&self) -> &AuthClient {
        &self.auth
    }

    pub fn auth_state(&self) -> Arc<MaybeAuth> {
        self.auth.load()
    }

    pub async fn orgs_and_projects(&self) -> Result<Vec<OrganizationWithProjects>> {
        let orgs = self.orgs().await?;
        let stream = n0_future::stream::iter(orgs.into_iter().map(async |org| {
            let projects = self.projects(&org.resource_id).await?;
            n0_error::Ok(OrganizationWithProjects { org, projects })
        }));
        stream.buffered_unordered(16).try_collect().await
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
        tracing::debug!("GET {url}");

        // Refresh access token if they are close to expiring.
        let auth_state = self.auth.load_refreshed().await?;
        let auth = auth_state.get()?;

        let res = self
            .http
            .get(&url)
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
}

#[derive(Debug, Clone, derive_more::Display)]
enum ResourceManager {
    #[display("/organizationmemberships")]
    OrganizationMemberships,
    #[display("/projects")]
    Projects,
}
