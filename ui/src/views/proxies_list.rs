use dioxus::prelude::*;
use lib::{TcpProxy, DATUM_CONNECT_GATEWAY_DOMAIN_NAME};
use dioxus::events::MouseEvent;

use crate::{
    state::AppState,
    Route,
};

#[component]
pub fn TempProxies() -> Element {
    let mut proxies = use_signal(Vec::<TcpProxy>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let state = consume_context::<AppState>();

    // Important: do async mutations from this parent component scope.
    // If we spawn from inside `TunnelCard` and then optimistically remove the card,
    // Dioxus will drop that scope and cancel the task before it runs.
    let on_delete = {
        let state_outer = state.clone();
        let proxies_sig_outer = proxies.clone();
        move |proxy: TcpProxy| {
            let state = state_outer.clone();
            let mut proxies_sig = proxies_sig_outer.clone();
            let proxy_id = proxy.id;

            // Optimistic UI: remove immediately, but re-sync with disk after the delete attempt.
            let prev = proxies_sig();
            let mut next = prev.clone();
            next.retain(|p| p.id != proxy_id);
            proxies_sig.set(next);

            spawn(async move {
                if let Err(err) = state.node().delete_proxy(&proxy).await {
                    tracing::warn!("delete tunnel failed: {err}");
                }
                match state.node().proxies().await {
                    Ok(lst) => proxies_sig.set(lst),
                    Err(err) => {
                        tracing::warn!("refresh tunnels after delete failed: {err}");
                        proxies_sig.set(prev);
                    }
                }
            });
        }
    };
    use_future(move || {
        let state = state.clone();
        let mut loading = loading.clone();
        let mut error = error.clone();
        let mut proxies = proxies.clone();
        async move {
            match state.node().proxies().await {
                Ok(lst) => proxies.set(lst),
                Err(err) => error.set(Some(err.to_string())),
            }
            loading.set(false);
        }
    });

    rsx! {
        div { class: "max-w-5xl mx-auto",
            if loading() {
                div { class: "space-y-4",
                    div { class: "h-20 rounded-2xl bg-white/70 border border-[#e3e7ee]" }
                    div { class: "h-20 rounded-2xl bg-white/70 border border-[#e3e7ee]" }
                    div { class: "h-20 rounded-2xl bg-white/70 border border-[#e3e7ee]" }
                }
            } else if let Some(err) = error() {
                div { class: "rounded-2xl border border-red-200 bg-red-50 text-red-800 p-5",
                    div { class: "text-sm font-semibold", "Couldn't load tunnels" }
                    div { class: "text-sm mt-1 break-words", "{err}" }
                }
            } else if proxies().is_empty() {
                div { class: "rounded-2xl border border-[#e3e7ee] bg-white/70 p-10 text-center",
                    div { class: "text-base font-semibold text-slate-900", "No tunnels yet" }
                    div { class: "text-sm text-slate-600 mt-2",
                        "Click ",
                        span { class: "font-medium", "\"Add tunnel\"" },
                        " in the left sidebar to create one."
                    }
                }
            } else {
                div { class: "space-y-5",
                    for (idx, proxy) in proxies().into_iter().enumerate() {
                        TunnelCard { proxy, proxies, show_wave: idx == 0, on_delete: on_delete.clone() }
                    }
                }
            }
        }
    }
}

// #[component]
// fn ProxyConnectionItem(conn: ConnectionInfo, connections: Signal<Vec<ConnectionInfo>>) -> Element {
//     let conn_2 = conn.clone();
//     rsx! {
//         div {
//             div {
//                 class: "flex mt-8",
//                 h3 {
//                     class: "text-xl flex-grow",
//                     "{conn.codename}"
//                 }
//                 CloseButton{
//                     onclick: move |_| {
//                         let conn_2 = proxy_2.clone();
//                         async move {
//                             let state = consume_context::<AppState>();
//                             let node = state.node();
//                             // TODO(b5) - remove unwrap
//                             // node.disconnect(&conn_2).await.unwrap();

//                             // refresh list of connections
//                             // let conns = node.connections().await;
//                             // connections.set(conns);
//                         }
//                     },
//                 }
//             }
//             Subhead { text: "{conn_2.addr}" }
//             // if let Some(ticket) = &proxy_2.ticket() {
//             //     p {
//             //         class: "text-sm break-all max-w-2/3 mt-1",
//             //         "{ticket}"
//             //     }
//             // }
//         }
//     }
// }

#[component]
fn TunnelCard(
    proxy: TcpProxy,
    proxies: Signal<Vec<TcpProxy>>,
    show_wave: bool,
    on_delete: EventHandler<TcpProxy>,
) -> Element {
    let proxy_2 = proxy.clone();
    let proxy_id = proxy.id;
    let mut menu_open = use_signal(|| false);
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let nav_card = nav.clone();
    let nav_details = nav.clone();
    let state_toggle = state.clone();
    let title = proxy
        .label
        .clone()
        .unwrap_or_else(|| codename_to_title(&proxy.codename));

    let domain = format!("{}.{}", proxy.codename, DATUM_CONNECT_GATEWAY_DOMAIN_NAME);
    let local_url = format!("http://{}:{}", proxy.host, proxy.port);
    // Always use codename for the datum:// URL (label is just display text)
    let datum_url = format!("datum://{}", proxy.codename);

    let enabled = proxy.enabled;

    rsx! {
        div {
            // darker shadow + hover lift
            class: "bg-white rounded-xl border border-[#e3e7ee] shadow-[0_10px_28px_rgba(17,24,39,0.10)] hover:shadow-[0_14px_34px_rgba(17,24,39,0.14)] transition-shadow overflow-hidden cursor-pointer",
            onclick: move |_| {
                // Clicking the card opens details (unless the kebab menu is open)
                if menu_open() {
                    return;
                }
                nav_card.push(Route::EditProxy { id: proxy_id.to_string() });
            },
            div { class: "p-6",
                // header row: title + toggle
                div { class: "flex items-start justify-between gap-6",
                    h2 { class: "text-xl font-semibold tracking-tight text-slate-900", "{title}" }
                    Toggle {
                        enabled,
                        on_toggle: move |next| {
                            let state = state_toggle.clone();
                            spawn(async move {
                                state
                                    .node()
                                    .set_proxy_enabled(proxy_2.id, next)
                                    .await
                                    .unwrap();
                                let lst = state.node().proxies().await.unwrap();
                                proxies.set(lst);
                            });
                        }
                    }
                }

                // divider under the header (Figma-style)
                div { class: "mt-5 border-t border-[#eceee9]" }

                // body: rows + kebab aligned to the right like Figma
                div { class: "mt-5 flex items-start justify-between gap-6",
                    div { class: "space-y-4",
                        div { class: "flex items-center gap-5",
                            GlobeIcon {}
                            span { class: "text-base font-medium text-slate-800", "{domain}" }
                        }
                        div { class: "flex items-center gap-5",
                            ArrowIcon {}
                            span { class: "text-base text-slate-700", "{local_url}" }
                        }
                        div { class: "flex items-center gap-5",
                            PlugIcon {}
                            span { class: "text-base text-slate-700", "{datum_url}" }
                        }
                    }
                    div { class: "relative",
                        button {
                            class: "w-10 h-10 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center text-slate-500 hover:text-slate-700 hover:bg-gray-50 shadow-sm cursor-pointer",
                            onclick: move |evt: MouseEvent| {
                                evt.stop_propagation();
                                menu_open.set(!menu_open());
                            },
                            "⋯"
                        }

                        if menu_open() {
                            // Full-screen click-catcher so any click outside closes the menu.
                            // This also prevents the card click handler from triggering.
                            div {
                                class: "fixed inset-0 z-40",
                                onclick: move |evt: MouseEvent| {
                                    evt.stop_propagation();
                                    menu_open.set(false);
                                }
                            }
                            div {
                                class: "absolute right-0 mt-2 w-44 rounded-xl border border-[#dfe3ea] bg-white shadow-[0_12px_30px_rgba(17,24,39,0.14)] overflow-hidden z-50",
                                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                                button {
                                    class: "w-full text-left px-4 py-3 text-sm text-slate-800 hover:bg-gray-50",
                                    onclick: move |evt: MouseEvent| {
                                        evt.stop_propagation();
                                        menu_open.set(false);
                                        nav_details.push(Route::EditProxy { id: proxy_id.to_string() });
                                    },
                                    "Details"
                                }
                                button {
                                    class: "w-full text-left px-4 py-3 text-sm text-slate-800 hover:bg-gray-50",
                                    onclick: move |evt: MouseEvent| {
                                        evt.stop_propagation();
                                        menu_open.set(false);
                                        nav_details.push(Route::TunnelBandwidth { id: proxy_id.to_string() });
                                    },
                                    "Bandwidth"
                                }
                                button {
                                    class: "w-full text-left px-4 py-3 text-sm text-red-600 hover:bg-red-50",
                                    onclick: move |evt: MouseEvent| {
                                        evt.stop_propagation();
                                        menu_open.set(false);
                                        on_delete.call(proxy_2.clone());
                                    },
                                    "Delete"
                                }
                            }
                        }
                    }
                }

                // no footer actions; card click opens details, kebab has delete
            }

            if enabled {
                div { class: "border-t border-[#eceee9] bg-white",
                    WaveFooter {}
                }
            }
        }
    }
}

#[component]
fn Toggle(enabled: bool, on_toggle: EventHandler<bool>) -> Element {
    // Figma-ish toggle colors (muted)
    let bg = if enabled { "bg-[#6f8f70]" } else { "bg-[#d8d8d2]" };
    let knob = if enabled { "translate-x-5" } else { "translate-x-0" };
    rsx! {
        button {
            class: "relative inline-flex h-7 w-12 items-center rounded-full transition-colors {bg} shadow-sm",
            onclick: move |evt| {
                evt.stop_propagation();
                on_toggle.call(!enabled)
            },
            span { class: "inline-block h-6 w-6 transform rounded-full bg-white transition-transform {knob} shadow-sm" }
        }
    }
}

#[component]
fn GlobeIcon() -> Element {
    rsx! {
        svg { width: "24", height: "24", view_box: "0 0 24 24", fill: "none", class: "text-[#9c8a87]",
            path { d: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Z", stroke: "currentColor", stroke_width: "1.6" }
            path { d: "M3 12h18", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round" }
            path { d: "M12 3c2.5 2.8 3.9 5.9 3.9 9S14.5 18.2 12 21c-2.5-2.8-3.9-5.9-3.9-9S9.5 5.8 12 3Z", stroke: "currentColor", stroke_width: "1.6" }
        }
    }
}

#[component]
fn ArrowIcon() -> Element {
    rsx! {
        svg { width: "24", height: "24", view_box: "0 0 24 24", fill: "none", class: "text-[#9c8a87]",
            path { d: "M5 5v14h14", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round" }
        }
    }
}

#[component]
fn PlugIcon() -> Element {
    rsx! {
        svg { width: "24", height: "24", view_box: "0 0 24 24", fill: "none", class: "text-[#9c8a87]",
            path { d: "M9 3v6M15 3v6", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round" }
            path { d: "M7 9h10v3a5 5 0 0 1-5 5h0a5 5 0 0 1-5-5V9Z", stroke: "currentColor", stroke_width: "1.6", stroke_linejoin: "round" }
            path { d: "M12 17v4", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round" }
        }
    }
}

#[component]
fn WaveFooter() -> Element {
    rsx! {
        // light wave similar to the Figma footer hint
        svg {
            width: "100%", height: "86", view_box: "0 0 800 120", fill: "none",
            preserve_aspect_ratio: "none",
            path {
                d: "M0 80 C 120 30, 220 120, 340 70 C 460 20, 560 120, 680 70 C 740 45, 780 55, 800 60 L 800 120 L 0 120 Z",
                fill: "#f1f2ee"
            }
            path {
                d: "M0 80 C 120 30, 220 120, 340 70 C 460 20, 560 120, 680 70 C 740 45, 780 55, 800 60",
                stroke: "#d9dbd5",
                stroke_width: "2"
            }
        }
    }
}

fn codename_to_title(codename: &str) -> String {
    codename
        .split('-')
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + ch.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
