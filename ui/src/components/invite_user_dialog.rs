use dioxus::events::FormEvent;
use dioxus::prelude::*;
use open::that;

use lib::datum_apis::user_invitation::RoleReference;
use lib::datum_cloud::RoleSummary;

use crate::{
    components::{
        dialog::{DialogContent, DialogRoot, DialogTitle},
        input::Input,
        select::{
            Select, SelectGroup, SelectItemIndicator, SelectList, SelectOptionItem, SelectTrigger,
            SelectValue,
        },
        Button, ButtonKind, IconSource,
    },
    state::AppState,
};

/// Label for the role in the UI; matches web app: annotations["kubernetes.io/display-name"] ?? displayName ?? name.
fn role_display_label(r: &RoleSummary) -> String {
    r.display_name.as_deref().unwrap_or(&r.name).to_string()
}

/// Description for the role dropdown; uses API description or a fallback for common roles.
fn role_display_description(r: &RoleSummary) -> Option<String> {
    r.description.clone().or_else(|| {
        Some(match r.name.as_str() {
            "Owner" => "Full access to all Datum Cloud resources in the organization".to_string(),
            "Editor" => "Edit access to all Datum Cloud resources in the organization, except managing team members".to_string(),
            "Viewer" => "View access to all Datum Cloud resources in the organization".to_string(),
            _ => return None,
        })
    })
}

