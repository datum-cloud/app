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
    Chrome, JoinProxy, Login, ProxiesList, SelectProject, Sidebar, Signup,
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

    // Set macOS dock icon programmatically (needed for dx serve / development mode)
    #[cfg(all(feature = "desktop", target_os = "macos"))]
    set_macos_dock_icon();

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
                            .with_title("Datum")
                            .with_inner_size(LogicalSize::new(630, 600))  // default width, height (logical pixels)
                            .with_min_inner_size(LogicalSize::new(630, 600))  // prevent resizing smaller
                            // Required for rounded app chrome: we render our own rounded container inside.
                            .with_transparent(true)
                            .with_decorations(false)
                            .with_window_icon(Some(window_icon())),
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

    // Set the macOS menu bar app name after the app launches (menu now exists)
    #[cfg(all(feature = "desktop", target_os = "macos"))]
    {
        use_effect(|| {
            set_macos_menu_name();
        });
    }

    #[cfg(feature = "desktop")]
    use_tray_menu_event_handler(move |event| {
        // The event ID corresponds to the menu item text
        match event.id.0.as_str() {
            "About Datum" => {
                let _ = open::that("https://datum.net");
            }
            "Show Window" => {
                use_window().set_visible(true);
            }
            "Hide" => {
                use_window().set_visible(false);
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
    let about_item = MenuItem::new("About Datum", true, None);
    let show_item = MenuItem::new("Show Window", true, None);
    let hide_item = MenuItem::new("Hide", true, None);
    let separator = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Quit", true, None);

    // Build the menu structure (macOS-style: About, Show, Hide, sep, Quit)
    tray_menu
        .append_items(&[&about_item, &show_item, &hide_item, &separator, &quit_item])
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

    let icon_bytes = include_bytes!("../assets/bundle/linux/128.png");
    let image = image::load_from_memory(icon_bytes).unwrap();

    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8().into_raw();

    Icon::from_rgba(rgba, width, height).expect("Failed to create icon from image")
}

/// Load an icon from a PNG file for the window
#[cfg(feature = "desktop")]
fn window_icon() -> dioxus_desktop::tao::window::Icon {
    use image::GenericImageView;

    let icon_bytes = include_bytes!("../assets/bundle/linux/128.png");
    let image = image::load_from_memory(icon_bytes).unwrap();

    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8().into_raw();

    dioxus_desktop::tao::window::Icon::from_rgba(rgba, width, height)
        .expect("Failed to create window icon from image")
}

/// Set the macOS dock icon programmatically (for development mode without a bundle)
#[cfg(all(feature = "desktop", target_os = "macos"))]
fn set_macos_dock_icon() {
    use objc2::AllocAnyThread;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let icon_bytes = include_bytes!("../assets/bundle/linux/128.png");

    // SAFETY: We're on the main thread when this is called during app initialization
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    let app = NSApplication::sharedApplication(mtm);

    // Set the dock icon
    let ns_data = NSData::with_bytes(icon_bytes);
    if let Some(ns_image) = NSImage::initWithData(NSImage::alloc(), &ns_data) {
        // SAFETY: We're setting a valid NSImage on the main thread
        unsafe { app.setApplicationIconImage(Some(&ns_image)) };
    }
}

/// Custom Objective-C class to handle About menu action and route navigation
#[cfg(all(feature = "desktop", target_os = "macos"))]
mod macos_menu_handler {
    use objc2::runtime::NSObject;
    use objc2::{define_class, extern_methods};
    use objc2_foundation::{NSObject as FoundationNSObject, NSString, NSURL};
    use objc2_app_kit::{NSWorkspace};
    use objc2::rc::Retained;

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
    use std::ffi::CStr;
    use std::sync::Once;
    use objc2::runtime::Sel;
    use objc2::{ClassType, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenuItem};
    use objc2_foundation::NSString;

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
                        let about_item =
                            app_submenu.insertItemWithTitle_action_keyEquivalent_atIndex(
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

                    // Add separator after "About Datum"
                    unsafe {
                        let separator = NSMenuItem::separatorItem(mtm);
                        app_submenu.insertItem_atIndex(&separator, 1);
                    }

                    // Add "Hide Datum" (Cmd+H), "Hide Others" (Option+Cmd+H), and "Show All"
                    let hide_title = NSString::from_str("Hide Datum");
                    let hide_others_title = NSString::from_str("Hide Others");
                    let show_all_title = NSString::from_str("Show All");
                    let quit_title = NSString::from_str("Quit Datum");
                    let key_h = NSString::from_str("h");
                    let key_q = NSString::from_str("q");
                    let count = app_submenu.numberOfItems();
                    // SAFETY: hide:, hideOtherApplications:, unhideAllApplications:, and terminate: are valid selectors on NSApplication
                    unsafe {
                        // Hide Datum (Cmd+H)
                        let hide_sel = Sel::register(CStr::from_bytes_with_nul(b"hide:\0").unwrap());
                        let hide_item =
                            app_submenu.insertItemWithTitle_action_keyEquivalent_atIndex(
                                &hide_title,
                                Some(hide_sel),
                                &key_h,
                                count,
                            );
                        hide_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
                        hide_item.setImage(None);
                        hide_item.setOnStateImage(None);
                        hide_item.setOffStateImage(None);

                        // Hide Others (Option+Cmd+H)
                        let hide_others_sel = Sel::register(
                            CStr::from_bytes_with_nul(b"hideOtherApplications:\0").unwrap(),
                        );
                        let hide_others_item =
                            app_submenu.insertItemWithTitle_action_keyEquivalent_atIndex(
                                &hide_others_title,
                                Some(hide_others_sel),
                                &key_h,
                                count + 1,
                            );
                        hide_others_item.setKeyEquivalentModifierMask(
                            NSEventModifierFlags::Option | NSEventModifierFlags::Command,
                        );
                        hide_others_item.setImage(None);
                        hide_others_item.setOnStateImage(None);
                        hide_others_item.setOffStateImage(None);

                        // Show All
                        let show_all_sel = Sel::register(
                            CStr::from_bytes_with_nul(b"unhideAllApplications:\0").unwrap(),
                        );
                        let show_all_item =
                            app_submenu.insertItemWithTitle_action_keyEquivalent_atIndex(
                                &show_all_title,
                                Some(show_all_sel),
                                &empty_key,
                                count + 2,
                            );
                        show_all_item.setImage(None);
                        show_all_item.setOnStateImage(None);
                        show_all_item.setOffStateImage(None);

                        // Add separator before Quit
                        let separator2 = NSMenuItem::separatorItem(mtm);
                        app_submenu.insertItem_atIndex(&separator2, count + 3);

                        // Quit Datum (Cmd+Q)
                        let term_sel =
                            Sel::register(CStr::from_bytes_with_nul(b"terminate:\0").unwrap());
                        let quit_item =
                            app_submenu.insertItemWithTitle_action_keyEquivalent_atIndex(
                                &quit_title,
                                Some(term_sel),
                                &key_q,
                                count + 4,
                            );
                        quit_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
                        quit_item.setImage(None);
                        quit_item.setOnStateImage(None);
                        quit_item.setOffStateImage(None);
                    }
                });
            }
        }
    }
}

