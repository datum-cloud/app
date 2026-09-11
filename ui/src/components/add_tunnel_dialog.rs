use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::{message_looks_like_quota, TcpProxyData, TunnelSummary};
use open::that;

use crate::{
    components::{
        dialog::{DialogContent, DialogRoot, DialogTitle},
        input::Input,
        Button, ButtonKind,
    },
    state::AppState,
    util::project_quotas_portal_url,
};

/// Strips "http://" or "https://" from the front of a string (case-insensitive).
fn strip_http_scheme(s: &str) -> String {
    let s = s.trim();
    let lower = s.to_lowercase();
    if lower.starts_with("https://") {
        s[8..].trim().to_string()
    } else if lower.starts_with("http://") {
        s[7..].trim().to_string()
    } else {
        s.to_string()
    }
}

/// Validates tunnel address: must be host:port, no http/https scheme.
/// Returns None when empty (no error shown) or when valid; only shows error when there is input that is invalid.
fn validate_tunnel_address(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(
            "Do not include http:// or https:// — use host:port only (e.g. 127.0.0.1:5173)."
                .to_string(),
        );
    }
    match TcpProxyData::from_host_port_str(s) {
        Ok(_) => None,
        Err(e) => Some(format!(
            "Invalid address: {}. Use host:port (e.g. 127.0.0.1:5173).",
            e
        )),
    }
}

