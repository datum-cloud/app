use dioxus::prelude::*;

#[component]
pub fn Splash() -> Element {
    const HERO_ILLUSTRATION: Asset = asset!("/assets/images/home_hero_illustration.webp");
    const LOGO: Asset = asset!("/assets/images/logo-datum-dark.svg");

    rsx! {
        div {
            class: "w-full grid h-screen bg-cover place-items-center",
            style: "background-image: url(\"{HERO_ILLUSTRATION}\");",
            div { class: "text-center pb-48",
                img { class: "w-12 h-12 mx-auto", src: "{LOGO}" }
            }
        }
    }
}
