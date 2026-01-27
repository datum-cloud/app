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
        state
            .listen_node()
            .validate_selected_context(state.datum())
            .await?;
        if state.selected_context().is_some() {
            nav.push(Route::ProxiesList {});
        } else {
            nav.push(Route::SelectProject {});
        }
        n0_error::Ok(())
    });

    // const HERO_ILLUSTRATION: Asset = asset!("/assets/images/home_hero_illustration.webp");

    rsx! {
        div { class: "w-full h-screen bg-cover bg-foreground",
            // style: "background-image: url(\"{HERO_ILLUSTRATION}\");",
            div { class: "flex flex-col items-center justify-center w-64 mx-auto gap-8",
                h1 { class: "text-2xl font-semibold text-center text-background", "Log in to continue" }
                Button {
                    kind: ButtonKind::Secondary,
                    class: if login.pending() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                    onclick: move |_| login.call(()),
                    text: if login.pending() { "Waiting for log in confirmation".to_string() } else { "Take me to datum.net".to_string() },
                }
                div { class: "text-center text-background/70 leading-4 text-xs",
                    "Once you’ve logged in, return back here to continue to Datum Desktop."
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
