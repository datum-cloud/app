use dioxus::events::FormEvent;
use dioxus::prelude::*;

use crate::{
    components::{
        dialog::{DialogContent, DialogRoot, DialogTitle},
        input::Input,
        Button, ButtonKind,
    },
    state::AppState,
};

#[component]
pub fn InviteUserDialog(open: ReadSignal<bool>, on_open_change: EventHandler<bool>) -> Element {
    let state = consume_context::<AppState>();
    let mut email = use_signal(String::new);

    // Get selected context (org and project)
    let selected_context = use_memo(move || state.selected_context());

    // Reset form when dialog closes
    use_effect(move || {
        if !open() {
            email.set(String::new());
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

    let email_validation = use_memo(move || validate_email(&email()));
    let email_invalid = use_memo(move || email().trim().is_empty() || email_validation().is_some());

    // Placeholder for invite action - can be implemented later
    let mut invite_user = use_action(move |_| async move {
        let state = consume_context::<AppState>();
        let _ctx = state.selected_context().context("No project selected")?;

        // TODO: Implement actual invite API call using:
        // - ctx.org_id
        // - ctx.project_id
        // - email().trim()

        // For now, just close the dialog
        on_open_change.call(false);
        n0_error::Ok(())
    });

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |v| on_open_change.call(v),
            is_modal: true,
            DialogContent {
                DialogTitle { "Invite a friend" }
                form { class: "space-y-5 mt-5 w-[452px]", autocomplete: "off",
                    Input {
                        id: Some("invite-email".into()),
                        label: Some("Email address".into()),
                        description: Some("The email of the person you’re inviting".into()),
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


                    if let Some(ctx) = selected_context() {
                        div { class: "p-5 rounded-lg bg-content-background flex flex-col gap-3.5",
                            p { class: "text-xs text-foreground",
                                "The Org and Project you’re inviting them to:"
                            }
                            div { class: "flex items-center gap-2",
                                div { class: "w-fit h-6 min-w-0 rounded-md border border-app-border bg-white px-2 text-left text-xs text-foreground focus:outline-none focus:ring-2 focus:ring-app-border inline-flex items-center justify-between gap-2 cursor-default",
                                    "{ctx.org_name}"
                                }
                                span { class: "text-foreground/10 text-md", "/" }
                                div { class: "w-fit h-6 min-w-0 rounded-md border border-app-border bg-white px-2 text-left text-xs text-foreground focus:outline-none focus:ring-2 focus:ring-app-border inline-flex items-center justify-between gap-2 cursor-default",
                                    "{ctx.project_name}"
                                }
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
                            class: if invite_user.pending() || email_invalid() { Some("opacity-60".to_string()) } else { None },
                            onclick: move |_| {
                                if email_invalid() {
                                    return;
                                }
                                invite_user.call(());
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
