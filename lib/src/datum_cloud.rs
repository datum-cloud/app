use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use n0_future::{BufferedStreamExt, TryStreamExt};
use tracing::warn;

pub use self::{
    auth::{AuthClient, AuthState, UserProfile},
    env::ApiEnv,
};

mod auth;
mod env;

#[derive(Debug, Clone)]
pub struct DatumCloudClient {
    env: ApiEnv,
    auth: Arc<AuthState>,
    http: reqwest::Client,
}

impl DatumCloudClient {
    pub async fn login(env: ApiEnv) -> Result<Self> {
        let auth_client = AuthClient::new(env).await?;
        let auth = auth_client.login().await?;
        Self::new(env, auth)
    }

    pub async fn login_or_refresh(env: ApiEnv, auth: Option<AuthState>) -> Result<Self> {
        let auth_client = AuthClient::new(env).await?;
        let auth = match auth {
            None => auth_client.login().await?,
            Some(auth)
                if auth
                    .tokens
                    .expires_in_less_than(Duration::from_secs(60 * 30)) =>
            {
                match auth_client.refresh(&auth.tokens).await {
                    Ok(auth) => auth,
                    Err(err) => {
                        warn!("Failed to refresh auth token: {err:#}");
                        auth_client.login().await?
                    }
                }
            }
            Some(auth) => auth,
        };
        Self::new(env, auth)
    }

    pub fn new(env: ApiEnv, auth: AuthState) -> Result<Self> {
        let http = reqwest::Client::builder().build()?;
        let auth = Arc::new(auth);
        Ok(Self { env, auth, http })
    }

    pub fn user_profile(&self) -> &UserProfile {
        &self.auth.profile
    }

    pub fn auth_state(&self) -> &AuthState {
        &self.auth
    }

    pub async fn orgs_and_projects(&self) -> Result<Vec<OrganizationWithProjects>> {
        let orgs = self.orgs().await?;
        let stream = n0_future::stream::iter(orgs.into_iter().map(async |org| {
            let projects = self.projects(&org.resource_id).await?;
            anyhow::Ok(OrganizationWithProjects { org, projects })
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
                Scope::user(&self.auth),
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

        // TODO: Refresh access token if expired once we get refresh tokens.
        // Likely need to put self.auth into a Mutex.
        // if self.auth.tokens.is_expired() {
        //     let auth_client = AuthClient::new(self.env).await?;
        //     self.auth = auth_client.refresh(&self.auth.tokens).await?;
        // }

        let res = self
            .http
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.auth.tokens.access_token.secret()),
            )
            .send()
            .await
            .inspect_err(|e| warn!(%url, "Failed to fetch: {e:#}"))?;
        let status = res.status();
        if !status.is_success() {
            let text = match res.text().await {
                Ok(text) => text,
                Err(err) => err.to_string(),
            };
            warn!(%url, "Request failed: {status} {text}");
            anyhow::bail!("Request failed with status {status}");
        }

        let json: serde_json::Value = res.json().await?;
        Ok(json)
    }
}

#[derive(Debug, Clone)]
pub struct Organization {
    pub resource_id: String,
    pub display_name: String,
    pub r#type: String,
}

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
    fn user(auth: &AuthState) -> Self {
        Self::User(auth.profile.user_id.to_string())
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
