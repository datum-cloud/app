//! Datum Cloud auth and API flow example
//!
//! Persists access tokens between different runs in /tmp/datum-cloud-example-login/oauth.yml.
//! On the first run, a browser window will open where you login to Datum Cloud.
//! On subsequent runs, the stored access token will be used, unless it expired.

use std::env::temp_dir;

use lib::{
    Repo,
    datum_cloud::{ApiEnv, DatumCloudClient},
};

#[tokio::main]
async fn main() -> n0_error::Result<()> {
    tracing_subscriber::fmt::init();

    // Open a repo in a temp dir.
    let repo_path = temp_dir().join("datum-cloud-example-login");
    println!("repo location: {}", repo_path.display());
    let repo = Repo::open_or_create(repo_path).await?;

    // Init client, and login if needed.
    // If a login is needed, a browser will open, and a local HTTP server awaits the OIDC redirect.
    // The access tokens will be persisted to the repo.
    let client = DatumCloudClient::with_repo(ApiEnv::Staging, repo).await?;
    client.auth().login().await?;
    let auth = client.auth_state();
    let auth = auth.get()?;
    println!("user {} logged in!", auth.profile.email);
    println!("access token expires at {}", auth.tokens.expires_at());
    println!("profile: {:?}", auth.profile);

    // Fetch orgs and projects.
    let orgs = client.orgs_and_projects().await?;
    for org in orgs {
        println!("org: {:?}", org.org);
        for project in org.projects {
            println!("    project: {project:?}");
        }
    }
    Ok(())
}
