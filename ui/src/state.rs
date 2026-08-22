use dioxus::prelude::WritableExt;
use lib::{
    datum_cloud::{ApiEnv, DatumCloudClient},
    HeartbeatAgent, ListenNode, Node, Repo, SelectedContext, TunnelActivityTracker,
    TunnelCreateQuota, TunnelService, TunnelSummary,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tracing::info;

#[derive(derive_more::Debug, Clone)]
pub struct AppState {
    node: Node,
    datum: DatumCloudClient,
    heartbeat: HeartbeatAgent,
    tunnel_refresh: std::sync::Arc<Notify>,
    tunnel_cache: dioxus::signals::Signal<Vec<TunnelSummary>>,
    tunnel_activity: Arc<Mutex<TunnelActivityTracker>>,
    /// IDs of tunnels we've just deleted locally but whose backend resources
    /// (HTTPProxy + ConnectorAdvertisement + …) may still appear in the next
    /// few `list_active` polls while Kubernetes is reaping them. Tombstones
    /// suppress the UI from showing a half-deleted tunnel with the toggle
    /// flipped off; they are cleared automatically by `proxies_list` once the
    /// API stops returning the ID.
    pending_deletions: dioxus::signals::Signal<HashSet<String>>,
    /// Latest create-quota snapshot for the selected project (refreshed with the
    /// tunnel list). `None` means not yet loaded or no project selected.
    tunnel_create_quota: dioxus::signals::Signal<Option<TunnelCreateQuota>>,
}

impl AppState {
    pub async fn load() -> n0_error::Result<Self> {
        let repo_path = Repo::default_location();
        info!(repo_path = %repo_path.display(), "ui: loading repo");
        let repo = Repo::open_or_create(repo_path).await?;
        let (node, datum) = tokio::try_join! {
            Node::new(repo.clone()),
            DatumCloudClient::with_repo(ApiEnv::default(), repo)
        }?;
        let heartbeat = HeartbeatAgent::new(datum.clone(), node.listen.clone());
        heartbeat.start().await;
        let app_state = AppState {
            node,
            datum,
            heartbeat,
            tunnel_refresh: std::sync::Arc::new(Notify::new()),
            tunnel_cache: dioxus::signals::Signal::new(Vec::new()),
            tunnel_activity: Arc::new(Mutex::new(TunnelActivityTracker::new())),
            pending_deletions: dioxus::signals::Signal::new(HashSet::new()),
            tunnel_create_quota: dioxus::signals::Signal::new(None),
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

    pub fn tunnel_create_quota(&self) -> dioxus::signals::Signal<Option<TunnelCreateQuota>> {
        self.tunnel_create_quota
    }

    pub fn set_tunnel_create_quota(&self, quota: Option<TunnelCreateQuota>) {
        let mut signal = self.tunnel_create_quota;
        signal.set(quota);
    }

    pub fn tunnel_activity(&self) -> Arc<Mutex<TunnelActivityTracker>> {
        self.tunnel_activity.clone()
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

    pub fn pending_deletions(&self) -> dioxus::signals::Signal<HashSet<String>> {
        self.pending_deletions
    }

    /// Mark a tunnel as deleted locally. Subsequent `list_active` polls will
    /// suppress this ID from the merged tunnel list until the API stops
    /// returning it (handled by `proxies_list`).
    pub fn add_pending_deletion(&self, tunnel_id: String) {
        let mut signal = self.pending_deletions;
        let mut set = signal();
        set.insert(tunnel_id);
        signal.set(set);
    }

    /// Drop tombstones for IDs the backend no longer reports — typically
    /// called from the poll loop with the latest API-returned IDs so a
    /// re-created tunnel with the same name can reappear later.
    pub fn reconcile_pending_deletions(&self, api_ids: &HashSet<String>) {
        let mut signal = self.pending_deletions;
        let set = signal();
        let next: HashSet<String> = set
            .iter()
            .filter(|id| api_ids.contains(*id))
            .cloned()
            .collect();
        if next.len() != set.len() {
            signal.set(next);
        }
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
        self.datum
            .set_selected_context(selected_context.clone())
            .await?;
        // Drop stale quota until the tunnel poll refreshes for the new project.
        self.set_tunnel_create_quota(None);
        Ok(())
    }
}
