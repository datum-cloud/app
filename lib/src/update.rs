use std::io::{Error as IoError, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use n0_error::{Result, StackResultExt, StdResultExt};
use semver::{BuildMetadata, Prerelease, Version};
use serde::{Deserialize, Serialize};

use crate::Repo;

const GITHUB_API_BASE: &str = "https://api.github.com";
const REPO_OWNER: &str = "datum-cloud";
const REPO_NAME: &str = "app";

/// Parse a tag or Cargo-style version into a [`Version`] for ordering.
/// Accepts optional `v` prefix, optional minor/patch (default 0), and optional pre-release after `-`.
/// Strips build metadata (`+...`). Invalid numeric core or invalid pre-release identifiers return `None`.
fn parse_update_version(raw: &str) -> Option<Version> {
    let s = raw.trim().trim_start_matches('v');
    let s = s.split('+').next()?.trim();
    let (core, pre_str) = match s.split_once('-') {
        Some((c, p)) => (c.trim(), p.trim()),
        None => (s, ""),
    };
    if core.is_empty() {
        return None;
    }
    let nums: Vec<&str> = core.split('.').collect();
    let major = nums.first()?.parse::<u64>().ok()?;
    let minor = nums.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = nums.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let pre = if pre_str.is_empty() {
        Prerelease::EMPTY
    } else {
        Prerelease::new(pre_str).ok()?
    };
    Some(Version {
        major,
        minor,
        patch,
        pre,
        build: BuildMetadata::EMPTY,
    })
}

/// Which GitHub release stream to follow for automatic updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Latest non-draft, non-prerelease release (excluding `rolling`).
    Stable,
    /// GitHub pre-releases only (excluding `rolling`). Does not jump to stable; switch channel for that.
    Beta,
}

impl UpdateChannel {
    /// Use the same rule as release tags: a semver pre-release suffix (`1.0.0-beta.1`) => beta track.
    pub fn infer_from_pkg_version(pkg_version: &str) -> Self {
        let v = pkg_version.trim_start_matches('v');
        if v.contains('-') {
            Self::Beta
        } else {
            Self::Stable
        }
    }

    /// Channel implied by this crate's `CARGO_PKG_VERSION` (the running app).
    pub fn infer_from_installed_version() -> Self {
        Self::infer_from_pkg_version(env!("CARGO_PKG_VERSION"))
    }
}

