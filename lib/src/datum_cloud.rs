use crate::state::{Connector, Project, User};
use anyhow::Result;

#[derive(Debug)]
pub(crate) struct DatumCloudClient {
    oauth_token: Option<String>,
}

impl DatumCloudClient {
    pub(crate) fn new(oauth_token: Option<String>) -> Self {
        Self { oauth_token }
    }

    pub(crate) async fn get_user_details(&self) -> Result<User> {
        todo!();
    }

    pub(crate) async fn get_projects(&self) -> Result<Vec<Project>> {
        todo!();
    }

    pub(crate) async fn post_project_connector(
        &self,
        _project_id: &str,
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
}
