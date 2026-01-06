use std::net::SocketAddrV4;

use anyhow::anyhow;
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreResponseType, CoreUserInfoClaims,
};
use openidconnect::reqwest;
use openidconnect::{
    AccessTokenHash, AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, RedirectUrl, Scope, TokenResponse,
};
use std::net::Ipv4Addr;
use tokio::io::AsyncBufReadExt;
use url::Url;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // let issuer_url = "https://keycloak.rdlnet.de/auth/realms/arso";
    // let client_id = "testapp";
    // let client_secret = "56rImCSKdCWPoI8d7LmrEIV0DcWK3B2D";

    let issuer_url = "https://auth.staging.env.datum.net";
    let client_id = "351641555150375458";
    // let client_secret = "56rImCSKdCWPoI8d7LmrEIV0DcWK3B2D";
    let port = 9876;
    let redirect_url = format!("http://localhost:{port}/oauth/redirect");

    tokio::spawn(async move {
        if let Err(err) =
            self::server::run(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into()).await
        {
            tracing::error!("Local server failed: {err:?}");
        }
    });

    let http_client = reqwest::ClientBuilder::new()
        // Following redirects opens the client up to SSRF vulnerabilities.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Client should build");

    // Use OpenID Connect Discovery to fetch the provider metadata.
    let provider_metadata =
        CoreProviderMetadata::discover_async(IssuerUrl::new(issuer_url.to_string())?, &http_client)
            .await?;

    // Create an OpenID Connect client by specifying the client ID, client secret, authorization URL
    // and token URL.
    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.to_string()),
        None, // Some(ClientSecret::new(client_secret.to_string())),
    )
    // Set the URL the user will be redirected to after the authorization process.
    .set_redirect_uri(RedirectUrl::new(redirect_url.to_string())?);

    // Generate a PKCE challenge.
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // Generate the full authorization URL.
    let (auth_url, _csrf_token, nonce) = client
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

    // This is the URL you should redirect the user to, in order to trigger the authorization
    // process.
    println!("Browse to: {}", auth_url);

    // Once the user has been redirected to the redirect URL, you'll have access to the
    // authorization code. For security reasons, your code should verify that the `state`
    // parameter returned by the server matches `csrf_state`.
    println!("enter auth code");
    // Read one line
    let mut line = String::new();
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    reader.read_line(&mut line).await?;
    let authorization_code = line.trim();

    // Now you can exchange it for an access token and ID token.
    let token_response = client
        .exchange_code(AuthorizationCode::new(authorization_code.to_string()))?
        // Set the PKCE code verifier.
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http_client)
        .await?;

    if let Some(token) = token_response.refresh_token() {
        let res = client
            .exchange_refresh_token(token)?
            .request_async(&http_client)
            .await?;
    }

    // Extract the ID token claims after verifying its authenticity and nonce.
    let id_token = token_response
        .id_token()
        .ok_or_else(|| anyhow!("Server did not return an ID token"))?;
    let id_token_verifier = client.id_token_verifier();
    let claims = id_token.claims(&id_token_verifier, &nonce)?;

    // Verify the access token hash to ensure that the access token hasn't been substituted for
    // another user's.
    if let Some(expected_access_token_hash) = claims.access_token_hash() {
        let actual_access_token_hash = AccessTokenHash::from_token(
            token_response.access_token(),
            id_token.signing_alg()?,
            id_token.signing_key(&id_token_verifier)?,
        )?;
        if actual_access_token_hash != *expected_access_token_hash {
            return Err(anyhow!("Invalid access token"));
        }
    }

    // The authenticated user's identity is now available. See the IdTokenClaims struct for a
    // complete listing of the available claims.
    println!(
        "User {} with e-mail address {} has authenticated successfully",
        claims.subject().as_str(),
        claims
            .email()
            .map(|email| email.as_str())
            .unwrap_or("<not provided>"),
    );

    // If available, we can use the user info endpoint to request additional information.

    // The user_info request uses the AccessToken returned in the token response. To parse custom
    // claims, use UserInfoClaims directly (with the desired type parameters) rather than using the
    // CoreUserInfoClaims type alias.
    let userinfo: CoreUserInfoClaims = client
        .user_info(token_response.access_token().to_owned(), None)?
        .request_async(&http_client)
        .await
        .map_err(|err| anyhow!("Failed requesting user info: {}", err))?;

    println!("userinfo: {userinfo:?}");
    // See the OAuth2TokenResponse trait for a listing of other available fields such as
    // access_token() and refresh_token().
    Ok(())
}

mod server {
    use axum::{
        Router,
        body::Bytes,
        http::{Method, Uri},
        routing::any,
    };
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    pub async fn run(addr: SocketAddr) -> anyhow::Result<()> {
        // Route all paths and all methods to the same handler
        let app = Router::new().route("/*path", any(log_all_requests));

        println!("Listening on http://{}", addr);

        let listener = TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app).await?;
        Ok(())
    }

    async fn log_all_requests(method: Method, uri: Uri, body: Bytes) -> String {
        let path = uri.path().to_string();
        let query = uri.query().unwrap_or("").to_string();

        // Only print body for POST / PUT, as requested
        let body_str = if method == Method::POST || method == Method::PUT {
            // Interpret as UTF-8 text (lossy to avoid panics on invalid UTF-8)
            let text = String::from_utf8_lossy(&body);
            format!(r#"body = "{}""#, text)
        } else {
            "body = <not printed for this method>".to_string()
        };

        println!(
            "[REQUEST] method = {} | path = {} | query = \"{}\" | {}",
            method, path, query, body_str
        );

        // Simple response so you can see something in curl/browser
        format!(
            "You requested: {}\nPath: {}\nQuery: {}\n",
            method, path, query
        )
    }
}
