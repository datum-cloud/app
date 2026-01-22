use lib::{
    SelectedContext,
    datum_cloud::{ApiEnv, DatumCloudClient, LoginState},
    ListenNode, Node, Repo,
};
use tracing::info;

#[derive(Debug, Clone)]
pub struct AppState {
    node: Node,
    datum: DatumCloudClient,
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
        let app_state = AppState { node, datum };
        if app_state.datum.login_state() != LoginState::Missing {
            app_state
                .listen_node()
                .validate_selected_context(app_state.datum())
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
        // TODO: scope control-plane clients to the selected project.
        self.listen_node().set_selected_context(selected_context).await
    }

}
