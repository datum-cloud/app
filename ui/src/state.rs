use dioxus::prelude::WritableExt;
use lib::{
    HeartbeatAgent, SelectedContext, TunnelSummary,
    datum_cloud::{ApiEnv, DatumCloudClient},
    ListenNode, Node, Repo, TunnelService,
};
use tokio::sync::Notify;
use tracing::info;

#[derive(derive_more::Debug, Clone)]
pub struct AppState {
    node: Node,
    datum: DatumCloudClient,
    heartbeat: HeartbeatAgent,
    tunnel_refresh: std::sync::Arc<Notify>,
    tunnel_cache: dioxus::signals::Signal<Vec<TunnelSummary>>,
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
        let heartbeat = HeartbeatAgent::new(datum.clone(), node.listen.clone());
        heartbeat.start().await;
        let app_state = AppState {
            node,
            datum,
            heartbeat,
            tunnel_refresh: std::sync::Arc::new(Notify::new()),
            tunnel_cache: dioxus::signals::Signal::new(Vec::new()),
        };
        Ok(app_state)
    }

    pub fn datum(&self) -> &DatumCloudClient {
        &self.datum
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn heartbeat(&self) -> &HeartbeatAgent {
        &self.heartbeat
    }

    pub fn listen_node(&self) -> &ListenNode {
        &self.node().listen
    }

    pub fn tunnel_service(&self) -> TunnelService {
        TunnelService::new(self.datum.clone(), self.node.listen.clone())
    }

    pub fn tunnel_refresh(&self) -> std::sync::Arc<Notify> {
        self.tunnel_refresh.clone()
    }

    pub fn bump_tunnel_refresh(&self) {
        self.tunnel_refresh.notify_waiters();
    }

    pub fn tunnel_cache(&self) -> dioxus::signals::Signal<Vec<TunnelSummary>> {
        self.tunnel_cache
    }

    pub fn set_tunnel_cache(&self, tunnels: Vec<TunnelSummary>) {
        let mut cache = self.tunnel_cache;
        cache.set(tunnels);
    }

    pub fn upsert_tunnel(&self, tunnel: TunnelSummary) {
        let mut cache = self.tunnel_cache;
        let mut list = cache();
        if let Some(existing) = list.iter_mut().find(|item| item.id == tunnel.id) {
            *existing = tunnel;
        } else {
            list.push(tunnel);
        }
        cache.set(list);
    }

    pub fn remove_tunnel(&self, tunnel_id: &str) {
        let mut cache = self.tunnel_cache;
        let mut list = cache();
        list.retain(|item| item.id != tunnel_id);
        cache.set(list);
    }

    pub fn selected_context(&self) -> Option<SelectedContext> {
        self.datum.selected_context()
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
        self.datum.set_selected_context(selected_context.clone()).await?;
        Ok(())
    }

}
