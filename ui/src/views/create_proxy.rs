use dioxus::prelude::*;

use crate::{
    components::{Button, ButtonKind},
    state::AppState,
    Route,
};

#[component]
pub fn CreateProxy() -> Element {
    let nav = use_navigator();

    let mut address = use_signal(|| "127.0.0.1:5173".to_string());
    let mut label = use_signal(|| "New Tunnel".to_string());

    let mut save = use_action(move |_| async move {
        let state = consume_context::<AppState>();
        let project_id = state
            .selected_context()
            .context("No project selected")?
            .project_id;
        let tunnel = state
            .tunnel_service()
            .create_active(&label(), &address())
            .await
            .context("Failed to create tunnel")?;
        state.upsert_tunnel(tunnel);
        state.bump_tunnel_refresh();
        state.heartbeat().register_project(project_id).await;
        let nav = use_navigator();
        nav.push(Route::ProxiesList {});
        n0_error::Ok(())
    });

    rsx! {
        div { id: "create-proxy", class: "max-w-4xl mx-auto px-1",
            // Header with back + title
            div { class: "flex items-center gap-4 mb-6",
                button {
                    class: "w-10 h-10 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center text-slate-600 hover:text-slate-800 hover:bg-gray-50 shadow-sm cursor-pointer",
                    onclick: move |_| {
                        nav.push(Route::ProxiesList {  });
                    },
                    "←"
                }
                div { class: "flex flex-col",
                    div { class: "text-2xl font-semibold text-slate-900", "Add tunnel" }
                    div { class: "text-sm text-slate-600", "Create a new local forward" }
                }
            }

            // Form card
            div { class: "bg-white rounded-2xl border border-[#e3e7ee] shadow-[0_10px_28px_rgba(17,24,39,0.10)] p-8 sm:p-10",
                div { class: "flex flex-col gap-8",
                    // Name
                    div { class: "space-y-3",
                        div { class: "text-sm font-medium text-slate-700", "Name" }
                        input {
                            class: "w-full max-w-xl rounded-xl border border-[#dfe3ea] bg-white px-4 py-3 text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-[#cfd6df]",
                            placeholder: "New Marketing Site",
                            value: "{label}",
                            onchange: move |e| label.set(e.value()),
                        }
                        div { class: "text-xs text-slate-500", "This is a display name. Your tunnel gets an auto-generated codename." }
                    }

                    div { class: "border-t border-[#eceee9]" }

                    // Local address
                    div { class: "space-y-3",
                        div { class: "text-sm font-medium text-slate-700", "Local address to forward" }
                        input {
                            class: "w-full max-w-xl rounded-xl border border-[#dfe3ea] bg-white px-4 py-3 text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-[#cfd6df]",
                            placeholder: "127.0.0.1:5173",
                            value: "{address}",
                            onchange: move |e| address.set(e.value()),
                        }
                        div { class: "text-xs text-slate-500", "Example: 127.0.0.1:5173" }
                    }

                    if let Some(Err(err)) = save.value() {
                        div { class: "rounded-xl border border-red-200 bg-red-50 p-4 text-red-800",
                            div { class: "text-sm font-semibold", "Couldn't create tunnel" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }

                    // Actions
                    div { class: "flex items-center gap-4 pt-2",
                        Button {
                            kind: ButtonKind::Primary,
                            class: if save.pending() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                            onclick: move |_| save.call(()),
                            text: if save.pending() { "Creating…".to_string() } else { "Create tunnel".to_string() }
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
