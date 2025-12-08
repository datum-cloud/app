use std::path::PathBuf;

use anyhow::Result;
use iroh::SecretKey;
use log::{info, warn};

use crate::{auth::Auth, config::Config, state::State};

// Repo builds up a series of file path conventions from a root directory path.
#[derive(Debug)]
pub struct Repo(PathBuf);

impl Repo {
    const CONNECT_KEY_FILE: &str = "connect_key";
    const LISTEN_KEY_FILE: &str = "listen_key";
    const CONFIG_FILE: &str = "config.yml";
    const OAUTH_TOKEN_FILE: &str = "oauth_token";
    const AUTH_FILE: &str = "auth.yml";
    const STATE_FILE: &str = "state.json";

    pub fn default_location() -> PathBuf {
        dirs_next::data_local_dir().unwrap().join("datum_connect")
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

    pub async fn load_state(&self) -> Result<State> {
        let state_file_path = self.0.join(Self::STATE_FILE);
        if !state_file_path.exists() {
            let state = State::default();
            state.write_to_file(state_file_path).await?;
            return Ok(state);
        };

        State::from_file(state_file_path).await
    }

    pub async fn write_state(&self, state: &State) -> Result<()> {
        state.write_to_file(self.0.join(Self::STATE_FILE)).await
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

    pub async fn connect_key(&self) -> Result<SecretKey> {
        let key_file_path = self.0.join(Self::CONNECT_KEY_FILE);
        self.secret_key(key_file_path).await
    }

    async fn secret_key(&self, key_file_path: PathBuf) -> Result<SecretKey> {
        if !key_file_path.exists() {
            warn!("secret key does not exist. creating new key");
            tokio::fs::create_dir_all(&self.0).await?;
            return self.create_key(&key_file_path).await;
        };

        let key = tokio::fs::read(key_file_path).await?;
        let key = key.as_slice().try_into()?;
        Ok(SecretKey::from_bytes(key))
    }

    async fn create_key(&self, key_file_path: &PathBuf) -> Result<SecretKey> {
        let key = SecretKey::generate(&mut rand::rng());
        tokio::fs::write(key_file_path, key.to_bytes()).await?;
        Ok(key)
    }
}
