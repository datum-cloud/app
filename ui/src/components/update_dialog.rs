use dioxus::prelude::*;
use lib::UpdateInfo;
use open::that;

use crate::components::{
    dialog::{DialogContent, DialogRoot, DialogTitle},
    Button, ButtonKind,
};

#[derive(Props, Clone, PartialEq)]
pub struct UpdateDialogProps {
    pub open: Signal<bool>,
    pub update_info: UpdateInfo,
    pub on_restart: EventHandler<()>,
    pub on_dismiss: EventHandler<()>,
}

#[component]
pub fn UpdateDialog(props: UpdateDialogProps) -> Element {
    let UpdateDialogProps {
        mut open,
        update_info,
        on_restart,
        on_dismiss,
    } = props;

    rsx! {
        DialogRoot {
            open: open(),
            on_open_change: move |is_open: bool| {
                open.set(is_open);
                if !is_open {
                    on_dismiss.call(());
                }
            },
            DialogContent { class: "max-w-md",
                DialogTitle { "Update Available" }
                div { class: "flex flex-col gap-4",
                    p { class: "text-sm text-foreground", "A new version of Datum is available:" }
                    div { class: "bg-background/50 rounded-lg p-4 border border-app-border",
                        div { class: "flex flex-col gap-1",
                            div { class: "font-medium text-sm text-foreground",
                                "{update_info.release_name}"
                            }
                            div { class: "text-1xs text-foreground/60", "Version {update_info.version}" }
                            div { class: "text-1xs text-foreground/60",
                                "Published {update_info.published_at.format(\"%B %d, %Y\")}"
                            }
                        }
                    }
                    div { class: "flex gap-2 justify-start",
                        Button {
                            text: "Later",
                            kind: ButtonKind::Secondary,
                            onclick: move |_| {
                                open.set(false);
                                on_dismiss.call(());
                            },
                        }
                        Button {
                            text: "Download Update",
                            kind: ButtonKind::Primary,
                            onclick: move |_| {
                                // Link to the GitHub releases page for the rolling tag
                                let releases_url = "https://github.com/datum-cloud/app/releases/tag/rolling";
                                let _ = that(releases_url);
                                open.set(false);
                                on_dismiss.call(());
                            },
                        }
                    }
                }
            }
        }
    }
}
