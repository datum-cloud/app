use dioxus::prelude::*;

// We can import assets in dioxus with the `asset!` macro. This macro takes a path to an asset relative to the crate root.
// The macro returns an `Asset` type that will display as the path to the asset in the browser or a local path in desktop bundles.
const FAVICON_LIGHT: Asset = asset!("/assets/icons/favicon-light-32x32.png");
const FAVICON_DARK: Asset = asset!("/assets/icons/favicon-dark-32x32.png");
// The asset macro also minifies some assets like CSS and JS to make bundled smaller
const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn Head() -> Element {
    rsx! {
        // Light mode favicon (default)
        document::Link { rel: "icon", href: FAVICON_LIGHT }
        // Dark mode favicon (prefers-color-scheme: dark)
        document::Link {
            rel: "icon",
            href: FAVICON_DARK,
            media: "(prefers-color-scheme: dark)"
        }
        document::Stylesheet { rel: "stylesheet", href: MAIN_CSS }
        document::Stylesheet { rel: "stylesheet", href: TAILWIND_CSS }
    }
}