#[component]
pub fn InviteUserDialog(open: ReadSignal<bool>, on_open_change: EventHandler<bool>) -> Element {
    let state = consume_context::<AppState>();
    let state_for_watch = state.clone();
    let mut email = use_signal(String::new);
    let mut selected_role_name = use_signal(|| None::<String>);
    let mut show_success = use_signal(|| false);

    // Keep selected context in sync when user switches org (use_memo would stay stale)
    let selected_context_signal = use_signal(|| state.selected_context());
    use_future(move || {
        let state_for_watch = state_for_watch.clone();
        let mut selected_context_signal = selected_context_signal;
        async move {
            let mut rx = state_for_watch.datum().selected_context_watch();
            loop {
                let ctx = rx.borrow().clone();
                selected_context_signal.set(ctx);
                if rx.changed().await.is_err() {
                    return;
                }
            }
        }
    });
    let selected_context = move || selected_context_signal();

    // Fetch roles when dialog is open and we have an org (re-runs when open or context change)
    let state_for_roles = state.clone();
    let roles_resource = use_resource(move || {
        let _ = selected_context_signal(); // re-run when context changes
        let state_for_roles = state_for_roles.clone();
        let ctx_signal = selected_context_signal;
        async move {
            let open = open();
            let org_id = ctx_signal().as_ref().map(|c| c.org_id.clone());
            if !open {
                return Ok::<_, String>(Vec::new());
            }
            let Some(org_id) = org_id else {
                return Ok(Vec::new());
            };
            let datum = state_for_roles.datum();
            datum
                .list_roles(&org_id, None)
                .await
                .map_err(|e| e.to_string())
        }
    });

    // Reset form and success state when dialog closes
    use_effect(move || {
        if !open() {
            email.set(String::new());
            selected_role_name.set(None);
            show_success.set(false);
        }
    });

    // Validate email format
    fn validate_email(email: &str) -> Option<String> {
        let email = email.trim();
        if email.is_empty() {
            return None;
        }
        if !email.contains('@') || !email.contains('.') {
            return Some("Please enter a valid email address.".to_string());
        }
        None
    }

    let can_invite = use_memo(move || {
        selected_context()
            .as_ref()
            .map_or(false, |c| c.can_send_invite())
    });
    let email_validation = use_memo(move || validate_email(&email()));
    let email_invalid = use_memo(move || email().trim().is_empty() || email_validation().is_some());
    let no_role_selected = use_memo(move || selected_role_name().is_none());
    let submit_disabled = use_memo(move || !can_invite() || email_invalid() || no_role_selected());

    let (roles_snapshot, roles_loading) = match roles_resource.read().as_ref() {
        None => (Vec::<lib::datum_cloud::RoleSummary>::new(), true),
        Some(Ok(list)) => (list.clone(), false),
        Some(Err(_)) => (Vec::new(), false),
    };
    // Select expects value: ReadSignal<Option<Option<T>>>
    let select_role_value = use_memo(move || Some(selected_role_name()));

    let mut invite_user = use_action(
        move |(email_value, role_ref): (String, Option<RoleReference>)| async move {
            let state = consume_context::<AppState>();
            let ctx = state.selected_context().context("No org selected")?;
            if !ctx.can_send_invite() {
                n0_error::bail_any!("Invitations are not available for personal organizations");
            }

            state
                .datum()
                .create_user_invitation_org(
                    &ctx.org_id,
                    email_value.trim(),
                    None,
                    None,
                    role_ref.map(|r| vec![r]),
                )
                .await
                .context("Failed to send invitation")?;

            n0_error::Ok(())
        },
    );

    // Show success view when invite completes successfully; cleared when dialog closes
    use_effect(move || {
        if invite_user.value().as_ref().map_or(false, Result::is_ok) {
            show_success.set(true);
        }
    });

    let invite_success =
        show_success() && invite_user.value().as_ref().map_or(false, Result::is_ok);
    let state_for_team_url = state.clone();
    let team_url = use_memo(move || {
        invite_user.value().as_ref().filter(|r| r.is_ok())?;
        let ctx = selected_context()?;
        let base = state_for_team_url.datum().web_url().trim_end_matches('/');
        Some(format!("{base}/org/{}/team", ctx.org_id))
    });

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |v| on_open_change.call(v),
            is_modal: true,
            DialogContent {
                DialogTitle {
                    if invite_success {
                        "Invitation sent"
                    } else {
                        "Invite a friend"
                    }
                }
                if invite_success {
                    div { class: "space-y-5 mt-5 w-[452px]",
                        p { class: "text-sm text-foreground",
                            "You can view and manage invitations for this organization on the Team page in Datum Cloud."
                        }
                        if let Some(url) = team_url() {
                            Button {
                                kind: ButtonKind::Secondary,
                                onclick: move |_| {
                                    let _ = that(url.clone());
                                },
                                text: "Open Datum Cloud",
                                trailing_icon: Some(IconSource::Named("external-link".into())),
                            }
                        }
                        div { class: "flex items-center gap-2.5 pt-2 justify-end",
                            Button {
                                kind: ButtonKind::Primary,
                                onclick: move |_| on_open_change.call(false),
                                text: "Done",
                            }
                        }
                    }
                } else if !can_invite() {
                    div { class: "space-y-5 mt-5 w-[452px]",
                        p { class: "text-sm text-foreground",
                            "Invitations are only available for team organizations, not personal organizations."
                        }
                        div { class: "flex justify-end pt-2",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| on_open_change.call(false),
                                text: "Close",
                            }
                        }
                    }
                } else {
                    form {
                        class: "space-y-5 mt-5 w-[452px]",
                        autocomplete: "off",
                        Input {
                            id: Some("invite-email".into()),
                            label: Some("Email address".into()),
                            value: "{email}",
                            placeholder: "user@example.com",
                            error: email_validation().clone(),
                            autocomplete: "off",
                            autocapitalize: "off",
                            autocorrect: "off",
                            oninput: move |e: FormEvent| email.set(e.value()),
                            onchange: move |e: FormEvent| email.set(e.value()),
                            r#type: "email",
                        }

                        div { class: "flex flex-col gap-2",
                            label { class: "text-xs text-form-label/90", "Role" }
                            Select {
                                value: select_role_value,
                                on_value_change: move |value: Option<String>| {
                                    selected_role_name.set(value);
                                },
                                placeholder: "Select a role".to_string(),
                                disabled: false,
                                SelectTrigger { SelectValue {} }
                                SelectList {
                                    if roles_loading {
                                        SelectOptionItem {
                                            value: "".to_string(),
                                            text_value: "Loading…".to_string(),
                                            index: 0,
                                            disabled: true,
                                            "Loading…"
                                        }
                                    } else if roles_snapshot.is_empty() {
                                        SelectOptionItem {
                                            value: "".to_string(),
                                            text_value: "No roles found".to_string(),
                                            index: 0,
                                            disabled: true,
                                            "No roles found"
                                        }
                                    } else {
                                        SelectGroup {
                                            for (i , r) in roles_snapshot.iter().enumerate() {
                                                SelectOptionItem {
                                                    value: r.name.clone(),
                                                    text_value: role_display_label(r),
                                                    index: i,
                                                    option_class: Some("whitespace-normal flex-nowrap".to_string()),
                                                    div { class: "flex items-center justify-between gap-2 w-full",
                                                        div { class: "flex flex-col gap-0.5 w-full",
                                                            span { class: "font-medium text-foreground",
                                                                "{role_display_label(r)}"
                                                            }
                                                            if let Some(desc) = role_display_description(r) {
                                                                span { class: "text-[11px] text-muted-foreground font-normal text-wrap",
                                                                    "{desc}"
                                                                }
                                                            }
                                                        }
                                                        SelectItemIndicator {}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(ctx) = selected_context() {
                            div { class: "p-5 rounded-lg bg-content-background flex flex-col gap-3.5",
                                p { class: "text-xs text-foreground",
                                    "The org you're inviting them to:"
                                }
                                div { class: "w-fit h-6 min-w-0 rounded-md border border-app-border bg-background px-2 text-left text-xs text-foreground focus:outline-none focus:ring-2 focus:ring-app-border inline-flex items-center justify-between gap-2 cursor-default",
                                    "{ctx.org_name}"
                                }
                            }
                        }
                        if let Some(err) = invite_user.value().and_then(|r| r.err()) {
                            div { class: "rounded-md border border-red-200 bg-red-50 p-4 text-alert-red-dark",
                                div { class: "text-sm font-semibold", "Couldn't send invitation" }
                                div { class: "text-sm mt-1 break-words", "{err}" }
                            }
                        }
                        div { class: "flex items-center gap-2.5 pt-2 justify-start",
                            Button {
                                kind: ButtonKind::Primary,
                                class: if invite_user.pending() || submit_disabled() { Some("opacity-60".to_string()) } else { None },
                                onclick: move |_| {
                                    if submit_disabled() {
                                        return;
                                    }
                                    let role_ref = selected_role_name()
                                        .and_then(|name| {
                                            roles_resource
                                                .read()
                                                .as_ref()
                                                .and_then(|r| r.as_ref().ok())
                                                .and_then(|list| {
                                                    list.iter()
                                                        .find(|r| r.name == name)
                                                        .map(|r| RoleReference {
                                                            name: r.name.clone(),
                                                            namespace: r.namespace.clone(),
                                                        })
                                                })
                                        });
                                    invite_user.call((email(), role_ref));
                                },
                                text: if invite_user.pending() { "Sending…" } else { "Send invite" },
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| on_open_change.call(false),
                                text: "Cancel",
                            }
                        }
                    }
                }
            }
        }
    }
}
