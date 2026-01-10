use dioxus::prelude::*;

use crate::{
    components::{Button, ButtonKind},
    state::AppState,
    Route,
};

/// The Blog page component that will be rendered when the current route is `[Route::Blog]`
///
/// The component takes a `id` prop of type `i32` from the route enum. Whenever the id changes, the component function will be
/// re-run and the rendered HTML will be updated.
#[component]
pub fn CreateProxy() -> Element {
    let mut address = use_signal(|| "127.0.0.1:5173".to_string());
    let mut label = use_signal(|| "New Tunnel".to_string());
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let error = use_signal(|| Option::<String>::None);
    let creating = use_signal(|| false);
    // let ticket = use_signal(|| "".to_string());

    rsx! {
        div { id: "create-proxy", class: "max-w-4xl mx-auto px-1",
            // Header with back + title
            div { class: "flex items-center gap-4 mb-6",
                button {
                    class: "w-10 h-10 rounded-xl border border-[#dfe3ea] bg-white flex items-center justify-center text-slate-600 hover:text-slate-800 hover:bg-gray-50 shadow-sm cursor-pointer",
                    onclick: move |_| {
                        nav.push(Route::TempProxies {  });
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

                    if let Some(err) = error() {
                        div { class: "rounded-xl border border-red-200 bg-red-50 p-4 text-red-800",
                            div { class: "text-sm font-semibold", "Couldn't create tunnel" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }

                    // Actions
                    div { class: "flex items-center gap-4 pt-2",
                        Button {
                            kind: ButtonKind::Primary,
                            class: if creating() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                            onclick: move |_| {
                                let state = state.clone();
                                let nav = nav.clone();
                                let mut error = error.clone();
                                let mut creating = creating.clone();
                                spawn(async move {
                                    if creating() {
                                        return;
                                    }
                                    creating.set(true);
                                    error.set(None);
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(5),
                                        state.node().start_listening(label(), address()),
                                    )
                                    .await
                                    {
                                        Ok(Ok(_)) => {
                                            let _ = nav.push(Route::TempProxies {  });
                                        }
                                        Ok(Err(err)) => error.set(Some(err.to_string())),
                                        Err(_) => error.set(Some(
                                            "Timed out creating tunnel. If you're using n0des, make sure it's running and N0DES_API_SECRET matches."
                                                .to_string(),
                                        )),
                                    }
                                    creating.set(false);
                                });
                            },
                            text: if creating() { "Creating…".to_string() } else { "Create tunnel".to_string() }
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
            // div {
            //     id: "ticket-container",
            //     class: "my-5",
            //     Subhead { text: "Ticket" },
            //     p {
            //         class: "max-w-5/10 break-all",
            //         "{ticket}"
            //     },
            // }
        }
    }
}
