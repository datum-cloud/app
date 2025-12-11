use dioxus::prelude::*;

use crate::components::{Head, Splash};
use crate::state::AppState;
use crate::views::{CreateDomain, CreateProxy, JoinProxy, Login, Navbar, Signup, TempProxies};

#[cfg(feature = "desktop")]
use dioxus_desktop::{
    trayicon::{
        menu::{Menu, MenuItem, PredefinedMenuItem},
        Icon, TrayIcon, TrayIconBuilder,
    },
    use_tray_menu_event_handler, use_window,
};

mod components;
mod state;
mod util;
mod views;

/// The Route enum is used to define the structure of internal routes in our app. All route enums need to derive
/// the [`Routable`] trait, which provides the necessary methods for the router to work.
///
/// Each variant represents a different URL pattern that can be matched by the router. If that pattern is matched,
/// the components for that route will be rendered.
#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[route("/login")]
    Login{},
    #[route("/signup")]
    Signup{},
    // The layout attribute defines a wrapper for all routes under the layout. Layouts are great for wrapping
    // many routes with a common UI like a navbar.
    #[layout(Navbar)]
        #[route("/")]
        TempProxies{},
        // The route attribute can include dynamic parameters that implement [`std::str::FromStr`] and [`std::fmt::Display`] with the `:` syntax.
        // In this case, id will match any integer like `/blog/123` or `/blog/-456`.
        #[route("/proxy/create")]
        CreateProxy {},
        #[route("/proxy/join")]
        JoinProxy {},
}

fn main() {
    dotenv::dotenv().ok();

    #[cfg(feature = "desktop")]
    let _tray_icon = init_menu_bar().unwrap();

    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let window = use_window();

    let mut app_state_ready = use_signal(|| false);
    use_future(move || async move {
        let state = AppState::load().await.unwrap();
        provide_context(state);
        app_state_ready.set(true);
    });

    #[cfg(feature = "desktop")]
    use_tray_menu_event_handler(move |event| {
        // The event ID corresponds to the menu item text
        match event.id.0.as_str() {
            "Show Window" => {
                use_window().set_visible(true);
            }
            "Quit" => {
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown menu event: {}", event.id.0);
            }
        }
    });

    if !app_state_ready() {
        return rsx! {
            Head {  }
            Splash {}
        };
    }

    rsx! {
        Head {  }
        button {
            onclick: move |_| {
                window.set_visible(false);
            },
            "hide"
        }
        Router::<Route> {}
    }
}

#[cfg(feature = "desktop")]
fn init_menu_bar() -> anyhow::Result<TrayIcon> {
    // Initialize the tray menu
    let tray_menu = Menu::new();

    // Create menu items with IDs for event handling
    let show_item = MenuItem::new("Show Window", true, None);
    let separator = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Quit", true, None);

    // Build the menu structure
    tray_menu
        .append_items(&[&show_item, &separator, &quit_item])
        .expect("Failed to build tray menu");

    let icon = load_icon_from_file("assets/images/logo-datum-light.png");

    // Build the tray icon
    TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Dioxus Tray App")
        .with_icon(icon)
        .build()
        .context("building tray icon")
}

/// Load an icon from a PNG file
#[allow(dead_code)]
fn load_icon_from_file(path: &str) -> Icon {
    let image = image::open(path)
        .expect("Failed to open icon file")
        .to_rgba8();

    let (width, height) = image.dimensions();
    let rgba = image.into_raw();

    Icon::from_rgba(rgba, width, height).expect("Failed to create icon from image")
}
