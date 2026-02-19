use std::io::{Error as IoError, ErrorKind};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use n0_error::{Result, StackResultExt, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::Repo;

const GITHUB_API_BASE: &str = "https://api.github.com";
const REPO_OWNER: &str = "datum-cloud";
const REPO_NAME: &str = "app";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// Check interval in hours (default: 12)
    #[serde(default = "default_check_interval")]
    pub check_interval_hours: u64,
    /// Last time we checked for updates (Unix timestamp)
    #[serde(default)]
    pub last_check_time: Option<u64>,
    /// Whether auto-update is enabled
    #[serde(default = "default_auto_update_enabled")]
    pub auto_update_enabled: bool,
}

fn default_check_interval() -> u64 {
    12
}

fn default_auto_update_enabled() -> bool {
    true
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_interval_hours: 12,
            last_check_time: None,
            auto_update_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: String,
    published_at: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub version: String,
    pub release_name: String,
    pub published_at: DateTime<Utc>,
    pub download_url: String,
    pub download_size: u64,
}

struct VersionParts {
    major: u32,
    minor: u32,
    patch: u32,
}

pub struct UpdateChecker {
    repo: Repo,
    current_version: String,
}

impl UpdateChecker {
    pub fn new(repo: Repo) -> Self {
        Self {
            repo,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub async fn load_settings(&self) -> Result<UpdateSettings> {
        let settings_file = self.repo.path().join("update_settings.yml");
        if settings_file.exists() {
            let content = tokio::fs::read_to_string(&settings_file)
                .await
                .context("failed to read update settings")?;
            let settings: UpdateSettings =
                serde_yml::from_str(&content).std_context("failed to parse update settings")?;
            Ok(settings)
        } else {
            Ok(UpdateSettings::default())
        }
    }

    pub async fn save_settings(&self, settings: &UpdateSettings) -> Result<()> {
        let settings_file = self.repo.path().join("update_settings.yml");
        let content = serde_yml::to_string(settings).anyerr()?;
        tokio::fs::write(&settings_file, content)
            .await
            .context("failed to write update settings")?;
        Ok(())
    }

    /// Check if we should check for updates based on the interval
    pub async fn should_check(&self) -> Result<bool> {
        let settings = self.load_settings().await?;

        if !settings.auto_update_enabled {
            return Ok(false);
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if let Some(last_check) = settings.last_check_time {
            let interval_seconds = settings.check_interval_hours * 3600;
            if now - last_check < interval_seconds {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Fetch the latest release info from GitHub
    pub async fn check_for_updates(&self) -> Result<Option<UpdateInfo>> {
        let settings = self.load_settings().await?;

        if !settings.auto_update_enabled {
            return Ok(None);
        }

        // Fetch all releases and filter out "rolling" tag
        // We want the latest tagged release (like v0.0.3), not the rolling release
        let url = format!(
            "{}/repos/{}/{}/releases",
            GITHUB_API_BASE, REPO_OWNER, REPO_NAME
        );

        let client = reqwest::Client::builder()
            .user_agent("DatumConnect/1.0")
            .build()
            .anyerr()?;

        let response = client.get(&url).send().await.anyerr()?;

        if !response.status().is_success() {
            return Err(IoError::new(
                ErrorKind::Other,
                format!("GitHub API returned status: {}", response.status()),
            ))
            .anyerr();
        }

        let releases: Vec<GitHubRelease> = response.json().await.anyerr()?;

        // Find the latest release that is not "rolling"
        let release = releases
            .into_iter()
            .find(|r| r.tag_name != "rolling")
            .ok_or_else(|| {
                IoError::new(
                    ErrorKind::NotFound,
                    "No non-rolling release found",
                )
            })
            .anyerr()?;

        // Update last check time
        let mut settings = settings;
        settings.last_check_time = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        self.save_settings(&settings).await?;

        // Extract version from tag_name (format: "v0.0.3" or "0.0.3")
        let latest_version = Self::extract_version(&release.tag_name);
        let current_version = Self::extract_version(&self.current_version);

        // Compare versions - if latest is newer, return update info
        if Self::is_newer_version(&latest_version, &current_version) {
            // Find the appropriate binary asset for the current platform
            let asset = self.find_platform_asset(&release.assets)?;

            let published_at = DateTime::parse_from_rfc3339(&release.published_at)
                .std_context("failed to parse published_at")?
                .with_timezone(&Utc);

            Ok(Some(UpdateInfo {
                version: latest_version,
                release_name: release.name,
                published_at,
                download_url: asset.browser_download_url.clone(),
                download_size: asset.size,
            }))
        } else {
            Ok(None)
        }
    }

    /// Extract version string from tag (handles formats like "v0.0.3" or "0.0.3")
    fn extract_version(tag: &str) -> String {
        // Remove 'v' prefix if present
        tag.trim_start_matches('v').to_string()
    }

    /// Compare semantic version strings - returns true if version1 > version2
    /// Handles semantic versions like "0.0.3", "0.1.0", "1.0.0", etc.
    fn is_newer_version(version1: &str, version2: &str) -> bool {
        let v1_parts = Self::parse_semantic_version(version1);
        let v2_parts = Self::parse_semantic_version(version2);

        // Compare major, minor, patch versions
        match v1_parts.major.cmp(&v2_parts.major) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => {}
        }

        match v1_parts.minor.cmp(&v2_parts.minor) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => {}
        }

        match v1_parts.patch.cmp(&v2_parts.patch) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => {
                // If versions are equal, check for pre-release/build metadata
                // For now, if versions are equal, consider it not newer
                false
            }
        }
    }

    /// Parse semantic version string (e.g., "0.0.3" or "0.0.3-beta")
    fn parse_semantic_version(version: &str) -> VersionParts {
        // Remove any pre-release or build metadata (everything after '-')
        let version = version.split('-').next().unwrap_or(version);
        
        let parts: Vec<&str> = version.split('.').collect();
        
        let major = parts.get(0).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

        VersionParts {
            major,
            minor,
            patch,
        }
    }

    /// Find the appropriate binary asset for the current platform
    fn find_platform_asset<'a>(&self, assets: &'a [GitHubAsset]) -> Result<&'a GitHubAsset> {
        let (platform_ext, arch_pattern) = if cfg!(target_os = "macos") {
            if cfg!(target_arch = "aarch64") {
                (".dmg", Some("aarch64"))
            } else {
                (".dmg", Some("x86_64"))
            }
        } else if cfg!(target_os = "windows") {
            (".exe", None)
        } else if cfg!(target_os = "linux") {
            (".AppImage", None)
        } else {
            return Err(IoError::new(ErrorKind::Unsupported, "Unsupported platform")).anyerr();
        };

        // Prefer assets with architecture match, then fall back to any matching extension
        if let Some(arch) = arch_pattern {
            if let Some(asset) = assets
                .iter()
                .find(|asset| asset.name.ends_with(platform_ext) && asset.name.contains(arch))
            {
                return Ok(asset);
            }
        }

        // Fall back to any asset with matching extension
        match assets
            .iter()
            .find(|asset| asset.name.ends_with(platform_ext))
        {
            Some(asset) => Ok(asset),
            None => Err(IoError::new(
                ErrorKind::NotFound,
                "No asset found for platform",
            ))
            .anyerr(),
        }
    }

    /// Download the update binary to a temporary location
    pub async fn download_update(&self, download_url: &str) -> Result<PathBuf> {
        let client = reqwest::Client::builder()
            .user_agent("DatumConnect/1.0")
            .build()
            .anyerr()?;

        let response = client.get(download_url).send().await.anyerr()?;

        if !response.status().is_success() {
            return Err(IoError::new(
                ErrorKind::Other,
                format!("Download failed with status: {}", response.status()),
            ))
            .anyerr();
        }

        let bytes = response.bytes().await.anyerr()?;

        // Save to a temporary file in the repo directory
        let temp_file = self.repo.path().join("update_temp.bin");
        tokio::fs::write(&temp_file, bytes)
            .await
            .context("failed to write update file")?;

        Ok(temp_file)
    }

    /// Get current version
    pub fn current_version(&self) -> &str {
        &self.current_version
    }
}
