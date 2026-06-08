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
    AccessToken, AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    DeviceAuthorizationUrl, IssuerUrl, Nonce, NonceVerifier, OAuth2TokenResponse,
    PkceCodeChallenge, RefreshToken, Scope, TokenResponse,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreDeviceAuthorizationResponse, CoreProviderMetadata,
    },
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::Repo;
use crate::http_user_agent::datum_http_user_agent;

use self::{redirect_server::RedirectServer, types::OidcTokenResponse};
use super::ApiEnv;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);
/// Refresh auth or relogin if access token is valid for less than 30min
const REFRESH_AUTH_WHEN: Duration = Duration::from_secs(60 * 30);

/// Surface of an in-flight OAuth2 device authorization grant. The
/// `display` callback passed to [`StatelessClient::login_device_code`]
/// receives one of these and is responsible for showing the verification
/// URL + user code to the operator.
#[derive(Debug, Clone)]
pub struct DeviceCodeInfo {
    /// URL the user opens on their other device to authorize.
    pub verification_uri: String,
    /// Short code the user enters at `verification_uri`.
    pub user_code: String,
    /// Optional URL that pre-fills the user code, so the user only has
    /// to follow one link.
    pub verification_uri_complete: Option<String>,
    /// How long the user has before the device code expires.
    pub expires_in: Duration,
    /// Operator-side polling interval recommended by the auth server.
    pub interval: Duration,
}

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
            .user_agent(datum_http_user_agent())
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

    pub async fn login<F, Fut>(&self, open_url: F) -> Result<AuthState>
    where
        F: FnOnce(String, CancellationToken) -> Fut,
        Fut: Future<Output = ()>,
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
            .add_extra_param("prompt", "select_account")
            .set_pkce_challenge(pkce_challenge)
            .url();
        debug!(auth_uri=%self.oidc.auth_uri(), "attempting login");

        // Bind a localhost HTTP server to receive the redirect.
        let mut redirect_server = RedirectServer::bind(csrf_token.clone()).await?;

        let cancel_token = CancellationToken::new();
        let cancel_token_for_opener = cancel_token.clone();

        open_url(auth_url.to_string(), cancel_token_for_opener).await;

        let authorization_code = redirect_server
            .recv_with_timeout(LOGIN_TIMEOUT, Some(&cancel_token))
            .await?;
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
        let state = self
            .parse_token_response(tokens, nonce_verifier, None)
            .await?;
        info!(email=%state.profile.email, expires_at=%state.tokens.expires_at(), "login succesfull");
        Ok(state)
    }

    /// OAuth2 Device Authorization grant (RFC 8628). Used by `datum-connect
    /// auth login --no-browser` and any other headless context where the
    /// localhost-redirect flow can't reach back to a browser (SSH, CI,
    /// containers). The caller receives a `DeviceCodeInfo` via the
    /// `display` callback and is responsible for showing the verification
    /// URL + user code to the operator; this method then polls the token
    /// endpoint until the user completes authorization.
    pub async fn login_device_code<F, Fut>(&self, display: F) -> Result<AuthState>
    where
        F: FnOnce(DeviceCodeInfo) -> Fut,
        Fut: Future<Output = ()>,
    {
        // `openidconnect::CoreProviderMetadata` doesn't surface the
        // `device_authorization_endpoint` from discovery, so refetch the
        // raw JSON to find it.
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.env.auth_provider().issuer_url.trim_end_matches('/'),
        );
        #[derive(Deserialize)]
        struct DiscoveryDoc {
            device_authorization_endpoint: Option<String>,
        }
        let discovery: DiscoveryDoc = self
            .http
            .get(&discovery_url)
            .send()
            .await
            .std_context("Failed to fetch OIDC discovery document")?
            .error_for_status()
            .std_context("OIDC discovery returned a non-success status")?
            .json()
            .await
            .std_context("Failed to parse OIDC discovery document")?;
        let device_endpoint = discovery.device_authorization_endpoint.context(
            "Auth server does not advertise a device_authorization_endpoint; \
             --no-browser is unsupported against this provider",
        )?;

        // Rebuild a CoreClient with the device URL set. The crate's
        // typestate prevents mutating the cached `self.oidc` in place,
        // and the discovery the constructor performs is cheap enough to
        // accept the duplication.
        let provider = self.env.auth_provider();
        let issuer = IssuerUrl::new(provider.issuer_url.clone())
            .std_context("Invalid OIDC provider issuer URL")?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &self.http)
            .await
            .std_context("Failed to discover OIDC provider metadata")?;
        let oidc = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(provider.client_id),
            provider.client_secret.clone().map(ClientSecret::new),
        )
        .set_device_authorization_url(
            DeviceAuthorizationUrl::new(device_endpoint).std_context(
                "Invalid device_authorization_endpoint in OIDC discovery",
            )?,
        );

        let details: CoreDeviceAuthorizationResponse = oidc
            .exchange_device_code()
            .add_scope(Scope::new("openid".to_string()))
            .add_scope(Scope::new("profile".to_string()))
            .add_scope(Scope::new("email".to_string()))
            .add_scope(Scope::new("offline_access".to_string()))
            .request_async(&self.http)
            .await
            .std_context("Failed to start device authorization")?;

        let info = DeviceCodeInfo {
            verification_uri: details.verification_uri().to_string(),
            user_code: details.user_code().secret().to_string(),
            verification_uri_complete: details
                .verification_uri_complete()
                .map(|u| u.secret().to_string()),
            expires_in: Duration::from_secs(details.expires_in().as_secs()),
            interval: Duration::from_secs(details.interval().as_secs()),
        };
        display(info).await;

        let tokens = oidc
            .exchange_device_access_token(&details)
            .std_context("Device-flow client misconfigured")?
            .request_async(&self.http, tokio::time::sleep, None)
            .await
            .std_context("Failed to exchange device access token")?;

        // Device flow doesn't bind a nonce (no user-agent round-trip carrying
        // one), so accept an absent nonce in the ID token.
        let nonce_verifier =
            |_received: Option<&Nonce>| -> std::result::Result<(), String> { Ok(()) };
        let state = self.parse_token_response(tokens, nonce_verifier, None).await?;
        info!(
            email=%state.profile.email,
            expires_at=%state.tokens.expires_at(),
            "device-code login successful"
        );
        Ok(state)
    }

    pub async fn refresh(
        &self,
        tokens: &AuthTokens,
        fallback_profile: Option<UserProfile>,
    ) -> std::result::Result<AuthState, RefreshError> {
        let refresh_token = tokens.refresh_token.as_ref().ok_or_else(|| {
            // No stored refresh token means we cannot exchange anything; only an
            // interactive login can repopulate it.
            RefreshError::Permanent(anyerr!("No refresh token available"))
        })?;
        debug!("Refreshing access token");
        let refresh_req = self
            .oidc
            .exchange_refresh_token(refresh_token)
            .std_context("Missing OIDC provider metadata")
            .map_err(RefreshError::Transient)?;
        let tokens = match refresh_req.request_async(&self.http).await {
            Ok(tokens) => tokens,
            Err(err) => {
                let permanent = classify_oidc_refresh_error(&err);
                let wrapped = anyerr!("OAuth refresh exchange failed: {err}");
                return Err(if permanent {
                    RefreshError::Permanent(wrapped)
                } else {
                    RefreshError::Transient(wrapped)
                });
            }
        };
        // ID token verification failures (e.g. stale JWKs after key rotation)
        // are transient: ensure_fresh_client() on the next attempt refetches
        // provider metadata. Profile-fetch failures are already handled by
        // fallback_profile inside parse_token_response.
        let state = self
            .parse_token_response(tokens, refresh_nonce_verifier, fallback_profile)
            .await
            .map_err(RefreshError::Transient)?;
        debug!("Access token refreshed");
        Ok(state)
    }

    async fn parse_token_response(
        &self,
        tokens: OidcTokenResponse,
        nonce_verifier: impl NonceVerifier,
        fallback_profile: Option<UserProfile>,
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

        // Fetch user profile from Datum Cloud API. If the fetch fails but we already
        // have a profile from a prior login, keep the prior one rather than dropping
        // the freshly minted tokens. This guards against transient API blips (e.g. a
        // 401 on /users/{id} while the new access token is still propagating) causing
        // a full logout. See datum-cloud/app#TBD.
        let profile = match self.fetch_user_profile(&auth_tokens, &user_id).await {
            Ok(profile) => profile,
            Err(err) => match fallback_profile {
                Some(profile) => {
                    warn!(
                        "Profile fetch after token refresh failed, keeping prior profile: {err:#}"
                    );
                    profile
                }
                None => return Err(err),
            },
        };

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

/// Outcome classification for a token refresh attempt.
///
/// The distinction matters for the heartbeat loop and the listener: a
/// `Transient` failure should keep auth state intact and let the next retry
/// recover, while a `Permanent` failure means the OAuth provider has
/// definitively rejected our credentials and only a fresh interactive login
/// can recover. Treating every refresh failure as permanent — which the
/// previous implementation did — meant a 30-second IdP wobble would log a
/// long-running tunnel out.
#[derive(Debug)]
pub enum RefreshError {
    /// The IdP definitively rejected the refresh (typically `invalid_grant`,
    /// `invalid_client`, etc.). Auth state has been cleared; the operator
    /// must log in again.
    Permanent(n0_error::AnyError),
    /// Transient failure (network, IdP 5xx, parse error, ID-token claim
    /// verification). Auth state is preserved; the caller should retry with
    /// backoff.
    Transient(n0_error::AnyError),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Permanent(e) => write!(f, "refresh permanently rejected by IdP: {e:#}"),
            Self::Transient(e) => write!(f, "transient refresh failure: {e:#}"),
        }
    }
}

