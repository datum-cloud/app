use crate::state::{Connector, Project, User};
use anyhow::{Context, Result};
use reqwest::Method;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub(crate) struct DatumCloudClient {
    oauth_token: Option<String>,
    client: reqwest::Client,
}

impl DatumCloudClient {
    const URL_BASE: &str = "https://api.datum.net";

    pub(crate) fn new(oauth_token: Option<String>) -> Self {
        let client = reqwest::Client::new();
        Self {
            oauth_token,
            client,
        }
    }

    pub(crate) async fn get_user_details(&self) -> Result<User> {
        todo!();
    }

    pub(crate) async fn get_projects(&self) -> Result<Vec<Project>> {
        todo!();
    }

    /// register a project connector
    pub(crate) async fn post_project_connector(
        &self,
        _project_id: &str,
        _namespace: &str,
        _connector: Connector,
    ) -> Result<Connector> {
        todo!();
    }

    pub(crate) async fn post_project_connector_proxy(
        &self,
        _project_id: &str,
        _connector: Connector,
    ) -> Result<Connector> {
        todo!();
    }

    async fn make_oauth_request<Res: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
    ) -> Result<Res> {
        let req = self
            .client
            .request(method, format!("{}{}", Self::URL_BASE, path))
            // .bearer_auth(self.oauth_token.map(|v| v))
            .build()?;

        self.client
            .execute(req)
            .await?
            .json()
            .await
            .context("parsing API response")
    }
}
