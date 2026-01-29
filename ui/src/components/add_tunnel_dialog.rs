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

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |v| on_open_change.call(v),
            is_modal: true,
            DialogContent {
                DialogTitle { "{title}" }
                div { class: "space-y-5 mt-5",
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
                        onchange: move |e: FormEvent| address.set(e.value()),
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
                            class: if save_proxy.pending() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                            onclick: move |_| save_proxy.call(initial_proxy().clone()),
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
