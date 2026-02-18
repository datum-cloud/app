use dioxus::prelude::*;
#[cfg(feature = "desktop")]
use n0_error::Result;
use std::sync::OnceLock;
use tracing::info;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::components::{Head, Splash, UpdateDialog};
use crate::state::AppState;
use crate::views::{
    Chrome, JoinProxy, Login, ProxiesList, SelectProject, Settings, TunnelBandwidth,
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

// Assets for favicons
const FAVICON_DARK_196: Asset = asset!("/assets/icons/favicon-dark-196x196.png");
const FAVICON_LIGHT_196: Asset = asset!("/assets/icons/favicon-light-196x196.png");

#[cfg(all(feature = "desktop", target_os = "macos"))]
static MANUAL_UPDATE_CHECK_FLAG: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The Route enum is used to define the structure of internal routes in our app. All route enums need to derive
/// the [`Routable`] trait, which provides the necessary methods for the router to work.
///
/// Each variant represents a different URL pattern that can be matched by the router. If that pattern is matched,
/// the components for that route will be rendered.
#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[route("/")]
    Login{},
    #[layout(Chrome)]
    #[route("/select")]
    SelectProject{},
    #[route("/proxies")]
    ProxiesList {},
    #[route("/proxy/edit/:id/bandwidth")]
    TunnelBandwidth { id: String },
    #[route("/proxy/join")]
    JoinProxy {},
    #[route("/settings")]
    Settings {},
}

fn main() {
    // Required before any TLS use (e.g. iroh, reqwest). Without this, the app panics when run from Applications.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("rustls default crypto provider");

    init_tracing();
    if let Ok(path) = dotenv::dotenv() {
        info!("Loaded environment variables from {}", path.display());
    }

    #[cfg(all(feature = "desktop", target_os = "linux"))]
    gtk::init().unwrap();

    #[cfg(feature = "desktop")]
    let _tray_icon = init_menu_bar().unwrap();

    #[cfg(feature = "desktop")]
    {
        use dioxus_desktop::{Config, LogicalSize, WindowBuilder, WindowCloseBehaviour};

        #[cfg(target_os = "macos")]
        use dioxus_desktop::tao::platform::macos::WindowBuilderExtMacOS;

        let mut window_builder = WindowBuilder::new()
            .with_title("")
            .with_inner_size(LogicalSize::new(630, 600)) // default width, height (logical pixels)
            .with_min_inner_size(LogicalSize::new(630, 600)) // prevent resizing smaller
            .with_decorations(true)
            .with_transparent(true)
            .with_window_icon(Some(window_icon()));

        // macOS-specific window options
        #[cfg(target_os = "macos")]
        {
            window_builder = window_builder
                .with_titlebar_transparent(true)
                .with_has_shadow(true)
                .with_fullsize_content_view(true);
        }

        dioxus::LaunchBuilder::desktop()
            .with_cfg(desktop! {
                Config::new()
                    // Make "close" behave like hide, so the tray icon can restore it.
                    .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                    .with_window(window_builder)
            })
            .launch(App);
    }

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(App);
}