impl std::error::Error for RefreshError {}

/// Decide whether a `RequestTokenError` from the OAuth refresh exchange is
/// permanent (re-login required) or transient (retry).
fn classify_oidc_refresh_error<RE>(
    err: &openidconnect::RequestTokenError<
        RE,
        openidconnect::StandardErrorResponse<openidconnect::core::CoreErrorResponseType>,
    >,
) -> bool
where
    RE: std::error::Error,
{
    use openidconnect::RequestTokenError;
    use openidconnect::core::CoreErrorResponseType;
    match err {
        // The IdP returned a structured OAuth error code. RFC 6749 §5.2 codes
        // all describe client-side problems that retry cannot fix.
        RequestTokenError::ServerResponse(resp) => !matches!(
            resp.error(),
            // Unknown extension code — conservatively treat as transient so a
            // custom code like "rate_limit_exceeded" does not log the user out.
            CoreErrorResponseType::Extension(_)
        ),
        // Network/transport, malformed response body, or some other oauth2
        // crate-level error. All retryable.
        RequestTokenError::Request(_)
        | RequestTokenError::Parse(_, _)
        | RequestTokenError::Other(_) => false,
    }
}

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
        let fresh =
            Arc::new(StatelessClient::with_provider(self.env, self.env.auth_provider()).await?);
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
        let auth = self.state.load();
        let auth = match auth.get() {
            Err(_) => {
                let client = self.ensure_fresh_client().await?;
                client
                    .login(|url, _cancel_token| async move {
                        if let Err(err) = open::that(&url) {
                            warn!("Failed to auto-open url: {err}");
                            eprintln!("Open this URL in a browser to complete the login:\n{url}");
                        }
                    })
                    .await?
            }
            Ok(auth) if auth.tokens.expires_in_less_than(REFRESH_AUTH_WHEN) => {
                let client = self.ensure_fresh_client().await?;
                match client
                    .refresh(&auth.tokens, Some(auth.profile.clone()))
                    .await
                {
                    Ok(auth) => auth,
                    Err(err) => {
                        // Either Permanent or Transient — falling through to a
                        // fresh interactive login is the right move in either
                        // case because the operator just asked us to log in.
                        warn!("Refresh during login failed, falling back to fresh login: {err:#}");
                        let client = self.ensure_fresh_client().await?;
                        client
                            .login(|url, _cancel_token| async move {
                                if let Err(e) = open::that(&url) {
                                    warn!("Failed to auto-open url: {e}");
                                    eprintln!(
                                        "Open this URL in a browser to complete the login:\n{url}"
                                    );
                                }
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

    /// `--no-browser` analog of [`AuthClient::login`]. Skips the auth-code
    /// localhost-redirect flow (which doesn't work over SSH because the
    /// remote machine's bound port can't be reached by a browser running
    /// on the operator's laptop) and uses the OAuth2 device authorization
    /// grant instead. Always performs a fresh login — if a refresh-eligible
    /// token exists the caller can use [`AuthClient::login`] without
    /// `--no-browser` to use it.
    pub async fn login_device_code<F, Fut>(&self, display: F) -> Result<()>
    where
        F: FnOnce(DeviceCodeInfo) -> Fut,
        Fut: Future<Output = ()>,
    {
        let client = self.ensure_fresh_client().await?;
        let auth = client.login_device_code(display).await?;
        self.state.set(Some(auth)).await?;
        Ok(())
    }

    pub async fn refresh(&self) -> Result<()> {
        let auth = self.state.load();
        let auth = auth.get()?;
        let client = self.ensure_fresh_client().await?;
        match client
            .refresh(&auth.tokens, Some(auth.profile.clone()))
            .await
        {
            Ok(new_auth) => {
                self.state.set(Some(new_auth)).await?;
                Ok(())
            }
            Err(RefreshError::Permanent(err)) => {
                // The IdP definitively rejected our refresh — only a fresh
                // interactive login can recover. Clear state and surface the
                // event prominently so a long-running session does not silently
                // wedge.
                error!(
                    "Datum login has expired or been revoked — the tunnel will \
                     stop accepting new connections. Run the CLI's login \
                     command (or restart the desktop app and sign in) to \
                     reconnect. Cause: {err:#}"
                );
                eprintln!(
                    "Datum login has expired or been revoked. \
                     Please log in again to restore the tunnel."
                );
                self.state.set(None).await?;
                Err(err).context("Refresh permanently rejected; re-login required")?
            }
            Err(RefreshError::Transient(err)) => {
                // Network/IdP blip. Keep tokens; the next retry (proactive
                // timer or 401-triggered) should recover.
                warn!(
                    "Transient token refresh failure — keeping existing \
                     credentials, will retry: {err:#}"
                );
                Err(err).context("Transient token refresh failure")?
            }
        }
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
    use data_encoding::BASE64;
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

    static LOGIN_SUCCESS_PNG: &[u8] = include_bytes!("../../../ui/assets/images/login-success.png");
    static ALLIANCE_NO1_REGULAR_TTF: &[u8] =
        include_bytes!("../../../ui/assets/fonts/AllianceNo1-Regular.ttf");
    static FAVICON_DARK_32: &[u8] =
        include_bytes!("../../../ui/assets/icons/favicon-dark-32x32.png");

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

    async fn oauth_redirect(
        state: State<AppState>,
        query: Query<OauthRedirectData>,
    ) -> axum::response::Html<String> {
        let data = query.0;
        state.sender.send(Ok(data)).await.ok();

        let hero_b64 = BASE64.encode(LOGIN_SUCCESS_PNG);
        let font_b64 = BASE64.encode(ALLIANCE_NO1_REGULAR_TTF);
        let favicon_dark_b64 = BASE64.encode(FAVICON_DARK_32);
        let html = OAUTH_REDIRECT_HTML
            .replace("{{HERO_B64}}", &hero_b64)
            .replace("{{FONT_B64}}", &font_b64)
            .replace("{{FAVICON_DARK_B64}}", &favicon_dark_b64);

        axum::response::Html(html)
    }

    static OAUTH_REDIRECT_HTML: &str = r##"<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Login successful - Datum</title>
    <link
      rel="icon"
      type="image/png"
      sizes="32x32"
      href="data:image/png;base64,{{FAVICON_DARK_B64}}"
    />
    <style>
      @font-face {
        font-family: "Alliance No1";
        src: url("data:font/ttf;base64,{{FONT_B64}}") format("truetype");
        font-weight: 400;
        font-style: normal;
        font-display: swap;
      }

      * {
        margin: 0;
        padding: 0;
        box-sizing: border-box;
      }

      body {
        min-height: 100svh;
        background-color: #0c1d31;
        color: #90969c;
        font-family:
          "Alliance No1",
          ui-sans-serif,
          system-ui,
          -apple-system,
          BlinkMacSystemFont,
          sans-serif;
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        padding: 2rem 1rem;
      }

      .header {
        display: flex;
        flex-direction: column;
        align-items: center;
        margin-bottom: 1rem;
      }

      .header .logo-link {
        display: inline-flex;
        line-height: 0;
        text-decoration: none;
      }

      .logo-icon {
        width: 81px;
        height: 66px;
        margin-bottom: 1rem;
      }

      .success-card {
        background-color: #18273a;
        border: 1px solid #384555;
        border-radius: 12px;
        padding: 48px;
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        gap: 1rem;
        width: 400px;
      }

      .success-title {
        font-size: 1.25rem;
        color: #F6F6F5;
        text-align: center;
      }

      .success-message {
        font-size: 0.9375rem;
        color: #90969c;
        text-align: center;
      }

      .footer {
        margin-top: 3rem;
        display: flex;
        align-items: center;
        gap: 3rem;
      }

      .footer a {
        color: #90969c;
        text-decoration: none;
        font-size: 12px;
        display: flex;
        align-items: center;
        gap: 0.5rem;
        transition: opacity 0.2s;
      }

      .footer a:hover {
        opacity: 0.8;
      }

      .footer svg.icon {
        width: 18px;
        height: 18px;
        fill: currentColor;
      }

      .hero-image {
        position: fixed;
        bottom: 0;
        left: 0;
        max-width: 50%;
        object-fit: contain;
        object-position: bottom left;
        pointer-events: none;
      }

      @media (max-width: 768px) {
        body {
          padding: 1.5rem 1rem;
          margin-top: 2rem;
          justify-content: flex-start;
        }

        .header {
          margin-bottom: 0.75rem;
        }

        .logo-icon {
          width: 64px;
          height: 52px;
          margin-bottom: 0.75rem;
        }

        .success-card {
          max-width: 100%;
          padding: 2rem 1.25rem;
        }

        .success-title {
          font-size: 1.1rem;
        }

        .success-message {
          font-size: 0.875rem;
        }

        .footer {
          margin-top: 2rem;
          gap: 2rem;
          flex-wrap: wrap;
          justify-content: center;
        }

        .footer a {
          font-size: 11px;
        }
      }

      @media (max-width: 480px) {
        body {
          padding: 1rem 0.75rem;
        }

        .success-title {
          font-size: 1rem;
        }

        .success-message {
          font-size: 0.8125rem;
        }

        .footer {
          flex-direction: column;
          gap: 1rem;
          margin-top: 1.5rem;
        }

        .footer a {
          font-size: 12px;
        }

        .hero-image {
          max-width: 80%; 
        }
      }
    </style>
  </head>
  <body>
    <img class="hero-image" src="data:image/png;base64,{{HERO_B64}}" alt="" />
    <header class="header">
      <a
        class="logo-link"
        href="https://www.datum.net"
        target="_blank"
        rel="noopener noreferrer"
      >
      <svg
        class="logo-icon"
        width="81"
        height="66"
        viewBox="0 0 81 66"
        fill="none"
        xmlns="http://www.w3.org/2000/svg"
      >
        <path
          d="M35.9866 0.000595093C35.4722 0.000595093 35.0528 0.418663 35.0528 0.934326C35.0528 1.44999 35.0528 10.058 35.0528 10.058C35.0528 10.5724 35.4709 10.9686 35.9866 10.9686L40.4172 10.9918C42.2478 10.9918 43.9913 11.7066 45.3273 13.003C46.6712 14.3086 47.4216 16.0324 47.4427 17.8589C47.4638 19.7304 46.7503 21.4923 45.4354 22.8217C44.1205 24.1511 42.3665 24.8831 40.4964 24.8831H40.4172C38.5907 24.862 36.8669 24.1115 35.5613 22.7677C34.2649 21.433 33.5295 19.6882 33.5295 17.8576V13.3749C33.5295 12.8606 33.132 12.4919 32.6163 12.4919H23.4953C22.9809 12.4919 22.5615 12.8592 22.5615 13.3749V22.4986C22.5615 23.013 22.9796 23.4599 23.4953 23.4599H30.6948C31.6628 23.4599 32.2866 23.4455 32.801 23.5154C33.4841 23.6078 33.9418 23.8029 34.2873 24.1472C34.6328 24.4914 34.8267 24.9503 34.919 25.6335C34.9889 26.1491 35.0528 26.7716 35.0528 27.741V34.9392C35.0528 35.4535 35.4202 35.9512 35.9358 35.9512H40.4977C42.5458 35.9512 44.5571 35.5287 46.476 34.8508C53.628 32.3226 58.4339 25.5253 58.4339 17.9368C58.4339 10.3482 53.628 3.5509 46.476 1.02269C44.5571 0.34481 42.5458 0.000595093 40.4977 0.000595093H35.9879H35.9866Z"
          fill="#E6F59E"
        />
        <path
          d="M67.2503 55.1795H67.2173L66.4593 64.8685H64.3823L65.1404 52.8848H68.2888L72.6432 62.1486L77.0635 52.8848H80.146L80.904 64.8685H78.8271L78.069 55.1795H78.036L73.5827 64.8685H71.7201L67.2503 55.1795Z"
          fill="#F6F6F5"
        />
        <path
          d="M60.6992 59.7256C60.6992 60.6926 60.5508 61.5223 60.2541 62.2146C59.9574 62.896 59.5343 63.4564 58.9848 63.896C58.4354 64.3355 57.765 64.6597 56.9738 64.8685C56.1936 65.0663 55.3144 65.1652 54.3364 65.1652C53.3584 65.1652 52.4737 65.0663 51.6825 64.8685C50.9023 64.6597 50.2374 64.3355 49.688 63.896C49.1385 63.4564 48.7154 62.896 48.4187 62.2146C48.122 61.5223 47.9736 60.6926 47.9736 59.7256V52.8848H50.3143V59.5607C50.3143 60.0552 50.3693 60.5223 50.4792 60.9619C50.5891 61.3904 50.7924 61.7696 51.0891 62.0992C51.3858 62.4289 51.7924 62.6872 52.3089 62.874C52.8364 63.0608 53.5122 63.1542 54.3364 63.1542C55.1606 63.1542 55.8309 63.0608 56.3474 62.874C56.8749 62.6872 57.287 62.4289 57.5837 62.0992C57.8804 61.7696 58.0837 61.3904 58.1936 60.9619C58.3035 60.5223 58.3585 60.0552 58.3585 59.5607V52.8848H60.6992V59.7256Z"
          fill="#F6F6F5"
        />
        <path
          d="M36.6463 54.8958H31.2891V52.8848H44.3443V54.8958H38.987V64.8685H36.6463V54.8958Z"
          fill="#F6F6F5"
        />
        <path
          d="M21.8299 52.8848H24.5003L30.3191 64.8685H27.8465L26.5938 62.1157H19.687L18.4178 64.8685H15.9287L21.8299 52.8848ZM25.6707 60.1047L23.1651 54.7639L20.6596 60.1047H25.6707Z"
          fill="#F6F6F5"
        />
        <path
          d="M0.000976562 52.8848H5.77033C6.7154 52.8848 7.60003 52.9947 8.42422 53.2144C9.25941 53.4232 9.98469 53.7639 10.6001 54.2364C11.2155 54.698 11.699 55.3079 12.0507 56.0661C12.4133 56.8134 12.5946 57.7255 12.5946 58.8025C12.5946 59.8245 12.4298 60.7146 12.1001 61.4728C11.7704 62.2311 11.3089 62.863 10.7155 63.3685C10.1221 63.874 9.41325 64.2531 8.58906 64.5059C7.76487 64.7476 6.85826 64.8685 5.86923 64.8685H0.000976562V52.8848ZM4.71536 62.8575C5.71538 62.8575 6.56155 62.7806 7.25387 62.6267C7.95718 62.4729 8.52862 62.2311 8.96819 61.9014C9.41875 61.5718 9.74293 61.1542 9.94073 60.6487C10.1495 60.1432 10.2539 59.5387 10.2539 58.8354C10.2539 58.0992 10.1495 57.4838 9.94073 56.9892C9.73194 56.4837 9.40226 56.0771 8.95171 55.7694C8.51214 55.4617 7.9407 55.242 7.23739 55.1101C6.53408 54.9672 5.6934 54.8958 4.71536 54.8958H2.34168V62.8575H4.71536Z"
          fill="#F6F6F5"
        />
      </svg>
      </a>
    </header>

    <main class="success-card">
<svg width="34" height="34" viewBox="0 0 34 34" fill="none" xmlns="http://www.w3.org/2000/svg">
<rect width="34" height="34" rx="4" fill="#4D6356"/>
<path d="M23.6666 12.4414L14.5 21.6081L10.3333 17.4414" stroke="#E6F59F" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round"/>
</svg>


      <h1 class="success-title">Login confirmed!</h1>
      <p class="success-message">Feel free to close this window</p>
    </main>

    <footer class="footer">
      <a href="https://www.datum.net" target="_blank" rel="noopener">
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          class="lucide lucide-app-window-icon lucide-app-window"
          style="width: 18px; height: 18px"
        >
          <rect x="2" y="4" width="20" height="16" rx="2" />
          <path d="M10 4v4" />
          <path d="M2 8h20" />
          <path d="M6 4v4" />
        </svg>
        datum.net
      </a>
      <a href="https://link.datum.net/discord" target="_blank" rel="noopener">
        <svg class="icon" viewBox="0 0 24 24" fill="currentColor">
          <path
            d="M20.317 4.37a19.791 19.791 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028 14.09 14.09 0 0 0 1.226-1.994.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.01c.12.098.246.198.373.292a.077.077 0 0 1-.006.127 12.299 12.299 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03zM8.02 15.33c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.956 2.418-2.157 2.418zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.418-2.157 2.418z"
          />
        </svg>
        Discord
      </a>
      <a href="https://github.com/datum-cloud" target="_blank" rel="noopener">
        <svg class="icon" viewBox="0 0 24 24" fill="currentColor">
          <path
            d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"
          />
        </svg>
        GitHub
      </a>
    </footer>
  </body>
</html>"##;
}
