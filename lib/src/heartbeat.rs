use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::Utc;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, ResourceExt};
use n0_error::Result;
use n0_future::task::AbortOnDropHandle;
use rand::Rng;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::ListenNode;
use crate::datum_apis::connector::{
    Connector, ConnectorConnectionDetails, ConnectorConnectionDetailsPublicKey,
    ConnectorConnectionType, ConnectorKeyBinding, PublicKeyConnectorAddress,
    PublicKeyDiscoveryMode,
};
use crate::datum_apis::lease::Lease;
use crate::datum_cloud::{DatumCloudClient, LoginState};
use crate::project_control_plane::is_kube_auth_failure;

type ProjectRunner = Arc<
    dyn Fn(
            String,
            DatumCloudClient,
            Arc<dyn HeartbeatDetailsProvider>,
            CancellationToken,
        ) -> tokio::task::JoinHandle<()>
        + Send
        + Sync,
>;

const DEFAULT_PCP_NAMESPACE: &str = "default";
const DEFAULT_LEASE_DURATION_SECS: i32 = 30;
const BACKOFF_INITIAL: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const READBACK_INTERVAL: Duration = Duration::from_secs(300);
const READBACK_MAX: Duration = Duration::from_secs(3600);

#[derive(derive_more::Debug, Clone)]
pub struct HeartbeatAgent {
    #[debug(skip)]
    inner: Arc<HeartbeatInner>,
}

struct HeartbeatInner {
    datum: DatumCloudClient,
    provider: Arc<dyn HeartbeatDetailsProvider>,
    runner: ProjectRunner,
    projects: Mutex<HashMap<String, ProjectHeartbeat>>,
    known_projects: Mutex<HashSet<String>>,
    login_task: Mutex<Option<AbortOnDropHandle<()>>>,
}

struct ProjectHeartbeat {
    cancel: CancellationToken,
    _task: AbortOnDropHandle<()>,
}

impl HeartbeatAgent {
    pub fn new(datum: DatumCloudClient, listen: ListenNode) -> Self {
        let provider = Arc::new(ListenNodeDetailsProvider::new(listen));
        let runner: ProjectRunner = Arc::new(|project_id, datum, provider, cancel| {
            tokio::spawn(run_project(project_id, datum, provider, cancel))
        });
        Self::new_with_runner(datum, provider, runner)
    }

    fn new_with_runner(
        datum: DatumCloudClient,
        provider: Arc<dyn HeartbeatDetailsProvider>,
        runner: ProjectRunner,
    ) -> Self {
        Self {
            inner: Arc::new(HeartbeatInner {
                datum,
                provider,
                runner,
                projects: Mutex::new(HashMap::new()),
                known_projects: Mutex::new(HashSet::new()),
                login_task: Mutex::new(None),
            }),
        }
    }

