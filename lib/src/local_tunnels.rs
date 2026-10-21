//! Project-less local tunnels: share a local TCP service with paired devices,
//! or open a service another device shares, without a Datum account.
//!
//! A thin layer over `connect-share`, the code behind `datumctl connect local`
//! and the Datum Connect Android app. A share prints a ticket; a device that
//! holds the ticket pairs over iroh and reaches the service peer to peer, or
//! through an iroh relay. Nothing here talks to Datum Cloud.
//!
//! The list of tunnels is saved to the repo so it survives restarts. Statuses
//! are not saved: every tunnel comes back stopped, because a share's ticket
//! changes each time it starts and a sharer may have restarted.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use connect_share::{DialEvent, DialOptions, ShareEvent, ShareOptions, ShareTicket};
use iroh::SecretKey;
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::{Repo, TcpProxyData};

/// Longest name accepted here and by connect-share.
pub const MAX_NAME: usize = 64;
/// Distinct devices that may pair with one share, as with `datumctl connect local`.
const MAX_PEERS: usize = 4;
const TICKET_PREFIX: &str = "DCSHARE2";
const FILE_FORMAT: &str = "datum-connect-local-tunnels";
const FILE_VERSION: u32 = 1;

/// What a local tunnel does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalTunnelKind {
    /// Shares `address` with devices that pair using the share's ticket.
    Share { address: String },
    /// Opens a service another device shares on `127.0.0.1:port`. Port 0
    /// lets the system pick.
    Connect { ticket: String, port: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalTunnelStatus {
    Stopped,
    /// Waiting for a relay, or pairing with the sharer.
    Starting,
    /// A share is running; devices pair with `ticket`.
    Sharing { ticket: String },
    /// A connection is open at `local_addr`, for example `127.0.0.1:8080`.
    Open { local_addr: String },
    Failed(String),
}

impl LocalTunnelStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Sharing { .. } | Self::Open { .. })
    }
}

/// How traffic travels to a peer right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Direct,
    Relay,
    Unknown,
}

/// The network path to a peer, reported while connections are open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalPath {
    pub kind: PathKind,
    /// The peer's IP and port on this path, or the relay URL.
    pub remote: Option<String>,
    pub rtt_ms: Option<u32>,
}

impl LocalPath {
    /// For display: "Direct · 12 ms", "Relayed · 150 ms", or "Path unknown".
    pub fn summary(&self) -> String {
        let kind = match self.kind {
            PathKind::Direct => "Direct",
            PathKind::Relay => "Relayed",
            PathKind::Unknown => "Path unknown",
        };
        match self.rtt_ms {
            Some(ms) => format!("{kind} · {ms} ms"),
            None => kind.to_string(),
        }
    }
}

impl From<connect_share::PathInfo> for LocalPath {
    fn from(path: connect_share::PathInfo) -> Self {
        Self {
            kind: match path.kind {
                connect_share::PathKind::Direct => PathKind::Direct,
                connect_share::PathKind::Relay => PathKind::Relay,
                connect_share::PathKind::Unknown => PathKind::Unknown,
            },
            remote: path.remote,
            rtt_ms: path
                .rtt
                .map(|rtt| rtt.as_millis().min(u32::MAX as u128) as u32),
        }
    }
}

/// A device that paired with a running share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedDevice {
    pub endpoint_id: String,
    /// The name the device sent when pairing, such as its model.
    pub name: Option<String>,
    pub path: Option<LocalPath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTunnel {
    pub id: String,
    pub name: String,
    /// True once the user chose the name; a sharer's name no longer replaces it.
    pub name_from_user: bool,
    pub kind: LocalTunnelKind,
    pub status: LocalTunnelStatus,
    /// Connect only: the path to the sharer while open.
    pub path: Option<LocalPath>,
    /// Share only: devices paired since the share started.
    pub devices: Vec<PairedDevice>,
}

/// What a ticket points at, read without contacting the sharer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketInfo {
    pub endpoint_id: String,
    pub port: u16,
}

/// Finds a ticket in pasted text, such as a full `datumctl connect dial` command.
pub fn extract_ticket(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    let start = upper.find(TICKET_PREFIX)?;
    let ticket: String = upper[start..]
        .chars()
        .take_while(|c| c.is_ascii_uppercase() || ('2'..='7').contains(c))
        .collect();
    (ticket.len() > TICKET_PREFIX.len()).then_some(ticket)
}

