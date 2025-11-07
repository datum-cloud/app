use std::{path::PathBuf, pin::Pin};

use anyhow::Context;
use iroh_proxy_utils::{
    error::AuthError,
    http_connect::{AuthHandler, Request},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {}

impl AuthHandler for Auth {
    fn authorize<'a>(
        &'a self,
        _req: &'a Request,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }
}

impl Default for Auth {
    fn default() -> Self {
        Self {}
    }
}

impl Auth {
    pub async fn from_file(path: PathBuf) -> anyhow::Result<Self> {
        let config = tokio::fs::read_to_string(path)
            .await
            .context("reading auth file")?;
        let config = serde_yml::from_str(&config).context("parsing auth file")?;
        Ok(config)
    }

    pub async fn write(&self, path: PathBuf) -> anyhow::Result<()> {
        let data = serde_yml::to_string(self)?;
        tokio::fs::write(path, data).await?;
        Ok(())
    }
}
