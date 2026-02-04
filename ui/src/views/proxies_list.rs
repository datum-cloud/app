use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::TunnelSummary;
use open::that;

use crate::{
    components::{
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
        },
        input::Input,
        skeleton::Skeleton,
        AddTunnelDialog, DeleteTunnelDialog, Icon, IconSource, Switch, SwitchThumb, Button, ButtonKind,
    },
    state::AppState,
    Route,
};


#[component]
pub fn ProxiesList() -> Element {
    let state = consume_context::<AppState>();
    let tunnels = state.tunnel_cache();
    // Check if we already have cached data - if so, we're already "loaded"
    let has_loaded = use_signal(|| !tunnels().is_empty());

    let state_for_future = state.clone();
    use_future(move || {
        let state_for_future = state_for_future.clone();
        let mut has_loaded_for_future = has_loaded.clone();
        async move {
        let mut ctx_rx = state_for_future.datum().selected_context_watch();
        let refresh = state_for_future.tunnel_refresh();
        loop {
            let list = state_for_future
                .tunnel_service()
                .list_active()
                .await
                .unwrap_or_default();
        // Check if any tunnel is missing a hostname or not yet accepted/programmed.
        // If so, poll more frequently.
        // TODO(zachsmith1): When pending, poll only the specific HTTPProxy
        // resource(s) instead of listing all tunnels each cycle.
        let has_pending_hostname = list.iter().any(|t| t.hostnames.is_empty());
        let has_pending_status = list.iter().any(|t| !t.accepted || !t.programmed);
            state_for_future.set_tunnel_cache(list);
            has_loaded_for_future.set(true);
            
        if has_pending_hostname || has_pending_status {
                // Poll every 3 seconds when waiting for hostname provisioning
                tokio::select! {
                    res = ctx_rx.changed() => {
                        if res.is_err() {
                            return;
                        }
                    }
                    _ = refresh.notified() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                }
            } else {
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
        }
    });

    // Important: do async mutations from this parent component scope.
    // If we spawn from inside `TunnelCard` and then optimistically remove the card,
    // Dioxus will drop that scope and cancel the task before it runs.
    let mut on_delete = use_action(move |tunnel: TunnelSummary| {
        let state = state.clone();
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
    let mut delete_confirm_open = use_signal(|| false);
    let mut tunnel_to_delete = use_signal(|| None::<TunnelSummary>);
    let mut tunnel_pending_delete = use_signal(|| None::<TunnelSummary>);
    
    let on_delete_handler = move |tunnel: TunnelSummary| {
        tunnel_pending_delete.set(Some(tunnel));
        delete_confirm_open.set(true);
    };
    
    // Set tunnel_to_delete when deletion is confirmed (for showing tunnel as deleting)
    // This happens when the dialog calls on_delete
    use_effect(move || {
        if on_delete.pending() {
            if let Some(tunnel) = tunnel_pending_delete() {
                tunnel_to_delete.set(Some(tunnel));
            }
        }
        if let Some(result) = on_delete.value() {
            if result.is_ok() {
                tunnel_to_delete.set(None);
            }
        }
    });

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
    let mut editing_tunnel = use_signal(|| None::<TunnelSummary>);
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
                    || t.hostnames.iter().any(|h| h.to_lowercase().contains(&query))
                    || t.endpoint.to_lowercase().contains(&query)
                    || t.id.to_lowercase().contains(&query)
            })
            .collect()
    };

    let list = if !has_loaded() {
        // Loading state: show 3 skeleton items
        rsx! {
            div { class: "space-y-5",
                for _ in 0..3 {
                    div { class: "bg-white rounded-lg border border-app-border shadow-card",
                        div { class: "px-4 py-2.5 flex items-center justify-between",
                            Skeleton { class: Some("h-5 w-32".to_string()) }
                            Skeleton { class: Some("h-4 w-4 rounded-full".to_string()) }
                        }
                        div { class: "border-t border-tunnel-card-border" }
                        div { class: "p-4 flex items-start justify-between bg-neutral-100/50",
                            div { class: "flex flex-col gap-1.5",
                                div { class: "flex items-center gap-2.5",
                                    Skeleton { class: Some("h-3.5 w-3.5".to_string()) }
                                    Skeleton { class: Some("h-3 w-24".to_string()) }
                                }
                                div { class: "flex items-center gap-2.5",
                                    Skeleton { class: Some("h-3.5 w-3.5".to_string()) }
                                    Skeleton { class: Some("h-3 w-32".to_string()) }
                                }
                                div { class: "flex items-center gap-2.5",
                                    Skeleton { class: Some("h-3.5 w-3.5".to_string()) }
                                    Skeleton { class: Some("h-3 w-28".to_string()) }
                                }
                            }
                            div { class: "relative",
                                Skeleton { class: Some("h-8 w-8 rounded-lg".to_string()) }
                            }
                        }
                    }
                }
            }
        }
    } else if tunnels().is_empty() {
        // Empty state: show empty message
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
        let tunnel_to_delete_for_cards = tunnel_to_delete.clone();
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
                        show_view_item: true,
                        show_bandwidth: false,
                        tunnel_to_delete: tunnel_to_delete_for_cards.clone(),
                        on_delete: on_delete_handler,
                        on_edit: move |t| {
                            editing_tunnel.set(Some(t));
                            dialog_open.set(true);
                        },
                    }
                }
            }
        }
    };

    let tunnel_name_for_dialog = tunnel_pending_delete()
        .as_ref()
        .map(|t| t.label.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "max-w-5xl mx-auto", {list} }
        AddTunnelDialog {
            open: dialog_open,
            on_open_change: move |open| {
                dialog_open.set(open);
                if !open {
                    editing_tunnel.set(None);
                }
            },
            initial_tunnel: editing_tunnel,
            on_save_success: move |_| {
                let state = consume_context::<AppState>();
                state.bump_tunnel_refresh();
            },
        }
        DeleteTunnelDialog {
            open: delete_confirm_open,
            on_open_change: move |open| {
                delete_confirm_open.set(open);
                if !open {
                    tunnel_pending_delete.set(None);
                }
            },
            tunnel: tunnel_pending_delete,
            on_delete: move |tunnel| {
                on_delete.call(tunnel);
            },
            delete_pending: {
                let mut pending_signal: Signal<bool> = use_signal(|| on_delete.pending());
                use_effect(move || {
                    pending_signal.set(on_delete.pending());
                });
                let read_signal: ReadSignal<bool> = pending_signal.into();
                read_signal
            },
            delete_result: {
                let mut error_signal: Signal<Option<String>> = use_signal(|| None::<String>);
                use_effect(move || {
                    if let Some(result) = on_delete.value() {
                        match result {
                            Ok(_) => error_signal.set(None),
                            Err(e) => error_signal.set(Some(e.to_string())),
                        }
                    }
                });
                let read_signal: ReadSignal<Option<String>> = error_signal.into();
                read_signal
            },
        }
    }
}

