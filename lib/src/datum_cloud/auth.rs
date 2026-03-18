use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use chrono::Utc;
use n0_error::{Result, StackResultExt, StdResultExt, anyerr, stack_error};
use openidconnect::{
    AccessToken, AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl,
    Nonce, NonceVerifier, OAuth2TokenResponse, PkceCodeChallenge, RefreshToken, Scope,
    TokenResponse,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::Repo;

use self::{redirect_server::RedirectServer, types::OidcTokenResponse};
use super::ApiEnv;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);
/// Refresh auth or relogin if access token is valid for less than 30min
const REFRESH_AUTH_WHEN: Duration = Duration::from_secs(60 * 30);

pub struct AuthProvider {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum LoginState {
    Missing,
    NeedsRefresh,
    Valid,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthState {
    pub tokens: AuthTokens,
    pub profile: UserProfile,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: AccessToken,
    pub refresh_token: Option<RefreshToken>,
    pub issued_at: chrono::DateTime<Utc>,
    pub expires_in: Duration,
}

impl AuthTokens {
    pub fn is_expired(&self) -> bool {
        self.issued_at + self.expires_in < chrono::Utc::now()
    }

    pub fn expires_at(&self) -> chrono::DateTime<Utc> {
        self.issued_at + self.expires_in
    }

    pub fn expires_in_less_than(&self, duration: Duration) -> bool {
        self.issued_at + self.expires_in < chrono::Utc::now() + duration
    }

