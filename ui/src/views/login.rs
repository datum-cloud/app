use dioxus::prelude::*;

use crate::Route;

#[component]
pub fn Login() -> Element {
    rsx! {
        div {
            id: "create-domain",
            h1 { "TODO: login" }
            Link {
                to: Route::TempProxies {  },
                "Proxies"
            }
        }
    }
}
