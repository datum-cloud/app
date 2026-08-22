//! Headless agent: one ListenNode plus a local HTTP control API.
//!
//! Tunnel create/delete must run in the same process as the listener, so the CLI
//! talks to this agent over loopback HTTP instead of constructing its own node.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use n0_error::{Result, StdResultExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::datum_cloud::{ApiEnv, DatumCloudClient, LoginState, NotLoggedIn, UserProfile};
use crate::http_user_agent::datum_http_user_agent;
use crate::tunnel_activity::metrics_bytes_for_tunnel;
use crate::tunnels::{TunnelCreateQuota, TunnelService, TunnelSummary};
use crate::{HeartbeatAgent, ListenNode, Repo, SelectedContext};

/// Hidden flag both the CLI and the desktop app honor so either binary can
/// spawn the same detached listener.
pub const HEADLESS_AGENT_FLAG: &str = "--headless-agent";

const DEFAULT_HOSTNAME_TIMEOUT: Duration = Duration::from_secs(30);
const HOSTNAME_POLL_INTERVAL: Duration = Duration::from_secs(1);
const AGENT_READY_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub endpoint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub pid: u32,
    pub port: u16,
    pub endpoint_id: String,
    pub logged_in: bool,
    pub login_state: String,
    pub email: Option<String>,
    pub context: Option<SelectedContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTunnelRequest {
    pub label: String,
    pub endpoint: String,
    #[serde(default)]
    pub wait_hostname: bool,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelView {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub url: Option<String>,
    pub hostnames: Vec<String>,
    pub enabled: bool,
    pub accepted: bool,
    pub programmed: bool,
}

impl From<&TunnelSummary> for TunnelView {
    fn from(summary: &TunnelSummary) -> Self {
        Self {
            id: summary.id.clone(),
            label: summary.label.clone(),
            endpoint: summary.endpoint.clone(),
            url: summary.public_url(),
            hostnames: summary.hostnames.clone(),
            enabled: summary.enabled,
            accepted: summary.accepted,
            programmed: summary.programmed,
        }
    }
}

impl From<TunnelView> for TunnelSummary {
    fn from(view: TunnelView) -> Self {
        Self {
            id: view.id,
            label: view.label,
            endpoint: view.endpoint,
            hostnames: view.hostnames,
            enabled: view.enabled,
            accepted: view.accepted,
            programmed: view.programmed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTunnelRequest {
    pub label: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelMetric {
    pub id: String,
    pub bytes_from_origin: u64,
    pub bytes_to_origin: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteTunnelResponse {
    pub id: String,
    pub project_id: String,
    pub connector_deleted: bool,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

struct ApiError {
    status: StatusCode,
    message: String,
    tunnel: Option<TunnelView>,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            tunnel: None,
        }
    }

    fn with_tunnel(mut self, tunnel: TunnelView) -> Self {
        self.tunnel = Some(tunnel);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "error": self.message });
        if let Some(tunnel) = self.tunnel
            && let Ok(value) = serde_json::to_value(tunnel)
        {
            body["tunnel"] = value;
        }
        (self.status, Json(body)).into_response()
    }
}

#[derive(Clone)]
struct AgentState {
    token: String,
    info: AgentInfo,
    datum: DatumCloudClient,
    listen: ListenNode,
    tunnels: TunnelService,
    heartbeat: HeartbeatAgent,
}

pub fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 does not deliver a signal; it only checks whether the pid exists.
        let rc = unsafe { libc::kill(pid as i32, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        windows_is_pid_alive(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(windows)]
fn windows_is_pid_alive(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
    }
    unsafe {
        // SAFETY: OpenProcess/GetExitCodeProcess/CloseHandle are used only to query
        // whether a numeric pid still refers to a live process.
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

fn kill_pid(pid: u32, force: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: pid is a recorded agent process id, not the current process.
        let rc = unsafe { libc::kill(pid as i32, signal) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error()).std_context("signaling agent process")
        }
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string()]);
        if force {
            cmd.arg("/F");
        }
        let status = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .std_context("taskkill")?;
        if status.success() {
            Ok(())
        } else {
            n0_error::bail_any!("taskkill exited with {status}");
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, force);
        n0_error::bail_any!("stopping the agent is not supported on this platform");
    }
}

pub fn read_agent_info(repo: &Repo) -> Result<Option<AgentInfo>> {
    let path = repo.agent_info_path();
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path).std_context("failed to read agent.json")?;
    let info: AgentInfo = serde_json::from_str(&data).std_context("failed to parse agent.json")?;
    Ok(Some(info))
}

pub fn remove_agent_info(repo: &Repo) -> Result<()> {
    let path = repo.agent_info_path();
    if path.exists() {
        std::fs::remove_file(&path).std_context("failed to remove agent.json")?;
    }
    Ok(())
}

fn write_agent_info_exclusive(repo: &Repo, info: &AgentInfo) -> Result<()> {
    let path = repo.agent_info_path();
    let json = serde_json::to_string_pretty(info).anyerr()?;
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(json.as_bytes())
                .std_context("failed to write agent.json")?;
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            if let Some(existing) = read_agent_info(repo)?
                && is_pid_alive(existing.pid)
            {
                n0_error::bail_any!(
                    "Agent is already running (pid {}). Stop it with `datum-connect agent stop`.",
                    existing.pid
                );
            }
            std::fs::remove_file(&path).ok();
            std::fs::write(&path, json.as_bytes()).std_context("failed to write agent.json")?;
            Ok(())
        }
        Err(err) => Err(err).std_context("failed to create agent.json"),
    }
}

