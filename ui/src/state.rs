use lib::{ListenNode, Node, Repo};

#[derive(Debug, Clone)]
pub struct AppState {
    node: Node,
}

impl AppState {
    pub async fn load() -> n0_error::Result<Self> {
        let repo = Repo::open_or_create(Repo::default_location()).await?;
        let node = Node::new(repo).await?;

        Ok(AppState { node })
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn listen_node(&self) -> &ListenNode {
        &self.node().listen
    }
}