    pub async fn start(&self) {
        let mut guard = self.inner.login_task.lock().await;
        if guard.is_some() {
            return;
        }
        let this = self.clone();
        let mut login_rx = this.inner.datum.auth().login_state_watch();
        let mut projects_rx = this.inner.datum.orgs_projects_watch();
        let task = tokio::spawn(async move {
            if *login_rx.borrow() != LoginState::Missing
                && let Err(err) = this.refresh_projects().await
            {
                warn!("heartbeat: bootstrap failed: {err:#}");
            }
            loop {
                tokio::select! {
                    res = login_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                        let login_state = *login_rx.borrow();
                        match login_state {
                            LoginState::Missing => {
                                this.clear_projects().await;
                                this.clear_known_projects().await;
                            }
                            _ => {
                                if let Err(err) = this.refresh_projects().await {
                                    warn!("heartbeat: bootstrap failed: {err:#}");
                                }
                            }
                        }
                    }
                    res = projects_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                        if *login_rx.borrow() != LoginState::Missing
                            && let Err(err) = this.refresh_projects().await {
                                warn!("heartbeat: bootstrap failed: {err:#}");
                            }
                    }
                }
            }
        });
        *guard = Some(AbortOnDropHandle::new(task));
    }

    pub async fn register_project(&self, project_id: impl Into<String>) {
        let project_id = project_id.into();
        let mut projects = self.inner.projects.lock().await;
        if projects.contains_key(&project_id) {
            return;
        }
        let cancel = CancellationToken::new();
        let task = (self.inner.runner)(
            project_id.clone(),
            self.inner.datum.clone(),
            self.inner.provider.clone(),
            cancel.clone(),
        );
        projects.insert(
            project_id,
            ProjectHeartbeat {
                cancel,
                _task: AbortOnDropHandle::new(task),
            },
        );
    }

    pub async fn deregister_project(&self, project_id: &str) {
        let mut projects = self.inner.projects.lock().await;
        if let Some(project) = projects.remove(project_id) {
            project.cancel.cancel();
        }
    }

    async fn clear_projects(&self) {
        let mut projects = self.inner.projects.lock().await;
        for (_, project) in projects.drain() {
            project.cancel.cancel();
        }
    }

    async fn clear_known_projects(&self) {
        let mut known = self.inner.known_projects.lock().await;
        known.clear();
    }

    pub async fn refresh_projects(&self) -> Result<()> {
        let orgs = self.inner.datum.orgs_and_projects().await?;
        let mut next_projects = HashSet::new();
        for org in orgs {
            for project in org.projects {
                next_projects.insert(project.resource_id);
            }
        }

        {
            let mut known = self.inner.known_projects.lock().await;
            if *known == next_projects {
                return Ok(());
            }
            *known = next_projects.clone();
        }

        let running: Vec<String> = {
            let projects = self.inner.projects.lock().await;
            projects.keys().cloned().collect()
        };
        for project_id in running {
            if !next_projects.contains(&project_id) {
                self.deregister_project(&project_id).await;
            }
        }

        for project_id in next_projects {
            let should_probe = {
                let projects = self.inner.projects.lock().await;
                !projects.contains_key(&project_id)
            };
            if !should_probe {
                continue;
            }
            match probe_connector(
                &project_id,
                self.inner.datum.clone(),
                self.inner.provider.clone(),
            )
            .await
            {
                Ok(true) => self.register_project(project_id).await,
                Ok(false) => {
                    debug!(%project_id, "heartbeat: no connector yet");
                }
                Err(err) => {
                    warn!(%project_id, "heartbeat: connector probe failed: {err:#}");
                }
            }
        }

        Ok(())
    }
}

struct ConnectorCache {
    name: String,
    uid: Option<String>,
    lease_name: Option<String>,
    lease_duration_seconds: Option<i32>,
    last_details: Option<serde_json::Value>,
    last_home_relay: Option<String>,
    sent_binding: Option<ConnectorKeyBinding>,
    readback_interval: Duration,
    next_readback: Instant,
    warned_missing_uid: bool,
}

impl ConnectorCache {
    fn new(connector: &Connector) -> Self {
        Self {
            name: connector.name_any(),
            uid: connector.uid(),
            lease_name: connector
                .status
                .as_ref()
                .and_then(|status| status.lease_ref.as_ref())
                .map(|lease| lease.name.clone()),
            lease_duration_seconds: None,
            last_details: None,
            last_home_relay: observed_public_key(connector)
                .map(|details| details.home_relay.clone()),
            sent_binding: None,
            readback_interval: READBACK_INTERVAL,
            next_readback: Instant::now() + READBACK_INTERVAL,
            warned_missing_uid: false,
        }
    }

