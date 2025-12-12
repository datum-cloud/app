use dioxus_desktop::trayicon::Icon;

/// Load an icon from a PNG file
pub(crate) fn load_tray_icon_from_file(path: &str) -> Icon {
    let image = image::open(path)
        .expect("Failed to open icon file")
        .to_rgba8();

    let (width, height) = image.dimensions();
    let rgba = image.into_raw();

    Icon::from_rgba(rgba, width, height).expect("Failed to create icon from image")
}

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