/// Load agent.json only if the recorded process is still alive.
pub fn running_agent_info(repo: &Repo) -> Result<Option<AgentInfo>> {
    let Some(info) = read_agent_info(repo)? else {
        return Ok(None);
    };
    if is_pid_alive(info.pid) {
        Ok(Some(info))
    } else {
        let _ = remove_agent_info(repo);
        Ok(None)
    }
}

fn new_token() -> String {
    rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

fn login_state_name(state: LoginState) -> &'static str {
    match state {
        LoginState::Missing => "missing",
        LoginState::NeedsRefresh => "needs_refresh",
        LoginState::Valid => "valid",
    }
}

fn profile_email(profile: Option<&UserProfile>) -> Option<String> {
    profile.map(|p| p.email.clone())
}

/// Run the agent until `shutdown` is cancelled.
pub async fn run_agent(repo: Repo, shutdown: CancellationToken) -> Result<()> {
    if let Some(existing) = running_agent_info(&repo)? {
        n0_error::bail_any!(
            "Agent is already running (pid {}). Stop it with `datum-connect agent stop`.",
            existing.pid
        );
    }

    let datum = DatumCloudClient::with_repo(ApiEnv::default(), repo.clone()).await?;
    let listen = ListenNode::new(repo.clone()).await?;
    let heartbeat = HeartbeatAgent::new(datum.clone(), listen.clone());
    heartbeat.start().await;
    let tunnels = TunnelService::new(datum.clone(), listen.clone());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .std_context("failed to bind agent control API")?;
    let port = listener
        .local_addr()
        .std_context("failed to read agent bind address")?
        .port();

    let info = AgentInfo {
        pid: std::process::id(),
        port,
        token: new_token(),
        endpoint_id: listen.endpoint_id().to_string(),
    };
    write_agent_info_exclusive(&repo, &info)?;
    let _guard = AgentFileGuard {
        path: repo.agent_info_path(),
    };

    let state = Arc::new(AgentState {
        token: info.token.clone(),
        info: info.clone(),
        datum,
        listen,
        tunnels,
        heartbeat,
    });

    let app = Router::new()
        .route("/status", get(status))
        .route("/quota", get(quota))
        .route("/metrics", get(metrics))
        .route("/tunnels", get(list_tunnels).post(create_tunnel))
        .route(
            "/tunnels/:id",
            get(get_tunnel).patch(update_tunnel).delete(delete_tunnel),
        )
        .route("/tunnels/:id/enabled", put(set_enabled))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state);

    info!(
        port,
        endpoint_id = %info.endpoint_id,
        "datum-connect agent listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .std_context("agent control API failed")?;
    Ok(())
}

struct AgentFileGuard {
    path: std::path::PathBuf,
}

