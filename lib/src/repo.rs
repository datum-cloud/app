use std::path::PathBuf;

use iroh::SecretKey;
use log::{info, warn};
use n0_error::{Result, StackResultExt, StdResultExt};

use crate::{
    StateWrapper,
    auth::Auth,
    config::{Config, GatewayConfig},
    datum_cloud::AuthState,
    state::State,
};

// Repo builds up a series of file path conventions from a root directory path.
#[derive(Debug, Clone)]
pub struct Repo(PathBuf);

impl Repo {
    /// Create a Repo from a path without opening/creating (for sync use cases like update install).
    pub fn from_path(path: PathBuf) -> Self {
        Self(path)
    }
    const CONNECT_KEY_FILE: &str = "connect_key";
    const LISTEN_KEY_FILE: &str = "listen_key";
    const GATEWAY_KEY_FILE: &str = "gateway_key";
    const CONFIG_FILE: &str = "config.yml";
    const OAUTH_FILE: &str = "oauth.yml";
    const AUTH_FILE: &str = "auth.yml";
    const STATE_FILE: &str = "state.yml";
    const PROJECTS_DIR: &str = "projects";

    pub fn default_location() -> PathBuf {
        match std::env::var("DATUM_CONNECT_REPO") {
            Ok(path) => path.into(),
            Err(_) => dirs_next::data_local_dir()
                .expect("Failed to get local data dir")
                .join("Datum"),
        }
    }

    /// Opens or creates a repo at the given base directory.
    pub async fn open_or_create(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        tokio::fs::create_dir_all(&base_dir).await?;
        info!("opening repo at {}", base_dir.display());

        let this = Self(base_dir);

        Ok(this)
    }

    pub async fn config(&self) -> Result<Config> {
        let config_file_path = self.0.join(Self::CONFIG_FILE);
        if !config_file_path.exists() {
            warn!("secret key does not exist. creating new key");
            let cfg = Config::default();
            cfg.write(config_file_path).await?;
            return Ok(cfg);
        };

        Config::from_file(config_file_path).await
    }

    pub async fn gateway_config(&self) -> Result<GatewayConfig> {
        let config_file_path = self.0.join(Self::CONFIG_FILE);
        if !config_file_path.exists() {
            warn!("gateway config does not exist. creating new config");
            let cfg = GatewayConfig::default();
            cfg.write(config_file_path).await?;
            return Ok(cfg);
        };

        GatewayConfig::from_file(config_file_path).await
    }

    pub async fn load_state(&self) -> Result<StateWrapper> {
        let state_file_path = self.0.join(Self::STATE_FILE);
        let state = if !state_file_path.exists() {
            let state = State::default();
            state.write_to_file(state_file_path).await?;
            state
        } else {
            State::from_file(state_file_path).await?
        };
        Ok(StateWrapper::new(state))
    }

    pub async fn write_state(&self, state: &State) -> Result<()> {
        state.write_to_file(self.0.join(Self::STATE_FILE)).await
    }

    pub async fn write_selected_context(
        &self,
        selected: Option<&crate::SelectedContext>,
    ) -> Result<()> {
        let path = self.0.join(Self::CONFIG_FILE);
        let mut config = if path.exists() {
            let data = tokio::fs::read_to_string(&path)
                .await
                .context("reading config file")?;
            serde_yml::from_str(&data).std_context("parsing config file")?
        } else {
            crate::config::Config::default()
        };
        config.selected_context = selected.cloned();
        config.write(path).await
    }

    pub async fn read_selected_context(&self) -> Result<Option<crate::SelectedContext>> {
        let path = self.0.join(Self::CONFIG_FILE);
        if path.exists() {
            let data = tokio::fs::read_to_string(path)
                .await
                .context("reading config file")?;
            let config: crate::config::Config =
                serde_yml::from_str(&data).std_context("parsing config file")?;
            return Ok(config.selected_context);
        }
        Ok(None)
    }

    pub async fn auth(&self) -> Result<Auth> {
        let auth_file_path = self.0.join(Self::AUTH_FILE);
        if !auth_file_path.exists() {
            warn!("auth file does not exist. creating new auth");
            let auth = Auth::default();
            auth.write(auth_file_path).await?;
            return Ok(auth);
        };

        Auth::from_file(auth_file_path).await
    }

    pub async fn listen_key(&self) -> Result<SecretKey> {
        let key_file_path = self.0.join(Self::LISTEN_KEY_FILE);
        self.secret_key(key_file_path).await
    }

    /// Project-scoped listen key. Each project gets its own iroh identity so
    /// Connectors registered in different projects don't collide on the iroh
    /// DNS record (the controller assigns ownership to one and leaves the
    /// others with `IrohDNSPublished=False; DeferredToOwner`, which manifests
    /// as a tunnel that reports ready but silently drops data).
    ///
    /// On first access for any project, if the legacy flat `listen_key` exists
    /// it is moved into this project's directory so the user keeps continuity
    /// with whatever Connector that key was registered as. Subsequent projects
    /// (no legacy file left) get freshly generated keys.
    pub async fn listen_key_for_project(&self, project_id: &str) -> Result<SecretKey> {
        let project_dir = self.0.join(Self::PROJECTS_DIR).join(project_id);
        let key_file_path = project_dir.join(Self::LISTEN_KEY_FILE);
        if !key_file_path.exists() {
            let legacy = self.0.join(Self::LISTEN_KEY_FILE);
            if legacy.exists() {
                tokio::fs::create_dir_all(&project_dir).await?;
                info!(
                    "migrating legacy listen_key {} -> {} for project {project_id}",
                    legacy.display(),
                    key_file_path.display(),
                );
                tokio::fs::rename(&legacy, &key_file_path).await?;
            }
        }
        self.secret_key(key_file_path).await
    }

