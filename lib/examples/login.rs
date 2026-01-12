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
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Open a repo in a temp dir.
    let repo_path = temp_dir().join("datum-cloud-example-login");
    println!("repo location: {}", repo_path.display());
    let repo = Repo::open_or_create(repo_path).await?;

    // Init client, and login if needed.
    // If a login is needed, a browser will open, and a local HTTP server awaits the OIDC redirect.
    // The access tokens will be persisted to the repo.
    let client = DatumCloudClient::with_repo(ApiEnv::Staging, repo).await?;
    println!("user {} logged in!", client.auth_state().profile.email);
    println!(
        "access token expires at {}",
        client.auth_state().tokens.expires_at()
    );
    println!("profile: {:?}", client.auth_state().profile);

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
