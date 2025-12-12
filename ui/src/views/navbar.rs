use crate::{components::Button, state::AppState, Route};
use dioxus::prelude::*;
use dioxus_desktop::use_window;

/// The Navbar component that will be rendered on all pages of our app since every page is under the layout.
///
///
/// This layout component wraps the UI of [Route::Home] and [Route::Blog] in a common navbar. The contents of the Home and Blog
/// routes will be rendered under the outlet inside this component
#[component]
pub fn Navbar() -> Element {
    let window = use_window();
    let state = consume_context::<AppState>();
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
                div {
                    class: "flex-grow"
                }
                Button {
                    onclick: move |_| {
                        window.set_visible(false);
                    },
                    text: "hide"
                }
            },
            div {
                class: "flex-1 h-full",
                Outlet::<Route> {}
            }
        }
    }
}
