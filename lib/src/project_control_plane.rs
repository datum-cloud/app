use std::sync::Arc;

use arc_swap::ArcSwap;
use kube::{Client, Config};
use n0_error::{Result, StdResultExt};
use n0_future::task::AbortOnDropHandle;
use secrecy::SecretString;
use tracing::warn;

use crate::datum_cloud::{DatumCloudClient, LoginState};

#[derive(derive_more::Debug, Clone)]
pub struct ProjectControlPlaneClient {
    project_id: String,
    server_url: String,
    access_token: Arc<ArcSwap<String>>,
    #[debug("kube::Client")]
    client: Arc<ArcSwap<Client>>,
    datum: DatumCloudClient,
    _auth_task: Option<Arc<AbortOnDropHandle<()>>>,
}

impl ProjectControlPlaneClient {
    pub fn new(
        project_id: String,
        server_url: String,
        access_token: String,
        datum: DatumCloudClient,
    ) -> Result<Self> {
        let client = Self::build_kube_client(&server_url, &access_token)?;
        let mut this = Self {
            project_id,
            server_url,
            access_token: Arc::new(ArcSwap::from_pointee(access_token)),
            client: Arc::new(ArcSwap::from_pointee(client)),
            datum,
            _auth_task: None,
        };
        this.start_auth_watch();
        Ok(this)
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn access_token(&self) -> String {
        self.access_token.load_full().as_ref().clone()
    }

    pub fn client(&self) -> Client {
        self.client.load_full().as_ref().clone()
    }

    pub async fn client_refreshed(&self) -> Result<Client> {
        let auth_state = self.datum.auth().load_refreshed().await?;
        let auth = auth_state.get()?;
        let access_token = auth.tokens.access_token.secret();
        self.rebuild_if_changed(access_token)?;
        Ok(self.client())
    }

    fn build_kube_client(server_url: &str, access_token: &str) -> Result<Client> {
        let uri = server_url
            .parse()
            .std_context("Invalid project control plane URL")?;
        let mut config = Config::new(uri);
        config.auth_info.token =
            Some(SecretString::new(access_token.to_string().into_boxed_str()));
        Client::try_from(config).std_context("Failed to create project control plane client")
    }

    fn rebuild_if_changed(&self, access_token: &str) -> Result<()> {
        let current = self.access_token.load_full();
        if current.as_ref().as_str() == access_token {
            return Ok(());
        }

        let client = Self::build_kube_client(&self.server_url, access_token)?;
        self.client.store(Arc::new(client));
        self.access_token
            .store(Arc::new(access_token.to_string()));
        Ok(())
    }

    async fn refresh_client_from_update(&self) -> Result<()> {
        let auth_state = self.datum.auth().load();
        let Ok(auth) = auth_state.get() else {
            return Ok(());
        };
        self.rebuild_if_changed(auth.tokens.access_token.secret())
    }

    fn start_auth_watch(&mut self) {
        if self._auth_task.is_some() {
            return;
        }
        let client = self.clone();
        let mut login_rx = self.datum.auth().login_state_watch();
        let mut auth_update_rx = self.datum.auth_update_watch();
        let task = tokio::spawn(async move {
            if *login_rx.borrow() != LoginState::Missing {
                if let Err(err) = client.refresh_client_from_update().await {
                    warn!("failed to refresh project control plane client: {err:#}");
                }
            }
            loop {
                tokio::select! {
                    res = login_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                    res = auth_update_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                }
                if *login_rx.borrow() != LoginState::Missing {
                    if let Err(err) = client.refresh_client_from_update().await {
                        warn!("failed to refresh project control plane client: {err:#}");
                    }
                }
            }
        });
        self._auth_task = Some(Arc::new(AbortOnDropHandle::new(task)));
    }
}
