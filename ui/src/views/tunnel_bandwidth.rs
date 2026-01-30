use chrono::{DateTime, Local};
use dioxus::prelude::*;
use lib::ProxyState;

use crate::{
    components::{Icon, IconSource},
    state::AppState,
    util::humanize_bytes,
    Route,
};
use super::{OpenEditTunnelDialog, TunnelCard};

#[derive(Debug, Clone, PartialEq)]
struct RatePoint {
    ts: DateTime<Local>,
    send_per_s: u64,
    recv_per_s: u64,
}

#[component]
pub fn TunnelBandwidth(id: String) -> Element {
    let nav = use_navigator();

    let mut loading = use_signal(|| true);
    let mut load_error = use_signal(|| Option::<String>::None);
    let mut proxy_loaded = use_signal(|| None::<ProxyState>);

    let mut title = use_signal(|| "".to_string());
    let mut codename = use_signal(|| "".to_string());

    let mut points = use_signal(Vec::<RatePoint>::new);
    let mut latest_send = use_signal(|| 0u64);
    let mut latest_recv = use_signal(|| 0u64);

    // Load proxy metadata and keep it in sync when state updates (e.g. after edit/save).
    use_future({
        let id = id.clone();
        move || {
            let id = id.clone();
            async move {
                let state = consume_context::<AppState>();
                let node = state.listen_node();
                let updated = node.state_updated();
                tokio::pin!(updated);

                loop {
                    if proxy_loaded().is_none() {
                        loading.set(true);
                    }
                    load_error.set(None);
                    let proxies = node.proxies();
                    loading.set(false);
                    match proxies.iter().find(|p| p.id() == &id) {
                        Some(proxy) => {
                            proxy_loaded.set(Some(proxy.clone()));
                            title.set(proxy.info.label().to_owned());
                            codename.set(proxy.id().to_owned());
                        }
                        None => {
                            load_error.set(Some("Tunnel not found".to_string()));
                        }
                    }
                    (&mut updated).await;
                    updated.set(node.state().updated());
                }
            }
        }
    });

    use_future(move || {
        let state = consume_context::<AppState>();
        async move {
            let mut metrics_sub = state.node().listen.metrics();

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

            while let Ok(metric) = metrics_sub.recv().await {
                let now = std::time::Instant::now();
                // First metric just initializes the baseline.
                let (Some(prev_send), Some(prev_recv)) = (last_sample_send, last_sample_recv)
                else {
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

    let mut on_delete = use_action(move |proxy: ProxyState| async move {
        let state = consume_context::<AppState>();
        debug!("on delete called: {}", proxy.id());
        state
            .listen_node()
            .remove_proxy(proxy.id())
            .await
            .inspect_err(|err| {
                tracing::warn!("delete tunnel failed: {err:#}");
            })?;
        n0_error::Ok(())
    });
    let mut open_edit_dialog = consume_context::<OpenEditTunnelDialog>();
    let proxy = proxy_loaded().expect("proxy loaded when not loading and no error");

    rsx! {
        div { id: "tunnel-bandwidth", class: "max-w-4xl mx-auto",
            // Back link
            button {
                class: "text-xs text-foreground flex items-center gap-1 mt-2 mb-7",
                onclick: move |_| {
                    let _ = nav.push(Route::ProxiesList {});
                },
                Icon {
                    source: IconSource::Named("chevron-down".into()),
                    class: "rotate-90 text-icon-select",
                    size: 10,
                }
                span { class: "underline", "Back to Tunnels List" }
            }

            TunnelCard {
                key: "{proxy.id()}",
                proxy: proxy.clone(),
                show_view_item: false,
                show_bandwidth: true,
                on_delete: move |proxy_to_delete: ProxyState| {
                    let nav = nav.clone();
                    let fut = on_delete.call(proxy_to_delete);
                    spawn(async move {
                        let _ = fut.await;
                        let _ = nav.push(Route::ProxiesList {});
                    });
                },
                on_edit: move |proxy_to_edit: ProxyState| {
                    open_edit_dialog.editing_proxy.set(Some(proxy_to_edit.clone()));
                    open_edit_dialog.dialog_open.set(true);
                },
            }

            // Panel
            div { class: "bg-white rounded-b-lg border border-t-tunnel-card-border border-app-border shadow-card p-5 sm:p-10",
                div { class: "border border-app-border rounded-lg p-6",
                    div { class: "flex items-center justify-start gap-5 mb-4",
                        div { class: "space-y-1.5 min-w-22",
                            div { class: "text-xs text-icon-select font-normal", "Send" }
                            div { class: "text-md font-medium text-foreground whitespace-nowrap leading-none ",
                                "{humanize_bytes(latest_send())}/s"
                            }
                        }
                        div { class: "space-y-1.5 min-w-22",
                            div { class: "text-xs text-icon-select font-normal", "Receive" }
                            div { class: "text-md font-medium text-foreground whitespace-nowrap leading-none ",
                                "{humanize_bytes(latest_recv())}/s"
                            }
                        }
                    }

                    div { class: "",
                        BandwidthChart { points: points() }
                    }
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
    let height = 400.0;
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
    let send_color = "#BF9595";
    let recv_color = "#4D6356";

    let y_ticks = 2;
    let mut y_labels = Vec::new();
    for i in 0..=y_ticks {
        let frac = i as f64 / y_ticks as f64;
        let y = padding_y + frac * h;
        let val = ((1.0 - frac) * max_v) as u64;
        y_labels.push((humanize_bytes(val), y));
    }

    rsx! {
        div { class: "w-full overflow-hidden h-[45vh] min-h-[200px] sm:h-[400px]",
            svg {
                width: "100%",
                height: "100%",
                view_box: "0 0 {width} {height}",
                defs {
                    linearGradient {
                        id: "sendFill",
                        x1: "0",
                        y1: "0",
                        x2: "0",
                        y2: "1",
                        stop {
                            offset: "0%",
                            stop_color: "{send_color}",
                            stop_opacity: "0.22",
                        }
                        stop {
                            offset: "100%",
                            stop_color: "{send_color}",
                            stop_opacity: "0.0",
                        }
                    }
                    linearGradient {
                        id: "recvFill",
                        x1: "0",
                        y1: "0",
                        x2: "0",
                        y2: "1",
                        stop {
                            offset: "0%",
                            stop_color: "{recv_color}",
                            stop_opacity: "0.24",
                        }
                        stop {
                            offset: "100%",
                            stop_color: "{recv_color}",
                            stop_opacity: "0.0",
                        }
                    }
                }
                // chart bg
                rect {
                    x: "0",
                    y: "0",
                    width: "{width}",
                    height: "{height}",
                    rx: "14",
                    fill: "transparent",
                    stroke: "none",
                }

                // grid + y labels
                for (label , y) in y_labels {
                    line {
                        x1: "{padding_x}",
                        y1: "{y}",
                        x2: "{width - padding_x}",
                        y2: "{y}",
                        stroke: "#eceee9",
                        stroke_width: "1.5",
                        stroke_dasharray: "10 10",
                    }
                    text {
                        x: "{padding_x - 12.0}",
                        y: "{y + 4.0}",
                        text_anchor: "end",
                        font_size: "17",
                        fill: "#94a3b8",
                        "{label}"
                    }
                }

                g { transform: "translate({padding_x}, {padding_y})",
                    // area fills (draw first, then lines on top)
                    path {
                        d: "{recv_area}",
                        fill: "url(#recvFill)",
                        stroke: "none",
                    }
                    path {
                        d: "{send_area}",
                        fill: "url(#sendFill)",
                        stroke: "none",
                    }
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
        }
    }
}
