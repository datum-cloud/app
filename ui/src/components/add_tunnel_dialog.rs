use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::{TcpProxyData, TunnelSummary};

use crate::{
    components::{
        dialog::{DialogContent, DialogRoot, DialogTitle},
        input::Input,
        switch::{Switch, SwitchThumb},
        Button, ButtonKind,
    },
    state::AppState,
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
        let project_id = state
            .selected_context()
            .context("No project selected")?
            .project_id;
        let tunnel = state
            .tunnel_service()
            .create_active(label().trim(), address().trim())
            .await
            .context("Failed to create tunnel")?;
        state.upsert_tunnel(tunnel);
        state.bump_tunnel_refresh();
        state.heartbeat().register_project(project_id).await;
        on_save_success.call(());
        on_open_change.call(false);
        n0_error::Ok(())
    });

    // Edit tunnel (same logic as edit_proxy.rs)
    let mut save_tunnel = use_action(move |tunnel_id: String| async move {
        let state = consume_context::<AppState>();
        let updated = state
            .tunnel_service()
            .update_active(&tunnel_id, label().trim(), address().trim())
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

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |v| on_open_change.call(v),
            is_modal: true,
            DialogContent {
                DialogTitle { "{title}" }
                form { class: "space-y-5 mt-5 w-[452px]", autocomplete: "off",
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
                    div { class: "flex flex-col gap-2",
                        div { class: "flex items-center justify-between",
                            label { class: "text-xs text-form-label/90", "Basic authentication" }
                            Switch {
                                checked: basic_auth_enabled(),
                                on_checked_change: move |checked| basic_auth_enabled.set(checked),
                                SwitchThumb {}
                            }
                        }
                        div { class: "text-1xs text-form-description",
                            "We'll automatically generate a username and password for you."
                        }
                    }
                    if let Some(err) = save_tunnel
                        .value()
                        .and_then(|r| r.err())
                        .or_else(|| save_create_tunnel.value().and_then(|r| r.err()))
                    {
                        div { class: "rounded-md border border-red-200 bg-red-50 p-4 text-red-800",
                            div { class: "text-sm font-semibold", "{error_title}" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }
                    div { class: "flex items-center gap-2.5 pt-2 justify-start",
                        Button {
                            kind: ButtonKind::Primary,
                            class: if save_tunnel.pending() || save_create_tunnel.pending() || address_invalid() { Some("opacity-60".to_string()) } else { None },
                            onclick: move |_| {
                                if address_invalid() {
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
