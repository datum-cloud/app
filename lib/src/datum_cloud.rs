use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use n0_future::boxed::BoxFuture;
use n0_future::{BufferedStreamExt, TryStreamExt};
use tracing::warn;

use crate::Repo;

pub use self::{
    auth::{AuthClient, AuthState, UserProfile},
    env::ApiEnv,
};

mod auth;
mod env;

/// Refresh auth or relogin if access token is valid for less than 30min
const REFRESH_AUTH_WHEN: Duration = Duration::from_secs(60 * 30);

#[derive(derive_more::Debug, Clone)]
pub struct DatumCloudClient {
    env: ApiEnv,
    auth: Arc<ArcSwap<AuthState>>,
    http: reqwest::Client,
    auth_client: AuthClient,
    #[debug("{:?}", on_auth_refresh.as_ref().map(|_| "Fn"))]
    on_auth_refresh:
        Option<Arc<dyn Fn(Arc<AuthState>) -> BoxFuture<Result<()>> + Send + Sync + 'static>>,
}

impl DatumCloudClient {
    pub async fn with_repo(env: ApiEnv, repo: Repo) -> Result<Self> {
        let auth_state = repo.read_oauth().await?;
        let mut client = Self::login(env, auth_state).await?;
        repo.write_oauth(&client.auth_state()).await?;
        client.on_auth_refresh = Some(Arc::new(move |auth_state| {
            let repo = repo.clone();
            Box::pin(async move {
                repo.write_oauth(&auth_state).await?;
                Ok(())
            })
        }));
        Ok(client)
    }

    pub async fn login(env: ApiEnv, last_auth_state: Option<AuthState>) -> Result<Self> {
        let auth_client = AuthClient::new(env).await?;
        let auth = match last_auth_state {
            None => auth_client.login().await?,
            Some(auth) if auth.tokens.expires_in_less_than(REFRESH_AUTH_WHEN) => {
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
        let auth = Arc::new(auth);
        let http = reqwest::Client::builder().build()?;
        Ok(Self {
            env,
            auth: Arc::new(ArcSwap::from(auth)),
            http,
            auth_client,
            on_auth_refresh: None,
        })
    }

    pub fn auth_state(&self) -> Arc<AuthState> {
        self.auth.load_full()
    }

    pub async fn refresh_auth(&self) -> Result<()> {
        let auth = self.auth.load();
        let new_auth = self.auth_client.refresh(&auth.tokens).await?;
        let new_auth = Arc::new(new_auth);
        self.auth.store(new_auth.clone());
        if let Some(f) = self.on_auth_refresh.as_ref() {
            f(new_auth).await?;
        }
        Ok(())
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
                Scope::user(&self.auth.load().profile),
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
        let mut auth = self.auth.load();
        if auth.tokens.expires_in_less_than(Duration::from_secs(60)) {
            self.refresh_auth().await?;
            auth = self.auth.load();
        }

        let res = self
            .http
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", auth.tokens.access_token.secret()),
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
