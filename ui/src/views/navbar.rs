use crate::{components::Button, Route};
use dioxus::prelude::*;
use dioxus_desktop::use_window;

/// The Navbar component that will be rendered on all pages of our app since every page is under the layout.
#[component]
pub fn Navbar() -> Element {
    let window = use_window();
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
