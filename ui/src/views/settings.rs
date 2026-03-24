use crate::{
    components::{input::Input, Button, ButtonKind, Icon, IconSource},
    state::AppState,
    Route, UpdateCheckContext,
};
use dioxus::prelude::*;
use lib::UpdateChannel;
use open::that;

#[component]
pub fn Settings() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let mut update_ctx = consume_context::<UpdateCheckContext>();
    let auth_state = state.datum().auth_state();
    let first_name: String = match auth_state.get() {
        Ok(auth) => auth.profile.first_name.clone().unwrap_or_default(),
        Err(_) => String::new(),
    };
    let last_name: String = match auth_state.get() {
        Ok(auth) => auth.profile.last_name.clone().unwrap_or_default(),
        Err(_) => String::new(),
    };
    let email = match auth_state.get() {
        Ok(auth) => auth.profile.email.clone(),
        Err(_) => String::new(),
    };
    rsx! {
        div { class: "space-y-5",
            // Back link
            button {
                class: "text-xs text-foreground flex items-center gap-1 mt-2 mb-7",
                onclick: move |_| {
                    let _ = nav.push(Route::ProxiesList {});
                },
                Icon {
                    source: IconSource::Named("chevron-down".into()),
                    class: "rotate-90 text-icon-select",
                    size: 10,
                }
                span { class: "underline", "Back to Tunnels List" }
            }
            div { class: "bg-card-background border border-card-border rounded-lg",
                div { class: "px-4 py-3 border-b border-card-border",
                    h2 { class: "text-sm text-foreground", "Account" }
                }
                div { class: "p-4 flex flex-col gap-2",
                    div { class: "flex items-start gap-2 flex-col w-full",
                        div { class: "flex items-center gap-4 w-full",
                            Input {
                                label: Some("First name".into()),
                                value: "{first_name}",
                                disabled: true,
                            }
                            Input {
                                label: Some("Last name".into()),
                                value: "{last_name}",
                                disabled: true,
                            }
                        }
                        Input {
                            label: Some("Email".into()),
                            value: "{email}",
                            disabled: true,
                        }
                    }
                    a {
                        class: "text-sm text-button-link-foreground cursor-pointer flex items-center gap-2 w-fit mt-2",
                        onclick: move |_| {
                            let _ = that("https://cloud.datum.net/account/general");
                        },
                        "View account details and settings"
                        Icon {
                            source: IconSource::Named("external-link".into()),
                            size: 14,
                            class: "text-icon-select",
                        }
                    }
                }
            }
            div { class: "bg-card-background border border-card-border rounded-lg",
                div { class: "px-4 py-3 border-b border-card-border",
                    h2 { class: "text-sm text-foreground", "Updates" }
                }
                div { class: "p-4 flex flex-col gap-4 max-w-md",
                    div { class: "flex flex-col gap-2",
                        p { class: "text-sm text-foreground",
                            "Current version: v{env!(\"CARGO_PKG_VERSION\")}"
                        }
                        p { class: "text-1xs text-foreground/60",
                            "Datum automatically checks for updates on startup and periodically."
                        }
                        div { class: "flex flex-col gap-2",
                            p { class: "text-sm text-foreground font-medium", "Update channel" }
                            p { class: "text-1xs text-foreground/60",
                                "Stable follows production releases. Beta follows GitHub pre-releases only (not stable); switch to Stable to get a main release."
                            }
                            div { class: "flex gap-2 flex-wrap",
                                Button {
                                    text: "Stable",
                                    kind: if (update_ctx.update_channel)() == UpdateChannel::Stable { ButtonKind::Primary } else { ButtonKind::Outline },
                                    leading_icon: if (update_ctx.update_channel)() == UpdateChannel::Stable {
                                        Some(IconSource::Named("check".into()))
                                    } else {
                                        None
                                    },
                                    onclick: move |_| {
                                        if (update_ctx.update_channel)() == UpdateChannel::Stable {
                                            return;
                                        }
                                        let mut ch_sig = update_ctx.update_channel;
                                        spawn(async move {
                                            let repo = match lib::Repo::open_or_create(lib::Repo::default_location())
                                                .await
                                            {
                                                Ok(r) => r,
                                                Err(e) => {
                                                    tracing::warn!("Failed to open repo for update channel: {e:#}");
                                                    return;
                                                }
                                            };
                                            let checker = lib::UpdateChecker::new(repo);
                                            let mut settings = match checker.load_settings().await {
                                                Ok(s) => s,
                                                Err(e) => {
                                                    tracing::warn!("Failed to load update settings: {e:#}");
                                                    return;
                                                }
                                            };
                                            settings.update_channel = UpdateChannel::Stable;
                                            if let Err(e) = checker.save_settings(&settings).await {
                                                tracing::warn!("Failed to save update channel: {e:#}");
                                                return;
                                            }
                                            ch_sig.set(UpdateChannel::Stable);
                                        });
                                    },
                                }
                                Button {
                                    text: "Beta",
                                    kind: if (update_ctx.update_channel)() == UpdateChannel::Beta { ButtonKind::Primary } else { ButtonKind::Outline },
                                    leading_icon: if (update_ctx.update_channel)() == UpdateChannel::Beta {
                                        Some(IconSource::Named("check".into()))
                                    } else {
                                        None
                                    },
                                    onclick: move |_| {
                                        if (update_ctx.update_channel)() == UpdateChannel::Beta {
                                            return;
                                        }
                                        let mut ch_sig = update_ctx.update_channel;
                                        spawn(async move {
                                            let repo = match lib::Repo::open_or_create(lib::Repo::default_location())
                                                .await
                                            {
                                                Ok(r) => r,
                                                Err(e) => {
                                                    tracing::warn!("Failed to open repo for update channel: {e:#}");
                                                    return;
                                                }
                                            };
                                            let checker = lib::UpdateChecker::new(repo);
                                            let mut settings = match checker.load_settings().await {
                                                Ok(s) => s,
                                                Err(e) => {
                                                    tracing::warn!("Failed to load update settings: {e:#}");
                                                    return;
                                                }
                                            };
                                            settings.update_channel = UpdateChannel::Beta;
                                            if let Err(e) = checker.save_settings(&settings).await {
                                                tracing::warn!("Failed to save update channel: {e:#}");
                                                return;
                                            }
                                            ch_sig.set(UpdateChannel::Beta);
                                        });
                                    },
                                }
                            }
                        }
                        if (update_ctx.downloading)() {
                            p { class: "text-xs text-foreground/80 flex items-center gap-2",
                                Icon {
                                    source: IconSource::Named("loader-circle".into()),
                                    size: 14,
                                    class: "animate-spin",
                                }
                                "Downloading update..."
                            }
                        }
                    }
                    if (update_ctx.has_pending_install)() {
                        div { class: "flex flex-col gap-2 p-3 rounded-lg bg-background/50 border border-app-border",
                            p { class: "text-sm text-foreground font-medium",
                                "Update ready to install"
                            }
                            if let Some(ref info) = (update_ctx.pending_update_info)() {
                                p { class: "text-xs text-foreground/80",
                                    "{info.release_name} (v{info.version})"
                                }
                            }
                            p { class: "text-1xs text-foreground/60",
                                "The update will install when you restart the app. Or install now to restart immediately."
                            }
                            div { class: "flex gap-2",
                                Button {
                                    class: "w-fit",
                                    text: "Install now",
                                    kind: ButtonKind::Primary,
                                    onclick: move |_| {
                                        update_ctx.install_now_trigger.set(true);
                                    },
                                }
                            }
                        }
                    }
                    Button {
                        class: "w-fit",
                        text: if (update_ctx.in_progress)() { "Checking…" } else { "Check for Updates" },
                        kind: ButtonKind::Secondary,
                        trailing_icon: if (update_ctx.in_progress)() { Some(IconSource::Named("loader-circle".into())) } else { None },
                        disabled: (update_ctx.in_progress)(),
                        onclick: move |_| {
                            if !(update_ctx.in_progress)() {
                                update_ctx.trigger.set(true);
                                update_ctx.in_progress.set(true);
                            }
                        },
                    }
                }
            }
        }
    }
}
