use dioxus::prelude::*;

// We can import assets in dioxus with the `asset!` macro. This macro takes a path to an asset relative to the crate root.
// The macro returns an `Asset` type that will display as the path to the asset in the browser or a local path in desktop bundles.

// Font files must be referenced via asset!() so they get hashed URLs and are served; CSS url() relative to fonts.css 404s.
const FONT_REGULAR: Asset = asset!("/assets/fonts/AllianceNo1-Regular.ttf");
const FONT_MEDIUM: Asset = asset!("/assets/fonts/AllianceNo1-Medium.ttf");
const FONT_SEMIBOLD: Asset = asset!("/assets/fonts/AllianceNo1-SemiBold.ttf");
const FAVICON_LIGHT: Asset = asset!("/assets/icons/favicon-light-32x32.png");
const FAVICON_DARK: Asset = asset!("/assets/icons/favicon-dark-32x32.png");
// The asset macro also minifies some assets like CSS and JS to make bundled smaller
const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn Head() -> Element {
    let font_face_css = format!(
        r#"@font-face {{ font-family: "Alliance No1"; src: url("{FONT_REGULAR}") format("truetype"); font-weight: 400; font-style: normal; font-display: swap; }}
@font-face {{ font-family: "Alliance No1"; src: url("{FONT_MEDIUM}") format("truetype"); font-weight: 500; font-style: normal; font-display: swap; }}
@font-face {{ font-family: "Alliance No1"; src: url("{FONT_SEMIBOLD}") format("truetype"); font-weight: 600; font-style: normal; font-display: swap; }}"#
    );
    let dark_mode_script = r#"
        (function() {
            function updateDarkClass() {
                const darkModeQuery = window.matchMedia('(prefers-color-scheme: dark)');
                const rootElement = document.querySelector('.theme-alpha') || document.documentElement;
                
                if (darkModeQuery.matches) {
                    rootElement.classList.add('dark');
                } else {
                    rootElement.classList.remove('dark');
                }
            }
            
            // Set initial state immediately
            updateDarkClass();
            
            // Listen for changes
            const darkModeQuery = window.matchMedia('(prefers-color-scheme: dark)');
            if (darkModeQuery.addEventListener) {
                darkModeQuery.addEventListener('change', updateDarkClass);
            } else {
                // Fallback for older browsers
                darkModeQuery.addListener(updateDarkClass);
            }
        })();
    "#;

    rsx! {
        // Light mode favicon (default)
        document::Link { rel: "icon", href: FAVICON_LIGHT }
        // Dark mode favicon (prefers-color-scheme: dark)
        document::Link {
            rel: "icon",
            href: FAVICON_DARK,
            media: "(prefers-color-scheme: dark)"
        }
        document::Style { "{font_face_css}" }
        document::Stylesheet { rel: "stylesheet", href: TAILWIND_CSS }
        document::Stylesheet { rel: "stylesheet", href: MAIN_CSS }
        document::Script { "{dark_mode_script}" }
    }
}
