use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::{
    datum_cloud::LoginState,
    local_tunnels::{default_share_name, extract_ticket, parse_ticket, share_address_error},
    message_looks_like_quota, TcpProxyData, TunnelSummary,
};
use open::that;

use crate::{
    components::{
        dialog::{DialogContent, DialogRoot, DialogTitle},
        input::Input,
        Button, ButtonKind,
    },
    state::AppState,
    util::project_quotas_portal_url,
    views::LocalTicketTarget,
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

/// What a new tunnel does. Only public tunnels need a Datum project.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NewTunnelKind {
    /// An HTTPS hostname through the selected project.
    Public,
    /// A project-less share that devices pair with using a ticket.
    Share,
    /// A project-less connection to a service another device shares.
    Connect,
}

impl NewTunnelKind {
    fn title(self) -> &'static str {
        match self {
            Self::Public => "Public URL",
            Self::Share => "Share with devices",
            Self::Connect => "Open a ticket",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Public => {
                "Publish a local service on an HTTPS hostname through your Datum project."
            }
            Self::Share => {
                "Pair your phone or another computer using a ticket. No account needed; traffic goes device to device."
            }
            Self::Connect => {
                "Open a service another device shares, on a port on this computer. No account needed."
            }
        }
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
    let LocalTicketTarget(mut ticket_target) = consume_context::<LocalTicketTarget>();
    let mut address = use_signal(String::new);
    let mut label = use_signal(String::new);
    let mut basic_auth_enabled = use_signal(|| false);
    let mut kind = use_signal(|| NewTunnelKind::Share);
    let mut ticket = use_signal(String::new);
    let mut port = use_signal(String::new);

    let state_for_kind = state.clone();
    let can_publish = move || {
        state_for_kind.datum().login_state() != LoginState::Missing
            && state_for_kind.selected_context().is_some()
    };

    // Reset form when dialog closes (after success or cancel) so next open starts clean
    use_effect(move || {
        if !open() {
            label.set(String::new());
            address.set(String::new());
            basic_auth_enabled.set(false);
            ticket.set(String::new());
            port.set(String::new());
        }
    });