#[component]
pub fn AddTunnelDialog(
    /// Pass a signal so the effect re-runs when open/initial_tunnel change and populates the form.
    open: ReadSignal<bool>,
    on_open_change: EventHandler<bool>,
    /// When set, the dialog is in edit mode (tunnel path, e.g. from TunnelBandwidth).
    #[props(optional)]
    initial_tunnel: Option<Signal<Option<TunnelSummary>>>,
    /// Called after a successful save so the parent can refresh the tunnels list.
    on_save_success: EventHandler<()>,
) -> Element {
    let state = consume_context::<AppState>();
    let mut address = use_signal(String::new);
    let mut label = use_signal(String::new);
    let mut basic_auth_enabled = use_signal(|| false);

    // Reset form when dialog closes (after success or cancel) so next open starts clean
    use_effect(move || {
        if !open() {
            label.set(String::new());
            address.set(String::new());
            basic_auth_enabled.set(false);
        }
    });

    use_effect(move || {
        if !open() {
            return;
        }
        let tunnel_opt = initial_tunnel.as_ref().and_then(|s| s());
        if let Some(t) = tunnel_opt {
            label.set(t.label.clone());
            address.set(strip_http_scheme(&t.endpoint));
        } else {
            // Create mode: empty form
            label.set(String::new());
            address.set(String::new());
            basic_auth_enabled.set(false);
        }
    });

    // Create tunnel (same logic as create_proxy.rs)
    let mut save_create_tunnel = use_action(move |_| async move {
        let state = consume_context::<AppState>();
        state.selected_context().context("No project selected")?;
        let tunnel = state
            .create_tunnel(label().trim(), address().trim())
            .await
            .context("Failed to create tunnel")?;
        state.upsert_tunnel(tunnel);
        state.bump_tunnel_refresh();
        on_save_success.call(());
        on_open_change.call(false);
        n0_error::Ok(())
    });

    // Edit tunnel (same logic as edit_proxy.rs)
    let mut save_tunnel = use_action(move |tunnel_id: String| async move {
        let state = consume_context::<AppState>();
        let updated = state
            .update_tunnel(&tunnel_id, label().trim(), address().trim())
            .await
            .context("Failed to update tunnel")?;
        state.upsert_tunnel(updated);
        state.bump_tunnel_refresh();
        on_save_success.call(());
        on_open_change.call(false);
        n0_error::Ok(())
    });

    let is_edit_tunnel = initial_tunnel.as_ref().and_then(|s| s()).is_some();
    let is_edit = is_edit_tunnel;
    let title = if is_edit {
        "Edit tunnel"
    } else {
        "Add a tunnel"
    };
    let submit_label = if is_edit {
        "Save changes"
    } else {
        "Create tunnel"
    };
    let submit_pending_label = if is_edit { "Saving…" } else { "Creating…" };
    let error_title = if is_edit {
        "Couldn't update tunnel"
    } else {
        "Couldn't create tunnel"
    };

    let address_validation = use_memo(move || validate_tunnel_address(&address()));
    let address_invalid =
        use_memo(move || address().trim().is_empty() || address_validation().is_some());

    let quota_exhausted = !is_edit
        && state.tunnel_create_quota()()
            .as_ref()
            .map(|q| q.is_exhausted())
            .unwrap_or(false);
    let quotas_url = state
        .selected_context()
        .map(|ctx| project_quotas_portal_url(state.datum().web_url(), &ctx.project_id));

    let create_blocked = !is_edit && quota_exhausted;
    let submit_disabled = save_tunnel.pending()
        || save_create_tunnel.pending()
        || address_invalid()
        || create_blocked;

    let save_err = save_tunnel
        .value()
        .and_then(|r| r.err())
        .or_else(|| save_create_tunnel.value().and_then(|r| r.err()));
    let save_err_text = save_err.as_ref().map(|e| e.to_string());
    let save_err_is_quota = save_err_text
        .as_ref()
        .is_some_and(|t| !is_edit && message_looks_like_quota(t));

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |v| on_open_change.call(v),
            is_modal: true,
            DialogContent {
                DialogTitle { "{title}" }
                form { class: "space-y-5 mt-5 w-[452px]", autocomplete: "off",
                    if create_blocked {
                        div { class: "rounded-md border border-card-border bg-card-background p-4 shadow-card",
                            div { class: "text-sm font-medium text-foreground",
                                "You've hit your tunnel limit"
                            }
                            div { class: "text-sm mt-1 text-foreground/60",
                                "You can delete a tunnel from the list, or review your quotas in the portal and contact support if you need further assistance."
                            }
                            if let Some(url) = quotas_url.clone() {
                                button {
                                    class: "mt-2 text-sm text-foreground/70 underline underline-offset-2 hover:text-foreground cursor-pointer",
                                    r#type: "button",
                                    onclick: move |_| {
                                        let _ = that(&url);
                                    },
                                    "Review quotas"
                                }
                            }
                        }
                    }
                    Input {
                        id: Some("tunnel-name".into()),
                        label: Some("Display name".into()),
                        description: Some("Your tunnel will also get an auto-generated resource name.".into()),
                        value: "{label}",
                        onchange: move |e: FormEvent| label.set(e.value()),
                    }
                    Input {
                        id: Some("tunnel-address".into()),
                        label: Some("Local address to forward".into()),
                        value: "{address}",
                        placeholder: "e.g. 127.0.0.1:5173",
                        error: address_validation().clone(),
                        autocomplete: "off",
                        autocapitalize: "off",
                        autocorrect: "off",
                        oninput: move |e: FormEvent| address.set(e.value()),
                        onchange: move |e: FormEvent| address.set(e.value()),
                        r#type: "text",
                    }
                    if save_err_is_quota {
                        div { class: "rounded-md border border-card-border bg-card-background p-4 shadow-card",
                            div { class: "text-sm font-medium text-foreground",
                                "You've hit your tunnel limit"
                            }
                            div { class: "text-sm mt-1 text-foreground/60",
                                "You can delete a tunnel from the list, or review your quotas in the portal and contact support if you need further assistance."
                            }
                            if let Some(url) = quotas_url.clone() {
                                button {
                                    class: "mt-2 text-sm text-foreground/70 underline underline-offset-2 hover:text-foreground cursor-pointer",
                                    r#type: "button",
                                    onclick: move |_| {
                                        let _ = that(&url);
                                    },
                                    "Review quotas"
                                }
                            }
                        }
                    } else if let Some(err_text) = save_err_text.clone() {
                        div { class: "rounded-md border border-red-200 bg-red-50 p-4 text-red-800",
                            div { class: "text-sm font-semibold", "{error_title}" }
                            div { class: "text-sm mt-1 break-words", "{err_text}" }
                        }
                    }
                    div { class: "flex items-center gap-2.5 pt-2 justify-start",
                        Button {
                            kind: ButtonKind::Primary,
                            disabled: submit_disabled,
                            class: if submit_disabled { Some("opacity-60".to_string()) } else { None },
                            onclick: move |_| {
                                if submit_disabled {
                                    return;
                                }
                                if let Some(tunnel_id) = initial_tunnel
                                    .as_ref()
                                    .and_then(|s| s())
                                    .map(|t| t.id.clone())
                                {
                                    save_tunnel.call(tunnel_id);
                                } else {
                                    save_create_tunnel.call(());
                                }
                            },
                            text: if save_tunnel.pending() || save_create_tunnel.pending() { submit_pending_label.to_string() } else { submit_label.to_string() },
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