    fn apply_readback(&mut self, connector: &Connector) -> Readback {
        let observed_uid = connector.uid();
        let action = readback_action(
            self.uid.as_deref(),
            observed_uid.as_deref(),
            self.sent_binding.as_ref(),
            observed_public_key(connector).and_then(|details| details.binding.as_ref()),
        );
        match action {
            Readback::Unchanged => {}
            Readback::Resign => {
                self.uid = observed_uid;
                self.last_details = None;
                self.sent_binding = None;
                self.warned_missing_uid = false;
            }
            Readback::Repatch => {
                self.last_details = None;
            }
        }
        self.readback_interval = next_readback_interval(action, self.readback_interval);
        self.next_readback = Instant::now() + self.readback_interval;
        action
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readback {
    Unchanged,
    Resign,
    Repatch,
}

fn readback_action(
    cached_uid: Option<&str>,
    observed_uid: Option<&str>,
    sent_binding: Option<&ConnectorKeyBinding>,
    observed_binding: Option<&ConnectorKeyBinding>,
) -> Readback {
    if cached_uid != observed_uid {
        return Readback::Resign;
    }
    match sent_binding {
        Some(sent) if observed_binding != Some(sent) => Readback::Repatch,
        _ => Readback::Unchanged,
    }
}

fn next_readback_interval(action: Readback, current: Duration) -> Duration {
    match action {
        Readback::Repatch => (current * 2).clamp(READBACK_INTERVAL, READBACK_MAX),
        Readback::Unchanged | Readback::Resign => READBACK_INTERVAL,
    }
}

fn details_need_patch(last_sent: Option<&serde_json::Value>, details: &serde_json::Value) -> bool {
    last_sent != Some(details)
}

fn observed_public_key(connector: &Connector) -> Option<&ConnectorConnectionDetailsPublicKey> {
    connector
        .status
        .as_ref()
        .and_then(|status| status.connection_details.as_ref())
        .and_then(|details| details.public_key.as_ref())
}

fn is_kube_not_found(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(resp) if resp.code == 404)
}

async fn run_project(
    project_id: String,
    datum: DatumCloudClient,
    provider: Arc<dyn HeartbeatDetailsProvider>,
    cancel: CancellationToken,
) {
    let mut backoff = Backoff::new();
    let mut cache: Option<ConnectorCache> = None;

    loop {
        if cancel.is_cancelled() {
            return;
        }

        let pcp = match datum.project_control_plane_client(&project_id).await {
            Ok(client) => client,
            Err(err) => {
                warn!(%project_id, "heartbeat: failed to get pcp client: {err:#}");
                sleep_with_cancel(backoff.next(), &cancel).await;
                continue;
            }
        };
        // Preflight refresh: ensure the kube client is built from a non-near-expiry token.
        let client = match pcp.client_refreshed().await {
            Ok(client) => client,
            Err(err) => {
                warn!(%project_id, "heartbeat: failed to refresh pcp client: {err:#}");
                sleep_with_cancel(backoff.next(), &cancel).await;
                continue;
            }
        };
        let connectors: Api<Connector> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let leases: Api<Lease> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        if cache.is_none() {
            match find_connector(&connectors, provider.endpoint_id()).await {
                Ok(Some(connector)) => {
                    cache = Some(ConnectorCache::new(&connector));
                    backoff.reset();
                }
                Ok(None) => {
                    debug!(%project_id, "heartbeat: no connector yet");
                    sleep_with_cancel(backoff.next(), &cancel).await;
                    continue;
                }
                Err(err) => {
                    warn!(%project_id, "heartbeat: connector lookup failed: {err:#}");
                    if is_kube_auth_failure(&err) {
                        warn!(%project_id, "heartbeat: kube auth failure; logging out");
                        let _ = datum.auth().logout().await;
                        return;
                    }
                    sleep_with_cancel(backoff.next(), &cancel).await;
                    continue;
                }
            }
        }

        let Some(mut cached) = cache.take() else {
            continue;
        };

        if cached.lease_name.is_none() {
            match connectors.get(&cached.name).await {
                Ok(connector) => {
                    cached.lease_name = connector
                        .status
                        .as_ref()
                        .and_then(|status| status.lease_ref.as_ref())
                        .map(|lease| lease.name.clone());
                    if cached.lease_name.is_none() {
                        sleep_with_cancel(backoff.next(), &cancel).await;
                        cache = Some(cached);
                        continue;
                    }
                    cached.apply_readback(&connector);
                    cached.last_home_relay =
                        observed_public_key(&connector).map(|details| details.home_relay.clone());
                }
                Err(err) => {
                    warn!(
                        %project_id,
                        connector = %cached.name,
                        "heartbeat: failed to fetch connector: {err:#}"
                    );
                    if is_kube_auth_failure(&err) {
                        warn!(%project_id, "heartbeat: kube auth failure; logging out");
                        let _ = datum.auth().logout().await;
                        return;
                    }
                    cache = None;
                    sleep_with_cancel(backoff.next(), &cancel).await;
                    continue;
                }
            }
        }

        if Instant::now() >= cached.next_readback {
            match connectors.get(&cached.name).await {
                Ok(connector) => {
                    if cached.apply_readback(&connector) == Readback::Repatch {
                        debug!(
                            %project_id,
                            connector = %cached.name,
                            retry_in = ?cached.readback_interval,
                            "heartbeat: connector key binding not in status; re-patching"
                        );
                    }
                }
                Err(err) if is_kube_not_found(&err) => {
                    debug!(%project_id, connector = %cached.name, "heartbeat: connector gone");
                    sleep_with_cancel(backoff.next(), &cancel).await;
                    continue;
                }
                Err(err) => {
                    warn!(
                        %project_id,
                        connector = %cached.name,
                        "heartbeat: failed to read back connector: {err:#}"
                    );
                    if is_kube_auth_failure(&err) {
                        warn!(%project_id, "heartbeat: kube auth failure; logging out");
                        let _ = datum.auth().logout().await;
                        return;
                    }
                    cached.next_readback = Instant::now() + cached.readback_interval;
                }
            }
        }

        if cached.uid.is_none() && !cached.warned_missing_uid {
            warn!(
                %project_id,
                connector = %cached.name,
                "heartbeat: connector has no uid; publishing details without a key binding"
            );
            cached.warned_missing_uid = true;
        }

        let details = match provider
            .connection_details(cached.uid.as_deref(), cached.last_home_relay.as_deref())
        {
            Some(details) => details,
            None => {
                warn!(%project_id, connector = %cached.name, "heartbeat: missing home relay");
                cache = Some(cached);
                sleep_with_cancel(backoff.next(), &cancel).await;
                continue;
            }
        };

        let details_value = match serde_json::to_value(&details) {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    %project_id,
                    connector = %cached.name,
                    "heartbeat: failed to serialize details: {err:#}"
                );
                cache = Some(cached);
                sleep_with_cancel(backoff.next(), &cancel).await;
                continue;
            }
        };