#[component]
pub fn TunnelCard(
    tunnel: TunnelSummary,
    show_view_item: bool,
    show_bandwidth: bool,
    tunnel_to_delete: ReadSignal<Option<TunnelSummary>>,
    on_delete: EventHandler<TunnelSummary>,
    on_edit: EventHandler<TunnelSummary>,
) -> Element {
    let tunnel_id = tunnel.id.clone();
    let mut menu_open = use_signal(|| None::<bool>);
    let nav = use_navigator();
    let state = consume_context::<AppState>();

    // Read the tunnel from cache using the ID - this ensures we always have fresh data
    // when the cache is updated (e.g., after edit or hostname provisioning)
    let tunnel_cache = state.tunnel_cache();
    let tunnel = tunnel_cache()
        .into_iter()
        .find(|t| t.id == tunnel_id)
        .unwrap_or(tunnel);

    let tunnel_id_for_toggle = tunnel_id.clone();
    let mut toggle_action = use_action(move |next_enabled: bool| {
        let state = state.clone();
        let tunnel_id = tunnel_id_for_toggle.clone();
        async move {
            let updated = state
                .tunnel_service()
                .set_enabled_active(&tunnel_id, next_enabled)
                .await?;
            if next_enabled {
                if let Some(selected) = state.selected_context() {
                    state.heartbeat().register_project(selected.project_id).await;
                }
            }
            state.upsert_tunnel(updated);
            state.bump_tunnel_refresh();
            n0_error::Ok(())
        }
    });
    let enabled = tunnel.enabled;
    let is_ready = tunnel.accepted && tunnel.programmed;
    let proxy_name = tunnel.id.clone();
    let public_hostname = tunnel
        .hostnames
        .iter()
        .find(|h| !h.starts_with("v4.") && !h.starts_with("v6."))
        .cloned()
        .or_else(|| tunnel.hostnames.first().cloned());
    let public_hostname_click = public_hostname.clone();
    let short_id = public_hostname
        .as_ref()
        .and_then(|h| h.split('.').next())
        .map(|s| s.to_string());
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

    // Clone tunnel_id and tunnel before they're moved into closures
    let tunnel_id_for_deleting = tunnel_id.clone();
    let tunnel_id_for_disabled = tunnel_id.clone();
    let tunnel_id_for_view = tunnel_id.clone();
    let tunnel_for_edit = tunnel.clone();
    let tunnel_for_delete = tunnel.clone();
    let tunnel_for_memo = tunnel.clone();

    // Compute is_deleting reactively based on whether this tunnel is being deleted
    // Only show as deleting when deletion has been confirmed (tunnel is in tunnel_to_delete)
    let is_deleting = use_memo(move || {
        tunnel_to_delete()
            .as_ref()
            .map(|t| t.id == tunnel_id_for_deleting)
            .unwrap_or(false)
    });
    
    // Compute is_disabled reactively from tunnel cache and deletion state
    let is_disabled = use_memo(move || {
        let tunnel_from_cache = tunnel_cache()
            .into_iter()
            .find(|t| t.id == tunnel_id_for_disabled)
            .unwrap_or(tunnel_for_memo.clone());
        !(tunnel_from_cache.accepted && tunnel_from_cache.programmed) || is_deleting()
    });

    rsx! {
        div { class: "{wrapper_class} relative",
            if is_disabled() {
                div { class: "absolute inset-0 bg-white/30 rounded-lg z-10 pointer-events-none" }
            }
            div { class: if is_disabled() { "opacity-90" } else { "" },
                // header row: title + toggle
                div { class: "px-4 py-2.5 flex items-center justify-between",
                    h2 { class: "text-md font-normal text-foreground", {tunnel.label.clone()} }
                    if is_ready && !is_deleting() {
                        Switch {
                            checked: enabled,
                            disabled: toggle_action.pending() || is_deleting(),
                            on_checked_change: move |next| toggle_action.call(next),
                            SwitchThumb {}
                        }
                    } else {
                        Icon {
                            source: IconSource::Named("loader-circle".into()),
                            size: 16,
                            class: "animate-spin text-icon-tunnel",
                        }
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
                            span { class: "text-xs text-foreground", {proxy_name} }
                        }
                        div { class: "flex items-center gap-2.5 text-icon-tunnel",
                            Icon {
                                source: IconSource::Named("down-right-arrow".into()),
                                size: 14,
                            }
                            if is_disabled() {
                                span { class: "text-xs text-foreground/80", {display_endpoint} }
                            } else {
                                a {
                                    class: "text-xs text-foreground",
                                    href: format!("http://{}", display_endpoint_href),
                                    {display_endpoint}
                                }
                            }
                        }
                        if let Some(id) = short_id.as_ref() {
                            div { class: "flex items-center gap-2.5 text-icon-tunnel",
                                Icon {
                                    source: IconSource::Named("external-link".into()),
                                    size: 14,
                                }
                                if is_ready {
                                    a {
                                        class: "text-xs text-foreground hover:underline cursor-pointer",
                                        onclick: move |_| {
                                            if let Some(h) = public_hostname_click.as_ref() {
                                                let url = format!("https://{}", h);
                                                let _ = that(&url);
                                            }
                                        },
                                        {format!("datum://{}", id)}
                                    }
                                } else {
                                    span { class: "text-xs text-foreground/80",
                                        {format!("datum://{}", id)}
                                    }
                                }
                            }
                        } else {
                            div { class: "flex items-center gap-2.5 text-icon-tunnel",
                                Icon {
                                    source: IconSource::Named("loader-circle".into()),
                                    size: 14,
                                }
                                span { class: "text-xs text-foreground/90 font-medium",
                                    "Hostname Provisioning..."
                                }
                            }
                        }
                    }
                    div { class: "relative",
                        DropdownMenu {
                            open: menu_open,
                            default_open: false,
                            on_open_change: move |v| menu_open.set(Some(v)),
                            disabled: is_disabled,
                            DropdownMenuTrigger { class: if is_disabled() { "w-8 h-8 rounded-lg border border-app-border text-foreground/50 flex items-center justify-center bg-transparent opacity-70 cursor-not-allowed pointer-events-none" } else { "w-8 h-8 rounded-lg border border-app-border text-foreground/60 flex items-center justify-center bg-transparent focus:outline-2 focus:outline-app-border/50" },
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
                                                disabled: is_disabled,
                                                on_select: move |_| {
                                                    nav.push(Route::TunnelBandwidth {
                                                        id: tunnel_id_for_view.clone(),
                                                    });
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
                                    disabled: is_disabled,
                                    on_select: move |_| on_edit.call(tunnel_for_edit.clone()),
                                    "Edit"
                                }
                                DropdownMenuSeparator {}
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "delete".to_string()),
                                    index: use_signal(|| 2),
                                    disabled: is_disabled,
                                    on_select: move |_| {
                                        on_delete.call(tunnel_for_delete.clone());
                                    },
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
