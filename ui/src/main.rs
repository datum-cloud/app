use dioxus::prelude::*;
use std::sync::OnceLock;
use tracing::info;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
#[cfg(feature = "desktop")]
use n0_error::Result;

use crate::components::{Head, Splash};
use crate::state::AppState;
use crate::views::{
    Chrome, CreateProxy, EditProxy, JoinProxy, Login, ProxiesList, SelectProject, Sidebar, Signup,
    TunnelBandwidth,
};

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

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// The Route enum is used to define the structure of internal routes in our app. All route enums need to derive
/// the [`Routable`] trait, which provides the necessary methods for the router to work.
///
/// Each variant represents a different URL pattern that can be matched by the router. If that pattern is matched,
/// the components for that route will be rendered.
#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Chrome)]
    #[route("/")]
    Login{},
    #[route("/select")]
    SelectProject{},
    #[route("/signup")]
    Signup{},
    // The layout attribute defines a wrapper for all routes under the layout. Layouts are great for wrapping
    // many routes with a common UI like a navbar.
    #[layout(Sidebar)]
        #[route("/proxies")]
        ProxiesList {},
        // The route attribute can include dynamic parameters that implement [`std::str::FromStr`] and [`std::fmt::Display`] with the `:` syntax.
        // In this case, id will match any integer like `/blog/123` or `/blog/-456`.
        #[route("/proxy/create")]
        CreateProxy {},
        #[route("/proxy/edit/:id")]
        EditProxy { id: String },
        #[route("/proxy/edit/:id/bandwidth")]
        TunnelBandwidth { id: String },
        #[route("/proxy/join")]
        JoinProxy {},
}

fn main() {
    init_tracing();
    if let Some(path) = dotenv::dotenv().ok() {
        info!("Loaded environment variables from {}", path.display());
    }

    #[cfg(all(feature = "desktop", target_os = "linux"))]
    gtk::init().unwrap();

    #[cfg(feature = "desktop")]
    let _tray_icon = init_menu_bar().unwrap();

    #[cfg(feature = "desktop")]
    {
        // Use a custom titlebar so we can theme the top chrome (height + color).
        use dioxus_desktop::{Config, LogicalSize, WindowBuilder, WindowCloseBehaviour};

        dioxus::LaunchBuilder::desktop()
            .with_cfg(desktop! {
                Config::new()
                    // Make "close" behave like hide, so the tray icon can restore it.
                    .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                    .with_window(
                        WindowBuilder::new()
                            .with_title("Datum Connect")
                            .with_inner_size(LogicalSize::new(630, 600))  // default width, height (logical pixels)
                            .with_min_inner_size(LogicalSize::new(630, 600))  // prevent resizing smaller
                            // Required for rounded app chrome: we render our own rounded container inside.
                            .with_transparent(true)
                            .with_decorations(false),
                    )
            })
            .launch(App);
    }

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(App);
}

fn init_tracing() {
    let repo_path = lib::Repo::default_location();
    if let Err(err) = std::fs::create_dir_all(&repo_path) {
        eprintln!("ui: failed to create repo dir {}: {err}", repo_path.display());
    }
    let file_appender = tracing_appender::rolling::never(&repo_path, "ui.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let _ = LOG_GUARD.set(guard);

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(fmt::layer().with_writer(non_blocking))
        .init();
}

#[component]
fn App() -> Element {
    let mut app_state_ready = use_signal(|| false);
    use_future(move || async move {
        let state = AppState::load().await.unwrap();
        // let nav = navigator();
        // if state.datum().login_state() == LoginState::Missing {
        //     nav.push(Route::Login {});
        // }
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
            Head {}
            Splash {}
        };
    }

    // Signal bumped on login/logout so title bar and other auth-dependent UI re-render.
    let mut auth_changed = use_signal(|| 0u32);
    provide_context(auth_changed);

    rsx! {
        Head {}
        Router::<Route> {}
    }
}

#[cfg(feature = "desktop")]
fn init_menu_bar() -> Result<TrayIcon> {
    // Initialize the tray menu

    use n0_error::StdResultExt;
    let tray_menu = Menu::new();

    // Create menu items with IDs for event handling
    let show_item = MenuItem::new("Show Window", true, None);
    let separator = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Quit", true, None);

    // Build the menu structure
    tray_menu
        .append_items(&[&show_item, &separator, &quit_item])
        .expect("Failed to build tray menu");

    let icon = icon();

    // Build the tray icon
    TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Datum Desktop")
        .with_icon(icon)
        .build()
        .std_context("building tray icon")
}

/// Load an icon from a PNG file
#[cfg(feature = "desktop")]
fn icon() -> Icon {
    use image::GenericImageView;

    let icon_bytes = include_bytes!("../assets/images/logo-datum-light.png");
    let image = image::load_from_memory(icon_bytes).unwrap();

    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8().into_raw();

    Icon::from_rgba(rgba, width, height).expect("Failed to create icon from image")
}
