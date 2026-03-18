use dioxus::prelude::*;
use crate::{
    components::{ Icon, IconSource},
};

#[component]
pub fn InstallingSplash() -> Element {
    const HERO_ILLUSTRATION: Asset = asset!("/assets/images/home_hero_illustration.webp");
    const LOGO: Asset = asset!("/assets/images/logo-datum-dark.svg");

    rsx! {
        div {
            class: "w-full grid h-screen bg-cover place-items-center",
            style: "background-image: url(\"{HERO_ILLUSTRATION}\");",
            div { class: "text-center pb-48 flex flex-col items-center gap-4 text-button-primary-foreground",
                img { class: "w-12 h-12 mx-auto", src: "{LOGO}" }
                p { class: "text-sm ", "Installing update..." }
                Icon {
                    source: IconSource::Named("loader-circle".into()),
                    size: 20,
                }
            }
        }
    }
}
