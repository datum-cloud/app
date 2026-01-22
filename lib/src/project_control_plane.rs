use std::sync::Arc;

use arc_swap::ArcSwap;
use kube::{Client, Config};
use n0_error::{Result, StdResultExt};
use secrecy::SecretString;

use crate::{SelectedContext, datum_cloud::DatumCloudClient};

#[derive(derive_more::Debug, Clone)]
pub struct ProjectControlPlaneClient {
    project_id: String,
    server_url: String,
    access_token: String,
    #[debug("kube::Client")]
    client: Arc<Client>,
}

impl ProjectControlPlaneClient {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn client(&self) -> &Client {
        self.client.as_ref()
    }
}

#[derive(derive_more::Debug, Clone)]
pub struct ProjectControlPlaneManager {
    datum: DatumCloudClient,
    current: Arc<ArcSwap<Option<ProjectControlPlaneClient>>>,
}

impl ProjectControlPlaneManager {
    pub fn new(datum: DatumCloudClient) -> Self {
        Self {
            datum,
            current: Arc::new(ArcSwap::from_pointee(None)),
        }
    }

    pub fn current(&self) -> Option<ProjectControlPlaneClient> {
        self.current.load_full().as_ref().clone()
    }

    /// Returns the current project client, refreshing the auth token if needed.
    /// The client is rebuilt when the token changes ("refresh on access").
    pub async fn client(&self) -> Result<Option<ProjectControlPlaneClient>> {
        let current = self.current.load_full();
        let Some(current) = current.as_ref().as_ref() else {
            return Ok(None);
        };

        let auth_state = self.datum.auth().load_refreshed().await?;
        let auth = auth_state.get()?;
        let access_token = auth.tokens.access_token.secret();
        if access_token == current.access_token() {
            return Ok(Some(current.clone()));
        }

        let refreshed = self
            .datum
            .project_control_plane_client_with_token(current.project_id(), access_token)?;
        self.current.store(Arc::new(Some(refreshed.clone())));
        Ok(Some(refreshed))
    }

    pub async fn set_selected_context(
        &self,
        selected_context: Option<&SelectedContext>,
    ) -> Result<()> {
        let next_project = selected_context.map(|ctx| ctx.project_id.as_str());
        let current = self.current.load_full();
        let current_project = current.as_ref().as_ref().map(|client| client.project_id());
        if current_project == next_project {
            return Ok(());
        }

        let next_client = match selected_context {
            Some(context) => Some(
                self.datum
                    .project_control_plane_client(&context.project_id)
                    .await?,
            ),
            None => None,
        };
        self.current.store(Arc::new(next_client));
        Ok(())
    }
}

impl DatumCloudClient {
    pub fn project_control_plane_url(&self, project_id: &str) -> String {
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
        self.project_control_plane_client_with_token(
            project_id,
            auth.tokens.access_token.secret(),
        )
    }

    fn project_control_plane_client_with_token(
        &self,
        project_id: &str,
        access_token: &str,
    ) -> Result<ProjectControlPlaneClient> {
        let server_url = self.project_control_plane_url(project_id);
        let uri = server_url
            .parse()
            .std_context("Invalid project control plane URL")?;
        let mut config = Config::new(uri);
        config.auth_info.token =
            Some(SecretString::new(access_token.to_string().into_boxed_str()));
        let client =
            Client::try_from(config).std_context("Failed to create project control plane client")?;
        Ok(ProjectControlPlaneClient {
            project_id: project_id.to_string(),
            server_url,
            access_token: access_token.to_string(),
            client: Arc::new(client),
        })
    }
}