        if details_need_patch(cached.last_details.as_ref(), &details_value) {
            let patch = json!({ "status": { "connectionDetails": details_value } });
            match connectors
                .patch_status(&cached.name, &PatchParams::default(), &Patch::Merge(&patch))
                .await
            {
                Ok(_) => {
                    cached.last_details = Some(patch["status"]["connectionDetails"].clone());
                    cached.sent_binding = details
                        .public_key
                        .as_ref()
                        .and_then(|details| details.binding.clone());
                }
                Err(err) => {
                    warn!(
                        %project_id,
                        connector = %cached.name,
                        "heartbeat: failed to patch connection details: {err:#}"
                    );
                    if is_kube_auth_failure(&err) {
                        warn!(%project_id, "heartbeat: kube auth failure; logging out");
                        let _ = datum.auth().logout().await;
                        return;
                    }
                }
            }
        }

        if cached.lease_duration_seconds.is_none() {
            let Some(lease_name) = cached.lease_name.as_ref() else {
                cache = Some(cached);
                sleep_with_cancel(backoff.next(), &cancel).await;
                continue;
            };
            match leases.get(lease_name).await {
                Ok(lease) => {
                    cached.lease_duration_seconds = lease
                        .spec
                        .as_ref()
                        .and_then(|spec| spec.lease_duration_seconds);
                }
                Err(err) => {
                    warn!(
                        %project_id,
                        lease = %lease_name,
                        "heartbeat: failed to fetch lease: {err:#}"
                    );
                    if is_kube_auth_failure(&err) {
                        warn!(%project_id, "heartbeat: kube auth failure; logging out");
                        let _ = datum.auth().logout().await;
                        return;
                    }
                    cache = Some(cached);
                    sleep_with_cancel(backoff.next(), &cancel).await;
                    continue;
                }
            }
        }

        let Some(lease_name) = cached.lease_name.as_ref() else {
            cache = Some(cached);
            sleep_with_cancel(backoff.next(), &cancel).await;
            continue;
        };

        let renew_time = MicroTime(Utc::now());
        let patch = json!({ "spec": { "renewTime": renew_time } });
        if let Err(err) = leases
            .patch(lease_name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
        {
            warn!(%project_id, lease = %lease_name, "heartbeat: lease renew failed: {err:#}");
            if is_kube_auth_failure(&err) {
                warn!(%project_id, "heartbeat: kube auth failure; logging out");
                let _ = datum.auth().logout().await;
                return;
            }
            if is_kube_not_found(&err) {
                cached.lease_name = None;
                cached.lease_duration_seconds = None;
                cached.next_readback = Instant::now();
            }
            cache = Some(cached);
            sleep_with_cancel(backoff.next(), &cancel).await;
            continue;
        }

        let lease_duration = cached
            .lease_duration_seconds
            .unwrap_or(DEFAULT_LEASE_DURATION_SECS);
        let interval = renewal_interval(lease_duration);
        backoff.reset();
        cache = Some(cached);
        sleep_with_cancel(interval, &cancel).await;
    }
}