impl Drop for AgentFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn require_token(
    State(state): State<Arc<AgentState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
        || headers
            .get("x-datum-agent-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == state.token);
    if authorized {
        next.run(request).await
    } else {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

async fn status(State(state): State<Arc<AgentState>>) -> Json<AgentStatus> {
    let _ = state.datum.reload_selected_context().await;
    Json(agent_status(&state))
}

fn agent_status(state: &AgentState) -> AgentStatus {
    let auth = state.datum.auth_state();
    let profile = auth.get().ok().map(|s| &s.profile);
    AgentStatus {
        pid: state.info.pid,
        port: state.info.port,
        endpoint_id: state.info.endpoint_id.clone(),
        logged_in: auth.get().is_ok(),
        login_state: login_state_name(state.datum.login_state()).to_string(),
        email: profile_email(profile),
        context: state.datum.selected_context(),
    }
}

async fn sync_selected_context(state: &AgentState) -> std::result::Result<(), ApiError> {
    state
        .datum
        .reload_selected_context()
        .await
        .map_err(map_tunnel_err)?;
    Ok(())
}

async fn list_tunnels(
    State(state): State<Arc<AgentState>>,
) -> std::result::Result<Json<Vec<TunnelView>>, ApiError> {
    sync_selected_context(&state).await?;
    let tunnels = state.tunnels.list_active().await.map_err(map_tunnel_err)?;
    Ok(Json(tunnels.iter().map(TunnelView::from).collect()))
}

async fn get_tunnel(
    State(state): State<Arc<AgentState>>,
    Path(id): Path<String>,
) -> std::result::Result<Json<TunnelView>, ApiError> {
    sync_selected_context(&state).await?;
    match state
        .tunnels
        .get_active(&id)
        .await
        .map_err(map_tunnel_err)?
    {
        Some(tunnel) => Ok(Json(TunnelView::from(&tunnel))),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Tunnel `{id}` not found"),
        )),
    }
}

async fn create_tunnel(
    State(state): State<Arc<AgentState>>,
    Json(req): Json<CreateTunnelRequest>,
) -> std::result::Result<Json<TunnelView>, ApiError> {
    if req.label.trim().is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "label is required"));
    }
    if req.endpoint.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "endpoint is required",
        ));
    }

    sync_selected_context(&state).await?;
    let mut summary = state
        .tunnels
        .create_active(req.label.trim(), req.endpoint.trim())
        .await
        .map_err(map_tunnel_err)?;

    if let Some(ctx) = state.datum.selected_context() {
        state.heartbeat.register_project(ctx.project_id).await;
    }

    if req.wait_hostname && summary.hostnames.is_empty() {
        let timeout = req
            .timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_HOSTNAME_TIMEOUT);
        summary = wait_for_hostname(&state.tunnels, &summary.id, timeout)
            .await
            .map_err(map_tunnel_err)?;
        if summary.hostnames.is_empty() {
            return Err(
                ApiError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "Timed out waiting for a public hostname. The tunnel was created; delete it with `datum-connect tunnel delete` or poll `datum-connect tunnel get`.",
                )
                .with_tunnel(TunnelView::from(&summary)),
            );
        }
    }

    Ok(Json(TunnelView::from(&summary)))
}

async fn delete_tunnel(
    State(state): State<Arc<AgentState>>,
    Path(id): Path<String>,
) -> std::result::Result<Json<DeleteTunnelResponse>, ApiError> {
    sync_selected_context(&state).await?;
    if state
        .tunnels
        .get_active(&id)
        .await
        .map_err(map_tunnel_err)?
        .is_none()
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Tunnel `{id}` not found"),
        ));
    }

    let outcome = state
        .tunnels
        .delete_active(&id)
        .await
        .map_err(map_tunnel_err)?;
    if outcome.connector_deleted {
        state
            .heartbeat
            .deregister_project(&outcome.project_id)
            .await;
    }
    Ok(Json(DeleteTunnelResponse {
        id,
        project_id: outcome.project_id,
        connector_deleted: outcome.connector_deleted,
    }))
}

