//! Shared icon component. Icons can be inline SVGs (IconKind enum) or loaded from
//! `assets/icons/*.svg` by name so they scale with `currentColor` and no extra network requests.

use dioxus::prelude::*;

/// Icons embedded in code (no file).
#[derive(Clone, Copy, PartialEq)]
pub enum IconKind {
    ExternalLink,
    LoaderCircle,
}

/// Icons loaded from `ui/assets/icons/<name>.svg`. Use `currentColor` in the SVG for styling.
#[derive(Clone, PartialEq)]
pub enum IconSource {
    #[allow(unused)]
    Kind(IconKind),
    /// Name of the SVG file in assets/icons (without .svg), e.g. "plus", "check".
    Named(String),
}

#[derive(Clone, PartialEq, Props)]
pub struct IconProps {
    pub source: IconSource,
    #[props(default = 20)]
    pub size: u32,
    #[props(default)]
    pub class: Option<String>,
}

/// Look up SVG content from assets/icons by name. Add new icons by placing a .svg file
/// in `ui/assets/icons/` and adding a match arm here. Use `currentColor` for stroke/fill.
fn svg_content_for(name: &str) -> Option<&'static str> {
    match name {
        "loader-circle" => Some(include_str!("../../assets/icons/loader-circle.svg")),
        "external-link" => Some(include_str!("../../assets/icons/external-link.svg")),
        "plus" => Some(include_str!("../../assets/icons/plus.svg")),
        "chevron-down" => Some(include_str!("../../assets/icons/chevron-down.svg")),
        "settings" => Some(include_str!("../../assets/icons/settings.svg")),
        "users" => Some(include_str!("../../assets/icons/users.svg")),
        "book-open" => Some(include_str!("../../assets/icons/book-open.svg")),
        "ellipsis" => Some(include_str!("../../assets/icons/ellipsis.svg")),
        "globe" => Some(include_str!("../../assets/icons/globe.svg")),
        "down-right-arrow" => Some(include_str!("../../assets/icons/down-right-arrow.svg")),
        "power-cable" => Some(include_str!("../../assets/icons/power-cable.svg")),
        "search" => Some(include_str!("../../assets/icons/search.svg")),
        _ => None,
    }
}

/// Rewrites SVG so all stroke and fill (except fill="none") use currentColor, so icons inherit CSS color.
fn force_current_color(svg: &str) -> String {
    let mut out = svg.to_string();
    // Replace every stroke="..." with stroke="currentColor"
    let stroke_attr = "stroke=\"";
    let mut i = 0;
    while let Some(start) = out[i..].find(stroke_attr) {
        let value_start = i + start + stroke_attr.len();
        if let Some(end) = out[value_start..].find('"') {
            let value_end = value_start + end;
            out.replace_range(value_start..value_end, "currentColor");
            i = value_end + 1;
        } else {
            break;
        }
    }
    // Replace every fill="..." with fill="currentColor" except fill="none"
    let fill_attr = "fill=\"";
    let mut i = 0;
    while let Some(start) = out[i..].find(fill_attr) {
        let value_start = i + start + fill_attr.len();
        if let Some(end) = out[value_start..].find('"') {
            let value_end = value_start + end;
            let value = &out[value_start..value_end];
            if value != "none" {
                out.replace_range(value_start..value_end, "currentColor");
            }
            i = value_end + 1;
        } else {
            break;
        }
    }
    out
}

/// Injects width/100% height/100% and display:block into the first <svg tag so the icon fills its container and doesn't shift from baseline.
fn svg_with_fill(svg: &str) -> String {
    let with_color = force_current_color(svg);
    with_color.replacen(
        "<svg",
        r#"<svg width="100%" height="100%" style="display:block;vertical-align:middle""#,
        1,
    )
}

#[component]
pub fn Icon(props: IconProps) -> Element {
    let size = props.size;
    let class = props.class.as_deref().unwrap_or("");
    let base = "shrink-0 block box-border align-self-center";
    let mut class = if class.is_empty() {
        base.to_string()
    } else {
        format!("{base} {class}")
    };

    match &props.source {
        IconSource::Named(name) => {
            if name.as_str() == "loader-circle" {
                class = format!("{class} animate-spin");
            }
            if let Some(svg) = svg_content_for(name) {
                let filled = svg_with_fill(svg);
                return rsx! {
                    span {
                        class: "{class}",
                        style: "width:{size}px;height:{size}px;min-width:{size}px;min-height:{size}px;",
                        dangerous_inner_html: "{filled}",
                    }
                };
            }
            // Fallback: unknown name, render nothing or a placeholder
            return rsx! {
                span { class: "{class}", width: "{size}", height: "{size}" }
                // Fallback: unknown name, render nothing or a placeholder
            };
        }
        IconSource::Kind(_kind) => {}
    }

    let kind = match &props.source {
        IconSource::Kind(k) => k,
        _ => return rsx! {},
    };

    rsx! {
        if *kind == IconKind::ExternalLink {
            svg {
                width: "{size}",
                height: "{size}",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "2",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                class: "{class}",
                path { d: "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" }
                polyline { points: "15 3 21 3 21 9" }
                line {
                    x1: "10",
                    y1: "14",
                    x2: "21",
                    y2: "3",
                }
            }
        } else if *kind == IconKind::LoaderCircle {
            svg {
                width: "{size}",
                height: "{size}",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "2",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                class: "{class}",
                path { d: "M12 21a9 9 0 1 1 0-18 9 9 0 0 1 0 18z" }
                path { d: "M9 10a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H10a1 1 0 0 1-1-1v-4z" }
            }
        }
    }
}
