use chrono::{DateTime, Local};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::{
    state::AppState,
    util::humanize_bytes,
    Route,
};

#[derive(Debug, Clone, PartialEq)]
struct RatePoint {
    ts: DateTime<Local>,
    send_per_s: u64,
    recv_per_s: u64,
}

#[component]
pub fn TunnelBandwidth(id: String) -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let state_for_load = state.clone();
    let state_for_metrics = state.clone();
    let id_for_load = id.clone();
    let id_for_back = id.clone();

    let mut loading = use_signal(|| true);
    let mut load_error = use_signal(|| Option::<String>::None);

    let mut title = use_signal(|| "".to_string());
    let mut codename = use_signal(|| "".to_string());

    let mut points = use_signal(Vec::<RatePoint>::new);
    let mut latest_send = use_signal(|| 0u64);
    let mut latest_recv = use_signal(|| 0u64);

    // Load proxy metadata (for display)
    use_future(move || {
        let state = state_for_load.clone();
        let id = id_for_load.clone();
        let mut loading = loading.clone();
        let mut load_error = load_error.clone();
        let mut title = title.clone();
        let mut codename = codename.clone();
        async move {
            loading.set(true);
            load_error.set(None);

            let uuid = match Uuid::parse_str(&id) {
                Ok(u) => u,
                Err(_) => {
                    load_error.set(Some("Invalid tunnel id".to_string()));
                    loading.set(false);
                    return;
                }
            };

            let proxies = match state.node().proxies().await {
                Ok(p) => p,
                Err(err) => {
                    load_error.set(Some(err.to_string()));
                    loading.set(false);
                    return;
                }
            };

            let Some(proxy) = proxies.iter().find(|p| p.id == uuid) else {
                load_error.set(Some("Tunnel not found".to_string()));
                loading.set(false);
                return;
            };

            let display = proxy
                .label
                .clone()
                .unwrap_or_else(|| proxy.codename.clone().split('-').map(|w| {
                    let mut ch = w.chars();
                    match ch.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + ch.as_str(),
                    }
                }).collect::<Vec<_>>().join(" "));

            title.set(display);
            codename.set(proxy.codename.clone());
            loading.set(false);
        }
    });

    // Stream iroh metrics and compute bytes/sec (device-level iroh bandwidth).
    use_future(move || {
        let state = state_for_metrics.clone();
        let mut points = points.clone();
        let mut latest_send = latest_send.clone();
        let mut latest_recv = latest_recv.clone();
        async move {
            let mut rx = match state.node().metrics().await {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::warn!("bandwidth: couldn't subscribe to metrics: {err}");
                    return;
                }
            };

            // We compute bytes/sec over the interval between *plotted* samples (not per-metric tick),
            // otherwise bursty traffic can happen between samples and we'd plot a flatline.
            let mut last_sample_at = std::time::Instant::now();
            let mut last_sample_send = None::<u64>;
            let mut last_sample_recv = None::<u64>;
            // Exponential moving average to make the chart look like a monitoring view.
            // (Traffic through a proxy is often bursty; EMA yields a steadier signal.)
            let mut ema_send: f64 = 0.0;
            let mut ema_recv: f64 = 0.0;
            // Stronger smoothing so bursty proxy traffic reads like a monitoring view.
            // higher = more responsive, lower = smoother
            let alpha: f64 = 0.12;

            loop {
                let metric = match rx.recv().await {
                    Ok(m) => m,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };

                let now = std::time::Instant::now();
                // First metric just initializes the baseline.
                let (Some(prev_send), Some(prev_recv)) = (last_sample_send, last_sample_recv) else {
                    last_sample_send = Some(metric.send);
                    last_sample_recv = Some(metric.recv);
                    last_sample_at = now;
                    continue;
                };

                // Downsample to ~2Hz so the UI stays smooth.
                let dt = now.duration_since(last_sample_at);
                if dt < std::time::Duration::from_millis(650) {
                    continue;
                }

                let dt_s = dt.as_secs_f64().max(0.001);
                let raw_send = (metric.send.saturating_sub(prev_send)) as f64 / dt_s;
                let raw_recv = (metric.recv.saturating_sub(prev_recv)) as f64 / dt_s;

                // EMA update
                ema_send = if ema_send == 0.0 {
                    raw_send
                } else {
                    ema_send * (1.0 - alpha) + raw_send * alpha
                };
                ema_recv = if ema_recv == 0.0 {
                    raw_recv
                } else {
                    ema_recv * (1.0 - alpha) + raw_recv * alpha
                };

                let send_per_s = ema_send.max(0.0) as u64;
                let recv_per_s = ema_recv.max(0.0) as u64;

                latest_send.set(send_per_s);
                latest_recv.set(recv_per_s);

                let mut next = points();
                next.push(RatePoint {
                    ts: Local::now(),
                    send_per_s,
                    recv_per_s,
                });
                // Keep last ~60s at 2Hz
                if next.len() > 120 {
                    let drain = next.len() - 120;
                    next.drain(0..drain);
                }
                points.set(next);

                last_sample_send = Some(metric.send);
                last_sample_recv = Some(metric.recv);
                last_sample_at = now;
            }
        }
    });

    if loading() {
        return rsx! {
            div { class: "max-w-4xl mx-auto",
                div { class: "rounded-2xl border border-[#e3e7ee] bg-white/70 p-8",
                    div { class: "text-sm text-slate-600", "Loading bandwidth…" }
                }
            }
        };
    }

    if let Some(err) = load_error() {
        return rsx! {
            div { class: "max-w-4xl mx-auto",
                div { class: "rounded-2xl border border-red-200 bg-red-50 text-red-800 p-6",
                    div { class: "text-sm font-semibold", "Couldn't load bandwidth" }
                    div { class: "text-sm mt-1 break-words", "{err}" }
                }
            }
        };
    }

    rsx! {
        div { id: "tunnel-bandwidth", class: "max-w-4xl mx-auto px-1",
            // Header
            div { class: "flex items-center gap-4 mb-6",
                button {
                    class: "w-10 h-10 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center text-slate-600 hover:text-slate-800 hover:bg-gray-50 shadow-sm cursor-pointer",
                    onclick: move |_| {
                        let _ = nav.push(Route::EditProxy { id: id_for_back.clone() });
                    },
                    "←"
                }
                div { class: "flex flex-col",
                    div { class: "text-2xl font-semibold text-slate-900", "Bandwidth" }
                    div { class: "text-sm text-slate-600", "{title()} · {codename()}" }
                }
            }

            // Panel
            div { class: "bg-white rounded-2xl border border-[#e3e7ee] shadow-[0_10px_28px_rgba(17,24,39,0.10)] p-8 sm:p-10",
                div { class: "grid grid-cols-1 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.2fr)] gap-6 items-start",
                    div { class: "space-y-2 min-w-0",
                        div { class: "text-sm font-medium text-slate-700", "Send" }
                        div { class: "text-2xl font-semibold text-slate-900 whitespace-nowrap tabular-nums leading-none",
                            "{humanize_bytes(latest_send())}/s"
                        }
                    }
                    div { class: "space-y-2 min-w-0",
                        div { class: "text-sm font-medium text-slate-700", "Receive" }
                        div { class: "text-2xl font-semibold text-slate-900 whitespace-nowrap tabular-nums leading-none",
                            "{humanize_bytes(latest_recv())}/s"
                        }
                    }
                    div { class: "text-xs text-slate-500 min-w-0",
                        "Note: this currently shows device-level iroh bandwidth (all tunnels + n0des), not strictly per-tunnel."
                    }
                }

                div { class: "mt-8",
                    BandwidthChart { points: points() }
                }
            }
        }
    }
}

