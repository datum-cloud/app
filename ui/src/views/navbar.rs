use crate::{state::AppState, Route};
use dioxus::prelude::*;

/// The Navbar component that will be rendered on all pages of our app since every page is under the layout.
///
///
/// This layout component wraps the UI of [Route::Home] and [Route::Blog] in a common navbar. The contents of the Home and Blog
/// routes will be rendered under the outlet inside this component
#[component]
pub fn Navbar() -> Element {
    let state = consume_context::<AppState>();
    let id = state.node().endpoint_id();

    rsx! {
        div {
            class: "flex flex-col p-5",
            div {
                id: "navbar",
                class: "flex gap-8 border-b border-gray-500 pb-5 mb-5",
                Link {
                    class: "hover:text-gray-700 text-lg cursor-pointer",
                    to: Route::TempProxies {  },
                    "Proxies"
                }
            },
            div {
                class: "flex-1 h-full",
                Outlet::<Route> {}
            },
            div {
                class: "fixed bottom-0 left-0 right-0 px-5 py-2",
                h5 { class: "text-sm text-gray-800", "endpoint_id: {id}" },
            }
        }
    }
}
