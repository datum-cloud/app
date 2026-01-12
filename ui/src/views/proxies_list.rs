use std::collections::VecDeque;

use chrono::Local;
use dioxus::prelude::*;
use lib::{MetricsUpdate, ProxyState, DATUM_CONNECT_GATEWAY_DOMAIN_NAME};

use crate::{
    components::{Button, BwTsChart, ChartData, CloseButton, Subhead},
    state::AppState,
    Route,
};

#[component]
pub fn TempProxies() -> Element {
    // let mut connections = use_signal(|| Vec::new());
    let mut listeners = use_signal(|| Vec::new());
    let metrics = use_signal(|| {
        let mut metrics = VecDeque::new();
        metrics.push_back(ChartData::default());
        metrics
    });
    use_future(move || async move {
        let state = consume_context::<AppState>();
        let inbound = &state.node().inbound;

        let updated = inbound.state().updated();
        tokio::pin!(updated);

        loop {
            let proxies = inbound.proxies();
            listeners.set(proxies);
            (&mut updated).await;
            updated.set(inbound.state().updated());
        }
    });

    use_future(move || {
        let state = consume_context::<AppState>();
        let mut metrics = metrics.clone();
        async move {
            let mut metrics_sub = state.node().inbound.metrics();
            let mut prior = MetricsUpdate::default();
            while let Ok(update) = metrics_sub.recv().await {
                let mut list = metrics.write();
                list.push_back(ChartData {
                    ts: Local::now(),
                    send: update.send_total - prior.send_total,
                    recv: update.recv_total - prior.recv_total,
                });

                if list.len() > 120 {
                    list.pop_front();
                }
                prior = update;
            }
        }
    });

    rsx! {
        BwTsChart{ data: metrics().into(), }
        // div {
        //     class: "my-5",
        //     div {
        //         class: "flex",
        //         Subhead { text: "Proxies" }
        //         div { class: "flex-grow" }
        //         Button {
        //             to: Some(Route::JoinProxy {  }),
        //             text: "Join Proxy"
        //         }
        //     }
        //     for conn in connections() {
        //         ProxyConnectionItem { conn, connections }
        //     }
        // }

        div {
            class: "my-5",
            div {
                class: "flex",
                Subhead { text: "Listeners" }
                div { class: "flex-grow" }
                Button {
                    to: Some(Route::CreateProxy {  }),
                    text: "Create Proxy"
                }
            }
            for proxy in listeners() {
                ProxyListenerItem { proxy, listeners }
            }
        }
    }
}

#[component]
fn ProxyListenerItem(proxy: ProxyState, listeners: Signal<Vec<ProxyState>>) -> Element {
    let proxy_2 = proxy.clone();
    let proxy_url = format!(
        "http://{}.{}",
        proxy.info.resource_id, DATUM_CONNECT_GATEWAY_DOMAIN_NAME
    );
    let proxy_target = format!("{}:{}", proxy.info.data.host, proxy.info.data.port);

    rsx! {
        div {
            div {
                class: "flex mt-8 gap-10",
                h3 {
                    class: "text-xl flex-grow",
                    "{proxy.info.label()}"
                }
                CloseButton{
                    onclick: move |_| {
                        let proxy_3 = proxy_2.clone();
                        async move {
                            let state = consume_context::<AppState>();
                            let node = state.node();
                            // TODO(b5) - remove unwrap
                            node.inbound.remove_proxy(&proxy_3.info.resource_id).await.unwrap();
                        }
                    },
                }
            }
            div {
                class: "flex gap-10",
                Subhead { text: "{proxy_target}" }
                Link {
                    class: "text-sm block mt-2 pl-20 flex-grow cursor-pointer text-gray-600/80",
                    to: Route::EditProxy { id: proxy.info.resource_id.to_string() },
                    "Edit"
                }
                Subhead { text: "{proxy.info.resource_id}" }
            }

            div {
                class: "flex gap-2 mt-2",

                // Clickable link to open in browser
                button {
                    class: "text-blue-400 hover:text-blue-300 underline text-sm cursor-pointer",
                    onclick: move |_| {
                        let url = proxy_url.clone();
                        spawn(async move {
                            if let Err(e) = open::that(&url) {
                                tracing::error!("Failed to open URL in browser: {}", e);
                            }
                        });
                    },
                    "{proxy_url}"
                }
            }
        }
    }
}