async fn update_tunnel(
    State(state): State<Arc<AgentState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateTunnelRequest>,
) -> std::result::Result<Json<TunnelView>, ApiError> {
    if req.label.trim().is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "label is required"));
    }
    if req.endpoint.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "endpoint is required",
        ));
    }
    sync_selected_context(&state).await?;
    let summary = state
        .tunnels
        .update_active(&id, req.label.trim(), req.endpoint.trim())
        .await
        .map_err(map_tunnel_err)?;
    Ok(Json(TunnelView::from(&summary)))
}

async fn set_enabled(
    State(state): State<Arc<AgentState>>,
    Path(id): Path<String>,
    Json(req): Json<SetEnabledRequest>,
) -> std::result::Result<Json<TunnelView>, ApiError> {
    sync_selected_context(&state).await?;
    let summary = state
        .tunnels
        .set_enabled_active(&id, req.enabled)
        .await
        .map_err(map_tunnel_err)?;
    if req.enabled
        && let Some(ctx) = state.datum.selected_context()
    {
        state.heartbeat.register_project(ctx.project_id).await;
    }
    Ok(Json(TunnelView::from(&summary)))
}

async fn quota(
    State(state): State<Arc<AgentState>>,
) -> std::result::Result<Json<Option<TunnelCreateQuota>>, ApiError> {
    sync_selected_context(&state).await?;
    let quota = state
        .tunnels
        .tunnel_create_quota_active()
        .await
        .map_err(map_tunnel_err)?;
    Ok(Json(quota))
}

async fn metrics(
    State(state): State<Arc<AgentState>>,
) -> std::result::Result<Json<Vec<TunnelMetric>>, ApiError> {
    sync_selected_context(&state).await?;
    let tunnels = state.tunnels.list_active().await.map_err(map_tunnel_err)?;
    let metrics = state.listen.metrics();
    Ok(Json(
        tunnels
            .iter()
            .map(|tunnel| {
                let (bytes_from_origin, bytes_to_origin) =
                    metrics_bytes_for_tunnel(metrics.as_ref(), tunnel);
                TunnelMetric {
                    id: tunnel.id.clone(),
                    bytes_from_origin,
                    bytes_to_origin,
                }
            })
            .collect(),
    ))
}

fn map_tunnel_err(err: n0_error::AnyError) -> ApiError {
    if err.downcast_ref::<NotLoggedIn>().is_some() {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Not logged in. Run `datum-connect login`.",
        );
    }
    let message = format!("{err:#}");
    if message.contains("No project selected") {
        return ApiError::new(
            StatusCode::BAD_REQUEST,
            "No project selected. Run `datum-connect context set --project <id>`.",
        );
    }
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message)
}

pub async fn wait_for_hostname(
    tunnels: &TunnelService,
    tunnel_id: &str,
    timeout: Duration,
) -> Result<TunnelSummary> {
    let deadline = Instant::now() + timeout;
    loop {
        let Some(summary) = tunnels.get_active(tunnel_id).await? else {
            n0_error::bail_any!("Tunnel `{tunnel_id}` disappeared while waiting for a hostname");
        };
        if !summary.hostnames.is_empty() || Instant::now() >= deadline {
            return Ok(summary);
        }
        tokio::time::sleep(HOSTNAME_POLL_INTERVAL).await;
    }
}

