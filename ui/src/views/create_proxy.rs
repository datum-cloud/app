use dioxus::prelude::*;

use crate::{
    components::{Button, Subhead},
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
    let mut label = use_signal(|| "".to_string());
    let mut ticket = use_signal(|| "".to_string());

    rsx! {
        div {
            id: "create-proxy",
            h1 {
                class: "text-xl font-bold mb-20",
                "Create Proxy"
            },
            // Subhead { text: "label" },
            // input {
            //     class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
            //     placeholder: "Label",
            //     value: "{label}",
            //     onchange: move |e| {
            //         label.set(e.value());
            //     }
            // }
            Subhead { text: "local address to forward" },
            input {
                class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
                placeholder: "Address",
                value: "{address}",
                onchange: move |e| {
                    address.set(e.value());
                }
            }

            Button {
                onclick: move |_| async move {
                    let state = consume_context::<AppState>();
                    state.node().start_listening(label(), address()).await.unwrap();
                    // let tkt = state.clone().node().listen().await.unwrap();
                    // ticket.set(tkt.to_string())
                    let nav = use_navigator();
                    nav.push(Route::TempProxies {  });
                },
                text: "Create"
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