#[component]
fn BandwidthChart(points: Vec<RatePoint>) -> Element {
    // Render with a fixed viewBox but scale to the container width to avoid overflow.
    // Give the left axis more room so labels don't get clipped.
    let width = 860.0;
    let height = 260.0;
    let padding_x = 52.0;
    let padding_y = 22.0;
    let w = width - padding_x * 2.0;
    let h = height - padding_y * 2.0;

    let max_v = points
        .iter()
        .map(|p| p.send_per_s.max(p.recv_per_s))
        .max()
        .unwrap_or(0)
        .max(1) as f64;

    #[derive(Clone, Copy)]
    struct Pt {
        x: f64,
        y: f64,
    }

    fn smooth_path(points: &[Pt]) -> String {
        // Catmull–Rom spline converted to cubic Bézier segments.
        // This yields a smooth "monitoring style" curve instead of sharp corners.
        if points.is_empty() {
            return String::new();
        }
        if points.len() == 1 {
            let p = points[0];
            return format!("M {} {}", p.x, p.y);
        }

        let mut d = String::new();
        d.push_str(&format!("M {} {}", points[0].x, points[0].y));

        // Tension factor: 1.0 is standard Catmull-Rom; smaller is tighter/less overshoot.
        let t = 0.85_f64;

        for i in 0..(points.len() - 1) {
            let p0 = if i == 0 { points[i] } else { points[i - 1] };
            let p1 = points[i];
            let p2 = points[i + 1];
            let p3 = if i + 2 < points.len() {
                points[i + 2]
            } else {
                points[i + 1]
            };

            // Convert Catmull-Rom to Bézier control points.
            let cp1 = Pt {
                x: p1.x + (p2.x - p0.x) / 6.0 * t,
                y: p1.y + (p2.y - p0.y) / 6.0 * t,
            };
            let cp2 = Pt {
                x: p2.x - (p3.x - p1.x) / 6.0 * t,
                y: p2.y - (p3.y - p1.y) / 6.0 * t,
            };

            d.push_str(&format!(
                " C {} {}, {} {}, {} {}",
                cp1.x, cp1.y, cp2.x, cp2.y, p2.x, p2.y
            ));
        }

        d
    }

    let mk_paths = |get: fn(&RatePoint) -> u64| -> (String, String) {
        if points.is_empty() {
            return (String::new(), String::new());
        }
        let pts: Vec<Pt> = points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let x = (i as f64 / (points.len().saturating_sub(1).max(1) as f64)) * w;
                let v = get(p) as f64;
                let y = h - (v / max_v * h);
                Pt { x, y }
            })
            .collect();

        let line = smooth_path(&pts);
        let first = pts.first().copied().unwrap_or(Pt { x: 0.0, y: h });
        let last = pts.last().copied().unwrap_or(Pt { x: w, y: h });
        let area = if line.is_empty() {
            String::new()
        } else {
            // Close the curve to the baseline to create an area fill under the line.
            format!("{line} L {} {} L {} {} Z", last.x, h, first.x, h)
        };

        (line, area)
    };

    let (send_path, send_area) = mk_paths(|p| p.send_per_s);
    let (recv_path, recv_area) = mk_paths(|p| p.recv_per_s);

    // Higher-contrast palette (still muted/brand-friendly).
    // Send: deep slate. Receive: saturated teal.
    let send_color = "#334155"; // slate-700
    let recv_color = "#0f766e"; // teal-700

    let y_ticks = 4;
    let mut y_labels = Vec::new();
    for i in 0..=y_ticks {
        let frac = i as f64 / y_ticks as f64;
        let y = padding_y + frac * h;
        let val = ((1.0 - frac) * max_v) as u64;
        y_labels.push((humanize_bytes(val), y));
    }

    rsx! {
        div { class: "w-full overflow-hidden",
            svg {
                width: "100%",
                height: "{height}",
                view_box: "0 0 {width} {height}",
                defs {
                    linearGradient { id: "sendFill", x1: "0", y1: "0", x2: "0", y2: "1",
                        stop { offset: "0%", stop_color: "{send_color}", stop_opacity: "0.22" }
                        stop { offset: "100%", stop_color: "{send_color}", stop_opacity: "0.0" }
                    }
                    linearGradient { id: "recvFill", x1: "0", y1: "0", x2: "0", y2: "1",
                        stop { offset: "0%", stop_color: "{recv_color}", stop_opacity: "0.24" }
                        stop { offset: "100%", stop_color: "{recv_color}", stop_opacity: "0.0" }
                    }
                }
                // chart bg
                rect { x: "0", y: "0", width: "{width}", height: "{height}", rx: "14", fill: "#fbfbf9", stroke: "#eceee9" }

                // grid + y labels
                for (label, y) in y_labels {
                    line { x1: "{padding_x}", y1: "{y}", x2: "{width - padding_x}", y2: "{y}", stroke: "#eceee9" }
                    text {
                        x: "{padding_x - 12.0}",
                        y: "{y + 4.0}",
                        text_anchor: "end",
                        font_size: "11",
                        fill: "#94a3b8",
                        "{label}"
                    }
                }

                g { transform: "translate({padding_x}, {padding_y})",
                    // area fills (draw first, then lines on top)
                    path { d: "{recv_area}", fill: "url(#recvFill)", stroke: "none" }
                    path { d: "{send_area}", fill: "url(#sendFill)", stroke: "none" }
                    // receive (green)
                    path {
                        d: "{recv_path}",
                        fill: "none",
                        stroke: "{recv_color}",
                        stroke_width: "2.2",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                    }
                    // send (slate)
                    path {
                        d: "{send_path}",
                        fill: "none",
                        stroke: "{send_color}",
                        stroke_width: "2.2",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                    }
                }
            }
            // legend
            div { class: "mt-4 flex items-center gap-3 text-xs",
                // Send pill
                div {
                    class: "inline-flex items-center justify-center gap-2 rounded-xl border px-3 py-2 min-w-[96px] select-none",
                    style: "border-color: {send_color}33; background-color: {send_color}12; color: {send_color};",
                    span { class: "inline-block w-3 h-0.5 rounded-full", style: "background-color: {send_color};" }
                    span { class: "font-medium leading-none text-center", "Send" }
                }
                // Receive pill
                div {
                    class: "inline-flex items-center justify-center gap-2 rounded-xl border px-3 py-2 min-w-[96px] select-none",
                    style: "border-color: {recv_color}33; background-color: {recv_color}12; color: {recv_color};",
                    span { class: "inline-block w-3 h-0.5 rounded-full", style: "background-color: {recv_color};" }
                    span { class: "font-medium leading-none text-center", "Receive" }
                }
            }
        }
    }
}