impl Default for UpdateChannel {
    fn default() -> Self {
        Self::infer_from_installed_version()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// Check interval in hours (default: 6)
    #[serde(default = "default_check_interval")]
    pub check_interval_hours: u64,
    /// Last time we checked for updates (Unix timestamp)
    #[serde(default)]
    pub last_check_time: Option<u64>,
    /// Whether auto-update is enabled
    #[serde(default = "default_auto_update_enabled")]
    pub auto_update_enabled: bool,
    /// Path to downloaded update ready to install (for "install on next restart")
    #[serde(default)]
    pub pending_install_path: Option<String>,
    /// Version of the pending update (for display in Settings)
    #[serde(default)]
    pub pending_update_version: Option<String>,
    /// Stable (production) or beta (pre-release) update stream
    #[serde(default = "default_update_channel")]
    pub update_channel: UpdateChannel,
}

fn default_check_interval() -> u64 {
    6
}

fn default_auto_update_enabled() -> bool {
    true
}

fn default_update_channel() -> UpdateChannel {
    UpdateChannel::infer_from_installed_version()
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_interval_hours: 6,
            last_check_time: None,
            auto_update_enabled: true,
            pending_install_path: None,
            pending_update_version: None,
            update_channel: UpdateChannel::infer_from_installed_version(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: String,
    published_at: String,
    assets: Vec<GitHubAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
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

fn release_eligible_for_channel(release: &GitHubRelease, channel: UpdateChannel) -> bool {
    if release.draft || release.tag_name == "rolling" {
        return false;
    }
    match channel {
        UpdateChannel::Stable => {
            if release.prerelease {
                return false;
            }
            let version_part = release.tag_name.trim_start_matches('v');
            !version_part.contains('-')
        }
        UpdateChannel::Beta => {
            // Never offer full releases on the beta track—even if a stable build is newer.
            if release.prerelease {
                return true;
            }
            let version_part = release.tag_name.trim_start_matches('v');
            version_part.contains('-')
        }
    }
}

/// Pick the newest GitHub release (API order) that matches the channel, has a platform asset, and is newer than `current_version`.
fn select_release_for_channel(
    releases: Vec<GitHubRelease>,
    channel: UpdateChannel,
    current_version: &str,
    has_platform_asset: impl Fn(&[GitHubAsset]) -> bool,
) -> Option<GitHubRelease> {
    let current = UpdateChecker::extract_version(current_version);
    for release in releases {
        if !release_eligible_for_channel(&release, channel) {
            continue;
        }
        if !has_platform_asset(&release.assets) {
            continue;
        }
        let v = UpdateChecker::extract_version(&release.tag_name);
        if UpdateChecker::is_newer_version(&v, &current) {
            return Some(release);
        }
    }
    None
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

        let url = format!(
            "{}/repos/{}/{}/releases?per_page=100",
            GITHUB_API_BASE, REPO_OWNER, REPO_NAME
        );

        let client = reqwest::Client::builder()
            .user_agent(crate::datum_http_user_agent())
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

        let channel = settings.update_channel;
        let release =
            select_release_for_channel(releases, channel, &self.current_version, |assets| {
                Self::try_find_platform_asset(assets).is_some()
            });

        // Update last check time
        let mut settings = settings;
        settings.last_check_time = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        self.save_settings(&settings).await?;

        let Some(release) = release else {
            return Ok(None);
        };

        let latest_version = Self::extract_version(&release.tag_name);
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
    }

    /// Extract version string from tag (handles formats like "v0.0.3" or "0.0.3")
    pub(crate) fn extract_version(tag: &str) -> String {
        // Remove 'v' prefix if present
        tag.trim_start_matches('v').to_string()
    }

    /// Compare semantic version strings — true if `version1` is strictly newer than `version2`.
    /// Uses full SemVer 2.0 ordering (including pre-release); build metadata is ignored.
    pub(crate) fn is_newer_version(version1: &str, version2: &str) -> bool {
        match (
            parse_update_version(version1),
            parse_update_version(version2),
        ) {
            (Some(v1), Some(v2)) => v1 > v2,
            _ => false,
        }
    }

    fn try_find_platform_asset<'a>(assets: &'a [GitHubAsset]) -> Option<&'a GitHubAsset> {
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
            return None;
        };

        if let Some(arch) = arch_pattern {
            if let Some(asset) = assets
                .iter()
                .find(|asset| asset.name.ends_with(platform_ext) && asset.name.contains(arch))
            {
                return Some(asset);
            }
        }

