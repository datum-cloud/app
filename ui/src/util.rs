// Convert bytes to human-readable format
pub fn humanize_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    format!("{:.1} {}", size, UNITS[unit_idx])
}

pub fn tunnel_edge_portal_url(web_url: &str, project_id: &str, tunnel_id: &str) -> String {
    let base = web_url.trim_end_matches('/');
    format!("{base}/project/{project_id}/edge/{tunnel_id}/overview")
}
