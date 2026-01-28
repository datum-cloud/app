use dioxus::prelude::*;
use lib::datum_cloud::LoginState;

use crate::{
    components::{Button, ButtonKind},
    state::AppState,
    Route,
};

#[component]
pub fn Login() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    use_effect(move || {
        if state.datum().login_state() == LoginState::Valid {
            if state.selected_context().is_some() {
                nav.push(Route::ProxiesList {});
            } else {
                nav.push(Route::SelectProject {});
            }
        }
    });

    let mut login = use_action(move |_: ()| async move {
        let state = consume_context::<AppState>();
        let datum = state.datum();
        match datum.login_state() {
            LoginState::Missing => datum.auth().login().await?,
            LoginState::NeedsRefresh => datum.auth().refresh().await?,
            LoginState::Valid => {}
        }
        datum.refresh_orgs_projects_and_validate_context().await?;
        if state.selected_context().is_some() {
            nav.push(Route::ProxiesList {});
        } else {
            nav.push(Route::SelectProject {});
        }
        n0_error::Ok(())
    });

    const HERO_ILLUSTRATION: Asset = asset!("/assets/images/home_hero_illustration.webp");

    rsx! {
        div {
            class: "w-full grid h-screen bg-cover place-items-center",
            style: "background-image: url(\"{HERO_ILLUSTRATION}\");",
            div { class: "p-6 m-6 bg-white rounded-xl border border-[#e3e7ee] shadow-[0_10px_28px_rgba(17,24,39,0.10)] hover:shadow-[0_14px_34px_rgba(17,24,39,0.14)] transition-shadow cursor-pointer",
                h1 {  class: "text-xl font-bold mb-6",
                    "Login to Datum cloud"
                }
                Button {
                    kind: ButtonKind::Primary,
                    class: if login.pending() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                    onclick: move |_| login.call(()),
                    text: if login.pending() { "Waiting for login to be confirmed…".to_string() } else { "Login".to_string() }
                }
                div { class: "py-6",
                    "Upon clicking the login button, the Datum Cloud login page will be opened in your default browser. After logging in you can return to the app."
                }
                if let Some(Err(err)) = login.value() {
                    div { class: "rounded-xl border border-red-200 bg-red-50 p-4 text-red-800",
                        div { class: "text-sm font-semibold", "Failed to login" }
                        div { class: "text-sm mt-1 break-words", "{err}" }
                    }
                }
            }
        }
    }
}
