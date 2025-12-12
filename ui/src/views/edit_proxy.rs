use dioxus::prelude::*;
use uuid::Uuid;

use crate::{
    components::{Button, Subhead},
    state::AppState,
    Route,
};

#[component]
pub fn EditProxy(id: String) -> Element {
    let mut address = use_signal(|| "".to_string());
    let mut codename = use_signal(|| "".to_string());
    let mut proxy_id = use_signal(|| None::<Uuid>);
    let mut loaded = use_signal(|| false);

    // Load the existing proxy data
    use_future(move || {
        let id_clone = id.clone();
        async move {
            let state = consume_context::<AppState>();
            let proxies = state.node().proxies().await.unwrap();

            // Find the proxy by ID
            if let Ok(uuid) = Uuid::parse_str(&id_clone) {
                if let Some(proxy) = proxies.iter().find(|p| p.id == uuid) {
                    address.set(format!("{}:{}", proxy.host, proxy.port));
                    codename.set(proxy.codename.clone());
                    proxy_id.set(Some(proxy.id));
                    loaded.set(true);
                }
            }
        }
    });

    if !loaded() {
        return rsx! {
            div {
                class: "flex items-center justify-center h-screen",
                p { "Loading proxy details..." }
            }
        };
    }

    rsx! {
        div {
            id: "edit-proxy",
            h1 {
                class: "text-xl font-bold mb-20",
                "Edit Proxy: {codename}"
            },
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
                    if let Some(id) = proxy_id() {
                        let state = consume_context::<AppState>();

                        // Parse the address into host and port
                        let addr_str = address();
                        let addr_parts: Vec<&str> = addr_str.split(':').collect();
                        if addr_parts.len() == 2 {
                            let host = addr_parts[0].to_string();
                            if let Ok(port) = addr_parts[1].parse::<u16>() {
                                // Create updated proxy
                                let updated_proxy = lib::TcpProxy {
                                    id,
                                    codename: codename(),
                                    host,
                                    port,
                                };

                                state.node().update_proxy(&updated_proxy).await.unwrap();

                                let nav = use_navigator();
                                nav.push(Route::TempProxies {  });
                            }
                        }
                    }
                },
                text: "Save Changes"
            }

            Button {
                onclick: move |_| {
                    let nav = use_navigator();
                    nav.push(Route::TempProxies {  });
                },
                text: "Cancel"
            }
        }
    }
}
