use dioxus::prelude::*;
use lib::{ConnectionInfo, ListnerInfo};

use crate::{
    components::{Button, CloseButton, Subhead},
    state::AppState,
    Route,
};

#[component]
pub fn TempProxies() -> Element {
    let mut connections = use_signal(|| Vec::new());
    let mut listeners = use_signal(|| Vec::new());
    use_future(move || async move {
        let state = consume_context::<AppState>();
        let node = state.node();
        let conns = node.connections().await;
        connections.set(conns);
        let lstnrs = node.listeners().await;
        listeners.set(lstnrs);
    });

    rsx! {
        div {
            class: "my-5",
            div {
                class: "flex",
                Subhead { text: "Connections" }
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
            for lstn in listeners() {
                ProxyListenerItem { lstn, listeners }
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
                    "{conn.label}"
                }
                CloseButton{
                    onclick: move |_| {
                        let conn_2 = conn.clone();
                        async move {
                            let state = consume_context::<AppState>();
                            let node = state.node();
                            // TODO(b5) - remove unwrap
                            node.disconnect(&conn_2).await.unwrap();

                            // refresh list of connections
                            let conns = node.connections().await;
                            connections.set(conns);
                        }
                    },
                }
            }
            Subhead { text: "{conn_2.addr}" }
            if let Some(ticket) = &conn_2.ticket {
                p {
                    class: "text-sm break-all max-w-2/3 mt-1",
                    "{ticket}"
                }
            }
        }
    }
}

#[component]
fn ProxyListenerItem(lstn: ListnerInfo, listeners: Signal<Vec<ListnerInfo>>) -> Element {
    let lstn_2 = lstn.clone();
    rsx! {
        div {
            div {
                class: "flex mt-8",
                h3 {
                    class: "text-xl flex-grow",
                    "{lstn.label}"
                }
                CloseButton{
                    onclick: move |_| {
                        let lstn_2 = lstn.clone();
                        async move {
                            let state = consume_context::<AppState>();
                            let node = state.node();
                            // TODO(b5) - remove unwrap
                            node.unlisten(&lstn_2).await.unwrap();

                            // refresh list of listeners
                            let lstns = node.listeners().await;
                            listeners.set(lstns);
                        }
                    },
                }
            }
            Subhead { text: "<address goes here>" }
            p {
                class: "text-sm break-all max-w-2/3 mt-1",
                "{lstn_2.ticket}"
            }
        }
    }
}
