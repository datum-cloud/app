use dioxus::events::MouseEvent;
use dioxus::prelude::*;
use lib::TunnelSummary;

use crate::{state::AppState, Route};

#[component]
pub fn ProxiesList() -> Element {
    let state = consume_context::<AppState>();
    let tunnels = state.tunnel_cache();

    let state_for_future = state.clone();
    use_future(move || {
        let state_for_future = state_for_future.clone();
        async move {
        let mut ctx_rx = state_for_future.datum().selected_context_watch();
        let refresh = state_for_future.tunnel_refresh();
        loop {
            let list = state_for_future
                .tunnel_service()
                .list_active()
                .await
                .unwrap_or_default();
            state_for_future.set_tunnel_cache(list);
            tokio::select! {
                res = ctx_rx.changed() => {
                    if res.is_err() {
                        return;
                    }
                }
                _ = refresh.notified() => {}
            }
        }
        }
    });

    // Important: do async mutations from this parent component scope.
    // If we spawn from inside `TunnelCard` and then optimistically remove the card,
    // Dioxus will drop that scope and cancel the task before it runs.
    let mut on_delete = use_action(move |tunnel: TunnelSummary| {
        let state = state.clone();
        async move {
            debug!("on delete called: {}", tunnel.id);
            let outcome = state
                .tunnel_service()
                .delete_active(&tunnel.id)
                .await
                .inspect_err(|err| {
                    tracing::warn!("delete tunnel failed: {err:#}");
                })?;
            if outcome.connector_deleted {
                state
                    .heartbeat()
                    .deregister_project(&outcome.project_id)
                    .await;
            }
            state.remove_tunnel(&tunnel.id);
            state.bump_tunnel_refresh();
            n0_error::Ok(())
        }
    });

    let on_delete_handler = move |proxy_state| on_delete.call(proxy_state);

    let list = if tunnels().is_empty() {
        rsx! {
            div { class: "rounded-2xl border border-[#e3e7ee] bg-white/70 p-10 text-center",
                div { class: "text-base font-semibold text-slate-900", "No tunnels yet" }
                div { class: "text-sm text-slate-600 mt-2",
                    "Click ",
                    span { class: "font-medium", "\"Add tunnel\"" },
                    " in the left sidebar to create one."
                }
            }
        }
    } else {
        rsx! {
            div { class: "space-y-5",
                for tunnel in tunnels().into_iter() {
                    TunnelCard { key: "{tunnel.id}", tunnel, on_delete: on_delete_handler }
                }
            }
        }
    };

    rsx! {
        div { class: "max-w-5xl mx-auto",
            {list}
        }
    }
}

