use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::ProxyState;
use open::that;

use crate::{
    components::{
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
        },
        input::Input,
        AddTunnelDialog, Button, ButtonKind, Icon, IconSource, Switch, SwitchThumb,
    },
    state::AppState,
    Route,
};


#[component]
pub fn ProxiesList() -> Element {
    let mut proxies = use_signal(Vec::<ProxyState>::new);

    use_future(move || async move {
        let state = consume_context::<AppState>();
        let node = state.listen_node();
        let updated = node.state_updated();
        tokio::pin!(updated);

        loop {
            let list = node.proxies();
            proxies.set(list);
            (&mut updated).await;
            updated.set(node.state().updated());
        }
    });

    let mut on_delete = use_action(move |proxy: ProxyState| async move {
        let state = consume_context::<AppState>();
        debug!("on delete called: {}", proxy.id());
        state
            .listen_node()
            .remove_proxy(proxy.id())
            .await
            .inspect_err(|err| {
                tracing::warn!("delete tunnel failed: {err:#}");
            })?;
        n0_error::Ok(())
    });
    let on_delete_handler = move |proxy_state: ProxyState| {
        on_delete.call(proxy_state);
    };

    let state = consume_context::<AppState>();
    let first_name = state
        .datum()
        .auth_state()
        .get()
        .ok()
        .and_then(|a| a.profile.first_name.clone())
        .unwrap_or_else(|| "there".to_string());

    const EMPTY_MOON: Asset = asset!("/assets/images/empty-card-moon.png");
    const EMPTY_ROCKS: Asset = asset!("/assets/images/empty-card-rocks.png");

    let mut dialog_open = use_signal(|| false);
    let mut editing_proxy = use_signal(|| None::<ProxyState>);
    let mut search_query = use_signal(|| String::new());

    let show_search = proxies().len() > 2;
    let query = search_query().trim().to_lowercase();
    let filtered_proxies: Vec<ProxyState> = if query.is_empty() {
        proxies().into_iter().collect()
    } else {
        proxies()
            .into_iter()
            .filter(|p| {
                p.info.label().to_lowercase().contains(&query)
                    || p.info.domain().to_lowercase().contains(&query)
                    || p.info.local_url().to_lowercase().contains(&query)
                    || p.info.datum_resource_url().to_lowercase().contains(&query)
            })
            .collect()
    };

    let list = if proxies().is_empty() {
        rsx! {
            div { class: "space-y-5",
                div { class: "relative rounded-lg border border-card-border bg-white h-48 p-10 text-center shadow-card text-foreground flex flex-col items-center justify-center gap-6 overflow-hidden",
                    img {
                        class: "absolute right-0 top-0 h-20 w-auto object-contain pointer-events-none",
                        src: "{EMPTY_MOON}",
                        alt: "",
                    }
                    img {
                        class: "absolute left-0 bottom-0 h-28 w-auto object-contain pointer-events-none",
                        src: "{EMPTY_ROCKS}",
                        alt: "",
                    }
                    div { class: "text-sm mt-2", "Hey {first_name}, let's create your first tunnel" }
                    Button {
                        kind: ButtonKind::Outline,
                        class: "w-fit text-foreground",
                        text: "Add tunnel",
                        leading_icon: Some(IconSource::Named("plus".into())),
                        onclick: move |_| dialog_open.set(true),
                    }
                }
                div { class: "rounded-lg bg-background h-48" }
                div { class: "rounded-lg bg-background h-48" }
            }
        }
    } else {
        rsx! {
            div { class: "space-y-5",
                if show_search {
                    div { class: "mb-4",
                        Input {
                            leading_icon: Some(IconSource::Named("search".into())),
                            placeholder: "Search tunnels...",
                            value: "{search_query}",
                            oninput: move |e: FormEvent| search_query.set(e.value()),
                        }
                    }
                }
                for proxy in filtered_proxies.into_iter() {
                    TunnelCard {
                        key: "{proxy.id()}",
                        proxy,
                        show_view_item: true,
                        show_bandwidth: false,
                        on_delete: on_delete_handler,
                        on_edit: move |p| {
                            editing_proxy.set(Some(p));
                            dialog_open.set(true);
                        },
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "max-w-5xl mx-auto", {list} }
        AddTunnelDialog {
            open: dialog_open,
            on_open_change: move |open| {
                dialog_open.set(open);
                if !open {
                    editing_proxy.set(None);
                }
            },
            initial_proxy: editing_proxy,
            on_save_success: move |_| {
                let state = consume_context::<AppState>();
                proxies.set(state.listen_node().proxies());
            },
        }
    }
}

#[component]
pub fn TunnelCard(
    proxy: ProxyState,
    show_view_item: bool,
    show_bandwidth: bool,
    on_delete: EventHandler<ProxyState>,
    on_edit: EventHandler<ProxyState>,
) -> Element {
    let initial = proxy.clone();
    let mut proxy_signal = use_signal(move || initial);
    let mut menu_open = use_signal(|| None::<bool>);
    let nav = use_navigator();
    // Sync prop into local state when the list refreshes (e.g. after edit).
    use_effect(move || proxy_signal.set(proxy.clone()));

    let mut toggle_action = use_action(move |state: bool| async move {
        let mut proxy = proxy_signal().clone();
        proxy.enabled = state;
        let state = consume_context::<AppState>();
        if let Err(err) = state.listen_node().set_proxy(proxy.clone()).await {
            // TODO: Move into UI
            warn!("Update proxy state failed: {err:#}");
            Err(err)
        } else {
            proxy_signal.set(proxy);
            Ok(())
        }
    });

    let proxy = proxy_signal();
    let enabled = proxy.enabled;

    let wrapper_class = if show_bandwidth {
        "bg-white rounded-lg border border-app-border shadow-none border-b-0 rounded-b-none"
    } else {
        "bg-white rounded-lg border border-app-border shadow-card"
    };

    rsx! {
        div { class: "{wrapper_class}",

            div { class: "",
                // header row: title + toggle
                div { class: "px-4 py-2.5 flex items-center justify-between",
                    h2 { class: "text-md font-normal text-foreground", {proxy.info.label()} }
                    Switch {
                        checked: enabled,
                        on_checked_change: move |next| toggle_action.call(next),
                        SwitchThumb {}
                    }
                }

                // divider under the header (Figma-style)
                div { class: "border-t border-tunnel-card-border" }

                // body: rows + kebab aligned to the right like Figma
                div { class: "p-4 flex items-start justify-between bg-neutral-100/50",
                    div { class: "flex flex-col gap-1.5",
                        div { class: "flex items-center gap-2.5 text-icon-tunnel",
                            Icon {
                                source: IconSource::Named("globe".into()),
                                size: 14,
                            }
                            a {
                                class: "text-xs text-foreground",
                                href: format!("http://{}", proxy.info.domain()),
                                {proxy.info.domain()}
                            }
                        }
                        div { class: "flex items-center gap-2.5 text-icon-tunnel",
                            Icon {
                                source: IconSource::Named("down-right-arrow".into()),
                                size: 14,
                            }
                            a {
                                class: "text-xs text-foreground",
                                href: proxy.info.local_url(),
                                {proxy.info.local_url()}
                            }
                        }
                        div { class: "flex items-center gap-2.5 text-icon-tunnel",
                            Icon {
                                source: IconSource::Named("power-cable".into()),
                                size: 14,
                            }
                            span { class: "text-xs text-foreground", {proxy.info.datum_resource_url()} }
                        }
                    }
                    div { class: "relative",
                        DropdownMenu {
                            open: menu_open,
                            default_open: false,
                            on_open_change: move |v| menu_open.set(Some(v)),
                            disabled: use_signal(|| false),
                            DropdownMenuTrigger { class: "w-8 h-8 rounded-lg border border-app-border text-foreground/60 flex items-center justify-center bg-transparent focus:outline-2 focus:outline-app-border/50",
                                Icon {
                                    source: IconSource::Named("ellipsis".into()),
                                    size: 16,
                                }
                            }
                            DropdownMenuContent { id: use_signal(|| None::<String>), class: "",
                                {
                                    if show_view_item {
                                        rsx! {
                                            DropdownMenuItem::<String> {
                                                value: use_signal(|| "view".to_string()),
                                                index: use_signal(|| 0),
                                                disabled: use_signal(|| false),
                                                on_select: move |_| {
                                                    let id = proxy_signal().id().to_string();
                                                    nav.push(Route::TunnelBandwidth { id });
                                                },
                                                "View"
                                            }
                                        }
                                    } else {
                                        rsx! {}
                                    }
                                }
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "edit".to_string()),
                                    index: use_signal(|| 0),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| on_edit.call(proxy_signal().clone()),
                                    "Edit"
                                }
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "dns".to_string()),
                                    index: use_signal(|| 1),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| {
                                        let url = proxy_signal().info.datum_url();
                                        spawn(async move {
                                            if let Err(e) = that(&url) {
                                                tracing::warn!("Failed to open URL: {e}");
                                            }
                                        });
                                    },
                                    icon: Some(IconSource::Named("external-link".into())),
                                    "DNS Config"
                                }
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "proxy".to_string()),
                                    index: use_signal(|| 2),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| {
                                        let url = proxy_signal().info.datum_url();
                                        spawn(async move {
                                            if let Err(e) = that(&url) {
                                                tracing::warn!("Failed to open URL: {e}");
                                            }
                                        });
                                    },
                                    icon: Some(IconSource::Named("external-link".into())),
                                    "Proxy Config"
                                }
                                DropdownMenuSeparator {}
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "delete".to_string()),
                                    index: use_signal(|| 3),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| on_delete.call(proxy_signal().clone()),
                                    destructive: true,
                                    "Delete"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}