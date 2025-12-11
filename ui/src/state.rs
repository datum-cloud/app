use lib::{Node, Repo};

#[derive(Debug, Clone)]
pub struct AppState {
    node: Node,
}

impl AppState {
    pub async fn load() -> anyhow::Result<Self> {
        let repo = Repo::open_or_create(Repo::default_location()).await?;
        let listen_key = repo.listen_key().await?;
        let node = Node::new(listen_key, repo).await?;

        Ok(AppState { node })
    }

    pub fn node(&self) -> &Node {
        &self.node
    }
}