    pub async fn gateway_key(&self) -> Result<SecretKey> {
        let key_file_path = self.0.join(Self::GATEWAY_KEY_FILE);
        self.secret_key(key_file_path).await
    }

    pub async fn connect_key(&self) -> Result<SecretKey> {
        let key_file_path = self.0.join(Self::CONNECT_KEY_FILE);
        self.secret_key(key_file_path).await
    }

    async fn secret_key(&self, key_file_path: PathBuf) -> Result<SecretKey> {
        if !key_file_path.exists() {
            warn!("secret key does not exist. creating new key");
            if let Some(parent) = key_file_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            return self.create_key(&key_file_path).await;
        };

        let key = tokio::fs::read(key_file_path).await?;
        let key = key.as_slice().try_into().anyerr()?;
        Ok(SecretKey::from_bytes(key))
    }

    async fn create_key(&self, key_file_path: &PathBuf) -> Result<SecretKey> {
        let key = SecretKey::generate(&mut rand::rng());
        tokio::fs::write(key_file_path, key.to_bytes()).await?;
        Ok(key)
    }

    /// OAuth state is stored per env (e.g. oauth.staging.yml, oauth.production.yml).
    pub fn oauth_file_path(&self, key: &str) -> PathBuf {
        self.0.join(format!("oauth.{key}.yml"))
    }

    pub async fn write_oauth(&self, state: Option<&AuthState>) -> Result<()> {
        self.write_oauth_for_key("staging", state).await
    }

    pub async fn write_oauth_for_key(&self, key: &str, state: Option<&AuthState>) -> Result<()> {
        let path = self.oauth_file_path(key);
        let data = serde_yml::to_string(&state).anyerr()?;
        tokio::fs::write(path, data).await?;
        Ok(())
    }

    pub async fn read_oauth(&self) -> Result<Option<AuthState>> {
        self.read_oauth_for_key("staging").await
    }

    /// Read OAuth state for an env key. For "staging", falls back to legacy oauth.yml if present.
    pub async fn read_oauth_for_key(&self, key: &str) -> Result<Option<AuthState>> {
        let path = self.oauth_file_path(key);
        let legacy = key == "staging";
        if path.exists() {
            let data = tokio::fs::read_to_string(path)
                .await
                .context("failed to read oauth file")?;
            let state: Option<AuthState> =
                serde_yml::from_str(&data).std_context("failed to parse oauth file")?;
            return Ok(state);
        }
        if legacy {
            let legacy_path = self.0.join(Self::OAUTH_FILE);
            if legacy_path.exists() {
                let data = tokio::fs::read_to_string(legacy_path)
                    .await
                    .context("failed to read legacy oauth file")?;
                let state: Option<AuthState> =
                    serde_yml::from_str(&data).std_context("failed to parse oauth file")?;
                return Ok(state);
            }
        }
        Ok(None)
    }

    /// Get the base directory path of this repo
    pub fn path(&self) -> &PathBuf {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("datum-repo-test-{}", uuid::Uuid::new_v4()));
        path
    }

    #[tokio::test]
    async fn listen_key_for_project_migrates_legacy_into_first_project() {
        // The legacy `listen_key` lived at the repo root and was reused for
        // every project the CLI talked to. The migration must move (not copy)
        // it into the first project that requests it, so the second project
        // gets a fresh identity instead of joining the cross-project DNS race.
        let repo = Repo::open_or_create(temp_repo_dir()).await.unwrap();
        let legacy = repo.listen_key().await.unwrap();
        let legacy_bytes = legacy.to_bytes();
        let legacy_path = repo.0.join(Repo::LISTEN_KEY_FILE);
        assert!(legacy_path.exists(), "precondition: legacy key exists");

        let p1 = repo.listen_key_for_project("project-a").await.unwrap();
        assert_eq!(
            p1.to_bytes(),
            legacy_bytes,
            "first project must adopt the legacy key"
        );
        assert!(!legacy_path.exists(), "legacy file must be gone after migration");
        let p1_path = repo
            .0
            .join(Repo::PROJECTS_DIR)
            .join("project-a")
            .join(Repo::LISTEN_KEY_FILE);
        assert!(p1_path.exists(), "key must now live under the project dir");

        let p2 = repo.listen_key_for_project("project-b").await.unwrap();
        assert_ne!(
            p2.to_bytes(),
            legacy_bytes,
            "second project must get a fresh key, not the legacy one"
        );
    }

    #[tokio::test]
    async fn listen_key_for_project_is_stable_across_calls() {
        let repo = Repo::open_or_create(temp_repo_dir()).await.unwrap();
        let first = repo.listen_key_for_project("project-x").await.unwrap();
        let second = repo.listen_key_for_project("project-x").await.unwrap();
        assert_eq!(
            first.to_bytes(),
            second.to_bytes(),
            "repeat calls must return the same persisted key"
        );
    }

    #[tokio::test]
    async fn listen_key_for_project_generates_fresh_without_legacy() {
        let repo = Repo::open_or_create(temp_repo_dir()).await.unwrap();
        let key = repo.listen_key_for_project("only-project").await.unwrap();
        let legacy_path = repo.0.join(Repo::LISTEN_KEY_FILE);
        assert!(!legacy_path.exists(), "no legacy must be created");
        let project_path = repo
            .0
            .join(Repo::PROJECTS_DIR)
            .join("only-project")
            .join(Repo::LISTEN_KEY_FILE);
        assert!(project_path.exists());
        assert_eq!(tokio::fs::read(&project_path).await.unwrap(), key.to_bytes());
    }
}
