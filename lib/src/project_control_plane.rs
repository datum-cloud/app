use std::future::Future;
use std::sync::Arc;

use arc_swap::ArcSwap;
use http::HeaderValue;
use http::header::USER_AGENT;
use kube::{Client, Config};
use n0_error::{Result, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use secrecy::SecretString;
use tracing::warn;

use crate::datum_cloud::{DatumCloudClient, LoginState, Unauthorized};
use crate::http_user_agent::datum_http_user_agent;

#[derive(derive_more::Debug, Clone)]
pub struct ProjectControlPlaneClient {
    project_id: String,
    server_url: String,
    access_token: Arc<ArcSwap<String>>,
    #[debug("kube::Client")]
    client: Arc<ArcSwap<Client>>,
    datum: DatumCloudClient,
    _auth_task: Option<Arc<AbortOnDropHandle<()>>>,
}

impl ProjectControlPlaneClient {
    pub fn new(
        project_id: String,
        server_url: String,
        access_token: String,
        datum: DatumCloudClient,
    ) -> Result<Self> {
        let client = Self::build_kube_client(&server_url, &access_token)?;
        let mut this = Self {
            project_id,
            server_url,
            access_token: Arc::new(ArcSwap::from_pointee(access_token)),
            client: Arc::new(ArcSwap::from_pointee(client)),
            datum,
            _auth_task: None,
        };
        this.start_auth_watch();
        Ok(this)
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn access_token(&self) -> String {
        self.access_token.load_full().as_ref().clone()
    }

    pub fn client(&self) -> Client {
        self.client.load_full().as_ref().clone()
    }

    pub async fn client_refreshed(&self) -> Result<Client> {
        let auth_state = self.datum.auth().load_refreshed().await?;
        let auth = auth_state.get()?;
        let access_token = auth.tokens.access_token.secret();
        self.rebuild_if_changed(access_token)?;
        Ok(self.client())
    }

    /// Build a `kube::Client` after forcing a token refresh, even if the access token
    /// is not near expiry. Used by [`Self::with_auth_retry`] when the server has just
    /// rejected the current bearer.
    async fn client_force_refreshed(&self) -> Result<Client> {
        self.datum.auth().force_refresh().await?;
        let auth_state = self.datum.auth().load();
        let auth = auth_state.get()?;
        let access_token = auth.tokens.access_token.secret();
        self.rebuild_if_changed(access_token)?;
        Ok(self.client())
    }

    /// Run a kube operation with auth handling (idempotent / read variant):
    ///
    /// 1. Preflight: refresh the access token if near expiry and rebuild the client.
    /// 2. Run the operation. On 401, force a refresh and retry once.
    /// 3. If the retry also returns 401, clear the local auth state and return
    ///    [`Unauthorized`]. The watch on `login_state` will then drive a redirect
    ///    to login in the UI.
    ///
    /// 403 responses (RBAC denial, quota admission, etc.) are returned as normal
    /// errors and do **not** clear the session.
    ///
    /// The closure is `Fn` so it can be invoked twice. Use this for idempotent
    /// operations (lists, gets, deletes). For non-idempotent writes, use
    /// [`Self::with_auth`] which preflights and logs out on auth failure without
    /// retrying.
    pub async fn with_auth_retry<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = kube::Result<T>>,
    {
        let client = self.client_refreshed().await?;
        let first_err = match op(client).await {
            Ok(val) => return Ok(val),
            Err(err) => err,
        };
        if !is_kube_auth_failure(&first_err) {
            return Err(first_err).std_context("kube operation failed");
        }

        warn!(
            err = %first_err,
            "kube returned auth failure; attempting forced token refresh"
        );
        let client = match self.client_force_refreshed().await {
            Ok(client) => client,
            Err(err) => {
                warn!("Forced auth refresh failed: {err:#}");
                return Err(Unauthorized.into());
            }
        };
        match op(client).await {
            Ok(val) => Ok(val),
            Err(err) if is_kube_auth_failure(&err) => {
                warn!(
                    err = %err,
                    "kube auth failure persisted after refresh; logging out"
                );
                if let Err(e) = self.datum.auth().logout().await {
                    warn!("Failed to clear auth state after persistent 401: {e:#}");
                }
                Err(Unauthorized.into())
            }
            Err(err) => Err(err).std_context("kube operation failed after refresh"),
        }
    }

    /// Run a kube operation with preflight refresh and auto-logout on auth failure,
    /// but without retrying. Use for non-idempotent writes (create/patch/etc.) where
    /// re-running the closure would produce secondary errors like "AlreadyExists".
    ///
    /// On 401 the local auth state is cleared and [`Unauthorized`] is returned;
    /// the UI's login-state watcher then routes the user back to the login screen.
    /// 403 responses (RBAC, quota, etc.) are returned as normal errors.
    pub async fn with_auth<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: FnOnce(Client) -> Fut,
        Fut: Future<Output = kube::Result<T>>,
    {
        let client = self.client_refreshed().await?;
        match op(client).await {
            Ok(val) => Ok(val),
            Err(err) if is_kube_auth_failure(&err) => {
                warn!(err = %err, "kube auth failure; logging out");
                if let Err(e) = self.datum.auth().logout().await {
                    warn!("Failed to clear auth state on 401: {e:#}");
                }
                Err(Unauthorized.into())
            }
            Err(err) => Err(err).std_context("kube operation failed"),
        }
    }

    fn build_kube_client(server_url: &str, access_token: &str) -> Result<Client> {
        let uri = server_url
            .parse()
            .std_context("Invalid project control plane URL")?;
        let mut config = Config::new(uri);
        config.auth_info.token = Some(SecretString::new(access_token.to_string().into_boxed_str()));
        let ua = HeaderValue::from_str(&datum_http_user_agent())
            .std_context("Invalid User-Agent for kube client")?;
        config.headers.push((USER_AGENT, ua));
        Client::try_from(config).std_context("Failed to create project control plane client")
    }

    fn rebuild_if_changed(&self, access_token: &str) -> Result<()> {
        let current = self.access_token.load_full();
        if current.as_ref().as_str() == access_token {
            return Ok(());
        }

        let client = Self::build_kube_client(&self.server_url, access_token)?;
        self.client.store(Arc::new(client));
        self.access_token.store(Arc::new(access_token.to_string()));
        Ok(())
    }

    async fn refresh_client_from_update(&self) -> Result<()> {
        let auth_state = self.datum.auth().load();
        let Ok(auth) = auth_state.get() else {
            return Ok(());
        };
        self.rebuild_if_changed(auth.tokens.access_token.secret())
    }

    fn start_auth_watch(&mut self) {
        if self._auth_task.is_some() {
            return;
        }
        let client = self.clone();
        let mut login_rx = self.datum.auth().login_state_watch();
        let mut auth_update_rx = self.datum.auth_update_watch();
        let task = tokio::spawn(async move {
            if *login_rx.borrow() != LoginState::Missing
                && let Err(err) = client.refresh_client_from_update().await
            {
                warn!("failed to refresh project control plane client: {err:#}");
            }
            loop {
                tokio::select! {
                    res = login_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                    res = auth_update_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                }
                if *login_rx.borrow() != LoginState::Missing
                    && let Err(err) = client.refresh_client_from_update().await
                {
                    warn!("failed to refresh project control plane client: {err:#}");
                }
            }
        });
        self._auth_task = Some(Arc::new(AbortOnDropHandle::new(task)));
    }
}

/// True if the kube error indicates the bearer token was rejected (HTTP 401).
///
/// 403 Forbidden is **not** treated as auth failure: it covers RBAC denials and
/// admission webhook rejections (e.g. Milo quota). Logging out on those would
/// dump the user to the login screen incorrectly.
pub fn is_kube_auth_failure(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(resp) if resp.code == 401)
}

/// True if the kube error is a quota admission rejection (403 with a quota message).
pub fn is_kube_quota_exceeded(err: &kube::Error) -> bool {
    match err {
        kube::Error::Api(resp) if resp.code == 403 => message_looks_like_quota(&resp.message),
        _ => false,
    }
}

/// True if an error chain (or Display string) looks like a Milo quota rejection.
pub fn error_looks_like_quota(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if message_looks_like_quota(&e.to_string()) {
            return true;
        }
        cur = e.source();
    }
    false
}

