use dioxus::prelude::*;
use lib::TcpProxyData;

use crate::{
    components::{Button, ButtonKind},
    state::AppState,
    Route,
};

#[component]
pub fn EditProxy(id: String) -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let proxy = use_signal(|| state.listen_node().proxy_by_id(&id));

    let mut loading = use_signal(|| false);
    let mut label = use_signal(String::new);
    let mut address = use_signal(String::new);
    let mut enabled = use_signal(|| false);
    let mut proxy_id = use_signal(String::new);

    use_effect(move || {
        if let Some(proxy) = proxy() {
            address.set(proxy.info.data.address());
            label.set(proxy.info.label().to_string());
            enabled.set(proxy.enabled);
            proxy_id.set(proxy.id().to_owned());
        }
        loading.set(false);
    });

    let mut save = use_action(move |_state| async move {
        let state = consume_context::<AppState>();
        let proxy = proxy().context("Proxy does not exist")?;
        let mut proxy = proxy.clone();
        proxy.info.data = TcpProxyData::from_host_port_str(&address())?;
        proxy.enabled = enabled();
        let label = label();
        proxy.info.label = (!label.is_empty()).then(|| label.clone());
        state.node().listen.set_proxy(proxy).await?;
        let nav = use_navigator();
        nav.push(Route::ProxiesList {});
        n0_error::Ok(())
    });

    let Some(proxy) = proxy() else {
        return rsx! {
            div { class: "max-w-4xl mx-auto",
                div { class: "rounded-2xl border border-red-200 bg-red-50 text-red-800 p-6",
                    div { class: "text-sm font-semibold", "Tunnel not found" }
                }
            }
        };
    };

    rsx! {
        div { id: "edit-proxy", class: "max-w-4xl mx-auto px-1",
            // Header with back + title
            div { class: "flex items-center justify-between gap-4 mb-6",
                // Left side: back + title
                div { class: "flex items-center gap-4",
                    button {
                        class: "w-10 h-10 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center text-slate-600 hover:text-slate-800 hover:bg-gray-50 shadow-sm cursor-pointer",
                        onclick: move |_| {
                            let _ = nav.push(Route::ProxiesList {  });
                        },
                        "←"
                    }
                    div { class: "flex flex-col",
                        div { class: "text-2xl font-semibold text-slate-900", "Tunnel details" }
                        div { class: "text-sm text-slate-600", {proxy.id()} }
                    }
                }

                // Right side: bandwidth action
                button {
                    class: "h-10 px-4 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center gap-2 text-slate-700 hover:text-slate-900 hover:bg-gray-50 shadow-sm cursor-pointer whitespace-nowrap",
                    onclick: move |_| {
                        let _ = nav.push(Route::TunnelBandwidth { id: proxy_id() });
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
                            span { class: "font-mono text-slate-800", {proxy.info.domain()} }
                        }
                        div { class: "text-sm text-slate-600",
                            span { class: "font-medium text-slate-700", "datum:// " }
                            span { class: "font-mono text-slate-800", {proxy.info.datum_url()} }
                        }
                    }

                    if let Some(Err(err)) = save.value() {
                        div { class: "rounded-xl border border-red-200 bg-red-50 p-4 text-red-800",
                            div { class: "text-sm font-semibold", "Couldn't save changes" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }

                    // Actions
                    div { class: "flex items-center gap-4 pt-2",
                        Button {
                            kind: ButtonKind::Primary,
                            class: if save.pending() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                            onclick: move |_| save.call(()),
                            text: if save.pending() { "Saving…".to_string() } else { "Save changes".to_string() }
                        }
                        Button {
                            kind: ButtonKind::Secondary,
                            onclick: move |_| {
                                let _ = nav.push(Route::ProxiesList {  });
                            },
                            text: "Cancel"
                        }
                    }
                }
            }
        }
    }
}