/// Parses a ticket. The error is a message for the user.
pub fn parse_ticket(ticket: &str) -> std::result::Result<TicketInfo, String> {
    let ticket: ShareTicket = ticket.trim().parse().map_err(
        |error: connect_share::TicketError| {
            let mut message = error.to_string();
            if let Some(first) = message.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            message
        },
    )?;
    Ok(TicketInfo {
        endpoint_id: ticket.endpoint.id.to_string(),
        port: ticket.port,
    })
}

/// Validates a share's local address. `None` means it is usable.
pub fn share_address_error(address: &str) -> Option<String> {
    let address = address.trim();
    let lower = address.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(
            "Do not include http:// or https:// — use host:port only (e.g. 127.0.0.1:5173)."
                .to_string(),
        );
    }
    match TcpProxyData::from_host_port_str(address) {
        Ok(data) if data.host.is_empty() => {
            Some("Include a host, for example 127.0.0.1:5173.".to_string())
        }
        Ok(data) if data.port == 0 => Some("Use a port from 1 to 65535.".to_string()),
        Ok(_) => None,
        Err(error) => Some(format!(
            "Invalid address: {error}. Use host:port (e.g. 127.0.0.1:5173)."
        )),
    }
}

/// The name devices see for a share unless the user picks one: this
/// computer's name and the shared port, as `datumctl connect local` does.
pub fn default_share_name(address: &str) -> String {
    let port = address.trim().rsplit_once(':').map_or("", |(_, port)| port);
    let name = format!("{}:{port}", device_name());
    name.chars().take(MAX_NAME).collect()
}

/// This computer's name, looked up once: on macOS that runs `scutil`.
fn device_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(crate::tunnels::friendly_device_name)
}

/// Validates a user-chosen name against other tunnels' names.
pub fn name_error<'a>(name: &str, others: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Name can't be empty".to_string());
    }
    if name.chars().count() > MAX_NAME {
        return Some(format!("Name can be at most {MAX_NAME} characters"));
    }
    if name.chars().any(char::is_control) {
        return Some("Name can't contain control characters".to_string());
    }
    if others.into_iter().any(|other| other.eq_ignore_ascii_case(name)) {
        return Some(format!("Another local tunnel is named \"{name}\""));
    }
    None
}

/// Returns `base`, or "base (2)", "base (3)", … if `names` already use it.
fn unique_name<'a>(base: &str, names: impl IntoIterator<Item = &'a str>) -> String {
    let taken: HashSet<String> = names.into_iter().map(str::to_lowercase).collect();
    if !taken.contains(&base.to_lowercase()) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|name| !taken.contains(&name.to_lowercase()))
        .expect("an unused name")
}

#[derive(Serialize, Deserialize)]
struct SavedTunnel {
    id: String,
    name: String,
    name_from_user: bool,
    #[serde(flatten)]
    kind: LocalTunnelKind,
}

#[derive(Serialize, Deserialize)]
struct SavedFile {
    format: String,
    version: u32,
    tunnels: Vec<SavedTunnel>,
}

/// A running share or dial. Callbacks carry the token their session started
/// with and are dropped once it no longer matches, so a stopped or restarted
/// session cannot change a tunnel's state.
struct Session {
    token: u64,
    cancel: CancellationToken,
}

#[derive(Default)]
struct Inner {
    tunnels: Vec<LocalTunnel>,
    sessions: HashMap<String, Session>,
    next_token: u64,
}

impl Inner {
    fn get_mut(&mut self, id: &str) -> Option<&mut LocalTunnel> {
        self.tunnels.iter_mut().find(|t| t.id == id)
    }

    fn names_except(&self, id: &str) -> Vec<String> {
        self.tunnels
            .iter()
            .filter(|t| t.id != id)
            .map(|t| t.name.clone())
            .collect()
    }

    /// Invalidates the session's callbacks, then cancels it.
    fn end_session(&mut self, id: &str) {
        if let Some(session) = self.sessions.remove(id) {
            session.cancel.cancel();
        }
    }
}

/// Saved local tunnels and their running sessions. Cheap to clone.
#[derive(Clone)]
pub struct LocalTunnels {
    inner: Arc<Mutex<Inner>>,
    file: PathBuf,
    /// This computer's identity when it connects to a share, so a dropped
    /// connection can resume against the same sharer without pairing again.
    device_key: SecretKey,
    /// Sent to sharers when pairing.
    device_name: String,
    changed: watch::Sender<u64>,
}

