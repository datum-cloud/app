use dioxus::prelude::*;
use lib::TunnelSummary;

use crate::components::{
    dialog::{DialogContent, DialogRoot, DialogTitle},
    Button, ButtonKind,
};

#[component]
pub fn DeleteTunnelDialog(
    open: ReadSignal<bool>,
    on_open_change: EventHandler<bool>,
    tunnel: ReadSignal<Option<TunnelSummary>>,
    on_delete: EventHandler<TunnelSummary>,
    delete_pending: ReadSignal<bool>,
    delete_result: ReadSignal<Option<String>>,
) -> Element {
    let tunnel_name = tunnel()
        .as_ref()
        .map(|t| t.label.clone())
        .unwrap_or_default();

    let confirm_delete_handler = move |_| {
        if !delete_pending() {
            if let Some(tunnel_to_delete) = tunnel() {
                on_delete.call(tunnel_to_delete);
            }
        }
    };

    let cancel_delete_handler = move |_| {
        if !delete_pending() {
            on_open_change.call(false);
        }
    };

    // Close dialog when deletion completes successfully
    // If delete_result is None, it means deletion succeeded (no error)
    use_effect(move || {
        if delete_result().is_none() && !delete_pending() {
            on_open_change.call(false);
        }
    });

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |open| {
                if !delete_pending() {
                    on_open_change.call(open);
                }
            },
            is_modal: true,
            DialogContent {
                DialogTitle { "Delete tunnel" }
                div { class: "mt-4 mb-6",
                    p { class: "text-sm text-foreground/80",
                        "Are you sure you want to delete \"{tunnel_name}\"? This action cannot be undone."
                    }
                    if let Some(err) = delete_result() {
                        div { class: "mt-4 rounded-md border border-red-200 bg-red-50 p-3 text-alert-red-dark",
                            div { class: "text-xs font-semibold", "Couldn't delete tunnel" }
                            div { class: "text-xs mt-1 break-words", "{err}" }
                        }
                    }
                }
                div { class: "flex items-center gap-4 justify-end",
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: cancel_delete_handler,
                        text: "Cancel",
                        class: if delete_pending() { Some("opacity-60 cursor-not-allowed".to_string()) } else { None },
                    }
                    Button {
                        kind: ButtonKind::Primary,
                        onclick: confirm_delete_handler,
                        text: if delete_pending() { "Deleting…" } else { "Delete" },
                        class: if delete_pending() { Some("opacity-60 cursor-not-allowed".to_string()) } else { None },
                    }
                }
            }
        }
    }
}
