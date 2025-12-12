use dioxus::prelude::*;
// use lib::TcpProxyTicket;
// use std::str::FromStr;

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
pub fn JoinProxy() -> Element {
    let mut local_address = use_signal(|| "127.0.0.1:9000".to_string());
    let mut codename = use_signal(|| "".to_string());
    // let mut ticket_str = use_signal(|| "".to_string());
    // let mut validation_error = use_signal(|| "".to_string());

    rsx! {
        div {
            id: "create-domain",
            class: "flex flex-col",
            h1 { "join proxy" },
            // p {
            //     class: "text-red-500",
            //     "{validation_error}"
            // }
            Subhead { text: "Local Address" }
            input {
                class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
                value: "{local_address}",
                onchange: move |e| local_address.set(e.value()),
            }
            Subhead { text: "Codename" }
            input {
                class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
                placeholder: "Label",
                value: "{codename}",
                onchange: move |e| codename.set(e.value()),
            }
            // Subhead { text: "Ticket" }
            // textarea {
            //     class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
            //     value: "{ticket_str}",
            //     onchange: move |e| ticket_str.set(e.value()),
            // },
            div {
                class: "flex gap-10",
                Button {
                    onclick: move |_| async move {
                        let state = consume_context::<AppState>();
                        // let ticket = match TcpProxyTicket::from_str(&ticket_str()) {
                        //     Ok(ticket) => ticket,
                        //     Err(err) => {
                        //         validation_error.set(format!("Invalid ticket: {}", err));
                        //         return;
                        //     }
                        // };
                        state.clone().node().connect(codename()).await.unwrap();
                        let nav = use_navigator();
                        nav.push(Route::TempProxies {  });
                    },
                    text: "Join"
                }
                Button {
                    onclick: move |_| async move {
                        let nav = use_navigator();
                        nav.push(Route::TempProxies {  });
                    },
                    text: "Cancel"
                }
            }
        }
    }
}
