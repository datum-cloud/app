use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use iroh_proxy_utils::upstream::UpstreamMetrics;

use crate::TunnelSummary;

const HOUR: Duration = Duration::from_secs(3600);

/// Snapshot of a tunnel's recent traffic for tray menu display.
#[derive(Debug, Clone)]
pub struct TunnelActivityEntry {
    pub tunnel_id: String,
    pub label: String,
    pub online: bool,
    pub bytes_last_hour: u64,
    pub rate_per_s: u64,
    pub last_activity_at: Instant,
}

#[derive(Debug)]
struct TunnelSampleState {
    label: String,
    online: bool,
    last_send: u64,
    last_recv: u64,
    last_sample_at: Instant,
    last_activity_at: Option<Instant>,
    hourly_deltas: VecDeque<(Instant, u64)>,
    rate_per_s: u64,
}

#[derive(Debug, Default)]
pub struct TunnelActivityTracker {
    tunnels: HashMap<String, TunnelSampleState>,
}

impl TunnelActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sample cumulative proxy metrics against the current tunnel list.
    ///
    /// Metrics are keyed by local endpoint authority; two tunnels targeting the same
    /// host:port share counters.
    pub fn tick(&mut self, tunnels: &[TunnelSummary], metrics: &UpstreamMetrics) {
        let counters: HashMap<String, (u64, u64)> = tunnels
            .iter()
            .map(|tunnel| (tunnel.id.clone(), metrics_bytes_for_tunnel(metrics, tunnel)))
            .collect();
        self.tick_counters(tunnels, &counters);
    }

    /// Same as [`Self::tick`], but takes per-tunnel byte counters (for a remote agent).
    pub fn tick_counters(
        &mut self,
        tunnels: &[TunnelSummary],
        counters: &HashMap<String, (u64, u64)>,
    ) {
        let now = Instant::now();
        let active_ids: std::collections::HashSet<&str> =
            tunnels.iter().map(|t| t.id.as_str()).collect();
        self.tunnels
            .retain(|id, _| active_ids.contains(id.as_str()));

        for tunnel in tunnels {
            let Some(_authority) = tunnel.origin_authority() else {
                continue;
            };
            let (send, recv) = counters.get(&tunnel.id).copied().unwrap_or((0, 0));

            let entry =
                self.tunnels
                    .entry(tunnel.id.clone())
                    .or_insert_with(|| TunnelSampleState {
                        label: tunnel.label.clone(),
                        online: tunnel_is_online(tunnel),
                        last_send: send,
                        last_recv: recv,
                        last_sample_at: now,
                        last_activity_at: None,
                        hourly_deltas: VecDeque::new(),
                        rate_per_s: 0,
                    });

            entry.label = tunnel.label.clone();
            entry.online = tunnel_is_online(tunnel);

            if send < entry.last_send || recv < entry.last_recv {
                entry.last_send = send;
                entry.last_recv = recv;
                entry.last_sample_at = now;
                entry.rate_per_s = 0;
                continue;
            }

            let delta_send = send.saturating_sub(entry.last_send);
            let delta_recv = recv.saturating_sub(entry.last_recv);
            let delta_total = delta_send + delta_recv;

            if delta_total > 0 {
                entry.hourly_deltas.push_back((now, delta_total));
                entry.last_activity_at = Some(now);
            }

            while let Some(front) = entry.hourly_deltas.front() {
                if now.duration_since(front.0) > HOUR {
                    entry.hourly_deltas.pop_front();
                } else {
                    break;
                }
            }

            let dt = now
                .duration_since(entry.last_sample_at)
                .as_secs_f64()
                .max(0.001);
            entry.rate_per_s = if delta_total > 0 {
                (delta_total as f64 / dt) as u64
            } else {
                0
            };

            entry.last_send = send;
            entry.last_recv = recv;
            entry.last_sample_at = now;
        }
    }

    /// Active tunnels for the tray menu, most recently used first (up to `limit`).
    /// Includes tunnels with no recent traffic.
    pub fn recent_active(&self, limit: usize) -> Vec<TunnelActivityEntry> {
        let mut entries: Vec<TunnelActivityEntry> = self
            .tunnels
            .iter()
            .map(|(id, state)| {
                let bytes_last_hour: u64 = state.hourly_deltas.iter().map(|(_, b)| *b).sum();
                TunnelActivityEntry {
                    tunnel_id: id.clone(),
                    label: state.label.clone(),
                    online: state.online,
                    bytes_last_hour,
                    rate_per_s: state.rate_per_s,
                    last_activity_at: state.last_activity_at.unwrap_or(state.last_sample_at),
                }
            })
            .collect();
        entries.sort_by(|a, b| {
            let a_active = a.bytes_last_hour > 0 || a.rate_per_s > 0;
            let b_active = b.bytes_last_hour > 0 || b.rate_per_s > 0;
            match (a_active, b_active) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b.last_activity_at.cmp(&a.last_activity_at),
            }
        });
        entries.truncate(limit);
        entries
    }
}

pub fn metrics_bytes_for_tunnel(metrics: &UpstreamMetrics, tunnel: &TunnelSummary) -> (u64, u64) {
    let Some(authority) = tunnel.origin_authority() else {
        return (0, 0);
    };
    for auth in authority_lookup_variants(&authority) {
        if let Some(m) = metrics.get(&auth) {
            return (m.bytes_from_origin(), m.bytes_to_origin());
        }
    }
    (0, 0)
}

fn tunnel_is_online(tunnel: &TunnelSummary) -> bool {
    tunnel.enabled && tunnel.accepted && tunnel.programmed
}

fn authority_lookup_variants(
    authority: &iroh_proxy_utils::Authority,
) -> Vec<iroh_proxy_utils::Authority> {
    use iroh_proxy_utils::Authority;
    let mut variants = vec![authority.clone()];
    if authority.host == "localhost" {
        variants.push(Authority {
            host: "127.0.0.1".to_string(),
            port: authority.port,
        });
    } else if authority.host == "127.0.0.1" {
        variants.push(Authority {
            host: "localhost".to_string(),
            port: authority.port,
        });
    }
    variants
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_proxy_utils::Authority;
    use std::str::FromStr;

    fn tunnel(id: &str, label: &str, endpoint: &str) -> TunnelSummary {
        TunnelSummary {
            id: id.to_string(),
            label: label.to_string(),
            endpoint: endpoint.to_string(),
            hostnames: vec![],
            enabled: true,
            accepted: true,
            programmed: true,
        }
    }

    #[test]
    fn recent_active_includes_idle_tunnels() {
        let mut tracker = TunnelActivityTracker::new();
        let tunnels = vec![tunnel("t1", "app", "http://127.0.0.1:8080")];
        let metrics = UpstreamMetrics::default();
        tracker.tick(&tunnels, &metrics);
        let recent = tracker.recent_active(5);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].tunnel_id, "t1");
        assert_eq!(recent[0].label, "app");
        assert!(recent[0].online);
        assert_eq!(recent[0].rate_per_s, 0);
    }

    #[test]
    fn origin_authority_parses_endpoint() {
        let t = tunnel("t1", "app", "http://127.0.0.1:8080");
        assert!(t.origin_authority().is_some());
        let auth = t.origin_authority().unwrap();
        assert_eq!(Authority::from_str("127.0.0.1:8080").ok(), Some(auth));
    }
}
