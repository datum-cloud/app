use lib::{Node, Repo};

#[derive(Debug, Clone)]
pub struct AppState {
    node: Node,
}

impl AppState {
    pub async fn load() -> anyhow::Result<Self> {
        let repo = Repo::open_or_create(Repo::default_location()).await?;
        let node = Node::new(repo).await?;

        Ok(AppState { node })
    }

    pub fn node(&self) -> &Node {
        &self.node
    }
}
