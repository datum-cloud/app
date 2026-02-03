use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::{ProxyState, TunnelSummary};

use super::OpenEditTunnelDialog;
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
    let state = consume_context::<AppState>();
    let tunnels = state.tunnel_cache();

    let state_for_future = state.clone();
    use_future(move || {
        let state_for_future = state_for_future.clone();
        async move {
        let mut ctx_rx = state_for_future.datum().selected_context_watch();
        let refresh = state_for_future.tunnel_refresh();
        loop {
            let list = state_for_future
                .tunnel_service()
                .list_active()
                .await
                .unwrap_or_default();
            state_for_future.set_tunnel_cache(list);
            tokio::select! {
                res = ctx_rx.changed() => {
                    if res.is_err() {
                        return;
                    }
                }
                _ = refresh.notified() => {}
            }
        }
        }
    });

    // Important: do async mutations from this parent component scope.
    // If we spawn from inside `TunnelCard` and then optimistically remove the card,
    // Dioxus will drop that scope and cancel the task before it runs.
    let state_for_delete = state.clone();
    let mut on_delete = use_action(move |tunnel: TunnelSummary| {
        let state = state_for_delete.clone();
        async move {
            debug!("on delete called: {}", tunnel.id);
            let outcome = state
                .tunnel_service()
                .delete_active(&tunnel.id)
                .await
                .inspect_err(|err| {
                    tracing::warn!("delete tunnel failed: {err:#}");
                })?;
            if outcome.connector_deleted {
                state
                    .heartbeat()
                    .deregister_project(&outcome.project_id)
                    .await;
            }
            state.remove_tunnel(&tunnel.id);
            state.bump_tunnel_refresh();
            n0_error::Ok(())
        }
    });
    let on_delete_handler = move |tunnel: TunnelSummary| {
        on_delete.call(tunnel);
    };

    let mut open_edit_dialog = consume_context::<OpenEditTunnelDialog>();
    let on_edit_handler = move |tunnel_to_edit: TunnelSummary| {
        open_edit_dialog.editing_tunnel.set(Some(tunnel_to_edit));
        open_edit_dialog.dialog_open.set(true);
    };

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
    let initial_proxy_for_dialog = use_signal(|| None::<ProxyState>);
    let mut search_query = use_signal(|| String::new());

    let show_search = tunnels().len() > 2;
    let query = search_query().trim().to_lowercase();
    let filtered_tunnels: Vec<TunnelSummary> = if query.is_empty() {
        tunnels().into_iter().collect()
    } else {
        tunnels()
            .into_iter()
            .filter(|t| {
                t.label.to_lowercase().contains(&query)
                    || t.id.to_lowercase().contains(&query)
                    || t.endpoint.to_lowercase().contains(&query)
                    || t.hostnames.iter().any(|h| h.to_lowercase().contains(&query))
            })
            .collect()
    };

    let list = if tunnels().is_empty() {
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
                for tunnel in filtered_tunnels.into_iter() {
                    TunnelCard {
                        key: "{tunnel.id}",
                        tunnel,
                        on_delete: on_delete_handler,
                        on_edit: on_edit_handler,
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "max-w-5xl mx-auto", {list} }
        AddTunnelDialog {
            open: dialog_open,
            on_open_change: move |open| dialog_open.set(open),
            initial_proxy: initial_proxy_for_dialog,
            on_save_success: move |_| {
                let state = consume_context::<AppState>();
                state.bump_tunnel_refresh();
            },
        }
    }
}

#[component]
pub fn TunnelCard(
    tunnel: TunnelSummary,
    on_delete: EventHandler<TunnelSummary>,
    #[props(default = true)]
    show_view_item: bool,
    #[props(default = false)]
    show_bandwidth: bool,
    #[props(optional)]
    on_edit: Option<EventHandler<TunnelSummary>>,
) -> Element {
    let tunnel_initial = tunnel.clone();
    let tunnel_for_effect = tunnel.clone();
    let mut tunnel_signal = use_signal(move || tunnel_initial.clone());
    use_effect(move || tunnel_signal.set(tunnel_for_effect.clone()));

    let mut menu_open = use_signal(|| None::<bool>);
    let nav = use_navigator();
    let state = consume_context::<AppState>();

    let mut toggle_action = use_action(move |next_enabled: bool| {
        let state = state.clone();
        async move {
            let tunnel = tunnel_signal().clone();
            let updated = state
                .tunnel_service()
                .set_enabled_active(&tunnel.id, next_enabled)
                .await?;
            if next_enabled {
                if let Some(selected) = state.selected_context() {
                    state.heartbeat().register_project(selected.project_id).await;
                }
            }
            state.upsert_tunnel(updated.clone());
            tunnel_signal.set(updated);
            state.bump_tunnel_refresh();
            n0_error::Ok(())
        }
    });

    let tunnel = tunnel_signal();
    let display_hostname = tunnel
        .hostnames
        .first()
        .cloned()
        .unwrap_or_else(|| tunnel.id.clone());
    let display_hostname_href = display_hostname.clone();
    let display_endpoint = if tunnel.endpoint.is_empty() {
        "unknown".to_string()
    } else {
        tunnel.endpoint.clone()
    };
    let display_endpoint_href = display_endpoint.clone();

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
                    h2 { class: "text-md font-normal text-foreground", {tunnel.label} }
                    Switch {
                        checked: tunnel.enabled,
                        disabled: toggle_action.pending(),
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
                                href: format!("http://{}", display_hostname_href),
                                {display_hostname}
                            }
                        }
                        div { class: "flex items-center gap-2.5 text-icon-tunnel",
                            Icon {
                                source: IconSource::Named("down-right-arrow".into()),
                                size: 14,
                            }
                            a {
                                class: "text-xs text-foreground",
                                href: format!("http://{}", display_endpoint_href),
                                {display_endpoint}
                            }
                        }
                        div { class: "flex items-center gap-2.5 text-icon-tunnel",
                            Icon {
                                source: IconSource::Named("power-cable".into()),
                                size: 14,
                            }
                            span { class: "text-xs text-foreground",
                                {format!("datum://{}", tunnel.id)}
                            }
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
                                                    let id = tunnel_signal().id.clone();
                                                    nav.push(Route::TunnelBandwidth { id });
                                                },
                                                "View"
                                            }
                                        }
                                    } else {
                                        rsx! {
                                            Fragment {}
                                        }
                                    }
                                }
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "edit".to_string()),
                                    index: use_signal(|| 1),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| {
                                        let t = tunnel_signal().clone();
                                        if let Some(ref on_edit) = on_edit {
                                            on_edit.call(t);
                                        }
                                    },
                                    "Edit"
                                }
                                DropdownMenuSeparator {}
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "delete".to_string()),
                                    index: use_signal(|| 2),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| on_delete.call(tunnel_signal().clone()),
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