use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::{Advertisment, ProxyState, TcpProxyData};

use crate::{
    components::{
        dialog::{DialogContent, DialogRoot, DialogTitle},
        input::Input,
        Button, ButtonKind,
    },
    state::AppState,
};

/// Validates tunnel address: must be host:port, no http/https scheme.
/// Returns None when empty (no error shown) or when valid; only shows error when there is input that is invalid.
fn validate_tunnel_address(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some("Do not include http:// or https:// — use host:port only (e.g. 127.0.0.1:5173).".to_string());
    }
    match TcpProxyData::from_host_port_str(s) {
        Ok(_) => None,
        Err(e) => Some(format!("Invalid address: {}. Use host:port (e.g. 127.0.0.1:5173).", e)),
    }
}

#[component]
pub fn AddTunnelDialog(
    /// Pass a signal so the effect re-runs when open/initial_proxy change and populates the form.
    open: ReadSignal<bool>,
    on_open_change: EventHandler<bool>,
    /// When set, the dialog is in edit mode. Pass a signal so the form sync runs when the dialog opens with a proxy.
    initial_proxy: ReadSignal<Option<ProxyState>>,
    /// Called after a successful save so the parent can refresh the tunnels list.
    on_save_success: EventHandler<()>,
) -> Element {
    let mut address = use_signal(|| String::new());
    let mut label = use_signal(|| String::new());

    use_effect(move || {
        if !open() {
            return;
        }
        if let Some(p) = initial_proxy() {
            label.set(p.info.label().to_string());
            address.set(p.info.service().address());
        } else {
            label.set(String::new());
            address.set(String::new());
        }
    });

    let mut save_proxy = use_action(move |existing: Option<ProxyState>| async move {
        let state = consume_context::<AppState>();
        let service = TcpProxyData::from_host_port_str(&address()).context("Invalid address")?;
        let proxy = match existing {
            Some(proxy) => {
                let info = Advertisment {
                    resource_id: proxy.info.resource_id.clone(),
                    label: Some(label()),
                    data: service,
                };
                ProxyState {
                    info,
                    enabled: proxy.enabled,
                }
            }
            None => {
                let info = Advertisment::new(service, Some(label()));
                ProxyState {
                    info,
                    enabled: true,
                }
            }
        };
        state
            .listen_node()
            .set_proxy(proxy)
            .await
            .context("Failed to save proxy")?;
        on_save_success.call(());
        on_open_change.call(false);
        n0_error::Ok(())
    });

    let is_edit = initial_proxy().is_some();
    let title = if is_edit { "Edit tunnel" } else { "Add a tunnel" };
    let submit_label = if is_edit { "Save changes" } else { "Create tunnel" };
    let submit_pending_label = if is_edit { "Saving…" } else { "Creating…" };
    let error_title = if is_edit { "Couldn't update tunnel" } else { "Couldn't create tunnel" };

    let address_validation = validate_tunnel_address(&address());
    let address_invalid = address().trim().is_empty() || address_validation.is_some();

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
                        description: Some("Your tunnel will also get an auto-generated codename.".into()),
                        value: "{label}",
                        onchange: move |e: FormEvent| label.set(e.value()),
                    }
                    Input {
                        id: Some("tunnel-address".into()),
                        label: Some("Local address to forward".into()),
                        value: "{address}",
                        placeholder: "e.g. 127.0.0.1:5173",
                        error: address_validation.clone(),
                        autocomplete: "off",
                        autocapitalize: "off",
                        autocorrect: "off",
                        onchange: move |e: FormEvent| address.set(e.value()),
                        r#type: "text",
                    }
                    if let Some(Err(err)) = save_proxy.value() {
                        div { class: "rounded-md border border-red-200 bg-red-50 p-4 text-red-800",
                            div { class: "text-sm font-semibold", "{error_title}" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }
                    div { class: "flex items-center gap-4 pt-2 justify-start",
                        Button {
                            kind: ButtonKind::Primary,
                            class: if save_proxy.pending() || address_invalid { Some("opacity-60".to_string()) } else { None },
                            onclick: move |_| {
                                if !address_invalid {
                                    save_proxy.call(initial_proxy().clone());
                                }
                            },
                            text: if save_proxy.pending() { submit_pending_label.to_string() } else { submit_label.to_string() },
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