pub async fn stop_agent(repo: &Repo) -> Result<()> {
    let Some(info) = read_agent_info(repo)? else {
        n0_error::bail_any!("Agent is not running.");
    };
    if !is_pid_alive(info.pid) {
        remove_agent_info(repo)?;
        n0_error::bail_any!("Agent is not running.");
    }
    if info.pid == std::process::id() {
        n0_error::bail_any!("Refusing to stop the current process via agent stop");
    }

    if let Err(err) = kill_pid(info.pid, false) {
        warn!("failed to send SIGTERM to agent: {err:#}");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !is_pid_alive(info.pid) {
            let _ = remove_agent_info(repo);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    kill_pid(info.pid, true)?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = remove_agent_info(repo);
    if is_pid_alive(info.pid) {
        n0_error::bail_any!("Failed to stop agent (pid {})", info.pid);
    }
    Ok(())
}

#[derive(Clone)]
pub struct AgentClient {
    http: reqwest::Client,
    base: String,
    token: String,
    pub info: AgentInfo,
}

impl AgentClient {
    pub fn from_info(info: AgentInfo) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(datum_http_user_agent())
            .build()
            .anyerr()?;
        Ok(Self {
            http,
            base: format!("http://127.0.0.1:{}", info.port),
            token: info.token.clone(),
            info,
        })
    }

    pub fn connect(repo: &Repo) -> Result<Self> {
        let Some(info) = running_agent_info(repo)? else {
            n0_error::bail_any!("Agent is not running. Run `datum-connect agent start`.");
        };
        Self::from_info(info)
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<(StatusCode, Vec<u8>)> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(body) = body {
            req = req.json(body);
        }
        let response = req.send().await.std_context("agent request failed")?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .std_context("reading agent response")?
            .to_vec();
        Ok((status, bytes))
    }

    fn decode_success<T: for<'de> Deserialize<'de>>(status: StatusCode, bytes: &[u8]) -> Result<T> {
        if status.is_success() {
            return serde_json::from_slice(bytes).std_context("decoding agent response");
        }
        let message = serde_json::from_slice::<ErrorBody>(bytes)
            .ok()
            .map(|body| body.error)
            .unwrap_or_else(|| String::from_utf8_lossy(bytes).trim().to_string());
        n0_error::bail_any!("{message}")
    }

    pub async fn status(&self) -> Result<AgentStatus> {
        let (status, bytes) = self
            .send(reqwest::Method::GET, "/status", None::<&()>)
            .await?;
        Self::decode_success(status, &bytes)
    }

    pub async fn list_tunnels(&self) -> Result<Vec<TunnelView>> {
        let (status, bytes) = self
            .send(reqwest::Method::GET, "/tunnels", None::<&()>)
            .await?;
        Self::decode_success(status, &bytes)
    }

    pub async fn get_tunnel(&self, id: &str) -> Result<TunnelView> {
        match self.get_tunnel_optional(id).await? {
            Some(tunnel) => Ok(tunnel),
            None => n0_error::bail_any!("Tunnel `{id}` not found"),
        }
    }

    pub async fn get_tunnel_optional(&self, id: &str) -> Result<Option<TunnelView>> {
        let (status, bytes) = self
            .send(reqwest::Method::GET, &format!("/tunnels/{id}"), None::<&()>)
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(Self::decode_success(status, &bytes)?))
    }

    pub async fn create_tunnel(&self, req: &CreateTunnelRequest) -> Result<TunnelView> {
        let (status, bytes) = self
            .send(reqwest::Method::POST, "/tunnels", Some(req))
            .await?;
        Self::decode_success(status, &bytes)
    }

    pub async fn delete_tunnel(&self, id: &str) -> Result<DeleteTunnelResponse> {
        let (status, bytes) = self
            .send(
                reqwest::Method::DELETE,
                &format!("/tunnels/{id}"),
                None::<&()>,
            )
            .await?;
        Self::decode_success(status, &bytes)
    }

    pub async fn update_tunnel(&self, id: &str, req: &UpdateTunnelRequest) -> Result<TunnelView> {
        let (status, bytes) = self
            .send(reqwest::Method::PATCH, &format!("/tunnels/{id}"), Some(req))
            .await?;
        Self::decode_success(status, &bytes)
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<TunnelView> {
        let (status, bytes) = self
            .send(
                reqwest::Method::PUT,
                &format!("/tunnels/{id}/enabled"),
                Some(&SetEnabledRequest { enabled }),
            )
            .await?;
        Self::decode_success(status, &bytes)
    }

    pub async fn quota(&self) -> Result<Option<TunnelCreateQuota>> {
        let (status, bytes) = self
            .send(reqwest::Method::GET, "/quota", None::<&()>)
            .await?;
        Self::decode_success(status, &bytes)
    }

    pub async fn metrics(&self) -> Result<Vec<TunnelMetric>> {
        let (status, bytes) = self
            .send(reqwest::Method::GET, "/metrics", None::<&()>)
            .await?;
        Self::decode_success(status, &bytes)
    }
}

/// True when this process was launched as the detached listener.
pub fn wants_headless_agent() -> bool {
    std::env::args().any(|arg| arg == HEADLESS_AGENT_FLAG)
}

/// `--repo` from argv, otherwise the default app-support location.
pub fn repo_path_from_cli_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--repo" {
            if let Some(path) = args.next() {
                return PathBuf::from(path);
            }
        } else if let Some(path) = arg.strip_prefix("--repo=") {
            return PathBuf::from(path);
        }
    }
    Repo::default_location()
}

