use lib::domains::{example_domains, Domain};
use lib::{Node, Repo};

#[derive(Debug, Clone)]
pub struct AppState {
    domains: Vec<Domain>,
    node: Node,
}

impl AppState {
    pub async fn load() -> anyhow::Result<Self> {
        let repo = Repo::open_or_create(Repo::default_location()).await?;
        let listen_key = repo.listen_key().await?;
        let node = Node::new(listen_key, repo).await?;

        Ok(AppState {
            node,
            domains: example_domains(),
        })
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn domains(&self) -> Vec<Domain> {
        self.domains.clone()
    }
}
