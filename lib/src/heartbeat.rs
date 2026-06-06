use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use chrono::Utc;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, ResourceExt};
use n0_error::{Result, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use rand::Rng;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::ListenNode;
use crate::datum_apis::connector::{
    Connector, ConnectorConnectionDetails, ConnectorConnectionDetailsPublicKey,
    ConnectorConnectionType, PublicKeyConnectorAddress, PublicKeyDiscoveryMode,
};
use crate::datum_apis::lease::Lease;
use crate::datum_cloud::{DatumCloudClient, LoginState};

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
    lease_name: Option<String>,
    lease_duration_seconds: Option<i32>,
    last_details: Option<serde_json::Value>,
    last_home_relay: Option<String>,
}

/// Returns true if `err` is a kube API error with HTTP status 401.
/// Used to decide whether a heartbeat retry should force an OAuth token refresh
/// (the proactive refresh timer in `AuthClient` only fires when the access token
/// is within `REFRESH_AUTH_WHEN` of expiry, so a token rejected before that
/// would otherwise spin until the timer catches up).
fn is_unauthorized(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(e) if e.code == 401)
}

/// Force an OAuth token refresh after a 401. The proactive timer only refreshes
/// when the token is near expiry, so a server-side rejection that arrives early
/// (clock skew, revocation, etc.) would otherwise leave the heartbeat retrying
/// with the same dead token until the timer eventually fires.
///
/// When auth is already in [`LoginState::Missing`] (e.g. after a previous
/// permanent refresh failure), this returns immediately without contacting the
/// IdP — the auth layer has already surfaced the loss to the operator and
/// there is nothing to refresh until they log in again.
async fn force_refresh_auth(project_id: &str, datum: &DatumCloudClient) {
    if matches!(datum.auth().login_state(), LoginState::Missing) {
        debug!(
            %project_id,
            "heartbeat: skipping forced refresh — auth state is missing, awaiting login"
        );
        return;
    }
    match datum.auth().refresh().await {
        Ok(()) => debug!(%project_id, "heartbeat: forced token refresh after 401"),
        Err(err) => {
            if matches!(datum.auth().login_state(), LoginState::Missing) {
                // refresh classified the failure as permanent and cleared state;
                // the auth layer has already error!/eprintln!'d the loss.
                debug!(
                    %project_id,
                    "heartbeat: forced refresh permanently rejected, will resume after re-login"
                );
            } else {
                warn!(
                    %project_id,
                    "heartbeat: forced token refresh failed (will retry): {err:#}"
                );
            }
        }
    }
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
        let client = pcp.client();
        let connectors: Api<Connector> = Api::namespaced(client.clone(), DEFAULT_PCP_NAMESPACE);
        let leases: Api<Lease> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);

        if cache.is_none() {
            match find_connector(&connectors, provider.endpoint_id()).await {
                Ok(Some(connector)) => {
                    let connector_name = connector.name_any();
                    let lease_name = connector
                        .status
                        .as_ref()
                        .and_then(|status| status.lease_ref.as_ref())
                        .map(|lease| lease.name.clone());
                    let last_home_relay = connector
                        .status
                        .as_ref()
                        .and_then(|status| status.connection_details.as_ref())
                        .and_then(|details| details.public_key.as_ref())
                        .map(|details| details.home_relay.clone());
                    info!(
                        %project_id,
                        connector = %connector_name,
                        lease = lease_name.as_deref().unwrap_or("<pending>"),
                        "heartbeat: registered connector, starting lease renewals"
                    );
                    cache = Some(ConnectorCache {
                        name: connector_name,
                        lease_name,
                        lease_duration_seconds: None,
                        last_details: None,
                        last_home_relay,
                    });
                    backoff.reset();
                }
                Ok(None) => {
                    debug!(%project_id, "heartbeat: no connector yet");
                    sleep_with_cancel(backoff.next(), &cancel).await;
                    continue;
                }
                Err(err) => {
                    warn!(%project_id, "heartbeat: connector lookup failed: {err:#}");
                    if is_unauthorized(&err) {
                        force_refresh_auth(&project_id, &datum).await;
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
                    cached.last_home_relay = connector
                        .status
                        .as_ref()
                        .and_then(|status| status.connection_details.as_ref())
                        .and_then(|details| details.public_key.as_ref())
                        .map(|details| details.home_relay.clone());
                }
                Err(err) => {
                    warn!(
                        %project_id,
                        connector = %cached.name,
                        "heartbeat: failed to fetch connector: {err:#}"
                    );
                    if is_unauthorized(&err) {
                        force_refresh_auth(&project_id, &datum).await;
                    }
                    cache = None;
                    sleep_with_cancel(backoff.next(), &cancel).await;
                    continue;
                }
            }
        }

        let details = match provider.connection_details(cached.last_home_relay.as_deref()) {
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

        if cached.last_details.as_ref() != Some(&details_value) {
            let patch = json!({ "status": { "connectionDetails": details_value } });
            match connectors
                .patch_status(&cached.name, &PatchParams::default(), &Patch::Merge(&patch))
                .await
            {
                Ok(_) => {
                    cached.last_details = Some(patch["status"]["connectionDetails"].clone());
                }
                Err(err) => {
                    warn!(
                        %project_id,
                        connector = %cached.name,
                        "heartbeat: failed to patch connection details: {err:#}"
                    );
                    if is_unauthorized(&err) {
                        force_refresh_auth(&project_id, &datum).await;
                        cache = Some(cached);
                        sleep_with_cancel(backoff.next(), &cancel).await;
                        continue;
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
                    if is_unauthorized(&err) {
                        force_refresh_auth(&project_id, &datum).await;
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
            if is_unauthorized(&err) {
                force_refresh_auth(&project_id, &datum).await;
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
    let client = pcp.client();
    let connectors: Api<Connector> = Api::namespaced(client, DEFAULT_PCP_NAMESPACE);
    let selector = provider.endpoint_id();
    Ok(find_connector(&connectors, selector)
        .await
        .std_context("connector lookup failed")?
        .is_some())
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

        Some(ConnectorConnectionDetails {
            connection_type: ConnectorConnectionType::PublicKey,
            public_key: Some(ConnectorConnectionDetailsPublicKey {
                id: endpoint.id().to_string(),
                discovery_mode: Some(PublicKeyDiscoveryMode::Dns),
                home_relay,
                addresses,
            }),
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
