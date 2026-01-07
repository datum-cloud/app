use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreUserInfoClaims,
};
use openidconnect::{
    AccessToken, AdditionalClaims, GenderClaim, IdTokenClaims, NonceVerifier, RefreshToken,
    StandardClaims, reqwest,
};
use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use self::{redirect_server::RedirectServer, types::OidcTokenResponse};
use super::ApiEnv;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(60);

pub struct AuthProvider {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}

pub struct AuthClient {
    oidc: types::OidcClient,
    http: reqwest::Client,
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
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

impl UserProfile {
    fn from_standard_claims<GC>(claims: &StandardClaims<GC>) -> Result<Self>
    where
        GC: GenderClaim,
    {
        Ok(Self {
            user_id: claims.subject().to_string(),
            email: claims
                .email()
                .map(|x| x.to_string())
                .context("missing email address")?,
            first_name: claims
                .given_name()
                .map(|x| x.iter())
                .into_iter()
                .flatten()
                .next()
                .map(|(_lang, name)| name.to_string()),
            last_name: claims
                .family_name()
                .map(|x| x.iter())
                .into_iter()
                .flatten()
                .next()
                .map(|(_lang, name)| name.to_string()),
        })
    }

    // TODO: `IdTokenClaims` contains `StandardClaims` but does not expose it directly.
    // File PR to openidconnect and then remove this function (which is a literal copy-paste)
    // of `from_standard_claims`).
    fn from_id_token_claims<AC, GC>(claims: &IdTokenClaims<AC, GC>) -> Result<Self>
    where
        AC: AdditionalClaims,
        GC: GenderClaim,
    {
        Ok(Self {
            user_id: claims.subject().to_string(),
            email: claims
                .email()
                .map(|x| x.to_string())
                .context("missing email address")?,
            first_name: claims
                .given_name()
                .map(|x| x.iter())
                .into_iter()
                .flatten()
                .next()
                .map(|(_lang, name)| name.to_string()),
            last_name: claims
                .family_name()
                .map(|x| x.iter())
                .into_iter()
                .flatten()
                .next()
                .map(|(_lang, name)| name.to_string()),
        })
    }
}

impl AuthClient {
    pub async fn new(env: ApiEnv) -> Result<Self> {
        Self::with_provider(env.auth_provider()).await
    }

