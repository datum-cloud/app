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

pub fn project_quotas_portal_url(web_url: &str, project_id: &str) -> String {
    let base = web_url.trim_end_matches('/');
    format!("{base}/project/{project_id}/quotas")
}

/// Copies `text` to the system clipboard.
///
/// The clipboard is kept open for the life of the app: on Linux, X11 and
/// Wayland clipboards are served by the process that set them.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    static CLIPBOARD: std::sync::OnceLock<Result<std::sync::Mutex<arboard::Clipboard>, String>> =
        std::sync::OnceLock::new();
    let clipboard = CLIPBOARD
        .get_or_init(|| {
            arboard::Clipboard::new()
                .map(std::sync::Mutex::new)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)?;
    clipboard
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_text(text)
        .map_err(|e| e.to_string())
}

/// Top offset for full-screen overlays so they clear the custom macOS title bar.
/// Windows/Linux use the native title bar (outside the webview), so no offset.
#[cfg(target_os = "macos")]
pub const OVERLAY_TITLEBAR_OFFSET: &str = "mt-[32px]";

#[cfg(not(target_os = "macos"))]
pub const OVERLAY_TITLEBAR_OFFSET: &str = "";