fn init_tracing() {
    let repo_path = lib::Repo::default_location();
    if let Err(err) = std::fs::create_dir_all(&repo_path) {
        eprintln!(
            "ui: failed to create repo dir {}: {err}",
            repo_path.display()
        );
    }
    let file_appender = tracing_appender::rolling::never(&repo_path, "ui.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let _ = LOG_GUARD.set(guard);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(fmt::layer().with_writer(non_blocking))
        .init();
}

#[component]
fn App() -> Element {
    let mut app_state_ready = use_signal(|| false);
    let mut update_dialog_open = use_signal(|| false);
    let update_info = use_signal(|| None::<lib::UpdateInfo>);
    let mut manual_update_check = use_signal(|| false);

    // Poll for macOS menu bar update check flag
    #[cfg(all(feature = "desktop", target_os = "macos"))]
    {
        use_future(move || {
            let mut manual_update_check = manual_update_check;
            async move {
                loop {
                    // Check the atomic flag
                    if MANUAL_UPDATE_CHECK_FLAG.swap(false, std::sync::atomic::Ordering::Acquire) {
                        manual_update_check.set(true);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        });
    }

    use_future(move || {
        async move {
            let state = AppState::load().await.unwrap();
            // Refresh user profile on app startup to fetch latest data from API
            if state.datum().login_state() != lib::datum_cloud::LoginState::Missing {
                if let Err(err) = state.datum().auth().refresh_profile().await {
                    tracing::warn!("Failed to refresh user profile on startup: {err:#}");
                }
            }
            // let nav = navigator();
            // if state.datum().login_state() == LoginState::Missing {
            //     nav.push(Route::Login {});
            // }
            provide_context(state);
            app_state_ready.set(true);
        }
    });

    // Check for updates on startup and periodically
    use_future(move || {
        let mut update_dialog_open = update_dialog_open;
        let mut update_info = update_info;
        let mut manual_update_check = manual_update_check;
        async move {
            // Wait for app state to be ready
            while !app_state_ready() {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }

            let repo = match lib::Repo::open_or_create(lib::Repo::default_location()).await {
                Ok(repo) => repo,
                Err(e) => {
                    tracing::warn!("Failed to open repo for update checking: {e:#}");
                    return ();
                }
            };

            let checker = lib::UpdateChecker::new(repo);

            // Check for updates on startup
            if let Ok(should_check) = checker.should_check().await {
                if should_check {
                    if let Ok(Some(info)) = checker.check_for_updates().await {
                        update_info.set(Some(info));
                        update_dialog_open.set(true);
                    }
                }
            }

            // Periodic update check (every 12 hours by default) and manual checks
            let mut last_periodic_check = std::time::Instant::now();
            loop {
                // Check for manual update check trigger (poll every 5 seconds)
                let should_check_manually = manual_update_check();
                if should_check_manually {
                    manual_update_check.set(false);
                    // Force check regardless of interval
                    if let Ok(Some(info)) = checker.check_for_updates().await {
                        update_info.set(Some(info));
                        update_dialog_open.set(true);
                    }
                }

                // Periodic update check (every 12 hours)
                if last_periodic_check.elapsed().as_secs() >= 12 * 3600 {
                    if let Ok(Some(info)) = checker.check_for_updates().await {
                        update_info.set(Some(info));
                        update_dialog_open.set(true);
                    }
                    last_periodic_check = std::time::Instant::now();
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    });

    // Set macOS menu bar name and dock icon after the app launches (run loop must be active)
    #[cfg(all(feature = "desktop", target_os = "macos"))]
    {
        use_effect(|| {
            set_macos_menu_name();
        });
    }

    #[cfg(feature = "desktop")]
    use_tray_menu_event_handler(move |event| -> () {
        // The event ID corresponds to the menu item text
        let _: () = match event.id.0.as_str() {
            "About Datum" => {
                let _ = open::that("https://datum.net");
                ()
            }
            "Show Window" => {
                use_window().set_visible(true);
                ()
            }
            "Hide" => {
                use_window().set_visible(false);
                ()
            }
            "Check for Updates..." => {
                manual_update_check.set(true);
                ()
            }
            "Quit" => {
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown menu event: {}", event.id.0);
                ()
            }
        };
    });

    if !app_state_ready() {
        return rsx! {
            div { class: "theme-alpha",
                Head {}
                Splash {}
            }
        };
    }

    // Signal bumped on login/logout so title bar and other auth-dependent UI re-render.
    let auth_changed = use_signal(|| 0u32);
    provide_context(auth_changed);

    // Provide manual update check trigger for Settings page
    provide_context(manual_update_check);

    rsx! {
        div { class: "theme-alpha",
            div {
                class: "h-[32px] flex items-center pl-20 bg-background z-50 cursor-default",
                onmousedown: move |_| {
                    #[cfg(feature = "desktop")]
                    {
                        use_window().drag();
                    }
                },
                // Show light favicon in dark mode (when .dark class is present), dark favicon in light mode
                img {
                    src: "{FAVICON_DARK_196}",
                    class: "w-6 h-6 ml-auto mr-2 dark:hidden",
                }
                img {
                    src: "{FAVICON_LIGHT_196}",
                    class: "w-6 h-6 ml-auto mr-2 hidden dark:block",
                }
            }
            div { class: "flex-1 overflow-hidden",
                Head {}
                Router::<Route> {}
                if let Some(info) = update_info() {
                    UpdateDialog {
                        open: update_dialog_open,
                        update_info: info.clone(),
                        on_restart: move |_| -> () {}, //TODO: Implement proper restart mechanism
                        on_dismiss: move |_| {
                            update_dialog_open.set(false);
                        },
                    }
                }
            }
        }
    }
}

#[cfg(feature = "desktop")]
fn init_menu_bar() -> Result<TrayIcon> {
    // Initialize the tray menu

    use n0_error::StdResultExt;
    let tray_menu = Menu::new();

    // Create menu items with IDs for event handling
    let about_item = MenuItem::new("About Datum", true, None);
    let show_item = MenuItem::new("Show Window", true, None);
    let hide_item = MenuItem::new("Hide", true, None);
    let separator1 = PredefinedMenuItem::separator();
    let check_updates_item = MenuItem::new("Check for Updates...", true, None);
    let separator2 = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Quit", true, None);

    // Build the menu structure (macOS-style: About, Show, Hide, sep, Check for Updates, sep, Quit)
    tray_menu
        .append_items(&[
            &about_item,
            &show_item,
            &hide_item,
            &separator1,
            &check_updates_item,
            &separator2,
            &quit_item,
        ])
        .expect("Failed to build tray menu");

    let icon = icon();

    // Build the tray icon
    TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Datum")
        .with_icon(icon)
        .build()
        .std_context("building tray icon")
}

/// Load an icon from a PNG file for the tray
#[cfg(feature = "desktop")]
fn icon() -> Icon {
    use image::GenericImageView;

    let icon_bytes = include_bytes!("../assets/bundle/linux/512.png");
    let image = image::load_from_memory(icon_bytes).unwrap();

    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8().into_raw();

    Icon::from_rgba(rgba, width, height).expect("Failed to create icon from image")
}

/// Load an icon from a PNG file for the window
#[cfg(feature = "desktop")]
fn window_icon() -> dioxus_desktop::tao::window::Icon {
    use image::GenericImageView;

    let icon_bytes = include_bytes!("../assets/bundle/linux/512.png");
    let image = image::load_from_memory(icon_bytes).unwrap();

    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8().into_raw();

    dioxus_desktop::tao::window::Icon::from_rgba(rgba, width, height)
        .expect("Failed to create window icon from image")
}

/// Custom Objective-C class to handle About menu action and route navigation
#[cfg(all(feature = "desktop", target_os = "macos"))]
mod macos_menu_handler {
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{define_class, extern_methods};
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSObject as FoundationNSObject, NSString, NSURL};

    define_class!(
        #[unsafe(super(FoundationNSObject))]
        pub struct MenuActionHandler;

        impl MenuActionHandler {
            #[unsafe(method(openAboutURL:))]
            fn open_about_url(&self, _sender: Option<&NSObject>) {
                // Open https://datum.net in the default browser
                let url_str = NSString::from_str("https://datum.net");
                if let Some(url) = NSURL::URLWithString(&url_str) {
                    // SAFETY: sharedWorkspace is safe to call
                    unsafe {
                        let workspace = NSWorkspace::sharedWorkspace();
                        let _ = workspace.openURL(&url);
                    }
                }
            }

            #[unsafe(method(checkForUpdates:))]
            fn check_for_updates(&self, _sender: Option<&NSObject>) {
                // Set the atomic flag to trigger update check
                #[cfg(all(feature = "desktop", target_os = "macos"))]
                {
                    use crate::MANUAL_UPDATE_CHECK_FLAG;
                    MANUAL_UPDATE_CHECK_FLAG.store(true, std::sync::atomic::Ordering::Release);
                }
            }

        }
    );

    // Expose the `new` method from NSObject
    impl MenuActionHandler {
        extern_methods!(
            #[unsafe(method(new))]
            pub fn new() -> Retained<Self>;
        );
    }
}

/// Set the macOS menu bar app name and add standard app menu items (About, Hide, Quit).
#[cfg(all(feature = "desktop", target_os = "macos"))]
fn set_macos_menu_name() {
    use objc2::runtime::Sel;
    use objc2::{ClassType, MainThreadMarker};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    use std::ffi::CStr;
    use std::sync::Once;

    // SAFETY: We're on the main thread (called from use_effect in the UI)
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    let app = NSApplication::sharedApplication(mtm);
    let app_name = NSString::from_str("Datum");

    // Set the menu bar app name by modifying the main menu's first item (app menu)
    if let Some(main_menu) = app.mainMenu() {
        if let Some(app_menu_item) = main_menu.itemAtIndex(0) {
            app_menu_item.setTitle(&app_name);

            // Also update the submenu title and add standard items (only once)
            if let Some(app_submenu) = app_menu_item.submenu() {
                app_submenu.setTitle(&app_name);

                static MENU_ITEMS_ADDED: Once = Once::new();
                static HANDLER: std::sync::OnceLock<
                    objc2::rc::Retained<macos_menu_handler::MenuActionHandler>,
                > = std::sync::OnceLock::new();
                MENU_ITEMS_ADDED.call_once(|| {
                    // Register the custom handler class
                    macos_menu_handler::MenuActionHandler::class();

                    // Create an instance of the handler to use as target (store it statically so it's retained)
                    let handler = macos_menu_handler::MenuActionHandler::new();
                    HANDLER.set(handler.clone()).ok();

                    // Remove any existing icons from menu items
                    let item_count = app_submenu.numberOfItems();

                    for i in 0..item_count {
                        if let Some(item) = app_submenu.itemAtIndex(i) {
                            unsafe {
                                item.setImage(None);
                                item.setOnStateImage(None);
                                item.setOffStateImage(None);
                            }
                        }
                    }

                    // Add "About Datum" at the beginning
                    let about_title = NSString::from_str("About Datum");
                    let empty_key = NSString::from_str("");
                    // SAFETY: openAboutURL: is a valid selector on our custom handler
                    unsafe {
                        let about_sel =
                            Sel::register(CStr::from_bytes_with_nul(b"openAboutURL:\0").unwrap());
                        let about_item = app_submenu
                            .insertItemWithTitle_action_keyEquivalent_atIndex(
                                &about_title,
                                Some(about_sel),
                                &empty_key,
                                0,
                            );
                        about_item.setTarget(Some(&*handler));
                        about_item.setImage(None);
                        // Also clear state images to ensure no icons appear
                        about_item.setOnStateImage(None);
                        about_item.setOffStateImage(None);
                    }

                    // Add "Check for Updates..." after About Datum
                    let check_updates_title = NSString::from_str("Check for Updates...");
                    let empty_key = NSString::from_str("");
                    // SAFETY: checkForUpdates: is a valid selector on our custom handler
                    unsafe {
                        let check_updates_sel = Sel::register(
                            CStr::from_bytes_with_nul(b"checkForUpdates:\0").unwrap(),
                        );
                        let check_updates_item = app_submenu
                            .insertItemWithTitle_action_keyEquivalent_atIndex(
                                &check_updates_title,
                                Some(check_updates_sel),
                                &empty_key,
                                1,
                            );
                        check_updates_item.setTarget(Some(&*handler));
                        check_updates_item.setImage(None);
                        check_updates_item.setOnStateImage(None);
                        check_updates_item.setOffStateImage(None);
                    }
                });
            }
        }
    }
}
