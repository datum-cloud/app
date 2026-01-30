use crate::{
    components::{
        AddTunnelDialog, Button, ButtonKind, Icon, IconSource,
        select::{Select, SelectAlign, SelectItemIndicator, SelectList, SelectOptionItem, SelectTrigger, SelectValue},
    },
    state::AppState,
    Route,
};
use lib::datum_cloud::{LoginState, OrganizationWithProjects};
use dioxus::events::MouseEvent;
use dioxus::prelude::*;
use dioxus_desktop::DesktopContext;

/// Provided by Sidebar so child routes (e.g. TunnelBandwidth) can open the Add/Edit tunnel dialog.
#[derive(Clone)]
pub struct OpenEditTunnelDialog {
    pub editing_proxy: Signal<Option<lib::ProxyState>>,
    pub dialog_open: Signal<bool>,
}

#[component]
pub fn Chrome() -> Element {
    rsx! {
        div { class: "h-screen overflow-hidden flex flex-col bg-content-background text-foreground rounded-[12px]",
            HeaderBar {}
            Outlet::<Route> {}
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let mut add_tunnel_dialog_open = use_signal(|| false);
    let editing_proxy = use_signal(|| None::<lib::ProxyState>);

    provide_context(OpenEditTunnelDialog {
        editing_proxy,
        dialog_open: add_tunnel_dialog_open,
    });

    use_effect(move || {
        if state.datum().login_state() == LoginState::Missing {
            nav.push(Route::Login {});
            return;
        }
        if state.selected_context().is_none() {
            nav.push(Route::SelectProject {});
        }
    });

    let route = use_route::<Route>();
    let sidebar_hidden = matches!(route, Route::TunnelBandwidth { .. });
    let sidebar_class = if sidebar_hidden {
        "sidebar-wrapper min-w-[190px] max-w-[190px] shrink-0 flex-none bg-background border-r border-app-border pt-5 pb-6 px-6 flex flex-col transition-transform duration-250 ease-out absolute left-0 -translate-x-[190px] z-10"
    } else {
        "sidebar-wrapper min-w-[190px] max-w-[190px] shrink-0 flex-none bg-background border-r border-app-border pt-5 pb-6 px-6 flex flex-col transition-transform duration-250 ease-out"
    };

    let sidebar = rsx! {
        // Sidebar
        div { class: "{sidebar_class}",
            // Full-width content with equal left/right padding
            div { class: "w-full",
                Button {
                    leading_icon: Some(IconSource::Named("plus".into())),
                    text: "Add tunnel",
                    kind: ButtonKind::Primary,
                    class: "w-full",
                    onclick: move |_| add_tunnel_dialog_open.set(true),
                }
            }

            // Bottom nav (visual-only for now)
            div { class: "w-full mt-auto space-y-4 pl-2",
                div { class: "flex items-center gap-3 cursor-pointer hover:opacity-80 duration-300 text-foreground text-xs",
                    Icon {
                        source: IconSource::Named("book-open".into()),
                        size: 16,
                    }
                    span { class: "text-xs leading-4", "Docs" }
                }
                div { class: "flex items-center gap-3 cursor-pointer hover:opacity-80 duration-300 text-foreground text-xs",
                    Icon { source: IconSource::Named("users".into()), size: 16 }
                    span { class: "text-xs", "Invite" }
                }
                div { class: "flex items-center gap-3 cursor-pointer hover:opacity-80 duration-300 text-foreground text-xs",
                    Icon {
                        source: IconSource::Named("settings".into()),
                        size: 16,
                    }
                    span { class: "text-xs", "Settings" }
                }
            }
        }
    };

    rsx! {
        // Content row
        div { class: "flex flex-1 min-h-0 relative",
            {sidebar}

            // Main content (style background via the class below, e.g. bg-background or bg-white)
            div { class: "flex-1 min-h-0 overflow-y-auto py-4.5 px-4.5 bg-content-background",
                Outlet::<Route> {}
            }

            AddTunnelDialog {
                open: add_tunnel_dialog_open(),
                on_open_change: move |open| add_tunnel_dialog_open.set(open),
                initial_proxy: editing_proxy,
                on_save_success: move |_| {},
            }
        }
    }
}

#[component]
pub fn HeaderBar() -> Element {
    let window = || consume_context::<DesktopContext>();
    let state = consume_context::<AppState>();
    let mut auth_changed = consume_context::<Signal<u32>>();
    let _ = auth_changed();
    let auth_state = state.datum().auth_state();
    let nav = use_navigator();
    let mut menu_open = use_signal(|| false);
    let mut selected_context = use_signal(|| state.selected_context());
    let mut orgs = use_signal(Vec::<OrganizationWithProjects>::new);
    let mut selected_org_id = use_signal(|| state.selected_context().map(|c| c.org_id));
    let mut selected_project_id = use_signal(|| state.selected_context().map(|c| c.project_id));
    let mut pending_org_switch = use_signal(|| false);
    let state_for_watch = state.clone();
    use_future(move || {
        let state_for_watch = state_for_watch.clone();
        async move {
            loop {
                state_for_watch.listen_node().state_updated().await;
                let ctx = state_for_watch.selected_context();
                selected_context.set(ctx.clone());
                if !pending_org_switch() {
                    selected_org_id.set(ctx.as_ref().map(|c| c.org_id.clone()));
                    selected_project_id.set(ctx.as_ref().map(|c| c.project_id.clone()));
                }
            }
        }
    });
    let state_for_orgs = state.clone();
    use_future(move || {
        let state_for_orgs = state_for_orgs.clone();
        async move {
            if state_for_orgs.datum().login_state() != LoginState::Valid {
                return;
            }
            if let Ok(list) = state_for_orgs.datum().orgs_and_projects().await {
                orgs.set(list);
            }
        }
    });
    let user_name = match auth_state.get() {
        Ok(auth) => auth.profile.display_name(),
        Err(_) => "Not logged in".to_string(),
    };

    let mut logout = use_action(move |_: ()| {
        let mut auth_changed = auth_changed.clone();
        async move {
            let state = consume_context::<AppState>();
            state.datum().auth().logout().await?;
            auth_changed.set(auth_changed() + 1);
            nav.push(Route::Login {});
            n0_error::Ok(())
        }
    });

    let orgs_snapshot = orgs.read().clone();
    let selected_org_snapshot = selected_org_id.read().clone();
    let selected_ctx = selected_context.read().clone();
    let org_options: Vec<(String, String)> = if orgs_snapshot.is_empty() {
        selected_ctx
            .as_ref()
            .map(|ctx| vec![(ctx.org_id.clone(), ctx.org_name.clone())])
            .unwrap_or_default()
    } else {
        orgs_snapshot
            .iter()
            .map(|org| (org.org.resource_id.clone(), org.org.display_name.clone()))
            .collect()
    };
    let project_options: Vec<(String, String)> = if orgs_snapshot.is_empty() {
        selected_ctx
            .as_ref()
            .map(|ctx| vec![(ctx.project_id.clone(), ctx.project_name.clone())])
            .unwrap_or_default()
    } else {
        selected_org_snapshot
            .as_ref()
            .and_then(|org_id| {
                orgs_snapshot.iter().find(|org| &org.org.resource_id == org_id)
            })
            .map(|org| {
                org.projects
                    .iter()
                    .map(|p| (p.resource_id.clone(), p.display_name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    rsx! {
        // Custom titlebar (color + height)
        div {
            class: "h-10 shrink-0 bg-background border-b border-app-border flex items-center select-none cursor-grab active:cursor-grabbing",
            onmousedown: move |_| window().drag(),
            // macOS-ish window controls
            div {
                class: "flex items-center gap-2 px-4 cursor-default",
                onmousedown: move |evt: MouseEvent| evt.stop_propagation(),
                button {
                    class: "w-3.5 h-3.5 rounded-full bg-[#ff5f57] border border-black/10 hover:brightness-95 cursor-pointer",
                    onclick: move |_| window().set_visible(false),
                }
                button {
                    class: "w-3.5 h-3.5 rounded-full bg-[#febc2e] border border-black/10 hover:brightness-95 cursor-pointer",
                    onclick: move |_| window().set_minimized(true),
                }
                button {
                    class: "w-3.5 h-3.5 rounded-full bg-[#28c840] border border-black/10 hover:brightness-95 cursor-pointer",
                    onclick: move |_| window().toggle_maximized(),
                }
            }
            div { class: "flex-1" }
            div {
                class: "flex items-center justify-center gap-3 pr-3",
                onmousedown: move |evt: MouseEvent| evt.stop_propagation(),
                if auth_state.get().is_ok() && selected_context.read().is_some() {
                    div { class: "flex items-center justify-center gap-2",
                        div {
                            class: "min-w-0",
                            style: "width: min(max-content, clamp(15ch, 12vw, 22ch));",
                            Select {
                                value: selected_org_id(),
                                on_value_change: move |value: Option<String>| {
                                    let Some(value) = value else { return };
                                    if selected_org_id.read().as_deref() == Some(&value) {
                                        return;
                                    }
                                    selected_org_id.set(Some(value));
                                    selected_project_id.set(None);
                                    pending_org_switch.set(true);
                                },
                                placeholder: "Select org".to_string(),
                                disabled: false,
                                SelectTrigger { SelectValue {} }
                                SelectList {
                                    if org_options.is_empty() {
                                        SelectOptionItem {
                                            value: "".to_string(),
                                            text_value: "No results".to_string(),
                                            index: 0,
                                            disabled: true,
                                            "No results"
                                        }
                                    } else {
                                        for (i , (id , label)) in org_options.clone().into_iter().enumerate() {
                                            SelectOptionItem {
                                                value: id.clone(),
                                                text_value: label.clone(),
                                                index: i,
                                                "{label}"
                                                SelectItemIndicator {}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        span { class: "text-foreground/10 text-md", "/" }
                        div {
                            class: "min-w-0",
                            style: "width: min(max-content, clamp(15ch, 12vw, 22ch));",
                            Select {
                                value: selected_project_id(),
                                on_value_change: move |value: Option<String>| {
                                    let Some(value) = value else { return };
                                    let org_id = match selected_org_id.read().clone() {
                                        Some(id) => id,
                                        None => return,
                                    };
                                    let orgs_snapshot = orgs.read().clone();
                                    let org = orgs_snapshot.iter().find(|org| org.org.resource_id == org_id);
                                    let project = org
                                        .and_then(|o| o.projects.iter().find(|p| p.resource_id == value));
                                    if let (Some(org), Some(project)) = (org, project) {
                                        let ctx = lib::SelectedContext {
                                            org_id: org.org.resource_id.clone(),
                                            org_name: org.org.display_name.clone(),
                                            project_id: project.resource_id.clone(),
                                            project_name: project.display_name.clone(),
                                        };
                                        pending_org_switch.set(false);
                                        spawn({
                                            let state = state.clone();
                                            async move {
                                                let _ = state.set_selected_context(Some(ctx)).await;
                                            }
                                        });
                                    }
                                    selected_project_id.set(Some(value));
                                },
                                placeholder: "Select project".to_string(),
                                disabled: selected_org_id.read().is_none(),
                                SelectTrigger { SelectValue {} }
                                SelectList {
                                    align: Some(SelectAlign::End),
                                    if project_options.is_empty() {
                                        SelectOptionItem {
                                            value: "".to_string(),
                                            text_value: "No results".to_string(),
                                            index: 0,
                                            disabled: true,
                                            "No results"
                                        }
                                    } else {
                                        for (i , (id , label)) in project_options.clone().into_iter().enumerate() {
                                            SelectOptionItem {
                                                value: id.clone(),
                                                text_value: label.clone(),
                                                index: i,
                                                "{label}"
                                                SelectItemIndicator {}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if auth_state.get().is_ok() {
                    // Profile icon (top-right) – only when logged in; wrapper is relative so dropdown anchors correctly
                    div { class: "relative",
                        button {
                            class: "w-6 h-6 rounded-md border border-app-border bg-white flex items-center justify-center cursor-pointer mt-0.5",
                            onclick: move |evt: MouseEvent| {
                                evt.stop_propagation();
                                menu_open.set(!menu_open());
                            },
                            svg {
                                width: "18",
                                height: "18",
                                view_box: "0 0 24 24",
                                fill: "none",
                                path {
                                    d: "M12 12a4 4 0 1 0-4-4 4 4 0 0 0 4 4Z",
                                    stroke: "currentColor",
                                    stroke_width: "1.6",
                                }
                                path {
                                    d: "M4 21c1.6-3.5 4.6-5 8-5s6.4 1.5 8 5",
                                    stroke: "currentColor",
                                    stroke_width: "1.6",
                                    stroke_linecap: "round",
                                }
                            }
                        }

                        if menu_open() {
                            // Full-screen click-catcher so any click outside closes the menu.
                            // This also prevents the card click handler from triggering.
                            div {
                                class: "fixed inset-0 z-40",
                                onclick: move |evt: MouseEvent| {
                                    evt.stop_propagation();
                                    menu_open.set(false);
                                },
                            }
                            div {
                                class: "absolute right-0 mt-2 w-44 rounded-xl border border-app-border bg-white overflow-hidden z-50",
                                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                                button { class: "w-full text-left px-4 py-3 text-sm text-slate-800 hover:bg-gray-50",
                                    {user_name}
                                }
                                button {
                                    class: "w-full text-left px-4 py-3 text-sm text-red-600 hover:bg-red-50",
                                    onclick: move |evt: MouseEvent| {
                                        evt.stop_propagation();
                                        menu_open.set(false);
                                        logout.call(());
                                    },
                                    "Logout"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}


