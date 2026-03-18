use dioxus::prelude::*;

use crate::components::{Button, ButtonKind};
use crate::PendingUpdate;

#[derive(Props, Clone, PartialEq)]
pub struct UpdateToastProps {
    pub pending: PendingUpdate,
    pub on_later: EventHandler<()>,
    pub on_install_now: EventHandler<()>,
}

#[component]
pub fn UpdateToast(props: UpdateToastProps) -> Element {
    let UpdateToastProps {
        pending,
        on_later,
        on_install_now,
    } = props;

    rsx! {
        div { class: "toast flex gap-3 min-h-0 w-fit", "data-type": "info",
            div { class: "toast-content flex-1 flex flex-col gap-1",
                div { class: "toast-title", "Update ready to install" }
                p { class: "toast-description",
                    "{pending.info.release_name} (v{pending.info.version})"
                }
            }
            div { class: "flex gap-2 justify-end shrink-0",
                Button {
                    text: "Later",
                    kind: ButtonKind::Secondary,
                    onclick: move |_| on_later.call(()),
                }
                Button {
                    text: "Install now",
                    kind: ButtonKind::Primary,
                    onclick: move |_| on_install_now.call(()),
                }
            }
        }
    }
}