impl fmt::Debug for LocalTunnels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalTunnels")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl LocalTunnels {
    const FILE: &str = "local_tunnels.json";
    const DEVICE_KEY_FILE: &str = "local_device_key";

    /// Loads saved tunnels, all stopped, and this computer's device key.
    pub async fn load(repo: &Repo) -> Result<Self> {
        let file = repo.path().join(Self::FILE);
        let saved = match tokio::fs::read(&file).await {
            Ok(data) => match serde_json::from_slice::<SavedFile>(&data) {
                Ok(saved) if saved.format == FILE_FORMAT && saved.version <= FILE_VERSION => {
                    saved.tunnels
                }
                Ok(_) | Err(_) => {
                    warn!(path = %file.display(), "local tunnels: ignoring unreadable file");
                    Vec::new()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err).std_context("reading local tunnels"),
        };
        let device_key = load_or_create_key(&repo.path().join(Self::DEVICE_KEY_FILE)).await?;
        let tunnels = saved
            .into_iter()
            .map(|s| LocalTunnel {
                id: s.id,
                name: s.name,
                name_from_user: s.name_from_user,
                kind: s.kind,
                status: LocalTunnelStatus::Stopped,
                path: None,
                devices: Vec::new(),
            })
            .collect();
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                tunnels,
                ..Default::default()
            })),
            file,
            device_key,
            device_name: device_name().to_string(),
            changed: watch::Sender::new(0),
        })
    }

    pub fn list(&self) -> Vec<LocalTunnel> {
        self.lock().tunnels.clone()
    }

    /// Changes whenever the list or any status changes.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Validates a name for tunnel `id`, or for a new tunnel when `id` is `None`.
    pub fn name_error(&self, name: &str, id: Option<&str>) -> Option<String> {
        let names = self.lock().names_except(id.unwrap_or_default());
        name_error(name, names.iter().map(String::as_str))
    }

    /// Port to request for a new connection: the shared port when it is not
    /// privileged and no active connection uses it, otherwise 0 so the
    /// system picks.
    pub fn default_port(&self, shared_port: u16) -> u16 {
        if shared_port >= 1024 && !self.used_ports().contains(&shared_port) {
            shared_port
        } else {
            0
        }
    }

    /// Ports held or requested by active connections.
    pub fn used_ports(&self) -> HashSet<u16> {
        self.lock()
            .tunnels
            .iter()
            .filter(|t| t.status.is_active())
            .flat_map(|t| {
                let requested = match &t.kind {
                    LocalTunnelKind::Connect { port, .. } if *port != 0 => Some(*port),
                    _ => None,
                };
                let bound = match &t.status {
                    LocalTunnelStatus::Open { local_addr } => local_addr
                        .parse::<SocketAddr>()
                        .ok()
                        .map(|addr| addr.port()),
                    _ => None,
                };
                requested.into_iter().chain(bound)
            })
            .collect()
    }

    /// Adds a share of `address` and starts it. An empty `name` uses
    /// [`default_share_name`]. Returns the new tunnel's id.
    pub fn add_share(&self, name: &str, address: &str) -> Result<String> {
        let address = address.trim();
        if let Some(error) = share_address_error(address) {
            n0_error::bail_any!("{error}");
        }
        let chosen = name.trim();
        let base = if chosen.is_empty() {
            default_share_name(address)
        } else {
            chosen.to_string()
        };
        let kind = LocalTunnelKind::Share {
            address: address.to_string(),
        };
        self.add(base, !chosen.is_empty(), kind)
    }

    /// Adds a connection to the share named by `ticket` and starts it. An
    /// empty `name` adopts the sharer's name once paired. `port` `None`
    /// uses [`Self::default_port`].
    pub fn add_connect(&self, ticket: &str, name: &str, port: Option<u16>) -> Result<String> {
        let ticket = extract_ticket(ticket).unwrap_or_else(|| ticket.trim().to_string());
        let info = parse_ticket(&ticket).map_err(|error| n0_error::anyerr!("{error}"))?;
        let port = port.unwrap_or_else(|| self.default_port(info.port));
        let chosen = name.trim();
        let base = if chosen.is_empty() {
            format!("Port {}", info.port)
        } else {
            chosen.to_string()
        };
        self.add(base, !chosen.is_empty(), LocalTunnelKind::Connect { ticket, port })
    }

    fn add(&self, base: String, name_from_user: bool, kind: LocalTunnelKind) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut inner = self.lock();
            if name_from_user {
                let names = inner.names_except("");
                if let Some(error) = name_error(&base, names.iter().map(String::as_str)) {
                    n0_error::bail_any!("{error}");
                }
            }
            let name = unique_name(&base, inner.tunnels.iter().map(|t| t.name.as_str()));
            inner.tunnels.push(LocalTunnel {
                id: id.clone(),
                name,
                name_from_user,
                kind,
                status: LocalTunnelStatus::Stopped,
                path: None,
                devices: Vec::new(),
            });
            self.save(&inner);
        }
        self.start(&id);
        Ok(id)
    }

    /// Starts tunnel `id`, or restarts it if it is running.
    pub fn start(&self, id: &str) {
        let mut inner = self.lock();
        inner.end_session(id);
        let token = inner.next_token;
        inner.next_token += 1;
        let cancel = CancellationToken::new();
        let Some(tunnel) = inner.get_mut(id) else {
            return;
        };
        tunnel.status = LocalTunnelStatus::Starting;
        tunnel.path = None;
        tunnel.devices.clear();
        let kind = tunnel.kind.clone();
        let name = tunnel.name.clone();
        inner.sessions.insert(
            id.to_string(),
            Session {
                token,
                cancel: cancel.clone(),
            },
        );
        drop(inner);
        self.notify();

        let this = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let result = match kind {
                LocalTunnelKind::Share { address } => this.run_share(&id, token, address, name, cancel).await,
                LocalTunnelKind::Connect { ticket, port } => {
                    this.run_connect(&id, token, ticket, port, cancel).await
                }
            };
            if let Err(error) = &result {
                warn!(%id, %error, "local tunnel stopped");
            }
            this.finish(&id, token, result.err());
        });
    }

    async fn run_share(
        &self,
        id: &str,
        token: u64,
        address: String,
        name: String,
        cancel: CancellationToken,
    ) -> std::result::Result<(), String> {
        let options = ShareOptions {
            target: address.into(),
            relay_urls: None,
            max_peers: MAX_PEERS,
            name: Some(name),
            udp_port: None,
            public_addrs: Vec::new(),
        };
        connect_share::share(options, cancel, |event| {
            self.update(id, token, |tunnel| match event {
                ShareEvent::Ready { ticket, target } => {
                    info!(%target, "local tunnel sharing");
                    tunnel.status = LocalTunnelStatus::Sharing {
                        ticket: ticket.to_string(),
                    };
                }
                ShareEvent::Paired { peer, name } => {
                    let endpoint_id = peer.to_string();
                    if !tunnel.devices.iter().any(|d| d.endpoint_id == endpoint_id) {
                        tunnel.devices.push(PairedDevice {
                            endpoint_id,
                            name,
                            path: None,
                        });
                    }
                }
                ShareEvent::Path { peer, path } => {
                    let endpoint_id = peer.to_string();
                    if let Some(device) = tunnel
                        .devices
                        .iter_mut()
                        .find(|d| d.endpoint_id == endpoint_id)
                    {
                        device.path = Some(path.into());
                    }
                }
            })
        })
        .await
        .map_err(|error| error.to_string())
    }

    async fn run_connect(
        &self,
        id: &str,
        token: u64,
        ticket: String,
        port: u16,
        cancel: CancellationToken,
    ) -> std::result::Result<(), String> {
        let ticket: ShareTicket = ticket.parse().map_err(|e: connect_share::TicketError| e.to_string())?;
        let options = DialOptions {
            ticket,
            bind: SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            secret_key: Some(self.device_key.clone()),
            name: Some(self.device_name.clone()),
            relay_urls: None,
        };
        connect_share::dial(options, cancel, |event| match event {
            DialEvent::Ready(ready) => {
                if let Some(name) = &ready.sharer_name {
                    self.offer_sharer_name(id, token, name);
                }
                self.update(id, token, |tunnel| {
                    tunnel.status = LocalTunnelStatus::Open {
                        local_addr: ready.local_addr.to_string(),
                    };
                });
            }
            DialEvent::Path(path) => self.update(id, token, |tunnel| {
                tunnel.path = Some(path.into());
            }),
        })
        .await
        .map_err(|error| error.to_string())
    }

    /// Records how a session ended, unless it was stopped or replaced.
    fn finish(&self, id: &str, token: u64, error: Option<String>) {
        let mut inner = self.lock();
        if inner.sessions.get(id).map(|s| s.token) != Some(token) {
            return;
        }
        inner.sessions.remove(id);
        if let Some(tunnel) = inner.get_mut(id) {
            tunnel.status = match error {
                Some(error) => LocalTunnelStatus::Failed(error),
                None => LocalTunnelStatus::Stopped,
            };
            tunnel.path = None;
        }
        drop(inner);
        self.notify();
    }

    pub fn stop(&self, id: &str) {
        let mut inner = self.lock();
        inner.end_session(id);
        let Some(tunnel) = inner.get_mut(id) else {
            return;
        };
        if tunnel.status.is_active() {
            tunnel.status = LocalTunnelStatus::Stopped;
        }
        tunnel.path = None;
        tunnel.devices.clear();
        drop(inner);
        self.notify();
    }

    /// Stops and forgets tunnel `id`.
    pub fn remove(&self, id: &str) {
        let mut inner = self.lock();
        inner.end_session(id);
        inner.tunnels.retain(|t| t.id != id);
        self.save(&inner);
        drop(inner);
        self.notify();
    }

    /// Renames tunnel `id`. Returns a message for the user on failure. A
    /// running share keeps sending its old name to devices until it restarts.
    pub fn rename(&self, id: &str, name: &str) -> std::result::Result<(), String> {
        let mut inner = self.lock();
        let names = inner.names_except(id);
        if let Some(error) = name_error(name, names.iter().map(String::as_str)) {
            return Err(error);
        }
        let Some(tunnel) = inner.get_mut(id) else {
            return Err("That tunnel no longer exists".to_string());
        };
        tunnel.name = name.trim().to_string();
        tunnel.name_from_user = true;
        self.save(&inner);
        drop(inner);
        self.notify();
        Ok(())
    }

    /// Stops every running tunnel, for example before the app quits.
    pub fn stop_all(&self) {
        let mut inner = self.lock();
        let ids: Vec<String> = inner.sessions.keys().cloned().collect();
        for id in ids {
            inner.end_session(&id);
        }
    }

    /// Adopts the sharer's name unless the user already chose one.
    fn offer_sharer_name(&self, id: &str, token: u64, sharer_name: &str) {
        let mut inner = self.lock();
        if inner.sessions.get(id).map(|s| s.token) != Some(token) {
            return;
        }
        let names = inner.names_except(id);
        let Some(tunnel) = inner.get_mut(id) else {
            return;
        };
        if tunnel.name_from_user {
            return;
        }
        let base: String = sharer_name.chars().take(MAX_NAME).collect();
        tunnel.name = unique_name(&base, names.iter().map(String::as_str));
        self.save(&inner);
        drop(inner);
        self.notify();
    }

    /// Applies `change` to tunnel `id` while session `token` is current.
    fn update(&self, id: &str, token: u64, change: impl FnOnce(&mut LocalTunnel)) {
        let mut inner = self.lock();
        if inner.sessions.get(id).map(|s| s.token) != Some(token) {
            return;
        }
        if let Some(tunnel) = inner.get_mut(id) {
            change(tunnel);
        }
        drop(inner);
        self.notify();
    }

    fn notify(&self) {
        self.changed.send_modify(|version| *version = version.wrapping_add(1));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes names, kinds, and tickets atomically. Tickets are bearer
    /// secrets, so the file is readable only by this user. A failed write
    /// keeps the in-memory list and is retried on the next change.
    fn save(&self, inner: &Inner) {
        let saved = SavedFile {
            format: FILE_FORMAT.to_string(),
            version: FILE_VERSION,
            tunnels: inner
                .tunnels
                .iter()
                .map(|t| SavedTunnel {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    name_from_user: t.name_from_user,
                    kind: t.kind.clone(),
                })
                .collect(),
        };
        let result = serde_json::to_vec_pretty(&saved)
            .map_err(std::io::Error::other)
            .and_then(|data| write_private(&self.file, &data));
        if let Err(error) = result {
            warn!(path = %self.file.display(), %error, "local tunnels: failed to save");
        }
    }
}

