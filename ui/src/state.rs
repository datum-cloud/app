use lib::{
    datum_cloud::{ApiEnv, DatumCloudClient},
    ListenNode, Node, Repo,
};

#[derive(Debug, Clone)]
pub struct AppState {
    node: Node,
    datum: DatumCloudClient,
}

impl AppState {
    pub async fn load() -> n0_error::Result<Self> {
        let repo = Repo::open_or_create(Repo::default_location()).await?;
        let (node, datum) = tokio::try_join! {
            Node::new(repo.clone()),
            DatumCloudClient::with_repo(ApiEnv::Staging, repo)
        }?;

        Ok(AppState { node, datum })
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
}
