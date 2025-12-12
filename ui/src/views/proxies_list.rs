use std::collections::VecDeque;

use chrono::Local;
use dioxus::prelude::*;
use lib::{ConnectionInfo, Metrics, TcpProxy, DATUM_CONNECT_GATEWAY_DOMAIN_NAME};

use crate::{
    components::{Button, BwTsChart, ChartData, CloseButton, Subhead},
    state::AppState,
    Route,
};

#[component]
pub fn TempProxies() -> Element {
    let mut connections = use_signal(|| Vec::new());
    let mut listeners = use_signal(|| Vec::new());

    use_future(move || async move {
        let state = consume_context::<AppState>();
        let lstnrs = state.node().proxies().await.unwrap();
        listeners.set(lstnrs);
        let conns = state.node().connections().await.unwrap();
        connections.set(conns);
    });

    let metrics = use_signal(|| {
        let mut metrics = VecDeque::new();
        metrics.push_back(ChartData::default());
        metrics
    });
    let mut metrics_2 = metrics.clone();

    use_future(move || {
        let state = consume_context::<AppState>();
        async move {
            let mut metrics_sub = state.node().metrics().await.unwrap();
            let mut prior = Metrics::default();
            while let Ok(metrics) = metrics_sub.recv().await {
                let mut update = metrics_2();
                update.push_back(ChartData {
                    ts: Local::now(),
                    send: metrics.send - prior.send,
                    recv: metrics.recv - prior.recv,
                });

                if update.len() > 120 {
                    update.pop_front();
                }

                metrics_2.set(update);
                prior = metrics;
            }
        }
    });

    rsx! {
        BwTsChart{ data: metrics_2().into(), }
        div {
            class: "my-5",
            div {
                class: "flex",
                Subhead { text: "Published Proxies" }
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

        div {
            class: "my-5",
            div {
                class: "flex",
                Subhead { text: "Subscribed Proxies" }
                div { class: "flex-grow" }
                Button {
                    to: Some(Route::JoinProxy {  }),
                    text: "Join Proxy"
                }
            }
            for conn in connections() {
                ProxyConnectionItem { conn, connections }
            }
        }
    }
}

#[component]
fn ProxyConnectionItem(conn: ConnectionInfo, connections: Signal<Vec<ConnectionInfo>>) -> Element {
    let conn_2 = conn.clone();
    rsx! {
        div {
            div {
                class: "flex mt-8",
                h3 {
                    class: "text-xl flex-grow",
                    "{conn.codename}"
                }
                CloseButton{
                    onclick: move |_| {
                        let conn_2 = conn_2.clone();
                        async move {
                            let state = consume_context::<AppState>();
                            let node = state.node();
                            // TODO(b5) - remove unwrap
                            node.disconnect(&conn_2).await.unwrap();

                            // refresh list of connections
                            let conns = node.connections().await.unwrap();
                            connections.set(conns);
                        }
                    },
                }
            }
            Subhead { text: "{conn.host}:{conn.port}" }
            // if let Some(ticket) = &proxy_2.ticket() {
            //     p {
            //         class: "text-sm break-all max-w-2/3 mt-1",
            //         "{ticket}"
            //     }
            // }
        }
    }
}

#[component]
fn ProxyListenerItem(proxy: TcpProxy, listeners: Signal<Vec<TcpProxy>>) -> Element {
    let proxy_2 = proxy.clone();
    let proxy_url = format!(
        "http://{}.{}",
        proxy.codename, DATUM_CONNECT_GATEWAY_DOMAIN_NAME
    );

    rsx! {
        div {
            div {
                class: "flex mt-8 gap-10",
                h3 {
                    class: "text-xl flex-grow",
                    "{proxy.codename}"
                }
                CloseButton{
                    onclick: move |_| {
                        let proxy_3 = proxy_2.clone();
                        async move {
                            let state = consume_context::<AppState>();
                            let node = state.node();
                            // TODO(b5) - remove unwrap
                            node.stop_listening(&proxy_3).await.unwrap();

                            // refresh list of listeners
                            let lstns = node.proxies().await.unwrap();
                            listeners.set(lstns);
                        }
                    },
                }
            }
            div {
                class: "flex gap-10",
                Subhead { text: "{proxy.host}:{proxy.port}" }
                Link {
                    class: "text-sm block mt-2 pl-20 flex-grow cursor-pointer text-gray-600/80",
                    to: Route::EditProxy { id: proxy.id.to_string() },
                    "Edit"
                }
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

            // p {
            //     class: "text-sm break-all max-w-2/3 mt-1",
            //     "{proxy_2.ticket()}"
            // }
        }
    }
}