    pub fn login_state(&self) -> LoginState {
        match self.expires_in_less_than(Duration::from_secs(60 * 5)) {
            true => LoginState::NeedsRefresh,
            false => LoginState::Valid,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub avatar_url: Option<String>,
    pub registration_approval: Option<String>,
}

impl UserProfile {
    pub fn display_name(&self) -> String {
        match (self.first_name.as_ref(), self.last_name.as_ref()) {
            (Some(x), Some(y)) => format!("{x} {y}"),
            (Some(x), None) => x.clone(),
            (None, Some(y)) => y.clone(),
            (None, None) => self.email.clone(),
        }
    }

    // fn from_standard_claims<GC>(claims: &StandardClaims<GC>) -> Result<Self>
    // where
    //     GC: GenderClaim,
    // {
    //     Ok(Self {
    //         user_id: claims.subject().to_string(),
    //         email: claims
    //             .email()
    //             .map(|x| x.to_string())
    //             .context("missing email address")?,
    //         first_name: claims
    //             .given_name()
    //             .map(|x| x.iter())
    //             .into_iter()
    //             .flatten()
    //             .next()
    //             .map(|(_lang, name)| name.to_string()),
    //         last_name: claims
    //             .family_name()
    //             .map(|x| x.iter())
    //             .into_iter()
    //             .flatten()
    //             .next()
    //             .map(|(_lang, name)| name.to_string()),
    //     })
    // }
}

#[derive(Debug, Clone)]
pub struct StatelessClient {
    oidc: types::OidcClient,
    http: reqwest::Client,
    env: ApiEnv,
}

impl StatelessClient {
    pub async fn new(env: ApiEnv) -> Result<Self> {
        Self::with_provider(env, env.auth_provider()).await
    }

    pub async fn with_provider(env: ApiEnv, provider: AuthProvider) -> Result<Self> {
        let http = reqwest::ClientBuilder::new()
            // Following redirects opens the client up to SSRF vulnerabilities.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Client should build");

        // Use OpenID Connect Discovery to fetch the provider metadata (including JWKs).
        // We fetch fresh metadata each time to avoid "No matching key found" when
        // Datum Cloud rotates signing keys (see datum-cloud/app#121).
        let provider_metadata = CoreProviderMetadata::discover_async(
            IssuerUrl::new(provider.issuer_url).std_context("Invalid OIDC provider issuer URL")?,
            &http,
        )
        .await
        .std_context("Failed to discover OIDC provider metadata")?;
        debug!(
            jwks_uri=?provider_metadata.jwks_uri(),
            "fetched fresh OIDC provider metadata"
        );

        // Create an OpenID Connect client
        let oidc = CoreClient::from_provider_metadata(
            provider_metadata,
            ClientId::new(provider.client_id),
            provider.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(RedirectServer::url());

        Ok(Self { oidc, http, env })
    }

    pub async fn login<F, Fut, C>(&self, open_url: F) -> Result<AuthState>
    where
        F: FnOnce(String, CancellationToken) -> Fut,
        Fut: Future<Output = Option<C>>,
        C: FnOnce() + Send + 'static,
    {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let (auth_url, csrf_token, nonce) = self
            .oidc
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("openid".to_string()))
            .add_scope(Scope::new("profile".to_string()))
            .add_scope(Scope::new("email".to_string()))
            .add_scope(Scope::new("offline_access".to_string()))
            .set_pkce_challenge(pkce_challenge)
            .url();
        debug!(auth_uri=%self.oidc.auth_uri(), "attempting login");

        // Bind a localhost HTTP server to receive the redirect.
        let mut redirect_server = RedirectServer::bind(csrf_token.clone()).await?;

        let cancel_token = CancellationToken::new();
        let cancel_token_for_opener = cancel_token.clone();

        // Open the auth URL; opener may return a close handle to close the window when done.
        let mut close_handle = open_url(auth_url.to_string(), cancel_token_for_opener).await;

        let recv_result = redirect_server
            .recv_with_timeout(LOGIN_TIMEOUT, Some(&cancel_token))
            .await;

        // Close the auth window if one was opened (e.g. in-app webview), on success or error.
        if let Some(close) = close_handle.take() {
            close();
        }

        let authorization_code = recv_result?;
        debug!("received redirect with authorization code");

        // Exchange auth code for ID and access tokens.
        let tokens = self
            .oidc
            .exchange_code(AuthorizationCode::new(authorization_code))
            .std_context("Missing OIDC provider metadata")?
            .set_pkce_verifier(pkce_verifier)
            .request_async(&self.http)
            .await
            .std_context("Failed to exchange auth code to access token")
            .inspect_err(|e| error!("{e:#}"))?;

        let expected_nonce = nonce.clone();
        let nonce_verifier = move |received_nonce: Option<&Nonce>| -> Result<(), String> {
            match received_nonce {
                Some(received) => {
                    let received_str = format!("{:?}", received);
                    let expected_str = format!("{:?}", expected_nonce);
                    if received_str == expected_str {
                        Ok(())
                    } else {
                        Err("Nonce mismatch".to_string())
                    }
                }
                None => Err("Missing nonce in ID token".to_string()),
            }
        };
        let state = self.parse_token_response(tokens, nonce_verifier).await?;
        info!(email=%state.profile.email, expires_at=%state.tokens.expires_at(), "login succesfull");
        Ok(state)
    }

    pub async fn refresh(&self, tokens: &AuthTokens) -> Result<AuthState> {
        let refresh_token = tokens.refresh_token.as_ref().context("No refresh token")?;
        debug!("Refreshing access token");
        let tokens = self
            .oidc
            .exchange_refresh_token(refresh_token)
            .std_context("Missing OIDC provider metadata")?
            .request_async(&self.http)
            .await
            .std_context("Failed to refresh tokens")?;
        let state = self
            .parse_token_response(tokens, refresh_nonce_verifier)
            .await?;
        debug!("Access token refreshed");
        Ok(state)
    }

    async fn parse_token_response(
        &self,
        tokens: OidcTokenResponse,
        nonce_verifier: impl NonceVerifier,
    ) -> Result<AuthState> {
        // Extract the ID token claims after verifying its authenticity and nonce.
        let id_token = tokens
            .id_token()
            .ok_or_else(|| anyerr!("Server did not return an ID token"))?;
        let id_token_verifier = self
            .oidc
            .id_token_verifier()
            // Datum auth backend includes multiple audiences in the id tokens
            .set_other_audience_verifier_fn(|_audience| true);

        let claims = id_token
            .claims(&id_token_verifier, nonce_verifier)
            .map_err(|e| {
                error!(
                    error=%e,
                    signing_alg=?id_token.signing_alg(),
                    "Failed to verify ID token claims, try logging in again"
                );
                anyerr!("Failed to verify login. Please try again — if the problem persists, your session may need to be refreshed.")
            })?;

        // Verify the access token hash to ensure that the access token hasn't been substituted for
        // another user's.
        if let Some(expected_access_token_hash) = claims.access_token_hash() {
            let actual_access_token_hash = AccessTokenHash::from_token(
                tokens.access_token(),
                id_token
                    .signing_alg()
                    .std_context("Invalid id token signing algorithm")?,
                id_token
                    .signing_key(&id_token_verifier)
                    .std_context("Missing id token signing key")?,
            )
            .std_context("failed to create access token hash from token")?;
            if actual_access_token_hash != *expected_access_token_hash {
                return Err(anyerr!("Invalid access token"));
            }
        }

        // Extract user_id from ID token claims
        let user_id = claims.subject().to_string();
        let issued_at = claims.issue_time();

        // Create auth tokens
        let auth_tokens = AuthTokens {
            issued_at,
            access_token: tokens.access_token().clone(),
            refresh_token: tokens.refresh_token().cloned(),
            expires_in: tokens.expires_in().context("Missing expires_in claim")?,
        };

        // Fetch user profile from Datum Cloud API
        let profile = self.fetch_user_profile(&auth_tokens, &user_id).await?;

        Ok(AuthState {
            tokens: auth_tokens,
            profile,
        })
    }

    pub(crate) async fn fetch_user_profile(
        &self,
        tokens: &AuthTokens,
        user_id: &str,
    ) -> Result<UserProfile> {
        fn parse_user(json: &serde_json::Value) -> Option<UserProfile> {
            let metadata = json.get("metadata")?.as_object()?;
            let spec = json.get("spec").and_then(|s| s.as_object());
            let status = json.get("status").and_then(|s| s.as_object());

            // Extract user_id from metadata.name
            let user_id = metadata.get("name")?.as_str()?.to_string();

            // Extract email from spec or status (try spec first, then status)
            let email = spec
                .and_then(|s| s.get("email"))
                .or_else(|| status.and_then(|s| s.get("email")))
                .and_then(|e| e.as_str())
                .map(|s| s.to_string());

            // Extract first_name from spec (API uses givenName, not firstName)
            let first_name = spec
                .and_then(|s| s.get("givenName"))
                .or_else(|| spec.and_then(|s| s.get("firstName")))
                .or_else(|| spec.and_then(|s| s.get("first_name")))
                .or_else(|| status.and_then(|s| s.get("givenName")))
                .or_else(|| status.and_then(|s| s.get("firstName")))
                .or_else(|| status.and_then(|s| s.get("first_name")))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());

            // Extract last_name from spec (API uses familyName, not lastName)
            let last_name = spec
                .and_then(|s| s.get("familyName"))
                .or_else(|| spec.and_then(|s| s.get("lastName")))
                .or_else(|| spec.and_then(|s| s.get("last_name")))
                .or_else(|| status.and_then(|s| s.get("familyName")))
                .or_else(|| status.and_then(|s| s.get("lastName")))
                .or_else(|| status.and_then(|s| s.get("last_name")))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());

            let avatar_url = status
                .and_then(|s| s.get("avatarUrl"))
                .or_else(|| status.and_then(|s| s.get("avatar_url")))
                .or_else(|| spec.and_then(|s| s.get("avatarUrl")))
                .or_else(|| spec.and_then(|s| s.get("avatar_url")))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());

            let registration_approval = status
                .and_then(|s| s.get("registrationApproval"))
                .or_else(|| status.and_then(|s| s.get("registration_approval")))
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());