async fn probe_connector(
    project_id: &str,
    datum: DatumCloudClient,
    provider: Arc<dyn HeartbeatDetailsProvider>,
) -> Result<bool> {
    let pcp = datum.project_control_plane_client(project_id).await?;
    let selector = provider.endpoint_id();
    let result = pcp
        .with_auth_retry(|client| {
            let selector = selector.clone();
            async move {
                let connectors: Api<Connector> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
                Ok(find_connector(&connectors, selector).await?.is_some())
            }
        })
        .await?;
    Ok(result)
}

async fn find_connector(
    connectors: &Api<Connector>,
    endpoint_id: String,
) -> kube::Result<Option<Connector>> {
    let selector = format!("status.connectionDetails.publicKey.id={endpoint_id}");
    let list = connectors
        .list(&ListParams::default().fields(&selector))
        .await?;
    if list.items.is_empty() {
        return Ok(None);
    }
    if list.items.len() > 1 {
        warn!(
            %selector,
            count = list.items.len(),
            "heartbeat: multiple connectors found, using first"
        );
    }
    Ok(list.items.into_iter().next())
}

trait HeartbeatDetailsProvider: Send + Sync {
    fn endpoint_id(&self) -> String;
    fn connection_details(
        &self,
        connector_uid: Option<&str>,
        fallback_home_relay: Option<&str>,
    ) -> Option<ConnectorConnectionDetails>;
}

struct ListenNodeDetailsProvider {
    listen: ListenNode,
}

impl ListenNodeDetailsProvider {
    fn new(listen: ListenNode) -> Self {
        Self { listen }
    }
}

impl HeartbeatDetailsProvider for ListenNodeDetailsProvider {
    fn endpoint_id(&self) -> String {
        self.listen.endpoint_id().to_string()
    }

    fn connection_details(
        &self,
        connector_uid: Option<&str>,
        fallback_home_relay: Option<&str>,
    ) -> Option<ConnectorConnectionDetails> {
        let endpoint = self.listen.endpoint();
        let endpoint_addr = endpoint.addr();
        let home_relay = endpoint_addr
            .relay_urls()
            .next()
            .map(|url| url.to_string())
            .or_else(|| fallback_home_relay.map(|relay| relay.to_string()))?;
        let addresses: Vec<PublicKeyConnectorAddress> = endpoint_addr
            .ip_addrs()
            .map(|addr| PublicKeyConnectorAddress {
                address: addr.ip().to_string(),
                port: addr.port() as i32,
            })
            .collect();

        let details = ConnectorConnectionDetails {
            connection_type: ConnectorConnectionType::PublicKey,
            public_key: Some(ConnectorConnectionDetailsPublicKey {
                id: endpoint.id().to_string(),
                discovery_mode: Some(PublicKeyDiscoveryMode::Dns),
                home_relay,
                addresses,
                binding: None,
            }),
        };
        Some(match connector_uid {
            Some(uid) => details.bind(endpoint.secret_key(), uid),
            None => details,
        })
    }
}

fn renewal_interval(lease_duration_seconds: i32) -> Duration {
    let lease_duration_seconds = lease_duration_seconds.max(1) as u64;
    let base = Duration::from_secs((lease_duration_seconds / 2).max(1));
    let jitter_max = (base.as_secs() / 5).max(1);
    let mut rng = rand::rng();
    let jitter = rng.random_range(0..=jitter_max);
    base + Duration::from_secs(jitter)
}

async fn sleep_with_cancel(duration: Duration, cancel: &CancellationToken) {
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = tokio::time::sleep(duration) => {}
    }
}