        assets
            .iter()
            .find(|asset| asset.name.ends_with(platform_ext))
    }

    /// Find the appropriate binary asset for the current platform
    fn find_platform_asset<'a>(&self, assets: &'a [GitHubAsset]) -> Result<&'a GitHubAsset> {
        Self::try_find_platform_asset(assets)
            .ok_or_else(|| IoError::new(ErrorKind::NotFound, "No asset found for platform"))
            .anyerr()
    }

    /// Download the update binary to a temporary location
    pub async fn download_update(&self, download_url: &str) -> Result<PathBuf> {
        let client = reqwest::Client::builder()
            .user_agent(crate::datum_http_user_agent())
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

    /// Check if there is a pending update ready to install
    pub async fn has_pending_install(&self) -> Result<bool> {
        let settings = self.load_settings().await?;
        Ok(settings
            .pending_install_path
            .as_ref()
            .map(|p| Path::new(p).exists())
            .unwrap_or(false))
    }

    /// Get the path of the pending update, if any
    pub async fn get_pending_install_path(&self) -> Result<Option<PathBuf>> {
        let settings = self.load_settings().await?;
        Ok(settings.pending_install_path.as_ref().and_then(|p| {
            let path = PathBuf::from(p);
            if path.exists() { Some(path) } else { None }
        }))
    }

    /// Set the pending install path and version (call after download completes)
    pub async fn set_pending_install(&self, path: &Path, version: &str) -> Result<()> {
        let mut settings = self.load_settings().await?;
        settings.pending_install_path = Some(path.to_string_lossy().to_string());
        settings.pending_update_version = Some(version.to_string());
        self.save_settings(&settings).await
    }

    /// Clear the pending install path
    pub async fn clear_pending_install(&self) -> Result<()> {
        let mut settings = self.load_settings().await?;
        settings.pending_install_path = None;
        settings.pending_update_version = None;
        self.save_settings(&settings).await
    }

    /// Get the pending update version, if any
    pub async fn get_pending_update_version(&self) -> Result<Option<String>> {
        let settings = self.load_settings().await?;
        Ok(settings.pending_update_version)
    }

    /// Apply the update (install the downloaded file). Blocks until complete.
    /// Call this when the app is about to exit or on next startup.
    pub fn apply_update(&self, downloaded_path: &Path) -> Result<()> {
        Self::apply_update_static(downloaded_path)
    }

    /// Apply the update without needing a checker instance (for use in blocking contexts).
    pub fn apply_update_static(downloaded_path: &Path) -> Result<()> {
        if !downloaded_path.exists() {
            return Err(IoError::new(
                ErrorKind::NotFound,
                "Downloaded update file does not exist",
            ))
            .anyerr();
        }

        // Dev bypass: skip actual install when DATUM_UPDATE_FAKE=1 (for testing toast/Settings UI)
        if std::env::var("DATUM_UPDATE_FAKE").as_deref() == Ok("1") {
            tracing::info!("DATUM_UPDATE_FAKE=1: skipping actual install");
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        Self::apply_update_macos_static(downloaded_path)?;

        #[cfg(target_os = "windows")]
        Self::apply_update_windows_static(downloaded_path)?;

        #[cfg(target_os = "linux")]
        Self::apply_update_linux_static(downloaded_path)?;

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = downloaded_path;
            return Err(IoError::new(
                ErrorKind::Unsupported,
                "Automatic updates not supported on this platform",
            ))
            .anyerr();
        }

        Ok(())
    }

    /// Parse mount point from `hdiutil attach` stdout. Must not use `-quiet` (it closes stdout).
    #[cfg(target_os = "macos")]
    fn parse_hdiutil_attach_mount_point(stdout: &str) -> Option<PathBuf> {
        for line in stdout.lines().rev() {
            if !line.contains("/Volumes/") {
                continue;
            }
            let last_col = line
                .split('\t')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .last()?;
            if last_col.starts_with("/Volumes/") {
                return Some(PathBuf::from(last_col));
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    fn apply_update_macos_static(dmg_path: &Path) -> Result<()> {
        use std::process::Stdio;

        // Do not pass `-quiet`: it closes stdout, so the mount point cannot be parsed (man hdiutil).
        let output = Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-readonly"])
            .arg(dmg_path)
            .output()
            .anyerr()?;

        if !output.status.success() {
            return Err(IoError::new(
                ErrorKind::Other,
                format!(
                    "Failed to mount DMG: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            ))
            .anyerr();
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let volume = Self::parse_hdiutil_attach_mount_point(&stdout).ok_or_else(|| {
            IoError::new(
                ErrorKind::Other,
                format!(
                    "No /Volumes mount point in hdiutil output: {}",
                    stdout.trim()
                ),
            )
        })?;

        // Find .app in the volume
        let app_path = std::fs::read_dir(&volume)
            .anyerr()?
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().map_or(false, |ext| ext == "app"))
            .map(|e| e.path())
            .ok_or_else(|| IoError::new(ErrorKind::NotFound, "No .app found in DMG"))?;

        let dest = Path::new("/Applications").join(app_path.file_name().unwrap());

        // Copy to Applications
        let status = Command::new("rsync")
            .args(["-a", "--delete"])
            .arg(&app_path)
            .arg(&dest)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .anyerr()?;

        // Unmount
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&volume)
            .status();

        if !status.success() {
            return Err(IoError::new(
                ErrorKind::Other,
                "Failed to copy app to Applications",
            ))
            .anyerr();
        }

        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn apply_update_windows_static(installer_path: &Path) -> Result<()> {
        // NSIS installer - /S for silent install
        let status = Command::new(installer_path).arg("/S").status().anyerr()?;

        if !status.success() {
            return Err(IoError::new(ErrorKind::Other, "Installer failed")).anyerr();
        }

        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn apply_update_linux_static(new_appimage_path: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let current_exe = std::env::current_exe().anyerr()?;
        let temp_backup = current_exe.with_extension("appimage.bak");

        // Move current to backup, new to current location
        std::fs::rename(&current_exe, &temp_backup).anyerr()?;
        std::fs::copy(new_appimage_path, &current_exe).anyerr()?;

        let mut perms = std::fs::metadata(&current_exe).anyerr()?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&current_exe, perms).anyerr()?;

        // Remove backup on success
        let _ = std::fs::remove_file(&temp_backup);

        Ok(())
    }

    /// Spawn a process that waits for this app to exit, then applies the update and restarts.
    /// Call this before exiting for "Install now".
    pub fn spawn_install_and_restart(&self, downloaded_path: &Path) -> Result<()> {
        let path = downloaded_path.to_path_buf();
        let path_str = path.to_string_lossy().to_string();
        let parent_pid = std::process::id();

        #[cfg(unix)]
        {
            let exe = std::env::current_exe().anyerr()?;
            let exe_str = exe.to_string_lossy().to_string();

            // Write helper script to repo
            let script_path = self.repo.path().join("update_install.sh");
            let install_script = self.get_install_script_unix();
            std::fs::write(&script_path, install_script).anyerr()?;

            #[cfg(target_os = "macos")]
            let (shell, arg) = ("/bin/bash", "-c");
            #[cfg(target_os = "linux")]
            let (shell, arg) = ("/bin/bash", "-c");

            let script_path_str = script_path.to_string_lossy();
            let wait_and_run = format!(
                "while kill -0 {} 2>/dev/null; do sleep 0.5; done; chmod +x \"{}\" && \"{}\" \"{}\" \"{}\"",
                parent_pid,
                script_path_str,
                script_path_str,
                path_str.replace('"', "\\\""),
                exe_str.replace('"', "\\\"")
            );

            Command::new(shell)
                .arg(arg)
                .arg(wait_and_run)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .anyerr()?;
        }

        #[cfg(windows)]
        {
            let exe = std::env::current_exe().anyerr()?;
            let exe_str = exe.to_string_lossy().to_string();

            // Use PowerShell to wait for process and run installer
            let ps_script = if std::env::var("DATUM_UPDATE_FAKE").as_deref() == Ok("1") {
                // Dev bypass: skip installer, just restart the app
                format!(
                    r#"while (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 500 }}
Start-Process -FilePath '{}'
"#,
                    parent_pid,
                    exe_str.replace('\'', "''")
                )
            } else {
                format!(
                    r#"while (Get-Process -Id {} -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 500 }}
Start-Process -FilePath '{}' -ArgumentList '/S' -Wait
Start-Process -FilePath '{}'
"#,
                    parent_pid,
                    path_str.replace('\'', "''"),
                    exe_str.replace('\'', "''")
                )
            };

            Command::new("powershell")
                .args(["-NoProfile", "-Command", &ps_script])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .anyerr()?;
        }

        Ok(())
    }

    #[cfg(unix)]
    fn get_install_script_unix(&self) -> String {
        // Dev bypass: when DATUM_UPDATE_FAKE=1, just restart the app (skip actual install)
        if std::env::var("DATUM_UPDATE_FAKE").as_deref() == Ok("1") {
            #[cfg(target_os = "macos")]
            return "open -a \"Datum\"".to_string();
            #[cfg(target_os = "linux")]
            return r#"#!/bin/bash
exec "$2"
"#
            .to_string();
        }

        #[cfg(target_os = "macos")]
        {
            // Do not use hdiutil -quiet when capturing the mount point (quiet closes stdout).
            r#"#!/bin/bash
set -euo pipefail
DMG="$1"
ATTACH_OUT=$(hdiutil attach -nobrowse -readonly "$DMG")
VOLUME=$(echo "$ATTACH_OUT" | grep '/Volumes/' | tail -n1 | awk -F'	' '{gsub(/^ +| +$/,"",$NF); print $NF}')
[ -n "$VOLUME" ] || { echo "hdiutil attach: no /Volumes mount in output" >&2; exit 1; }
APP=$(find "$VOLUME" -maxdepth 1 -name "*.app" -print -quit)
[ -n "$APP" ] || { echo "No .app bundle in DMG at $VOLUME" >&2; exit 1; }
DEST="/Applications/$(basename "$APP")"
rsync -a --delete "$APP" /Applications/
hdiutil detach -force -quiet "$VOLUME"
open "$DEST"
"#
            .to_string()
        }

        #[cfg(target_os = "linux")]
        {
            r#"#!/bin/bash
set -e
NEW="$1"
CURRENT="$2"
cp -f "$NEW" "$CURRENT"
chmod +x "$CURRENT"
exec "$CURRENT"
"#
            .to_string()
        }
    }

    /// Get current version
    pub fn current_version(&self) -> &str {
        &self.current_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_infer_stable_pkg_version() {
        assert_eq!(
            UpdateChannel::infer_from_pkg_version("1.2.3"),
            UpdateChannel::Stable
        );
        assert_eq!(
            UpdateChannel::infer_from_pkg_version("v1.2.3"),
            UpdateChannel::Stable
        );
    }

    #[test]
    fn channel_infer_beta_pkg_version() {
        assert_eq!(
            UpdateChannel::infer_from_pkg_version("1.2.3-beta.1"),
            UpdateChannel::Beta
        );
        assert_eq!(
            UpdateChannel::infer_from_pkg_version("v2.0.0-rc.1"),
            UpdateChannel::Beta
        );
    }

    fn sample_asset() -> GitHubAsset {
        GitHubAsset {
            name: "Datum_aarch64.dmg".to_string(),
            browser_download_url: "https://example.com/a.dmg".to_string(),
            size: 1,
        }
    }

    fn gh_release(tag: &str, draft: bool, prerelease: bool, with_asset: bool) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.to_string(),
            name: format!("Release {tag}"),
            published_at: "2020-01-01T00:00:00Z".to_string(),
            assets: if with_asset {
                vec![sample_asset()]
            } else {
                vec![]
            },
            draft,
            prerelease,
        }
    }

    #[test]
    fn stable_skips_draft_prerelease_and_rolling() {
        let releases = vec![
            gh_release("v9.0.0", true, false, true),
            gh_release("v8.0.0", false, true, true),
            gh_release("rolling", false, false, true),
            gh_release("v2.0.0", false, false, true),
        ];
        let picked = select_release_for_channel(releases, UpdateChannel::Stable, "1.0.0", |_| true);
        assert_eq!(picked.unwrap().tag_name, "v2.0.0");
    }

    #[test]
    fn stable_prefers_full_release_over_newer_prerelease() {
        let releases = vec![
            gh_release("v3.0.0", false, true, true),
            gh_release("v2.0.0", false, false, true),
        ];
        let picked = select_release_for_channel(releases, UpdateChannel::Stable, "1.0.0", |_| true);
        assert_eq!(picked.unwrap().tag_name, "v2.0.0");
    }

    #[test]
    fn beta_takes_newest_eligible_including_prerelease() {
        let releases = vec![
            gh_release("v3.0.0", false, true, true),
            gh_release("v2.0.0", false, false, true),
        ];
        let picked = select_release_for_channel(releases, UpdateChannel::Beta, "1.0.0", |_| true);
        assert_eq!(picked.unwrap().tag_name, "v3.0.0");
    }

    #[test]
    fn beta_skips_newer_stable_and_takes_prerelease() {
        let releases = vec![
            gh_release("v2.0.0", false, false, true),
            gh_release("v1.5.0-beta", false, true, true),
        ];
        let picked = select_release_for_channel(releases, UpdateChannel::Beta, "1.0.0", |_| true);
        assert_eq!(picked.unwrap().tag_name, "v1.5.0-beta");
    }

    #[test]
    fn beta_offers_no_update_when_only_stable_is_newer() {
        let releases = vec![gh_release("v2.0.0", false, false, true)];
        let picked = select_release_for_channel(releases, UpdateChannel::Beta, "1.0.0", |_| true);
        assert!(picked.is_none());
    }

    #[test]
    fn beta_accepts_hyphen_tag_without_prerelease_flag() {
        let releases = vec![gh_release("v2.0.0-beta", false, false, true)];
        let picked = select_release_for_channel(releases, UpdateChannel::Beta, "1.0.0", |_| true);
        assert_eq!(picked.unwrap().tag_name, "v2.0.0-beta");
    }

    #[test]
    fn stable_excludes_hyphen_tag_even_without_prerelease_flag() {
        let releases = vec![gh_release("v2.0.0-beta", false, false, true)];
        let picked = select_release_for_channel(releases, UpdateChannel::Stable, "1.0.0", |_| true);
        assert!(picked.is_none());
    }

    #[test]
    fn skips_release_without_matching_asset_predicate() {
        let releases = vec![
            gh_release("v3.0.0", false, false, false),
            gh_release("v2.0.0", false, false, true),
        ];
        let picked =
            select_release_for_channel(releases, UpdateChannel::Stable, "1.0.0", |a| !a.is_empty());
        assert_eq!(picked.unwrap().tag_name, "v2.0.0");
    }

    #[test]
    fn none_when_nothing_newer() {
        let releases = vec![gh_release("v2.0.0", false, false, true)];
        let picked = select_release_for_channel(releases, UpdateChannel::Stable, "2.0.0", |_| true);
        assert!(picked.is_none());
    }

    #[test]
    fn newer_orders_prerelease_suffixes() {
        assert!(UpdateChecker::is_newer_version(
            "1.0.0-beta.2",
            "1.0.0-beta.1"
        ));
        assert!(!UpdateChecker::is_newer_version(
            "1.0.0-beta.1",
            "1.0.0-beta.2"
        ));
        assert!(UpdateChecker::is_newer_version(
            "1.0.0-rc.1",
            "1.0.0-beta.99"
        ));
    }

    #[test]
    fn release_newer_than_prerelease_same_core() {
        assert!(UpdateChecker::is_newer_version("1.0.0", "1.0.0-beta.99"));
        assert!(!UpdateChecker::is_newer_version("1.0.0-beta.99", "1.0.0"));
    }

    #[test]
    fn newer_accepts_v_prefix_build_metadata_ignored() {
        assert!(UpdateChecker::is_newer_version("v1.2.4", "1.2.3"));
        // SemVer: build metadata does not affect precedence
        assert!(!UpdateChecker::is_newer_version(
            "1.2.3+build2",
            "1.2.3+build1"
        ));
    }
}