    pub async fn with_provider(provider: AuthProvider) -> Result<Self> {
        let http = reqwest::ClientBuilder::new()
            // Following redirects opens the client up to SSRF vulnerabilities.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Client should build");

        // Use OpenID Connect Discovery to fetch the provider metadata.
        let provider_metadata =
            CoreProviderMetadata::discover_async(IssuerUrl::new(provider.issuer_url)?, &http)
                .await?;

        // Create an OpenID Connect client by specifying the client ID, client secret, authorization URL
        // and token URL.
        let oidc = CoreClient::from_provider_metadata(
            provider_metadata,
            ClientId::new(provider.client_id),
            provider.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(RedirectServer::url());

        Ok(Self { oidc, http })
    }

    pub async fn login(&self) -> Result<AuthState> {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        // Generate the full authorization URL.
        let (auth_url, csrf_token, nonce) = self
            .oidc
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            // Set the desired scopes.
            .add_scope(Scope::new("openid".to_string()))
            .add_scope(Scope::new("profile".to_string()))
            .add_scope(Scope::new("email".to_string()))
            // Set the PKCE code challenge.
            .set_pkce_challenge(pkce_challenge)
            .url();
        debug!(auth_uri=%self.oidc.auth_uri(), "attempting login");

        // Bind a localhost HTTP server to receive the redirect.
        let mut redirect_server = RedirectServer::bind(csrf_token.clone()).await?;

        // Open the auth URL in the platform's default browser.
        if let Err(err) = open::that(auth_url.to_string()) {
            tracing::warn!("Failed to auto-open url: {err}");
            println!("Please open this URL in a browser:\n{auth_url}")
        }

        let authorization_code = redirect_server.recv_with_timeout(LOGIN_TIMEOUT).await?;
        debug!("received redirect with authorization code");
        // Now you can exchange it for an access token and ID token.
        let tokens = self
            .oidc
            .exchange_code(AuthorizationCode::new(authorization_code))?
            .set_pkce_verifier(pkce_verifier)
            .request_async(&self.http)
            .await
            .inspect_err(|e| error!("Failed to exchange auth code to access token: {e:#}"))?;

        let state = self.parse_token_response(tokens, &nonce)?;
        info!(email=%state.profile.email, expires_at=%state.tokens.expires_at(), "login succesfull");
        Ok(state)
    }

    pub async fn fetch_profile(&self, tokens: &AuthTokens) -> Result<UserProfile> {
        let userinfo: CoreUserInfoClaims = self
            .oidc
            .user_info(tokens.access_token.clone(), None)?
            .request_async(&self.http)
            .await
            .map_err(|err| anyhow!("Failed requesting user info: {}", err))?;
        let profile = UserProfile::from_standard_claims(userinfo.standard_claims())?;
        Ok(profile)
    }

    pub async fn refresh(&self, tokens: &AuthTokens) -> Result<AuthState> {
        let refresh_token = tokens.refresh_token.as_ref().context("No refresh token")?;
        let tokens = self
            .oidc
            .exchange_refresh_token(refresh_token)?
            .request_async(&self.http)
            .await?;
        let state = self.parse_token_response(tokens, refresh_nonce_verifier)?;
        Ok(state)
    }

    fn parse_token_response(
        &self,
        tokens: OidcTokenResponse,
        nonce_verifier: impl NonceVerifier,
    ) -> Result<AuthState> {
        // Extract the ID token claims after verifying its authenticity and nonce.
        let id_token = tokens
            .id_token()
            .ok_or_else(|| anyhow!("Server did not return an ID token"))?;
        let id_token_verifier = self
            .oidc
            .id_token_verifier()
            // Datum auth backend includes multiple audiences in the id tokens
            .set_other_audience_verifier_fn(|_audience| true);

        let claims = id_token
            .claims(&id_token_verifier, nonce_verifier)
            .inspect_err(|e| error!("Failed to verify claims: {e:#}"))?;

        // Verify the access token hash to ensure that the access token hasn't been substituted for
        // another user's.
        if let Some(expected_access_token_hash) = claims.access_token_hash() {
            let actual_access_token_hash = AccessTokenHash::from_token(
                tokens.access_token(),
                id_token.signing_alg()?,
                id_token.signing_key(&id_token_verifier)?,
            )
            .context("failed to create access token hash from token")?;
            if actual_access_token_hash != *expected_access_token_hash {
                return Err(anyhow!("Invalid access token"));
            }
        }

        let profile = UserProfile::from_id_token_claims(claims)?;
        let tokens = AuthTokens {
            issued_at: claims.issue_time(),
            access_token: tokens.access_token().clone(),
            refresh_token: tokens.refresh_token().cloned(),
            expires_in: tokens.expires_in().context("Missing expires_in claim")?,
        };

        Ok(AuthState { tokens, profile })
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
    use anyhow::Context;
    use axum::{
        Router,
        extract::{Query, State},
        routing::get,
    };
    use openidconnect::{CsrfToken, RedirectUrl};
    use serde::Deserialize;
    use std::{
        net::{Ipv4Addr, SocketAddr},
        time::Duration,
    };
    use tokio::{net::TcpListener, sync::mpsc};
    use tokio_util::sync::CancellationToken;

    pub const REDIRECT_SERVER_PORT: u16 = 7076;

    #[derive(Deserialize, Debug)]
    struct OauthRedirectData {
        pub code: String,
        pub state: String,
    }

    pub struct RedirectServer {
        rx: mpsc::Receiver<anyhow::Result<OauthRedirectData>>,
        cancel_token: CancellationToken,
        csrf_token: CsrfToken,
    }

    impl RedirectServer {
        pub async fn bind(csrf_token: CsrfToken) -> anyhow::Result<Self> {
            let bind_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), REDIRECT_SERVER_PORT);
            let cancel_token = CancellationToken::new();
            let (tx, rx) = mpsc::channel(1);
            let state = AppState { sender: tx.clone() };
            // Route all paths and all methods to the same handler
            let app = Router::new()
                .route("/oauth/redirect", get(oauth_redirect))
                .with_state(state);
            let listener = TcpListener::bind(bind_addr)
                .await
                .with_context(|| format!("Failed to bind TCP server on {bind_addr}"))?;

            tokio::spawn({
                let cancel_token = cancel_token.clone();
                async move {
                    if let Err(err) = axum::serve(listener, app)
                        .with_graceful_shutdown(cancel_token.cancelled_owned())
                        .await
                    {
                        tx.send(Err(err.into())).await.ok();
                    }
                }
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

        pub async fn recv_with_timeout(&mut self, timeout: Duration) -> anyhow::Result<String> {
            let res = tokio::time::timeout(timeout, self.recv()).await;
            self.cancel_token.cancel();
            res?
        }

        pub async fn recv(&mut self) -> anyhow::Result<String> {
            let code = loop {
                let reply = self
                    .rx
                    .recv()
                    .await
                    .context("web server closed")?
                    .context("web server failed")?;
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
        sender: mpsc::Sender<anyhow::Result<OauthRedirectData>>,
    }

    async fn oauth_redirect(state: State<AppState>, query: Query<OauthRedirectData>) -> String {
        let data = query.0;
        state.sender.send(Ok(data)).await.ok();
        "You are now logged in and can close this window.".to_string()
    }
}
