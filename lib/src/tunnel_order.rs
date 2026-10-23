//! The display order of Datum Cloud tunnels, kept on this computer.
//!
//! The API has no field for the order of a project's tunnels, so the app
//! remembers the order the user arranged them in. Tunnels created elsewhere
//! since then are not in the saved order; they come first, in the order the
//! API returned them, so they are not hidden below familiar ones.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::local_tunnels::write_private;
use crate::{Repo, TunnelSummary};

const FILE: &str = "tunnel_order.json";
const FILE_FORMAT: &str = "datum-tunnel-order";
const FILE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct SavedFile {
    format: String,
    version: u32,
    /// Project ID to tunnel IDs, in display order.
    #[serde(default)]
    projects: HashMap<String, Vec<String>>,
}

/// The order project tunnels were arranged in on this computer. Cheap to
/// clone. Shared by every project, keyed by project ID.
#[derive(Clone)]
pub struct TunnelOrder {
    projects: Arc<Mutex<HashMap<String, Vec<String>>>>,
    file: PathBuf,
}

impl fmt::Debug for TunnelOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TunnelOrder")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl TunnelOrder {
    /// Loads the saved order. A missing or unreadable file is an empty
    /// order, which leaves every list in the API's order.
    pub async fn load(repo: &Repo) -> Result<Self> {
        let file = repo.path().join(FILE);
        let projects = match tokio::fs::read(&file).await {
            Ok(data) => match serde_json::from_slice::<SavedFile>(&data) {
                Ok(saved) if saved.format == FILE_FORMAT && saved.version <= FILE_VERSION => {
                    saved.projects
                }
                Ok(_) | Err(_) => {
                    warn!(path = %file.display(), "tunnel order: ignoring unreadable file");
                    HashMap::new()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(err) => return Err(err).std_context("reading tunnel order"),
        };
        Ok(Self {
            projects: Arc::new(Mutex::new(projects)),
            file,
        })
    }

    /// Tunnel IDs for `project`, in the order the user arranged them.
    pub fn order(&self, project: &str) -> Vec<String> {
        self.lock().get(project).cloned().unwrap_or_default()
    }

    /// Replaces the order for `project` with `ids` and saves it. The caller
    /// passes the project's current tunnels, so IDs that no longer exist
    /// fall out of the file.
    pub fn set(&self, project: &str, ids: Vec<String>) {
        let mut projects = self.lock();
        projects.insert(project.to_string(), ids);
        let saved = SavedFile {
            format: FILE_FORMAT.to_string(),
            version: FILE_VERSION,
            projects: projects.clone(),
        };
        let result = serde_json::to_vec_pretty(&saved)
            .map_err(std::io::Error::other)
            .and_then(|data| write_private(&self.file, &data));
        if let Err(error) = result {
            warn!(path = %self.file.display(), %error, "tunnel order: failed to save");
        }
    }

    /// Applies the saved order for `project` to `tunnels`, which are in API
    /// order. Tunnels the order doesn't know about yet come first.
    pub fn apply(&self, project: &str, tunnels: Vec<TunnelSummary>) -> Vec<TunnelSummary> {
        apply_order(&self.order(project), tunnels)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<String>>> {
        self.projects.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Sorts `tunnels` into `stored` order. IDs missing from `stored` keep their
/// API order and come first, so they stay in reach below the fold.
pub fn apply_order(stored: &[String], tunnels: Vec<TunnelSummary>) -> Vec<TunnelSummary> {
    if stored.is_empty() {
        return tunnels;
    }
    let positions: HashMap<&str, usize> = stored
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let mut unknown = Vec::new();
    let mut known = Vec::new();
    for tunnel in tunnels {
        match positions.get(tunnel.id.as_str()) {
            Some(&position) => known.push((position, tunnel)),
            None => unknown.push(tunnel),
        }
    }
    known.sort_by_key(|(position, _)| *position);
    unknown.extend(known.into_iter().map(|(_, tunnel)| tunnel));
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tunnel(id: &str) -> TunnelSummary {
        TunnelSummary {
            id: id.to_string(),
            label: id.to_string(),
            endpoint: "127.0.0.1:80".to_string(),
            hostnames: Vec::new(),
            enabled: true,
            accepted: true,
            programmed: true,
        }
    }

    fn ids(tunnels: &[TunnelSummary]) -> Vec<&str> {
        tunnels.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn keeps_api_order_without_a_saved_order() {
        let tunnels = vec![tunnel("a"), tunnel("b"), tunnel("c")];
        assert_eq!(ids(&apply_order(&[], tunnels.clone())), ["a", "b", "c"]);
    }

    #[test]
    fn applies_the_saved_order_with_new_tunnels_first() {
        let stored: Vec<String> = ["c", "a"].iter().map(|s| s.to_string()).collect();
        let tunnels = vec![tunnel("a"), tunnel("b"), tunnel("c")];
        assert_eq!(ids(&apply_order(&stored, tunnels)), ["b", "c", "a"]);
    }

    #[test]
    fn drops_ids_that_no_longer_exist() {
        let stored: Vec<String> = ["gone", "b"].iter().map(|s| s.to_string()).collect();
        let tunnels = vec![tunnel("a"), tunnel("b")];
        assert_eq!(ids(&apply_order(&stored, tunnels)), ["a", "b"]);
    }

    #[tokio::test]
    async fn saves_and_restores_per_project() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::open_or_create(dir.path()).await.unwrap();
        let order = TunnelOrder::load(&repo).await.unwrap();
        order.set("project-a", vec!["b".to_string(), "a".to_string()]);
        order.set("project-b", vec!["c".to_string()]);

        let restored = TunnelOrder::load(&repo).await.unwrap();
        assert_eq!(restored.order("project-a"), ["b", "a"]);
        assert_eq!(restored.order("project-b"), ["c"]);
        assert!(restored.order("project-c").is_empty());
    }

    #[tokio::test]
    async fn missing_file_is_an_empty_order() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::open_or_create(dir.path()).await.unwrap();
        let order = TunnelOrder::load(&repo).await.unwrap();
        assert!(order.order("project-a").is_empty());
    }
}
