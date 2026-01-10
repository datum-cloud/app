use dioxus::prelude::*;
use uuid::Uuid;

use lib::DATUM_CONNECT_GATEWAY_DOMAIN_NAME;

use crate::{
    components::{Button, ButtonKind},
    state::AppState,
    Route,
};

#[component]
pub fn EditProxy(id: String) -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let state_loader = state.clone();
    let id_for_load = id.clone();
    let id_for_bandwidth = id.clone();

    let mut loading = use_signal(|| true);
    let mut load_error = use_signal(|| Option::<String>::None);
    let mut saving = use_signal(|| false);
    let mut save_error = use_signal(|| Option::<String>::None);

    let mut label = use_signal(|| "".to_string());
    let mut address = use_signal(|| "".to_string());
    let mut codename = use_signal(|| "".to_string());
    let mut enabled = use_signal(|| true);
    let mut proxy_id = use_signal(|| None::<Uuid>);

    // Load the existing proxy data
    use_future(move || {
        let id = id_for_load.clone();
        let state = state_loader.clone();
        let mut loading = loading.clone();
        let mut load_error = load_error.clone();
        let mut proxy_id = proxy_id.clone();
        let mut label = label.clone();
        let mut address = address.clone();
        let mut codename = codename.clone();
        let mut enabled = enabled.clone();
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

            proxy_id.set(Some(proxy.id));
            label.set(proxy.label.clone().unwrap_or_default());
            address.set(format!("{}:{}", proxy.host, proxy.port));
            codename.set(proxy.codename.clone());
            enabled.set(proxy.enabled);
            loading.set(false);
        }
    });

    if loading() {
        return rsx! {
            div { class: "max-w-4xl mx-auto",
                div { class: "rounded-2xl border border-[#e3e7ee] bg-white/70 p-8",
                    div { class: "text-sm text-slate-600", "Loading tunnel…" }
                }
            }
        };
    }

    if let Some(err) = load_error() {
        return rsx! {
            div { class: "max-w-4xl mx-auto",
                div { class: "rounded-2xl border border-red-200 bg-red-50 text-red-800 p-6",
                    div { class: "text-sm font-semibold", "Couldn't load tunnel" }
                    div { class: "text-sm mt-1 break-words", "{err}" }
                }
            }
        };
    }

    let domain = format!("{}.{}", codename(), DATUM_CONNECT_GATEWAY_DOMAIN_NAME);
    // Render only the codename; the UI label already includes `datum://`.
    let datum_url = codename();

    rsx! {
        div { id: "edit-proxy", class: "max-w-4xl mx-auto px-1",
            // Header with back + title
            div { class: "flex items-center justify-between gap-4 mb-6",
                // Left side: back + title
                div { class: "flex items-center gap-4",
                    button {
                        class: "w-10 h-10 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center text-slate-600 hover:text-slate-800 hover:bg-gray-50 shadow-sm cursor-pointer",
                        onclick: move |_| {
                            let _ = nav.push(Route::TempProxies {  });
                        },
                        "←"
                    }
                    div { class: "flex flex-col",
                        div { class: "text-2xl font-semibold text-slate-900", "Tunnel details" }
                        div { class: "text-sm text-slate-600", "{codename()}" }
                    }
                }

                // Right side: bandwidth action
                button {
                    class: "h-10 px-4 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center gap-2 text-slate-700 hover:text-slate-900 hover:bg-gray-50 shadow-sm cursor-pointer whitespace-nowrap",
                    onclick: move |_| {
                        let _ = nav.push(Route::TunnelBandwidth { id: id_for_bandwidth.clone() });
                    },
                    svg {
                        width: "16", height: "16", view_box: "0 0 24 24", fill: "none",
                        path { d: "M4 19V5", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round" }
                        path { d: "M4 19h16", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round" }
                        path { d: "M7 15l3-4 3 3 4-6", stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round" }
                    }
                    span { class: "text-sm font-medium", "Bandwidth" }
                }
            }

            // Main panel
            div { class: "bg-white rounded-2xl border border-[#e3e7ee] shadow-[0_10px_28px_rgba(17,24,39,0.10)] p-8 sm:p-10",
                // Stack sections with consistent spacing so nothing feels squished.
                div { class: "flex flex-col gap-8",
                    // Name / label
                    div { class: "space-y-3",
                        div { class: "text-sm font-medium text-slate-700", "Name" }
                        input {
                            class: "w-full max-w-xl rounded-xl border border-[#dfe3ea] bg-white px-4 py-3 text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-[#cfd6df]",
                            placeholder: "New Tunnel",
                            value: "{label}",
                            onchange: move |e| label.set(e.value()),
                        }
                        div { class: "text-xs text-slate-500", "This is just a display name. Your tunnel’s address uses the codename." }
                    }

                    div { class: "border-t border-[#eceee9]" }

                    // Address
                    div { class: "space-y-3",
                        div { class: "text-sm font-medium text-slate-700", "Local address to forward" }
                        input {
                            class: "w-full max-w-xl rounded-xl border border-[#dfe3ea] bg-white px-4 py-3 text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-[#cfd6df]",
                            placeholder: "127.0.0.1:5173",
                            value: "{address}",
                            onchange: move |e| address.set(e.value()),
                        }
                    }

                    // Read-only info
                    div { class: "grid grid-cols-1 gap-3",
                        div { class: "text-sm text-slate-600",
                            span { class: "font-medium text-slate-700", "Domain: " }
                            span { class: "font-mono text-slate-800", "{domain}" }
                        }
                        div { class: "text-sm text-slate-600",
                            span { class: "font-medium text-slate-700", "datum:// " }
                            span { class: "font-mono text-slate-800", "{datum_url}" }
                        }
                    }

                    if let Some(err) = save_error() {
                        div { class: "rounded-xl border border-red-200 bg-red-50 p-4 text-red-800",
                            div { class: "text-sm font-semibold", "Couldn't save changes" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }

                    // Actions
                    div { class: "flex items-center gap-4 pt-2",
                        Button {
                            kind: ButtonKind::Primary,
                            class: if saving() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                            onclick: move |_| {
                                let state = state.clone();
                                let nav = nav.clone();
                                let mut saving = saving.clone();
                                let mut save_error = save_error.clone();
                                let address = address();
                                let label_val = label();
                                let codename_val = codename();
                                let enabled_val = enabled();
                                let id = proxy_id();
                                spawn(async move {
                                    if saving() {
                                        return;
                                    }
                                    saving.set(true);
                                    save_error.set(None);

                                    let Some(id) = id else {
                                        save_error.set(Some("Missing tunnel id".to_string()));
                                        saving.set(false);
                                        return;
                                    };

                                    let Some((host, port_str)) = address.split_once(':') else {
                                        save_error.set(Some("Address must be in the form host:port".to_string()));
                                        saving.set(false);
                                        return;
                                    };
                                    let host = host.trim().to_string();
                                    let port: u16 = match port_str.trim().parse() {
                                        Ok(p) => p,
                                        Err(_) => {
                                            save_error.set(Some("Port must be a number".to_string()));
                                            saving.set(false);
                                            return;
                                        }
                                    };

                                    let label_val = match label_val.trim() {
                                        "" => None,
                                        s => Some(s.to_string()),
                                    };

                                    let updated_proxy = lib::TcpProxy {
                                        id,
                                        label: label_val,
                                        codename: codename_val,
                                        host,
                                        port,
                                        enabled: enabled_val,
                                    };

                                    match state.node().update_proxy(&updated_proxy).await {
                                        Ok(_) => {
                                            let _ = nav.push(Route::TempProxies {  });
                                        }
                                        Err(err) => save_error.set(Some(err.to_string())),
                                    }
                                    saving.set(false);
                                });
                            },
                            text: if saving() { "Saving…".to_string() } else { "Save changes".to_string() }
                        }
                        Button {
                            kind: ButtonKind::Secondary,
                            onclick: move |_| {
                                let _ = nav.push(Route::TempProxies {  });
                            },
                            text: "Cancel"
                        }
                    }
                }
            }
        }
    }
}