/// True if a message string looks like a Milo quota rejection.
pub fn message_looks_like_quota(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("quota") || lower.contains("you've reached your quota")
}

#[cfg(test)]
mod is_kube_auth_failure_tests {
    use super::*;
    use kube::core::ErrorResponse;

    fn api_err(code: u16) -> kube::Error {
        kube::Error::Api(ErrorResponse {
            status: "Failure".to_string(),
            message: "denied".to_string(),
            reason: "Unauthorized".to_string(),
            code,
        })
    }

    fn api_err_msg(code: u16, message: &str) -> kube::Error {
        kube::Error::Api(ErrorResponse {
            status: "Failure".to_string(),
            message: message.to_string(),
            reason: "Forbidden".to_string(),
            code,
        })
    }

    #[test]
    fn classifies_401_as_auth_failure() {
        assert!(is_kube_auth_failure(&api_err(401)));
    }

    #[test]
    fn does_not_treat_403_as_auth_failure() {
        assert!(!is_kube_auth_failure(&api_err(403)));
        assert!(!is_kube_auth_failure(&api_err_msg(
            403,
            "You've reached your quota for this resource type (Insufficient quota resources.)"
        )));
    }

    #[test]
    fn ignores_other_api_status_codes() {
        for code in [200u16, 403, 404, 409, 410, 500, 503] {
            assert!(
                !is_kube_auth_failure(&api_err(code)),
                "code {code} should not be an auth failure"
            );
        }
    }

    #[test]
    fn ignores_non_api_errors() {
        let err = kube::Error::TlsRequired;
        assert!(!is_kube_auth_failure(&err));
    }

    #[test]
    fn detects_quota_exceeded_403() {
        let err = api_err_msg(
            403,
            "httpproxies.networking.datumapis.com \"tunnel-jw8vf\" is forbidden: You've reached your quota for this resource type (Insufficient quota resources. Contact your account administrator to review quota limits and usage.).",
        );
        assert!(is_kube_quota_exceeded(&err));
        assert!(!is_kube_auth_failure(&err));
    }

    #[test]
    fn ignores_non_quota_403() {
        let err = api_err_msg(403, "httpproxies.networking.datumapis.com is forbidden");
        assert!(!is_kube_quota_exceeded(&err));
    }

    #[test]
    fn error_looks_like_quota_from_display() {
        let err = std::io::Error::new(
            std::io::ErrorKind::Other,
            "Insufficient quota resources available",
        );
        assert!(error_looks_like_quota(&err));
    }
}