async fn load_or_create_key(path: &Path) -> Result<SecretKey> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            if let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return Ok(SecretKey::from_bytes(&bytes));
            }
            warn!(path = %path.display(), "local tunnels: replacing malformed device key");
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).std_context("reading local device key"),
    }
    let key = SecretKey::generate();
    write_private(path, &key.to_bytes()).std_context("writing local device key")?;
    Ok(key)
}

/// Writes `data` to `path` through a temporary file, so a crash mid-write
/// leaves the old copy, with permissions limited to this user.
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&tmp)
        .and_then(|mut file| file.write_all(data).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_ticket_from_command() {
        assert_eq!(
            extract_ticket("datumctl connect dial dcshare2abc234 --port 1"),
            Some("DCSHARE2ABC234".to_string())
        );
        assert_eq!(extract_ticket("DCSHARE2"), None);
        assert_eq!(extract_ticket("nothing here"), None);
    }

    #[test]
    fn rejects_bad_ticket() {
        assert!(parse_ticket("DCSHARE2AAAA").is_err());
    }

    #[test]
    fn validates_names() {
        assert!(name_error("api", ["API"]).is_some());
        assert!(name_error("  ", []).is_some());
        assert!(name_error("a\u{7}", []).is_some());
        assert!(name_error(&"x".repeat(MAX_NAME + 1), []).is_some());
        assert!(name_error("api", ["web"]).is_none());
    }

    #[test]
    fn makes_unique_names() {
        assert_eq!(unique_name("api", ["web"]), "api");
        assert_eq!(unique_name("api", ["API", "api (2)"]), "api (3)");
    }

    #[test]
    fn validates_share_addresses() {
        assert!(share_address_error("127.0.0.1:5173").is_none());
        assert!(share_address_error("localhost:3000").is_none());
        assert!(share_address_error("http://127.0.0.1:5173").is_some());
        assert!(share_address_error("127.0.0.1").is_some());
        assert!(share_address_error(":3000").is_some());
        assert!(share_address_error("127.0.0.1:0").is_some());
    }

    #[tokio::test]
    async fn saves_and_restores_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::open_or_create(dir.path()).await.unwrap();
        let tunnels = LocalTunnels::load(&repo).await.unwrap();
        let id = tunnels.add_share("api", "127.0.0.1:1").unwrap();
        tunnels.rename(&id, "renamed").unwrap();
        tunnels.stop_all();

        let restored = LocalTunnels::load(&repo).await.unwrap();
        let list = restored.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "renamed");
        assert_eq!(list[0].status, LocalTunnelStatus::Stopped);
        assert_eq!(
            list[0].kind,
            LocalTunnelKind::Share {
                address: "127.0.0.1:1".to_string()
            }
        );
        assert_eq!(tunnels.device_key.public(), restored.device_key.public());
    }

    #[tokio::test]
    async fn share_reaches_connect() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let service = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service_addr = service.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = service.accept().await {
                let _ = stream.write_all(b"hello").await;
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::open_or_create(dir.path()).await.unwrap();
        let tunnels = LocalTunnels::load(&repo).await.unwrap();
        let mut changes = tunnels.subscribe();
        let share = tunnels.add_share("svc", &service_addr.to_string()).unwrap();
        let ticket = wait_for(&tunnels, &mut changes, &share, |status| match status {
            LocalTunnelStatus::Sharing { ticket } => Some(ticket.clone()),
            _ => None,
        })
        .await;

        let connect = tunnels.add_connect(&ticket, "", Some(0)).unwrap();
        let local_addr = wait_for(&tunnels, &mut changes, &connect, |status| match status {
            LocalTunnelStatus::Open { local_addr } => Some(local_addr.clone()),
            _ => None,
        })
        .await;
        // The sharer's name replaces the default "Port N".
        let name = tunnels.list().into_iter().find(|t| t.id == connect).unwrap().name;
        assert_eq!(name, "svc (2)");

        let mut stream = tokio::net::TcpStream::connect(&local_addr).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello");
        tunnels.stop_all();
    }

    async fn wait_for<T>(
        tunnels: &LocalTunnels,
        changes: &mut watch::Receiver<u64>,
        id: &str,
        pick: impl Fn(&LocalTunnelStatus) -> Option<T>,
    ) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let tunnel = tunnels.list().into_iter().find(|t| t.id == id).unwrap();
                if let Some(value) = pick(&tunnel.status) {
                    return value;
                }
                if let LocalTunnelStatus::Failed(error) = &tunnel.status {
                    panic!("{id} failed: {error}");
                }
                changes.changed().await.unwrap();
            }
        })
        .await
        .expect("timed out")
    }
}