/// Spawn a detached listener using this binary, then wait until it answers.
pub async fn ensure_agent(repo: &Repo) -> Result<AgentClient> {
    if let Ok(client) = AgentClient::connect(repo)
        && client.status().await.is_ok()
    {
        return Ok(client);
    }
    spawn_detached_agent(repo)?;
    match wait_until_ready(repo, Duration::from_secs(20)).await {
        Ok(client) => Ok(client),
        Err(err) => n0_error::bail_any!(
            "{err:#}. Check {} for agent logs.",
            repo.agent_log_path().display()
        ),
    }
}

/// Fork this executable with [`HEADLESS_AGENT_FLAG`] so the listener outlives
/// the GUI or CLI that started it.
pub fn spawn_detached_agent(repo: &Repo) -> Result<()> {
    let exe = std::env::current_exe().std_context("failed to resolve current executable")?;
    let log_path = repo.agent_log_path();
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .std_context("failed to open agent.log")?;
    let log_err = log.try_clone().std_context("failed to clone agent.log")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--repo").arg(repo.path());
    cmd.arg(HEADLESS_AGENT_FLAG);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(log));
    cmd.stderr(Stdio::from(log_err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .std_context("failed to spawn datum-connect agent")?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}

/// Entry point for `--headless-agent` in the CLI and the desktop app.
pub async fn run_headless_agent_from_args() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();

    let repo = Repo::open_or_create(repo_path_from_cli_args()).await?;
    let shutdown = CancellationToken::new();
    let shutdown_for_signal = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_for_signal.cancel();
    });
    run_agent(repo, shutdown).await
}

pub async fn wait_until_ready(repo: &Repo, timeout: Duration) -> Result<AgentClient> {
    let deadline = Instant::now() + timeout;
    let mut last_err: Option<String> = None;
    while Instant::now() < deadline {
        match AgentClient::connect(repo) {
            Ok(client) => match client.status().await {
                Ok(_) => return Ok(client),
                Err(err) => last_err = Some(format!("{err:#}")),
            },
            Err(err) => last_err = Some(format!("{err:#}")),
        }
        tokio::time::sleep(AGENT_READY_POLL).await;
    }
    match last_err {
        Some(err) => n0_error::bail_any!("Timed out waiting for the agent to start: {err}"),
        None => n0_error::bail_any!("Timed out waiting for the agent to start"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnels::TunnelSummary;

    #[test]
    fn current_pid_is_alive() {
        assert!(is_pid_alive(std::process::id()));
    }

    #[test]
    fn bogus_pid_is_not_alive() {
        assert!(!is_pid_alive(u32::MAX - 1));
    }

    #[test]
    fn agent_info_roundtrip() {
        let info = AgentInfo {
            pid: 42,
            port: 1234,
            token: "abc".into(),
            endpoint_id: "ep".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: AgentInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.port, 1234);
        assert_eq!(parsed.token, "abc");
    }

    #[test]
    fn tunnel_view_prefers_named_hostname() {
        let summary = TunnelSummary {
            id: "t1".into(),
            label: "app".into(),
            endpoint: "http://127.0.0.1:4123".into(),
            hostnames: vec![
                "v4.example.iroh.datum.net".into(),
                "vast-gold-mine.iroh.datum.net".into(),
            ],
            enabled: true,
            accepted: true,
            programmed: true,
        };
        let view = TunnelView::from(&summary);
        assert_eq!(
            view.url.as_deref(),
            Some("https://vast-gold-mine.iroh.datum.net")
        );
    }

    #[test]
    fn tunnel_view_empty_hostnames() {
        let summary = TunnelSummary {
            id: "t1".into(),
            label: "app".into(),
            endpoint: "http://127.0.0.1:4123".into(),
            hostnames: vec![],
            enabled: true,
            accepted: false,
            programmed: false,
        };
        assert!(TunnelView::from(&summary).url.is_none());
    }
}
