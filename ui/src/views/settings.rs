use crate::{
    components::{input::Input, Icon, IconSource},
    state::AppState,
    Route,
};
use dioxus::prelude::*;
use open::that;

#[component]
pub fn Settings() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
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
            div { class: "bg-white border border-card-border rounded-lg",
                div { class: "px-4 py-3 border-b border-card-border",
                    h2 { class: "text-sm text-foreground", "Account" }
                }
                div { class: "p-4 flex flex-col gap-2",
                    div { class: "flex items-start gap-2 flex-col w-full",
                        div { class: "flex items-center gap-2 justify-between w-full",
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
        }
    }
}