struct Backoff {
    current: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            current: BACKOFF_INITIAL,
        }
    }

    fn next(&mut self) -> Duration {
        let wait = self.current;
        self.current = (self.current * 2).min(BACKOFF_MAX);
        wait
    }

    fn reset(&mut self) {
        self.current = BACKOFF_INITIAL;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TestProvider {
        endpoint_id: String,
    }

    impl HeartbeatDetailsProvider for TestProvider {
        fn endpoint_id(&self) -> String {
            self.endpoint_id.clone()
        }

        fn connection_details(
            &self,
            _connector_uid: Option<&str>,
            _fallback_home_relay: Option<&str>,
        ) -> Option<ConnectorConnectionDetails> {
            None
        }
    }

    fn test_repo_path() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("datum-connect-test-{}", uuid::Uuid::new_v4()));
        path
    }

    #[tokio::test]
    async fn register_project_idempotent() {
        let repo = crate::Repo::open_or_create(test_repo_path()).await.unwrap();
        let datum = crate::datum_cloud::DatumCloudClient::with_repo(
            crate::datum_cloud::ApiEnv::Staging,
            repo,
        )
        .await
        .unwrap();
        let provider = Arc::new(TestProvider {
            endpoint_id: "test-endpoint".to_string(),
        });
        let runner: ProjectRunner = Arc::new(|_project_id, _datum, _provider, cancel| {
            tokio::spawn(async move {
                cancel.cancelled().await;
            })
        });
        let agent = HeartbeatAgent::new_with_runner(datum, provider, runner);

        agent.register_project("project-1").await;
        agent.register_project("project-1").await;

        let count = agent.inner.projects.lock().await.len();
        assert_eq!(count, 1);

        agent.deregister_project("project-1").await;
        let count = agent.inner.projects.lock().await.len();
        assert_eq!(count, 0);
    }

    #[test]
    fn renewal_interval_in_range() {
        for lease_duration_seconds in [1, 2, 10, 60] {
            let lease_duration_seconds = lease_duration_seconds as u64;
            let base = Duration::from_secs((lease_duration_seconds / 2).max(1));
            let jitter_max = (base.as_secs() / 5).max(1);
            let max = base + Duration::from_secs(jitter_max);
            for _ in 0..50 {
                let interval = renewal_interval(lease_duration_seconds as i32);
                assert!(
                    interval >= base && interval <= max,
                    "interval {interval:?} outside [{base:?}, {max:?}]"
                );
            }
        }
    }

    fn binding(signature: &str) -> ConnectorKeyBinding {
        ConnectorKeyBinding {
            signature: signature.to_string(),
        }
    }

    #[test]
    fn readback_repatches_when_binding_missing() {
        let sent = binding("sig");
        assert_eq!(
            readback_action(Some("uid"), Some("uid"), Some(&sent), None),
            Readback::Repatch
        );
        assert_eq!(
            readback_action(
                Some("uid"),
                Some("uid"),
                Some(&sent),
                Some(&binding("other"))
            ),
            Readback::Repatch
        );
    }

    #[test]
    fn readback_resigns_on_uid_change() {
        let sent = binding("sig");
        assert_eq!(
            readback_action(Some("old"), Some("new"), Some(&sent), Some(&sent)),
            Readback::Resign
        );
        assert_eq!(
            next_readback_interval(Readback::Resign, READBACK_MAX),
            READBACK_INTERVAL
        );
    }

    #[test]
    fn readback_backs_off_while_binding_is_dropped() {
        let sent = binding("sig");
        let mut interval = READBACK_INTERVAL;
        let mut previous = Duration::ZERO;
        for _ in 0..20 {
            let action = readback_action(Some("uid"), Some("uid"), Some(&sent), None);
            assert_eq!(action, Readback::Repatch);
            interval = next_readback_interval(action, interval);
            assert!(interval >= previous && interval >= READBACK_INTERVAL);
            assert!(interval <= READBACK_MAX);
            previous = interval;
        }
        assert_eq!(interval, READBACK_MAX);
    }

    #[test]
    fn readback_keeps_details_when_binding_matches() {
        let sent = binding("sig");
        assert_eq!(
            readback_action(Some("uid"), Some("uid"), Some(&sent), Some(&sent)),
            Readback::Unchanged
        );
        assert_eq!(readback_action(None, None, None, None), Readback::Unchanged);
        let details = json!({ "publicKey": { "id": "ep", "binding": { "signature": "sig" } } });
        assert!(!details_need_patch(Some(&details), &details));
        assert!(details_need_patch(None, &details));
    }

    #[test]
    fn backoff_doubles_and_resets() {
        let mut backoff = Backoff::new();
        let first = backoff.next();
        assert_eq!(first, BACKOFF_INITIAL);

        let second = backoff.next();
        assert_eq!(second, BACKOFF_INITIAL * 2);

        let mut last = second;
        for _ in 0..10 {
            last = backoff.next();
        }
        assert_eq!(last, BACKOFF_MAX);

        backoff.reset();
        let reset = backoff.next();
        assert_eq!(reset, BACKOFF_INITIAL);
    }
}
