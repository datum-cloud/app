use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    me: Option<User>,
    org: Option<Organization>,
    project: Option<Project>,
    connectors: Vec<Connector>,
    proxies: Vec<Proxy>,
}

impl State {
    pub(crate) async fn from_file(path: PathBuf) -> anyhow::Result<Self> {
        let data = tokio::fs::read(path).await?;
        let state: State = serde_yml::from_slice(&data)?;
        Ok(state)
    }

    pub(crate) async fn write_to_file(&self, path: PathBuf) -> anyhow::Result<()> {
        let data = serde_yml::to_string(&path)?;
        tokio::fs::write(&path, &data).await?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct User {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Organization {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Project {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Connector {}

#[derive(Debug, Serialize, Deserialize)]
pub struct Proxy {}
