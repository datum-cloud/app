use crate::{
    components::{
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator,
            DropdownMenuTrigger,
        },
        AddTunnelDialog, Button, ButtonKind, Icon, IconSource, InviteUserDialog,
    },
    state::AppState,
    Route,
};
use dioxus::prelude::*;
use lib::datum_cloud::{LoginState, OrganizationWithProjects};
use open::that;

/// Provided by Sidebar so child routes (e.g. TunnelBandwidth) can open the Add/Edit tunnel dialog.
#[derive(Clone)]
pub struct OpenEditTunnelDialog {
    pub editing_tunnel: Signal<Option<lib::TunnelSummary>>,
    pub dialog_open: Signal<bool>,
}

#[component]
pub fn Chrome() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let auth_changed = consume_context::<Signal<u32>>();
    let _ = auth_changed();
    let mut add_tunnel_dialog_open = use_signal(|| false);
    let mut invite_user_dialog_open = use_signal(|| false);
    let mut editing_tunnel = use_signal(|| None::<lib::TunnelSummary>);

    provide_context(OpenEditTunnelDialog {
        editing_tunnel,
        dialog_open: add_tunnel_dialog_open,
    });

    use_effect(move || {
        // Only redirect if not already on login/signup pages (which are outside this layout)
        if state.datum().login_state() == LoginState::Missing {
            // Don't redirect if we're already on Login or Signup route
            // Those routes are outside the Chrome layout, so this effect won't run for them
            nav.push(Route::Login {});
            return;
        }
        if state.selected_context().is_none() {
            nav.push(Route::SelectProject {});
        }
    });

    rsx! {
        div { class: "h-screen overflow-hidden flex flex-col bg-content-background text-foreground",
            AppHeader { add_tunnel_dialog_open, invite_user_dialog_open }
            div { class: "flex-1 min-h-0 overflow-y-auto py-4 px-4 w-full mx-auto max-w-4xl bg-content-background",
                Outlet::<Route> {}
            }
            AddTunnelDialog {
                open: add_tunnel_dialog_open(),
                on_open_change: move |open| {
                    add_tunnel_dialog_open.set(open);
                    if !open {
                        editing_tunnel.set(None);
                    }
                },
                initial_tunnel: Some(editing_tunnel),
                on_save_success: move |_| {
                    editing_tunnel.set(None);
                },
            }
            InviteUserDialog {
                open: invite_user_dialog_open(),
                on_open_change: move |open| invite_user_dialog_open.set(open),
            }
        }
    }
}

#[derive(Props, Clone)]
pub struct AppHeaderProps {
    add_tunnel_dialog_open: Signal<bool>,
    invite_user_dialog_open: Signal<bool>,
}

impl PartialEq for AppHeaderProps {
    fn eq(&self, other: &Self) -> bool {
        // Signals are Copy types, so we compare them directly
        // For props comparison, this is sufficient since signals are handles
        self.add_tunnel_dialog_open == other.add_tunnel_dialog_open
            && self.invite_user_dialog_open == other.invite_user_dialog_open
    }
}