#[component]
fn TunnelCard(tunnel: TunnelSummary, on_delete: EventHandler<TunnelSummary>) -> Element {
    let mut tunnel_signal = use_signal(move || tunnel);

    let mut menu_open = use_signal(|| false);
    let nav = use_navigator();
    let state = consume_context::<AppState>();

    let mut toggle_action = use_action(move |next_enabled: bool| {
        let state = state.clone();
        async move {
            let tunnel = tunnel_signal().clone();
            let updated = state
                .tunnel_service()
                .set_enabled_active(&tunnel.id, next_enabled)
                .await?;
            if next_enabled {
                if let Some(selected) = state.selected_context() {
                    state.heartbeat().register_project(selected.project_id).await;
                }
            }
            state.upsert_tunnel(updated.clone());
            tunnel_signal.set(updated);
            state.bump_tunnel_refresh();
            n0_error::Ok(())
        }
    });

    let tunnel = tunnel_signal();
    let enabled = tunnel.enabled;
    let display_hostname = tunnel
        .hostnames
        .first()
        .cloned()
        .unwrap_or_else(|| tunnel.id.clone());
    let display_hostname_href = display_hostname.clone();
    let display_endpoint = if tunnel.endpoint.is_empty() {
        "unknown".to_string()
    } else {
        tunnel.endpoint.clone()
    };
    let display_endpoint_href = display_endpoint.clone();

    rsx! {
        div {
            // darker shadow + hover lift
            class: "bg-white rounded-xl border border-[#e3e7ee] shadow-[0_10px_28px_rgba(17,24,39,0.10)] hover:shadow-[0_14px_34px_rgba(17,24,39,0.14)] transition-shadow",
            // onclick: move |_| {
            //     let id = proxy_signal().id().to_string();
            //     // Clicking the card opens details (unless the kebab menu is open)
            //     if menu_open() {
            //         return;
            //     }
            //     nav.push(Route::EditProxy { id });
            // },
            div { class: "",
                // header row: title + toggle
                div { class: "p-4 flex items-start justify-between",
                    h2 { class: "text-md font-semibold tracking-tight text-slate-900", {tunnel.label} }
                    Toggle {
                        enabled,
                        on_toggle: move |next| toggle_action.call(next)
                    }
                }

                // divider under the header (Figma-style)
                div { class: "border-t border-[#eceee9]" }

                // body: rows + kebab aligned to the right like Figma
                div { class: "p-4 flex items-start justify-between",
                    div { class: "space-y-4",
                        div { class: "flex items-center gap-5",
                            GlobeIcon { class: "w-[20] h-[20]" }
                            a { class: "text-base font-medium text-slate-800",
                                href: format!("http://{}", display_hostname_href),
                                {display_hostname}
                            }
                        }
                        div { class: "flex items-center gap-5",
                            ArrowIcon { class: "w-[20] h-[20] "}
                            a { class: "text-base text-slate-800",
                                href: format!("http://{}", display_endpoint_href),
                                {display_endpoint}
                            }
                        }
                        div { class: "flex items-center gap-5",
                            PlugIcon { class: "w-[20] h-[20] "}
                            span { class: "text-base text-slate-700", {format!("datum://{}", tunnel.id)} }
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
                                        let id = tunnel_signal().id.to_string();
                                        nav.push(Route::EditProxy { id });
                                    },
                                    "Details"
                                }
                                button {
                                    class: "w-full text-left px-4 py-3 text-sm text-slate-800 hover:bg-gray-50",
                                    onclick: move |evt: MouseEvent| {
                                        evt.stop_propagation();
                                        menu_open.set(false);
                                        let id = tunnel_signal().id.to_string();
                                        nav.push(Route::TunnelBandwidth { id });
                                    },
                                    "Bandwidth"
                                }
                                button {
                                    class: "w-full text-left px-4 py-3 text-sm text-red-600 hover:bg-red-50",
                                    onclick: move |evt: MouseEvent| {
                                        evt.stop_propagation();
                                        menu_open.set(false);
                                        on_delete.call(tunnel_signal().clone());
                                    },
                                    "Delete"
                                }
                            }
                        }
                    }
                }
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
    let bg = if enabled {
        "bg-[#6f8f70]"
    } else {
        "bg-[#d8d8d2]"
    };
    let knob = if enabled {
        "translate-x-5"
    } else {
        "translate-x-0"
    };
    rsx! {
        button {
            class: "relative inline-flex h-6 w-12 items-center rounded-full transition-colors {bg} shadow-sm",
            onclick: move |evt| {
                evt.stop_propagation();
                on_toggle.call(!enabled)
            },
            span { class: "inline-block h-5 w-6 transform rounded-full bg-white transition-transform {knob} shadow-sm" }
        }
    }
}

#[component]
fn IconSvg(#[props(default)] class: String, children: Element) -> Element {
    rsx! {
        svg {  width: "24", height: "24", view_box: "0 0 24 24", fill: "none", class: "text-[#bf9595] {class}",
            {children}
        }
    }
}

#[component]
fn GlobeIcon(#[props(default)] class: String) -> Element {
    rsx! {
        IconSvg { class,
            path { d: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Z", stroke: "currentColor", stroke_width: "1.6" }
            path { d: "M3 12h18", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round" }
            path { d: "M12 3c2.5 2.8 3.9 5.9 3.9 9S14.5 18.2 12 21c-2.5-2.8-3.9-5.9-3.9-9S9.5 5.8 12 3Z", stroke: "currentColor", stroke_width: "1.6" }
        }
    }
}

#[component]
fn ArrowIcon(#[props(default)] class: String) -> Element {
    rsx! {
        IconSvg { class,
            path { d: "M5 5v14h14", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round" }
        }
    }
}

#[component]
fn PlugIcon(#[props(default)] class: String) -> Element {
    rsx! {
        IconSvg { class,
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
            width: "100%", height: "50", view_box: "0 0 800 120", fill: "none",
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
