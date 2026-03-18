use dioxus::prelude::*;
use lib::datum_cloud::LoginState;

use crate::{
    components::{Button, ButtonKind, IconSource},
    state::AppState,
    IsLoginPageSignal,
    Route,
};

#[component]
pub fn Login() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let IsLoginPageSignal(mut is_login_page) = consume_context::<IsLoginPageSignal>();

    use_effect(move || {
        is_login_page.set(true);
    });
    use_drop(move || {
        is_login_page.set(false);
    });
    let state_for_effect = state.clone();
    use_effect(move || {
        if state_for_effect.datum().login_state() == LoginState::Valid {
            // Check registration approval before navigating
            if let Ok(auth) = state_for_effect.datum().auth_state().get() {
                if let Some(approval) = &auth.profile.registration_approval {
                    if approval == "Pending" {
                        // Don't navigate if registration is pending
                        return;
                    }
                }
            }
            if state_for_effect.selected_context().is_some() {
                nav.push(Route::ProxiesList {});
            } else {
                nav.push(Route::SelectProject {});
            }
        }
    });

    let mut login = use_action(move |_: ()| async move {
        let state = consume_context::<AppState>();
        let mut auth_changed = consume_context::<Signal<u32>>();
        let datum = state.datum();
        let state_for_login = state.clone();
        let do_login = move || async move {
            state_for_login.datum().auth()
                .login_with_opener(|url, cancel_token| async move {
                    crate::auth_window::open_auth_window(url, cancel_token)
                })
                .await
        };
        match datum.login_state() {
            LoginState::Missing => do_login().await?,
            LoginState::NeedsRefresh => {
                if datum.auth().refresh().await.is_err() {
                    do_login().await?;
                }
            }
            LoginState::Valid => {}
        }
        // Refresh profile to get latest registration_approval status
        datum.auth().refresh_profile().await?;
        // Increment auth_changed to trigger navbar re-render with user info
        auth_changed.set(auth_changed() + 1);
        datum.refresh_orgs_projects_and_validate_context().await?;

        // Check registration approval before navigating
        if let Ok(auth) = datum.auth_state().get() {
            if let Some(approval) = &auth.profile.registration_approval {
                if approval == "Pending" {
                    // Don't navigate if registration is pending
                    return Ok(());
                }
            }
        }

        if state.selected_context().is_some() {
            nav.push(Route::ProxiesList {});
        } else {
            nav.push(Route::SelectProject {});
        }
        n0_error::Ok(())
    });

    const HERO_ILLUSTRATION: Asset = asset!("/assets/images/login-hero.png");
    const LOGO_DARK: Asset = asset!("/assets/images/logo-datum-dark.svg");

    // Watch auth_changed signal to make registration check reactive
    let _auth_changed = consume_context::<Signal<u32>>();
    let _ = _auth_changed(); // Read the signal to make this reactive

    // Check if registration is pending (clone state since it's moved into closures above)
    let state_for_check = state.clone();
    let datum = state_for_check.datum();
    let auth_state = datum.auth_state();
    let registration_pending = datum.login_state() == LoginState::Valid
        && auth_state
            .get()
            .ok()
            .and_then(|auth| auth.profile.registration_approval.as_ref())
            .map(|approval| approval == "Pending")
            .unwrap_or(false);

    let title_text = if registration_pending {
        if let Ok(auth) = auth_state.get() {
            format!(
                "Hey {}!",
                auth.profile.first_name.as_deref().unwrap_or("there")
            )
        } else {
            "Registration Pending".to_string()
        }
    } else {
        "".to_string()
    };

    rsx! {
        div {
            class: "w-full h-screen bg-bottom bg-no-repeat bg-contain flex items-center justify-center",
            style: "background-image: url(\"{HERO_ILLUSTRATION}\"); background-color: var(--midnight-fjord);",
            div { class: "flex flex-col items-center justify-center w-64 mx-auto gap-4 -mt-[20%]",
                img { class: "w-20 h-20", src: "{LOGO_DARK}", alt: "Datum" }
                h1 {
                    class: "text-2xl font-semibold text-center font-sans",
                    style: "color: var(--glacier-mist-700);",
                    "{title_text}"
                }
                if registration_pending {
                    div {
                        class: "rounded-lg border border-button-secondary-background bg-button-secondary-background/80 p-4 w-full",
                        style: "color: var(--glacier-mist-700);",
                        div { class: "text-sm font-semibold text-center", "Registration Pending" }
                        div { class: "text-sm mt-1 text-center",
                            "Your registration is still in progress."
                        }
                    }
                }
                if !registration_pending {
                    Button {
                        kind: ButtonKind::Secondary,
                        class: if login.pending() { Some("opacity-40 pointer-events-none".to_string()) } else { None },
                        onclick: move |_| login.call(()),
                        text: if login.pending() { "Waiting for log in confirmation".to_string() } else { "Log in to Datum".to_string() },
                        trailing_icon: if login.pending() { Some(IconSource::Named("loader-circle".into())) } else { Some(IconSource::Named("move-right".into())) },
                    }
                }
                if let Some(Err(err)) = login.value() {
                    if !err.to_string().contains("Login cancelled") {
                        div { class: "rounded-xl border border-red-200 bg-red-50 p-4 text-alert-red-dark",
                            div { class: "text-sm font-semibold", "Failed to login" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }
                }
            }
        }
    }
}
