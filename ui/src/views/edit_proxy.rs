use dioxus::prelude::*;
use lib::{ProxyState, TcpProxyData};

use crate::{
    components::{Button, Subhead},
    state::AppState,
    Route,
};

#[derive(Debug, Eq, PartialEq, Clone)]
enum LoadState<T> {
    Pending,
    Ready(T),
    Failed(String),
}

#[component]
pub fn EditProxy(id: String) -> Element {
    let mut address = use_signal(|| "".to_string());
    let mut label = use_signal(|| "".to_string());
    let mut loaded = use_signal::<LoadState<ProxyState>>(|| LoadState::Pending);
    let mut error = use_signal(|| None);

    // Load the existing proxy data
    use_future(move || {
        let id_clone = id.clone();
        async move {
            let state = consume_context::<AppState>();
            let proxies = state.node().listen.proxies();

            // Find the proxy by ID
            if let Some(proxy) = proxies.iter().find(|p| p.info.resource_id == id_clone) {
                address.set(proxy.info.data.address());
                label.set(proxy.info.label().to_string());
                loaded.set(LoadState::Ready(proxy.clone()));
            } else {
                loaded.set(LoadState::Failed("Proxy not found".to_string()))
            }
        }
    });

    let proxy = match (*loaded.read()).clone() {
        LoadState::Pending => {
            return rsx! {
                div {
                    class: "flex items-center justify-center h-screen",
                    p { "Loading proxy details..." }
                }
            }
        }
        LoadState::Failed(error) => {
            return rsx! {
                div {
                    class: "flex items-center justify-center h-screen",
                    {error}
                }
            }
        }
        LoadState::Ready(proxy) => proxy,
    };

    rsx! {
        div {
            id: "edit-proxy",
            h1 {
                class: "text-xl font-bold mb-20",
                "Edit Proxy: {proxy.info.resource_id}"
            },
            Subhead { text: "local address to forward" },
            input {
                class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
                placeholder: "Address",
                value: address(),
                onchange: move |e| {
                    address.set(e.value());
                }
            }

            input {
                class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
                placeholder: "Label",
                value: label(),
                onchange: move |e| {
                    label.set(e.value());
                }
            }

            {render_error(error)}

            Button {
                onclick: move |_event| async move {
                    match TcpProxyData::from_host_port_str(&address()) {
                        Err(err) => {
                            error.set(Some(err.to_string()));
                        },
                        Ok(new_data) => {
                            let state = consume_context::<AppState>();
                            let mut proxy = match (*loaded.read()).clone() {
                                LoadState::Ready(proxy) => proxy.clone(),
                                _ => unreachable!()
                            };
                            proxy.info.data = new_data;
                            let label = label().to_string();
                            proxy.info.label = if label.is_empty() {
                                None
                            } else {
                                Some(label)
                            };
                            if let Err(err) = state.node().listen.set_proxy(proxy).await {
                                error.set(Some(err.to_string()))
                            } else {
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

fn render_error(error: Signal<Option<String>>) -> Element {
    let inner = error.read();
    match &*inner {
        None => Element::Ok(VNode::placeholder()),
        Some(err) => {
            rsx! {
                div { {err.clone()} }
            }
        }
    }
}