#[component]
pub fn AppHeader(props: AppHeaderProps) -> Element {
    let mut add_tunnel_dialog_open = props.add_tunnel_dialog_open;
    let mut invite_user_dialog_open = props.invite_user_dialog_open;
    let state = consume_context::<AppState>();
    let auth_changed = consume_context::<Signal<u32>>();
    let _ = auth_changed();
    let auth_state = state.datum().auth_state();
    let nav = use_navigator();
    let mut profile_menu_open = use_signal(|| None::<bool>);
    let mut selected_context = use_signal(|| state.selected_context());
    let mut orgs = use_signal(Vec::<OrganizationWithProjects>::new);
    let mut selected_org_id = use_signal(|| state.selected_context().map(|c| c.org_id));
    let mut selected_project_id = use_signal(|| state.selected_context().map(|c| c.project_id));
    let mut pending_org_switch = use_signal(|| false);
    let state_for_watch = state.clone();
    use_future(move || {
        let state_for_watch = state_for_watch.clone();
        async move {
            let mut ctx_rx = state_for_watch.datum().selected_context_watch();
            let ctx = ctx_rx.borrow().clone();
            selected_context.set(ctx.clone());
            if !pending_org_switch() {
                selected_org_id.set(ctx.as_ref().map(|c| c.org_id.clone()));
                selected_project_id.set(ctx.as_ref().map(|c| c.project_id.clone()));
            }
            loop {
                if ctx_rx.changed().await.is_err() {
                    return;
                }
                let ctx = ctx_rx.borrow().clone();
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
    let user_email = match auth_state.get() {
        Ok(auth) => auth.profile.email.clone(),
        Err(_) => "Not logged in".to_string(),
    };
    let user_avatar_url = match auth_state.get() {
        Ok(auth) => auth.profile.avatar_url.clone(),
        Err(_) => None,
    };
    let mut logout = use_action(move |_: ()| {
        let mut auth_changed = auth_changed;
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
                orgs_snapshot
                    .iter()
                    .find(|org| &org.org.resource_id == org_id)
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
        // App header bar - below titlebar, contains Add tunnel button and user menu
        div { class: "shrink-0 bg-background border-b border-app-border flex items-center w-full mx-auto border-t",
            div { class: "max-w-4xl mx-auto flex items-center justify-between w-full p-4",
                // Left side: Add tunnel button
                if auth_state.get().is_ok() && selected_context.read().is_some() {
                    Button {
                        leading_icon: Some(IconSource::Named("plus".into())),
                        text: "Add New",
                        kind: ButtonKind::Primary,
                        onclick: move |_| add_tunnel_dialog_open.set(true),
                    }
                }
                div { class: "flex-1" }
                // Right side: Org/Project selectors and user menu
                div { class: "flex items-center justify-center gap-3",
                    if auth_state.get().is_ok() {
                        div { class: "relative",
                            DropdownMenu {
                                open: profile_menu_open,
                                default_open: false,
                                on_open_change: move |v| profile_menu_open.set(Some(v)),
                                disabled: use_signal(|| false),
                                DropdownMenuTrigger { class: "flex items-center gap-2 cursor-default focus:outline-2 focus:outline-app-border/50 hover:opacity-80 transition-opacity",
                                    span { class: "text-sm text-foreground font-medium",
                                        "{user_name}"
                                    }
                                    div { class: "w-10 h-10 rounded-lg border border-app-border bg-white flex items-center justify-center overflow-hidden shrink-0",
                                        if let Some(avatar_url) = user_avatar_url.as_ref() {
                                            img {
                                                src: "{avatar_url}",
                                                alt: "User avatar",
                                                class: "w-full h-full object-cover",
                                            }
                                        } else {
                                            svg {
                                                width: "16",
                                                height: "16",
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
                                    }
                                }
                                DropdownMenuContent {
                                    id: use_signal(|| None::<String>),
                                    class: "min-w-44",
                                    div { class: "flex items-start flex-col gap-1 p-2 cursor-default",
                                        span { class: "text-xs", "{user_name}" }
                                        span { class: "text-1xs text-foreground/50",
                                            "{user_email}"
                                        }
                                    }
                                    DropdownMenuSeparator {}
                                    DropdownMenuItem::<String> {
                                        value: use_signal(|| "select_project".to_string()),
                                        index: use_signal(|| 0),
                                        disabled: use_signal(|| false),
                                        on_select: move |_| {
                                            profile_menu_open.set(Some(false));
                                            nav.push(Route::SelectProject {});
                                        },
                                        div { class: "flex flex-col gap-0.5 w-full",
                                            div { class: "flex items-center gap-2",
                                                "Switch Project"
                                            }
                                            if let Some(ctx) = selected_context.read().as_ref() {
                                                div { class: "text-[10px] flex flex-col gap-0.5 max-w-fit",
                                                    span { class: "truncate text-foreground/60",
                                                        "{ctx.org_name}"
                                                    }
                                                    span { class: "text-foreground/40 truncate text-[8px]",
                                                        "{ctx.project_name}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    DropdownMenuSeparator {}
                                    DropdownMenuItem::<String> {
                                        value: use_signal(|| "docs".to_string()),
                                        index: use_signal(|| 1),
                                        disabled: use_signal(|| false),
                                        on_select: move |_| {
                                            profile_menu_open.set(Some(false));
                                            let _ = that("https://www.datum.net/docs/");
                                        },
                                        div { class: "flex items-center gap-2",
                                            Icon {
                                                source: IconSource::Named("book-open".into()),
                                                size: 14,
                                            }
                                            "Docs"
                                        }
                                    }
                                    DropdownMenuItem::<String> {
                                        value: use_signal(|| "invite".to_string()),
                                        index: use_signal(|| 2),
                                        disabled: use_signal(|| false),
                                        on_select: move |_| {
                                            profile_menu_open.set(Some(false));
                                            invite_user_dialog_open.set(true);
                                        },
                                        div { class: "flex items-center gap-2",
                                            Icon {
                                                source: IconSource::Named("users".into()),
                                                size: 14,
                                            }
                                            "Invite"
                                        }
                                    }
                                    DropdownMenuItem::<String> {
                                        value: use_signal(|| "settings".to_string()),
                                        index: use_signal(|| 3),
                                        disabled: use_signal(|| false),
                                        on_select: move |_| {
                                            profile_menu_open.set(Some(false));
                                            nav.push(Route::Settings {});
                                        },
                                        div { class: "flex items-center gap-2",
                                            Icon {
                                                source: IconSource::Named("settings".into()),
                                                size: 14,
                                            }
                                            "Settings"
                                        }
                                    }
                                    DropdownMenuSeparator {}
                                    DropdownMenuItem::<String> {
                                        value: use_signal(|| "logout".to_string()),
                                        index: use_signal(|| 4),
                                        disabled: use_signal(|| false),
                                        on_select: move |_| {
                                            profile_menu_open.set(Some(false));
                                            logout.call(());
                                        },
                                        destructive: true,
                                        "Logout"
                                    }
                                    DropdownMenuSeparator {}
                                    div { class: "px-2 py-1",
                                        div { class: "text-[10px] text-foreground/40 text-left",
                                            "v{env!(\"CARGO_PKG_VERSION\")} (beta)"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
