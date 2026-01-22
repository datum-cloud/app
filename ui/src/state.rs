use lib::{
    SelectedContext,
    datum_cloud::{ApiEnv, DatumCloudClient, LoginState},
    ListenNode, Node, ProjectControlPlaneClient, ProjectControlPlaneManager, Repo,
};
use tracing::info;

#[derive(derive_more::Debug, Clone)]
pub struct AppState {
    node: Node,
    datum: DatumCloudClient,
    project_control_plane: ProjectControlPlaneManager,
}

impl AppState {
    pub async fn load() -> n0_error::Result<Self> {
        let repo_path = Repo::default_location();
        info!(repo_path = %repo_path.display(), "ui: loading repo");
        let repo = Repo::open_or_create(repo_path).await?;
        let (node, datum) = tokio::try_join! {
            Node::new(repo.clone()),
            DatumCloudClient::with_repo(ApiEnv::Staging, repo)
        }?;
        let project_control_plane = ProjectControlPlaneManager::new(datum.clone());
        let app_state = AppState {
            node,
            datum,
            project_control_plane,
        };
        if app_state.datum.login_state() != LoginState::Missing {
            let selected = app_state
                .listen_node()
                .validate_selected_context(app_state.datum())
                .await?;
            app_state
                .project_control_plane
                .set_selected_context(selected.as_ref())
                .await?;
        }
        Ok(app_state)
    }

    pub fn datum(&self) -> &DatumCloudClient {
        &self.datum
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub async fn project_control_plane(
        &self,
    ) -> n0_error::Result<Option<ProjectControlPlaneClient>> {
        self.project_control_plane.client().await
    }

    pub fn project_control_plane_manager(&self) -> &ProjectControlPlaneManager {
        &self.project_control_plane
    }

    pub fn listen_node(&self) -> &ListenNode {
        &self.node().listen
    }

    pub fn selected_context(&self) -> Option<SelectedContext> {
        self.listen_node().selected_context()
    }

    pub async fn set_selected_context(
        &self,
        selected_context: Option<SelectedContext>,
    ) -> n0_error::Result<()> {
        info!(
            selected = %selected_context
                .as_ref()
                .map_or("<none>".to_string(), SelectedContext::label),
            "ui: setting selected context"
        );
        self.listen_node()
            .set_selected_context(selected_context.clone())
            .await?;
        self.project_control_plane
            .set_selected_context(selected_context.as_ref())
            .await?;
        Ok(())
    }

}