            Some(UserProfile {
                user_id,
                email: email?,
                first_name,
                last_name,
                avatar_url,
                registration_approval,
            })
        }

        let url = format!(
            "{}/apis/iam.miloapis.com/v1alpha1/users/{}",
            self.env.api_url(),
            user_id
        );

        let res = self
            .http
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", tokens.access_token.secret()),
            )
            .send()
            .await
            .inspect_err(|e| warn!(%url, "Failed to fetch user profile: {e:#}"))
            .with_std_context(|_| format!("Failed to fetch user profile from {url}"))?;

        let status = res.status();
        if !status.is_success() {
            let text = match res.text().await {
                Ok(text) => text,
                Err(err) => err.to_string(),
            };
            warn!(%url, "Request failed: {status} {text}");
            n0_error::bail_any!("Request failed with status {status}");
        }

        let json: serde_json::Value = res
            .json()
            .await
            .std_context("Failed to parse user profile response as JSON")?;

        parse_user(&json).context("Failed to parse user profile")
    }
}

#[stack_error(derive)]
#[error("Not logged in")]
pub struct NotLoggedIn;

#[derive(Default, Debug)]
pub struct MaybeAuth(Option<AuthState>);

impl MaybeAuth {
    pub fn get(&self) -> Result<&AuthState, NotLoggedIn> {
        self.0.as_ref().ok_or(NotLoggedIn)
    }

    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

#[derive(Debug, Clone)]
struct AuthStateWrapper {
    inner: Arc<ArcSwap<MaybeAuth>>,
    repo: Option<Repo>,
    oauth_key: String,
    login_state_tx: watch::Sender<LoginState>,
    auth_update_tx: watch::Sender<u64>,
    auth_update_counter: Arc<AtomicU64>,
}

impl AuthStateWrapper {
    fn empty() -> Self {
        let (login_state_tx, _) = watch::channel(LoginState::Missing);
        let (auth_update_tx, _) = watch::channel(0);
        Self {
            inner: Arc::new(ArcSwap::new(Default::default())),
            repo: None,
            oauth_key: String::new(),
            login_state_tx,
            auth_update_tx,
            auth_update_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    async fn from_repo(repo: Repo, oauth_key: &str) -> Result<Self> {
        let state = repo.read_oauth_for_key(oauth_key).await?;
        set_sentry_user(state.as_ref());
        let (login_state_tx, _) = watch::channel(login_state_for(state.as_ref()));
        let (auth_update_tx, _) = watch::channel(0);
        Ok(Self {
            inner: Arc::new(ArcSwap::new(Arc::new(MaybeAuth(state)))),
            repo: Some(repo),
            oauth_key: oauth_key.to_string(),
            login_state_tx,
            auth_update_tx,
            auth_update_counter: Arc::new(AtomicU64::new(0)),
        })
    }

    fn load(&self) -> Arc<MaybeAuth> {
        self.inner.load_full()
    }

    fn subscribe_login_state(&self) -> watch::Receiver<LoginState> {
        self.login_state_tx.subscribe()
    }

    fn subscribe_auth_updates(&self) -> watch::Receiver<u64> {
        self.auth_update_tx.subscribe()
    }

    async fn set(&self, auth: Option<AuthState>) -> Result<()> {
        if let Some(repo) = self.repo.as_ref() {
            repo.write_oauth_for_key(&self.oauth_key, auth.as_ref())
                .await?;
        }
        set_sentry_user(auth.as_ref());
        self.inner.store(Arc::new(MaybeAuth(auth)));
        let _ = self
            .login_state_tx
            .send(login_state_for(self.load().get().ok()));
        let next = self.auth_update_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.auth_update_tx.send(next);
        Ok(())
    }
}

fn login_state_for(auth: Option<&AuthState>) -> LoginState {
    match auth {
        None => LoginState::Missing,
        Some(state) => state.tokens.login_state(),
    }
}

fn set_sentry_user(auth: Option<&AuthState>) {
    sentry::configure_scope(|scope| {
        scope.set_user(auth.map(|state| sentry::User {
            id: Some(state.profile.user_id.clone()),
            email: Some(state.profile.email.clone()),
            username: Some(state.profile.display_name()),
            ..Default::default()
        }));
    });
}

#[derive(derive_more::Debug, Clone)]
pub struct AuthClient {
    state: AuthStateWrapper,
    env: ApiEnv,
    /// OIDC client with JWKs. Swapped before each login/refresh so we always have fresh keys
    /// (avoids "No matching key found" when Datum Cloud rotates signing keys; datum-cloud/app#121).
    client: Arc<ArcSwap<StatelessClient>>,
    _refresh_task: Option<Arc<n0_future::task::AbortOnDropHandle<()>>>,
}

impl AuthClient {
    pub async fn with_repo(env: ApiEnv, repo: Repo) -> Result<Self> {
        let auth = AuthStateWrapper::from_repo(repo, env.oauth_storage_key()).await?;
        let auth_client = Arc::new(StatelessClient::new(env).await?);
        let mut client = Self {
            state: auth,
            env,
            client: Arc::new(ArcSwap::new(auth_client)),
            _refresh_task: None,
        };
        client.start_refresh_loop();
        Ok(client)
    }

    pub async fn new(env: ApiEnv) -> Result<Self> {
        let auth = AuthStateWrapper::empty();
        let auth_client = Arc::new(StatelessClient::new(env).await?);
        let mut client = Self {
            state: auth,
            env,
            client: Arc::new(ArcSwap::new(auth_client)),
            _refresh_task: None,
        };
        client.start_refresh_loop();
        Ok(client)
    }

    /// Fetch fresh OIDC provider metadata (including JWKs) and swap in a new client.
    /// Call before login/refresh to avoid "No matching key found" when keys rotate.
    async fn ensure_fresh_client(&self) -> Result<Arc<StatelessClient>> {
        let fresh = Arc::new(
            StatelessClient::with_provider(self.env, self.env.auth_provider()).await?,
        );
        self.client.store(fresh.clone());
        Ok(fresh)
    }

    pub fn login_state(&self) -> LoginState {
        match self.state.load().get().ok() {
            None => LoginState::Missing,
            Some(state) => state.tokens.login_state(),
        }
    }

    pub fn load(&self) -> Arc<MaybeAuth> {
        self.state.load()
    }

    pub fn login_state_watch(&self) -> watch::Receiver<LoginState> {
        self.state.subscribe_login_state()
    }

    pub fn auth_update_watch(&self) -> watch::Receiver<u64> {
        self.state.subscribe_auth_updates()
    }

    fn start_refresh_loop(&mut self) {
        if self._refresh_task.is_some() {
            return;
        }
        let client = self.clone();
        let mut auth_update_rx = self.auth_update_watch();
        let task = tokio::spawn(async move {
            loop {
                if let Err(err) = client.refresh_if_needed().await {
                    warn!("auth refresh check failed: {err:#}");
                }
                let sleep_for = client.next_refresh_delay();
                tokio::select! {
                    _ = tokio::time::sleep(sleep_for) => {},
                    res = auth_update_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        self._refresh_task = Some(Arc::new(n0_future::task::AbortOnDropHandle::new(task)));
    }

    fn next_refresh_delay(&self) -> Duration {
        let state = self.state.load();
        let Ok(auth) = state.get() else {
            return Duration::from_secs(60);
        };
        let expires_at = auth.tokens.expires_at();
        let now = chrono::Utc::now();
        let refresh_at = expires_at - REFRESH_AUTH_WHEN;
        if refresh_at <= now {
            return Duration::from_secs(1);
        }
        let delay = refresh_at - now;
        Duration::from_secs(delay.num_seconds().max(1) as u64)
    }

    async fn refresh_if_needed(&self) -> Result<()> {
        let state = self.state.load();
        let Ok(auth) = state.get() else {
            return Ok(());
        };
        if auth.tokens.expires_in_less_than(REFRESH_AUTH_WHEN) {
            self.refresh().await?;
        }
        Ok(())
    }

    pub async fn load_refreshed(&self) -> Result<Arc<MaybeAuth>> {
        let state = self.state.load();
        match state.get() {
            Err(_) => Ok(state),
            Ok(inner) if inner.tokens.expires_in_less_than(REFRESH_AUTH_WHEN) => {
                self.refresh().await?;
                Ok(self.state.load())
            }
            Ok(_) => Ok(state),
        }
    }

    pub async fn logout(&self) -> Result<()> {
        self.state.set(None).await?;
        Ok(())
    }

    pub async fn login(&self) -> Result<()> {
        self.login_with_opener(|url, _cancel_token| async move {
            if let Err(err) = open::that(&url) {
                warn!("Failed to auto-open url: {err}");
                eprintln!("Open this URL in a browser to complete the login:\n{url}");
            }
            None::<Box<dyn FnOnce() + Send>>
        })
        .await
    }

    pub async fn login_with_opener<F, Fut, C>(&self, open_url: F) -> Result<()>
    where
        F: FnOnce(String, CancellationToken) -> Fut,
        Fut: Future<Output = Option<C>>,
        C: FnOnce() + Send + 'static,
    {
        let auth = self.state.load();
        let auth = match auth.get() {
            Err(_) => {
                let client = self.ensure_fresh_client().await?;
                client.login(open_url).await?
            }
            Ok(auth) if auth.tokens.expires_in_less_than(REFRESH_AUTH_WHEN) => {
                let client = self.ensure_fresh_client().await?;
                match client.refresh(&auth.tokens).await {
                    Ok(auth) => auth,
                    Err(err) => {
                        warn!("Failed to refresh auth token: {err:#}");
                        let client = self.ensure_fresh_client().await?;
                        client
                            .login(|url, _cancel_token| async move {
                                if let Err(e) = open::that(&url) {
                                    warn!("Failed to auto-open url: {e}");
                                    eprintln!("Open this URL in a browser to complete the login:\n{url}");
                                }
                                None::<Box<dyn FnOnce() + Send>>
                            })
                            .await?
                    }
                }
            }
            Ok(_) => return Ok(()),
        };
        self.state.set(Some(auth)).await?;
        Ok(())
    }

    pub async fn refresh(&self) -> Result<()> {
        let auth = self.state.load();
        let auth = auth.get()?;
        let client = self.ensure_fresh_client().await?;
        let new_auth = match client.refresh(&auth.tokens).await {
            Ok(auth) => auth,
            Err(err) => {
                warn!("Failed to refresh auth tokens, logging out: {err:#}");
                self.state.set(None).await?;
                Err(err).context("Failed to refresh auth tokens, needs login")?
            }
        };
        self.state.set(Some(new_auth)).await?;
        Ok(())
    }

    /// Refresh the user profile from the API without refreshing tokens
    pub async fn refresh_profile(&self) -> Result<()> {
        let auth = self.state.load();
        let auth = auth.get()?;
        let user_id = auth.profile.user_id.clone();
        let new_profile = self
            .client
            .load()
            .fetch_user_profile(&auth.tokens, &user_id)
            .await?;
        let new_auth = AuthState {
            tokens: AuthTokens {
                access_token: auth.tokens.access_token.clone(),
                refresh_token: auth.tokens.refresh_token.as_ref().cloned(),
                issued_at: auth.tokens.issued_at,
                expires_in: auth.tokens.expires_in,
            },
            profile: new_profile,
        };
        self.state.set(Some(new_auth)).await?;
        Ok(())
    }
}

/// Refresh requests don't have nonces.
fn refresh_nonce_verifier(_: Option<&Nonce>) -> Result<(), String> {
    Ok(())
}

mod types {
    use openidconnect::core::*;
    use openidconnect::*;

    /// An [`openidconnect::Client`] with all generics filled in.
    // Yes, this is as long as it looks.
    pub(super) type OidcClient = Client<
        EmptyAdditionalClaims,
        CoreAuthDisplay,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJsonWebKey,
        CoreAuthPrompt,
        StandardErrorResponse<CoreErrorResponseType>,
        StandardTokenResponse<
            IdTokenFields<
                EmptyAdditionalClaims,
                EmptyExtraTokenFields,
                CoreGenderClaim,
                CoreJweContentEncryptionAlgorithm,
                CoreJwsSigningAlgorithm,
            >,
            CoreTokenType,
        >,
        StandardTokenIntrospectionResponse<EmptyExtraTokenFields, CoreTokenType>,
        CoreRevocableToken,
        StandardErrorResponse<RevocationErrorResponseType>,
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >;

    pub(super) type OidcTokenResponse = StandardTokenResponse<
        IdTokenFields<
            EmptyAdditionalClaims,
            EmptyExtraTokenFields,
            CoreGenderClaim,
            CoreJweContentEncryptionAlgorithm,
            CoreJwsSigningAlgorithm,
        >,
        CoreTokenType,
    >;
}

mod redirect_server {
    //! Web server waiting for OAuth redirct requests

    use axum::{
        Router,
        extract::{Query, State},
        routing::get,
    };
    use n0_error::{StdResultExt, anyerr};
    use openidconnect::{CsrfToken, RedirectUrl};
    use serde::Deserialize;
    use std::{
        net::{Ipv4Addr, SocketAddr},
        time::Duration,
    };
    use tokio::net::TcpSocket;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;
    use tracing::{Instrument, debug, instrument, warn};

    pub const REDIRECT_SERVER_PORT: u16 = 7076;

    #[derive(Deserialize, Debug)]
    struct OauthRedirectData {
        pub code: String,
        pub state: String,
    }

    pub struct RedirectServer {
        rx: mpsc::Receiver<n0_error::Result<OauthRedirectData>>,
        cancel_token: CancellationToken,
        csrf_token: CsrfToken,
    }

    impl RedirectServer {
        #[instrument("oidc-redirect-server")]
        pub async fn bind(csrf_token: CsrfToken) -> std::io::Result<Self> {
            let bind_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), REDIRECT_SERVER_PORT);
            let cancel_token = CancellationToken::new();
            let (tx, rx) = mpsc::channel(1);
            let state = AppState { sender: tx.clone() };
            // Route all paths and all methods to the same handler
            let app = Router::new()
                .route("/oauth/redirect", get(oauth_redirect))
                .with_state(state);
            let socket = TcpSocket::new_v4()?;
            socket.set_reuseaddr(true)?;
            socket.bind(bind_addr)?;
            let listener = socket.listen(128)?;
            debug!(addr=%bind_addr, "OIDC redirect HTTP server listening");

            tokio::spawn({
                let cancel_token = cancel_token.clone();
                async move {
                    if let Err(err) = axum::serve(listener, app)
                        .with_graceful_shutdown(cancel_token.cancelled_owned())
                        .await
                    {
                        warn!("OIDC redirect HTTP server failed: {err:#}");
                        tx.send(Err(err.into())).await.ok();
                    } else {
                        debug!("OIDC redirect HTTP server stopped");
                    }
                }
                .instrument(tracing::Span::current())
            });

            Ok(Self {
                cancel_token,
                rx,
                csrf_token,
            })
        }

        pub fn url() -> RedirectUrl {
            RedirectUrl::new(format!(
                "http://localhost:{}/oauth/redirect",
                REDIRECT_SERVER_PORT
            ))
            .expect("valid url")
        }

        pub async fn recv_with_timeout(
            &mut self,
            timeout: Duration,
            cancel: Option<&CancellationToken>,
        ) -> n0_error::Result<String> {
            let res = if let Some(cancel_token) = cancel {
                tokio::select! {
                    _ = cancel_token.cancelled() => Err(anyerr!("Login cancelled")),
                    r = tokio::time::timeout(timeout, self.recv()) => r.anyerr()?,
                }
            } else {
                tokio::time::timeout(timeout, self.recv()).await.anyerr()?
            };
            self.cancel_token.cancel();
            res
        }

        pub async fn recv(&mut self) -> n0_error::Result<String> {
            let code = loop {
                let reply = self
                    .rx
                    .recv()
                    .await
                    .std_context("web server closed")?
                    .std_context("web server failed")?;
                if reply.state == *self.csrf_token.secret() {
                    break reply.code;
                }
            };
            self.cancel_token.cancel();
            Ok(code)
        }
    }

    impl Drop for RedirectServer {
        fn drop(&mut self) {
            self.cancel_token.cancel();
        }
    }

    #[derive(Clone)]
    struct AppState {
        sender: mpsc::Sender<n0_error::Result<OauthRedirectData>>,
    }

    async fn oauth_redirect(state: State<AppState>, query: Query<OauthRedirectData>) -> axum::response::Html<String> {
        let data = query.0;
        state.sender.send(Ok(data)).await.ok();
        axum::response::Html(
            r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Login complete</title></head>
<body>
<p>You are now logged in. This window will close automatically.</p>
<script>window.close();</script>
</body>
</html>"#
                .to_string(),
        )
    }
}
