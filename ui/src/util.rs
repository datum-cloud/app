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

/// The clipboard is kept open for the life of the app: on Linux, X11 and
/// Wayland clipboards are served by the process that set them.
static CLIPBOARD: std::sync::OnceLock<Result<std::sync::Mutex<arboard::Clipboard>, String>> =
    std::sync::OnceLock::new();

fn clipboard() -> Result<std::sync::MutexGuard<'static, arboard::Clipboard>, String> {
    let clipboard = CLIPBOARD
        .get_or_init(|| {
            arboard::Clipboard::new()
                .map(std::sync::Mutex::new)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)?;
    Ok(clipboard.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Copies `text` to the system clipboard.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    clipboard()?.set_text(text).map_err(|e| e.to_string())
}

/// Reads a ticket from the clipboard: a QR image decodes to its ticket
/// text, or the text itself holds one, possibly inside a pasted command.
/// The error is a message for the user when neither holds a ticket.
pub fn read_ticket_from_clipboard() -> Result<String, String> {
    let mut board = clipboard()?;
    if let Ok(image) = board.get_image() {
        if let Some(text) = decode_qr_text(&image) {
            if let Some(ticket) = lib::local_tunnels::extract_ticket(&text) {
                return Ok(ticket);
            }
            return Err("The QR code on the clipboard isn't a Datum ticket".to_string());
        }
    }
    match board.get_text() {
        Ok(text) => lib::local_tunnels::extract_ticket(&text)
            .ok_or_else(|| "No ticket on the clipboard".to_string()),
        Err(_) => Err("No ticket on the clipboard".to_string()),
    }
}

/// Decodes the first QR code in `image` to its text, or `None` when there
/// is none.
fn decode_qr_text(image: &arboard::ImageData) -> Option<String> {
    let width = image.width as u32;
    let height = image.height as u32;
    let bytes = image.bytes.as_ref();
    let luma = image::GrayImage::from_fn(width, height, |x, y| {
        let i = ((y * width + x) * 4) as usize;
        let value =
            (bytes[i] as u32 * 299 + bytes[i + 1] as u32 * 587 + bytes[i + 2] as u32 * 114) / 1000;
        image::Luma([value as u8])
    });
    let mut prepared = rqrr::PreparedImage::prepare(luma);
    prepared
        .detect_grids()
        .into_iter()
        .find_map(|grid| grid.decode().ok().map(|(_, text)| text))
}

/// Top offset for full-screen overlays so they clear the custom macOS title bar.
/// Windows/Linux use the native title bar (outside the webview), so no offset.
#[cfg(target_os = "macos")]
pub const OVERLAY_TITLEBAR_OFFSET: &str = "mt-[32px]";

#[cfg(not(target_os = "macos"))]
pub const OVERLAY_TITLEBAR_OFFSET: &str = "";