    let can_publish_on_open = can_publish.clone();
    use_effect(move || {
        if !open() {
            return;
        }
        let tunnel_opt = initial_tunnel.as_ref().and_then(|s| s());
        if let Some(t) = tunnel_opt {
            kind.set(NewTunnelKind::Public);
            label.set(t.label.clone());
            address.set(strip_http_scheme(&t.endpoint));
        } else {
            // Create mode: empty form, defaulting to what this user can create
            kind.set(if can_publish_on_open() {
                NewTunnelKind::Public
            } else {
                NewTunnelKind::Share
            });
            label.set(String::new());
            address.set(String::new());
            basic_auth_enabled.set(false);
            ticket.set(String::new());
            port.set(String::new());
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

    // Create a local tunnel. It starts at once; a new share opens its ticket.
    let mut save_local = use_action(move |new_kind: NewTunnelKind| async move {
        let state = consume_context::<AppState>();
        let tunnels = state.local_tunnels();
        match new_kind {
            NewTunnelKind::Share => {
                let id = tunnels.add_share(&label(), &address())?;
                ticket_target.set(Some(id));
            }
            NewTunnelKind::Connect => {
                let chosen_port = port().parse::<u16>().ok();
                tunnels.add_connect(&ticket(), &label(), chosen_port)?;
            }
            NewTunnelKind::Public => n0_error::bail_any!("Public tunnels need a project"),
        }
        on_open_change.call(false);
        n0_error::Ok(())
    });

    let is_edit_tunnel = initial_tunnel.as_ref().and_then(|s| s()).is_some();
    let is_edit = is_edit_tunnel;
    let current_kind = kind();
    let publish_allowed = can_publish();
    let title = if is_edit {
        "Edit tunnel"
    } else {
        "Add a tunnel"
    };
    let submit_label = match (is_edit, current_kind) {
        (true, _) => "Save changes",
        (false, NewTunnelKind::Public) => "Create tunnel",
        (false, NewTunnelKind::Share) => "Start sharing",
        (false, NewTunnelKind::Connect) => "Connect",
    };
    let submit_pending_label = match (is_edit, current_kind) {
        (true, _) => "Saving…",
        (false, NewTunnelKind::Public) => "Creating…",
        (false, NewTunnelKind::Share) => "Starting…",
        (false, NewTunnelKind::Connect) => "Connecting…",
    };
    let error_title = match (is_edit, current_kind) {
        (true, _) => "Couldn't update tunnel",
        (false, NewTunnelKind::Connect) => "Couldn't connect",
        (false, NewTunnelKind::Share) => "Couldn't share",
        (false, NewTunnelKind::Public) => "Couldn't create tunnel",
    };

    let address_validation = use_memo(move || match kind() {
        NewTunnelKind::Share if !address().trim().is_empty() => share_address_error(&address()),
        _ => validate_tunnel_address(&address()),
    });
    let address_invalid =
        use_memo(move || address().trim().is_empty() || address_validation().is_some());

    // Local tunnel names are optional; a chosen one must be unique.
    let state_for_name = state.clone();
    let local_name_error = use_memo(move || {
        let name = label();
        if kind() == NewTunnelKind::Public || name.trim().is_empty() {
            return None;
        }
        state_for_name.local_tunnels().name_error(&name, None)
    });
    let parsed_ticket = use_memo(move || {
        let text = ticket();
        let text = text.trim();
        (!text.is_empty()).then(|| parse_ticket(text))
    });
    let state_for_port = state.clone();
    let default_port = use_memo(move || match parsed_ticket() {
        Some(Ok(info)) => Some(state_for_port.local_tunnels().default_port(info.port)),
        _ => None,
    });
    let state_for_used = state.clone();
    let port_error = use_memo(move || {
        let text = port();
        if text.is_empty() {
            return None;
        }
        match text.parse::<u16>() {
            Ok(p) if p == 0 => Some("Use a port from 1 to 65535".to_string()),
            Ok(p) if state_for_used.local_tunnels().used_ports().contains(&p) => {
                Some(format!("Another local tunnel uses port {p}"))
            }
            Ok(_) => None,
            Err(_) => Some("Use a port from 1 to 65535".to_string()),
        }
    });

    let quota_exhausted = !is_edit
        && state.tunnel_create_quota()()
            .as_ref()
            .map(|q| q.is_exhausted())
            .unwrap_or(false);
    let quotas_url = state
        .selected_context()
        .map(|ctx| project_quotas_portal_url(state.datum().web_url(), &ctx.project_id));

    let create_blocked = !is_edit && current_kind == NewTunnelKind::Public && quota_exhausted;
    let any_pending =
        save_tunnel.pending() || save_create_tunnel.pending() || save_local.pending();
    let form_invalid = match current_kind {
        NewTunnelKind::Public => address_invalid() || create_blocked || !publish_allowed,
        NewTunnelKind::Share => address_invalid() || local_name_error().is_some(),
        NewTunnelKind::Connect => {
            !matches!(parsed_ticket(), Some(Ok(_)))
                || local_name_error().is_some()
                || port_error().is_some()
        }
    };
    let submit_disabled = any_pending || form_invalid;

    let save_err = save_tunnel
        .value()
        .and_then(|r| r.err())
        .or_else(|| save_create_tunnel.value().and_then(|r| r.err()))
        .or_else(|| save_local.value().and_then(|r| r.err()));
    let save_err_text = save_err.as_ref().map(|e| e.to_string());
    let save_err_is_quota = save_err_text.as_ref().is_some_and(|t| {
        !is_edit && current_kind == NewTunnelKind::Public && message_looks_like_quota(t)
    });

    let share_name_placeholder = if address_invalid() {
        "Computer name and port".to_string()
    } else {
        default_share_name(&address())
    };
    let ticket_hint = match parsed_ticket() {
        None => "Paste the ticket from the computer that shares the service.".to_string(),
        Some(Ok(info)) => format!(
            "Port {} on {}…",
            info.port,
            info.endpoint_id.chars().take(12).collect::<String>()
        ),
        Some(Err(message)) => message,
    };
    let ticket_invalid = matches!(parsed_ticket(), Some(Err(_)));
    let port_placeholder = match default_port() {
        Some(p) if p != 0 => p.to_string(),
        _ => "Any free port".to_string(),
    };

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |v| on_open_change.call(v),
            is_modal: true,
            DialogContent {
                DialogTitle { "{title}" }
                form { class: "space-y-5 mt-5 w-[452px]", autocomplete: "off",
                    if !is_edit {
                        div { class: "space-y-2",
                            div {
                                class: "grid grid-cols-3 gap-1 rounded-lg border border-app-border bg-content-background p-1",
                                role: "radiogroup",
                                for option in [NewTunnelKind::Public, NewTunnelKind::Share, NewTunnelKind::Connect] {
                                    {
                                        let selected = option == current_kind;
                                        let unavailable = option == NewTunnelKind::Public && !publish_allowed;
                                        let class = if selected {
                                            "rounded-md bg-card-background shadow-card px-2 py-1.5 text-xs font-medium text-foreground cursor-default"
                                        } else if unavailable {
                                            "rounded-md px-2 py-1.5 text-xs text-foreground/30 cursor-not-allowed"
                                        } else {
                                            "rounded-md px-2 py-1.5 text-xs text-foreground/60 hover:text-foreground cursor-pointer"
                                        };
                                        rsx! {
                                            button {
                                                key: "{option.title()}",
                                                r#type: "button",
                                                role: "radio",
                                                aria_checked: "{selected}",
                                                class: "{class}",
                                                disabled: unavailable,
                                                title: if unavailable { "Log in to Datum and pick a project to create a public URL." } else { "" },
                                                onclick: move |_| kind.set(option),
                                                "{option.title()}"
                                            }
                                        }
                                    }
                                }
                            }
                            p { class: "text-1xs text-form-description leading-relaxed",
                                "{current_kind.description()}"
                            }
                        }
                    }
                    match current_kind {
                        NewTunnelKind::Public => rsx! {
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
                        },
                        NewTunnelKind::Share => rsx! {
                            Input {
                                id: Some("share-address".into()),
                                label: Some("Local address to share".into()),
                                value: "{address}",
                                placeholder: "e.g. 127.0.0.1:5173",
                                error: address_validation().clone(),
                                autocomplete: "off",
                                autocapitalize: "off",
                                autocorrect: "off",
                                oninput: move |e: FormEvent| address.set(e.value()),
                                r#type: "text",
                            }
                            Input {
                                id: Some("share-name".into()),
                                label: Some("Name (optional)".into()),
                                description: Some("Paired devices show the share by this name.".into()),
                                error: local_name_error(),
                                value: "{label}",
                                placeholder: "{share_name_placeholder}",
                                maxlength: "64",
                                oninput: move |e: FormEvent| label.set(e.value()),
                            }
                        },
                        NewTunnelKind::Connect => rsx! {
                            div { class: "flex flex-col gap-2",
                                label { r#for: "connect-ticket", class: "text-xs text-form-label/90", "Ticket" }
                                textarea {
                                    id: "connect-ticket",
                                    class: if ticket_invalid { "w-full h-20 resize-none rounded-lg border border-alert-red-dark bg-card-background px-2 py-2 font-mono text-1xs text-foreground placeholder:text-form-description focus:outline-none focus:ring-2 focus:ring-alert-red-dark" } else { "w-full h-20 resize-none rounded-lg border border-app-border bg-card-background px-2 py-2 font-mono text-1xs text-foreground placeholder:text-form-description focus:outline-none focus:ring-2 focus:ring-app-border" },
                                    placeholder: "DCSHARE2…",
                                    spellcheck: "false",
                                    autocapitalize: "off",
                                    autocomplete: "off",
                                    value: "{ticket}",
                                    oninput: move |e: FormEvent| {
                                        let text = e.value();
                                        ticket.set(extract_ticket(&text).unwrap_or(text));
                                    },
                                }
                                div { class: if ticket_invalid { "text-1xs text-alert-red-dark" } else { "text-1xs text-form-description" },
                                    "{ticket_hint}"
                                }
                            }
                            Input {
                                id: Some("connect-name".into()),
                                label: Some("Name (optional)".into()),
                                error: local_name_error(),
                                value: "{label}",
                                placeholder: "Name from the sharing computer",
                                maxlength: "64",
                                oninput: move |e: FormEvent| label.set(e.value()),
                            }
                            Input {
                                id: Some("connect-port".into()),
                                label: Some("Local port (optional)".into()),
                                description: Some("The service opens at http://127.0.0.1 on this port.".into()),
                                error: port_error(),
                                value: "{port}",
                                placeholder: "{port_placeholder}",
                                inputmode: "numeric",
                                oninput: move |e: FormEvent| {
                                    port.set(e.value().chars().filter(char::is_ascii_digit).take(5).collect());
                                },
                            }
                        },
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
                                } else if current_kind == NewTunnelKind::Public {
                                    save_create_tunnel.call(());
                                } else {
                                    save_local.call(current_kind);
                                }
                            },
                            text: if any_pending { submit_pending_label.to_string() } else { submit_label.to_string() },
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
