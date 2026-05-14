use dioxus::prelude::*;
use std::sync::OnceLock;
use tracing::info;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::components::{Head, InstallingSplash, Splash, UpdateToast};
use crate::state::AppState;
use crate::views::{
    Chrome, JoinProxy, Login, ProxiesList, SelectProject, Settings, TrayNavHandler, TunnelBandwidth,
};

#[cfg(feature = "desktop")]
use dioxus_desktop::{
    trayicon::{
        menu::{IconMenuItem, Menu, MenuItem, NativeIcon, PredefinedMenuItem},
        Icon, TrayIcon, TrayIconBuilder,
    },
    use_muda_event_handler, use_window,
};

mod components;
mod state;
mod util;
mod views;

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

fn spawn_update_download(
    repo: lib::Repo,
    tx: tokio::sync::mpsc::UnboundedSender<Option<PendingUpdate>>,
    info: lib::UpdateInfo,
) {
    let checker = lib::UpdateChecker::new(repo);
    let url = info.download_url.clone();
    let version = info.version.clone();
    tokio::spawn(async move {
        let result = match checker.download_update(&url).await {
            Ok(path) => {
                if let Err(e) = checker.set_pending_install(&path, &version).await {
                    tracing::warn!("Failed to persist pending install: {e:#}");
                }
                Some(PendingUpdate { info, path })
            }
            Err(e) => {
                tracing::warn!("Update download failed: {e:#}");
                None
            }
        };
        let _ = tx.send(result);
    });
}

/// Pending update ready to install (downloaded, awaiting user choice)
#[derive(Clone, PartialEq)]
pub struct PendingUpdate {
    pub info: lib::UpdateInfo,
    pub path: std::path::PathBuf,
}

/// Signal indicating we're on the login page (for TitleBar background color).
#[derive(Clone)]
pub struct IsLoginPageSignal(pub Signal<bool>);

/// Context for update checks, downloads, pending installs, and manual check triggers.
#[derive(Clone)]
pub struct UpdateCheckContext {
    pub trigger: Signal<bool>,
    pub in_progress: Signal<bool>,
    pub downloading: Signal<bool>,
    pub update_ready: Signal<Option<PendingUpdate>>,
    /// True when there's a downloaded update ready (either showing toast or user chose "Later")
    pub has_pending_install: Signal<bool>,
    /// Update info for display when has_pending (kept when user clicks "Later")
    pub pending_update_info: Signal<Option<lib::UpdateInfo>>,
    /// When set to true, triggers install now (from Settings or toast)
    pub install_now_trigger: Signal<bool>,
    /// Stable (production) or beta (GitHub pre-releases)
    pub update_channel: Signal<lib::UpdateChannel>,
}

/// Context for network diagnostics opt-in state.
#[derive(Clone)]
pub struct DiagnosticsContext {
    /// Whether diagnostics collection is enabled.
    pub enabled: Signal<bool>,
    /// Whether the user has already seen the opt-in prompt.
    pub prompted: Signal<bool>,
}

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
    #[layout(TrayNavHandler)]
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

    // Load .env first so any process-env-driven config (DATUM_API_ENV, etc.) is
    // visible to the rest of init. We keep the load result so we can log it
    // *after* tracing is set up.
    let dotenv_path = dotenv::dotenv().ok();

    // Initialize Sentry before tracing so the tracing layer registered below
    // dispatches to a real Hub from the first event onwards.
    //
    // SENTRY_DSN is read at compile time via `option_env!`. CI release builds
    // bake it in (see .github/workflows/bundle.yml). Dev builds typically
    // have no SENTRY_DSN at compile time, so Sentry naturally runs as a
    // no-op in development.
    let _sentry_guard = sentry::init(sentry::ClientOptions {
        dsn: option_env!("SENTRY_DSN")
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse().ok()),
        release: sentry::release_name!(),
        send_default_pii: true,
        traces_sample_rate: 0.1,
        ..Default::default()
    });

    init_tracing();

    if let Some(path) = dotenv_path {
        info!("Loaded environment variables from {}", path.display());
    }

    #[cfg(all(feature = "desktop", target_os = "linux"))]
    gtk::init().unwrap();

    #[cfg(feature = "desktop")]
    {
        use dioxus_desktop::{Config, LogicalSize, WindowBuilder, WindowCloseBehaviour};

        #[cfg(target_os = "macos")]
        use dioxus_desktop::tao::platform::macos::WindowBuilderExtMacOS;

        let window_builder = WindowBuilder::new()
            .with_title("")
            .with_inner_size(LogicalSize::new(630, 600)) // default width, height (logical pixels)
            .with_min_inner_size(LogicalSize::new(630, 600)) // prevent resizing smaller
            .with_decorations(true)
            .with_transparent(cfg!(target_os = "macos"))
            .with_window_icon(Some(window_icon()));

        // macOS-specific window options
        #[cfg(target_os = "macos")]
        let window_builder = window_builder
            .with_titlebar_transparent(true)
            .with_has_shadow(true)
            .with_fullsize_content_view(true);

        let mut config = Config::new()
            // Close-as-hide: the user re-opens the window via the tray menu's
            // "Show Window" item (see init_tray and the muda event handler).
            .with_close_behaviour(WindowCloseBehaviour::WindowHides)
            // Left-clicking the tray icon should only surface the context menu,
            // not unhide the main window. Restoring is an explicit action
            // through the tray menu rather than an accidental side-effect of
            // clicking the icon.
            .with_tray_icon_show_window_on_click(false)
            .with_window(window_builder);

        dioxus::LaunchBuilder::desktop()
            .with_cfg(desktop! { config })
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

    // Promote WARN to a Sentry event (in addition to the default ERROR -> Event
    // and INFO -> Breadcrumb). The codebase uses `warn!` for most meaningful
    // failure paths (auth, kube, refresh) and very few `error!` sites, so the
    // default filter sends almost nothing to Sentry.
    let sentry_layer = sentry::integrations::tracing::layer().event_filter(|md| {
        use sentry::integrations::tracing::EventFilter;
        match *md.level() {
            tracing::Level::ERROR => EventFilter::Event | EventFilter::Breadcrumb,
            tracing::Level::WARN => EventFilter::Event | EventFilter::Breadcrumb,
            tracing::Level::INFO => EventFilter::Breadcrumb,
            _ => EventFilter::Ignore,
        }
    });

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(fmt::layer().with_writer(non_blocking))
        .with(sentry_layer)
        .init();
}

/// Custom title bar (drag region + favicon) shown only on macOS; empty on Windows/Linux which use the native title bar.
#[component]
fn TitleBar() -> Element {
    let IsLoginPageSignal(is_login_page) = consume_context::<IsLoginPageSignal>();
    let title_bar_bg = if is_login_page() {
        "h-[32px] flex items-center pl-20 z-50 cursor-default bg-[var(--midnight-fjord)]"
    } else {
        "h-[32px] flex items-center pl-20 z-50 cursor-default bg-background"
    };

    #[cfg(target_os = "macos")]
    {
        rsx! {
            div {
                class: "{title_bar_bg}",
                onmousedown: move |_| {
                    #[cfg(feature = "desktop")]
                    {
                        use_window().drag();
                    }
                },
                if is_login_page() {
                    img {
                        src: "{FAVICON_LIGHT_196}",
                        class: "w-6 h-6 ml-auto mr-2 block",
                    }
                } else {
                    img {
                        src: "{FAVICON_DARK_196}",
                        class: "w-6 h-6 ml-auto mr-2 dark:hidden",
                    }
                    img {
                        src: "{FAVICON_LIGHT_196}",
                        class: "w-6 h-6 ml-auto mr-2 hidden dark:block",
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        rsx! {}
    }
}

#[component]
fn App() -> Element {
    let mut app_state_ready = use_signal(|| false);
    let mut installing_update =
        use_signal(|| std::env::var("DATUM_UPDATE_FAKE_INSTALLING").as_deref() == Ok("1"));
    let mut manual_update_check = use_signal(|| false);
    let mut update_check_in_progress = use_signal(|| false);
    let mut update_downloading = use_signal(|| false);
    let mut update_ready = use_signal(|| None::<PendingUpdate>);
    let mut has_pending_install = use_signal(|| false);
    let mut pending_update_info = use_signal(|| None::<lib::UpdateInfo>);
    let mut install_now_trigger = use_signal(|| false);
    let update_channel = use_signal(|| lib::UpdateChannel::infer_from_installed_version());

    // Poll for macOS menu bar update check flag
    #[cfg(all(feature = "desktop", target_os = "macos"))]
    {
        use_future(move || {
            let mut manual_update_check = manual_update_check;
            let mut update_check_in_progress = update_check_in_progress;
            async move {
                loop {
                    if MANUAL_UPDATE_CHECK_FLAG.swap(false, std::sync::atomic::Ordering::Acquire) {
                        manual_update_check.set(true);
                        update_check_in_progress.set(true);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        });
    }

    // Install pending update on startup (before main UI)
    use_future(move || {
        let mut installing_update = installing_update;
        async move {
            let repo = match lib::Repo::open_or_create(lib::Repo::default_location()).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Failed to open repo for install check: {e:#}");
                    return;
                }
            };
            let checker = lib::UpdateChecker::new(repo);
            if let Ok(Some(path)) = checker.get_pending_install_path().await {
                installing_update.set(true);
                if let Err(e) = checker.clear_pending_install().await {
                    tracing::warn!("Failed to clear pending install: {e:#}");
                    installing_update.set(false);
                    return;
                }
                let path = path.clone();
                let apply_result = tokio::task::spawn_blocking(move || {
                    lib::UpdateChecker::apply_update_static(&path)
                })
                .await;
                match apply_result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!("Failed to apply update: {e:#}");
                        installing_update.set(false);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("Update task failed: {e}");
                        installing_update.set(false);
                        return;
                    }
                }
                let exe = match std::env::current_exe() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("Failed to get current exe: {e:#}");
                        installing_update.set(false);
                        return;
                    }
                };
                #[cfg(unix)]
                let _ = std::process::Command::new(&exe).spawn();
                #[cfg(windows)]
                let _ = std::process::Command::new(&exe).spawn();
                std::process::exit(0);
            }
        }
    });

    use_future(move || async move {
        let state = AppState::load().await.unwrap();
        if state.datum().login_state() != lib::datum_cloud::LoginState::Missing {
            if let Err(err) = state.datum().auth().refresh_profile().await {
                tracing::warn!("Failed to refresh user profile on startup: {err:#}");
            }
        }
        provide_context(state);
        app_state_ready.set(true);
    });

    // Check for updates on startup and periodically; download in background when found
    use_future(move || {
        let mut manual_update_check = manual_update_check;
        let mut update_check_in_progress = update_check_in_progress;
        let mut update_downloading = update_downloading;
        let mut update_ready = update_ready;
        let mut has_pending_install = has_pending_install;
        let mut pending_update_info = pending_update_info;
        let mut update_channel = update_channel;
        async move {
            while !app_state_ready() {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }

            let repo = match lib::Repo::open_or_create(lib::Repo::default_location()).await {
                Ok(repo) => repo,
                Err(e) => {
                    tracing::warn!("Failed to open repo for update checking: {e:#}");
                    return;
                }
            };

            // Dev bypass: inject fake update when DATUM_UPDATE_FAKE=1 (for testing toast/Settings UI)
            if std::env::var("DATUM_UPDATE_FAKE").as_deref() == Ok("1") {
                let path = repo.path().join("update_temp.bin");
                tokio::fs::write(&path, b"").await.ok();
                let info = lib::UpdateInfo {
                    version: "99.0.0".to_string(),
                    release_name: "Test Update (fake)".to_string(),
                    published_at: chrono::Utc::now(),
                    download_url: String::new(),
                    download_size: 0,
                };
                let pending = PendingUpdate {
                    info: info.clone(),
                    path: path.clone(),
                };
                update_ready.set(Some(pending));
                has_pending_install.set(true);
                pending_update_info.set(Some(info));
                tracing::info!("DATUM_UPDATE_FAKE=1: injected fake update ready");

                // Still run the loop to handle Later/Install now, but skip real updates
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Option<PendingUpdate>>();
                let checker = lib::UpdateChecker::new(repo.clone());
                let mut last_periodic_check = std::time::Instant::now();
                loop {
                    tokio::select! {
                        result = rx.recv() => {
                            if let Some(Some(r)) = result {
                                update_ready.set(Some(r.clone()));
                                update_downloading.set(false);
                                has_pending_install.set(true);
                                pending_update_info.set(Some(r.info));
                            }
                        }
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {
                            let should_check_manually = manual_update_check();
                            if should_check_manually {
                                manual_update_check.set(false);
                                update_check_in_progress.set(false);
                            }
                            if last_periodic_check.elapsed().as_secs() >= 12 * 3600 {
                                last_periodic_check = std::time::Instant::now();
                            }
                        }
                    }
                }
            }

            let checker = lib::UpdateChecker::new(repo.clone());
            if let Ok(s) = checker.load_settings().await {
                update_channel.set(s.update_channel);
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Option<PendingUpdate>>();

            // Check for updates on startup
            if let Ok(should_check) = checker.should_check().await {
                if should_check {
                    if let Ok(Some(info)) = checker.check_for_updates().await {
                        update_downloading.set(true);
                        spawn_update_download(repo.clone(), tx.clone(), info);
                    }
                }
            }

            // Periodic and manual checks; also process download completion
            let mut last_periodic_check = std::time::Instant::now();
            loop {
                tokio::select! {
                    result = rx.recv() => {
                        if let Some(Some(r)) = result {
                            update_ready.set(Some(r.clone()));
                            update_downloading.set(false);
                            has_pending_install.set(true);
                            pending_update_info.set(Some(r.info));
                        }
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {
                        let should_check_manually = manual_update_check();
                        if should_check_manually {
                            manual_update_check.set(false);
                            if let Ok(Some(info)) = checker.check_for_updates().await {
                                update_downloading.set(true);
                                spawn_update_download(repo.clone(), tx.clone(), info);
                            }
                            update_check_in_progress.set(false);
                        }

                        if last_periodic_check.elapsed().as_secs() >= 12 * 3600 {
                            if let Ok(Some(info)) = checker.check_for_updates().await {
                                update_downloading.set(true);
                                spawn_update_download(repo.clone(), tx.clone(), info);
                            }
                            last_periodic_check = std::time::Instant::now();
                        }
                    }
                }
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

    let mut open_add_tunnel_from_tray = use_signal(|| false);
    let mut login_state_for_tray = use_signal(|| None::<lib::datum_cloud::LoginState>);

    // Create tray when app state is ready. Must run before early return.
    #[cfg(feature = "desktop")]
    {
        let mut tray_holder = use_signal(|| None::<TrayIcon>);

        use_effect(move || {
            if !app_state_ready() {
                return;
            }
            if (*tray_holder.peek()).is_some() {
                return;
            }
            let tray = init_tray();
            tray_holder.set(Some(tray));
        });

        let window = use_window();
        use_muda_event_handler(move |event| -> () {
            let _: () = match event.id.0.as_str() {
                "about" => {
                    let _ = open::that("https://datum.net");
                }
                "show" => {
                    window.set_visible(true);
                    window.set_focus();
                }
                "hide" => {
                    window.set_visible(false);
                }
                "new_tunnel" => {
                    if login_state_for_tray() == Some(lib::datum_cloud::LoginState::Missing)
                        || login_state_for_tray().is_none()
                    {
                        return;
                    }
                    window.set_visible(true);
                    window.set_focus();
                    open_add_tunnel_from_tray.set(true);
                }
                _ => {}
            };
        });
    }

    if installing_update() {
        return rsx! {
            div { class: "theme-alpha",
                Head {}
                InstallingSplash {}
            }
        };
    }

    if !app_state_ready() {
        return rsx! {
            div { class: "theme-alpha",
                Head {}
                Splash {}
            }
        };
    }

    // Signal bumped on login/logout and auth state transitions so auth-dependent UI re-renders.
    let auth_changed = use_signal(|| 0u32);
    provide_context(auth_changed);

    let state_for_auth_watch = consume_context::<AppState>();
    use_effect(move || {
        let _ = auth_changed();
        let state = consume_context::<AppState>();
        login_state_for_tray.set(Some(state.datum().login_state()));
    });
    use_future(move || {
        let state_for_auth_watch = state_for_auth_watch.clone();
        let mut auth_changed = auth_changed;
        async move {
            let mut login_rx = state_for_auth_watch.datum().auth().login_state_watch();
            loop {
                if login_rx.changed().await.is_err() {
                    return;
                }
                auth_changed.set(auth_changed().wrapping_add(1));
            }
        }
    });

    // Provide update context for Settings and toast
    provide_context(UpdateCheckContext {
        trigger: manual_update_check,
        in_progress: update_check_in_progress,
        downloading: update_downloading,
        update_ready,
        has_pending_install,
        pending_update_info,
        install_now_trigger,
        update_channel,
    });

    // Diagnostics opt-in state (loaded from repo settings).
    let mut diag_enabled = use_signal(|| true);
    let mut diag_prompted = use_signal(|| true); // default true to hide prompt until loaded
    use_future(move || async move {
        let repo = match lib::Repo::open_or_create(lib::Repo::default_location()).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Failed to open repo for diagnostics settings: {e:#}");
                return;
            }
        };
        match repo.diagnostics_settings().await {
            Ok(s) => {
                diag_enabled.set(s.enabled);
                diag_prompted.set(s.prompted);
            }
            Err(e) => {
                tracing::warn!("Failed to load diagnostics settings: {e:#}");
            }
        }
    });

    provide_context(DiagnosticsContext {
        enabled: diag_enabled,
        prompted: diag_prompted,
    });

    // Handle install now (toast or Settings). Read path from update_ready before clearing it —
    // the toast handler must not clear update_ready before this runs, or we rely only on disk.
    use_effect(move || {
        if !install_now_trigger() {
            return;
        }
        install_now_trigger.set(false);
        let path_from_ui = update_ready().as_ref().map(|p| p.path.clone());
        update_ready.set(None);

        if let Some(path) = path_from_ui {
            let repo = lib::Repo::from_path(lib::Repo::default_location());
            let checker = lib::UpdateChecker::new(repo);
            if let Err(e) = checker.spawn_install_and_restart(&path) {
                tracing::warn!("Failed to spawn install: {e:#}");
                return;
            }
            std::process::exit(0);
        }

        let repo = lib::Repo::from_path(lib::Repo::default_location());
        let checker = lib::UpdateChecker::new(repo);
        tokio::spawn(async move {
            match checker.get_pending_install_path().await {
                Ok(Some(path)) => {
                    if let Err(e) = checker.spawn_install_and_restart(&path) {
                        tracing::warn!("Failed to spawn install: {e:#}");
                        return;
                    }
                    std::process::exit(0);
                }
                Ok(None) => {
                    tracing::warn!(
                        "Install now: no pending update on disk (settings missing or file gone)"
                    );
                }
                Err(e) => {
                    tracing::warn!("Install now: failed to read pending update path: {e:#}");
                }
            }
        });
    });

    // Tray can trigger opening the add tunnel dialog; Chrome (inside Router) handles navigation + dialog.
    provide_context(open_add_tunnel_from_tray);

    // Signal for whether we're on the login page (used by TitleBar for background color).
    // Start true since we always land on login after splash; Login's use_effect/use_drop keep it in sync.
    let is_login_page = use_signal(|| true);
    provide_context(IsLoginPageSignal(is_login_page));

    rsx! {
        div { class: "theme-alpha",
            TitleBar {}
            div { class: "flex-1 overflow-hidden",
                Head {}
                Router::<Route> {}
            }
            if let Some(ref pending) = update_ready() {
                div { class: "fixed bottom-4 right-4 z-[100]",
                    UpdateToast {
                        pending: pending.clone(),
                        on_later: move |_| {
                            if let Some(p) = update_ready() {
                                pending_update_info.set(Some(p.info));
                            }
                            update_ready.set(None);
                        },
                        on_install_now: move |_| {
                            if let Some(p) = update_ready() {
                                pending_update_info.set(Some(p.info));
                            }
                            install_now_trigger.set(true);
                            // update_ready cleared in install effect after copying the DMG path
                        },
                    }
                }
            }
            // One-time diagnostics opt-in prompt
            if !diag_prompted() {
                DiagnosticsPrompt {
                    on_continue: move |_| {
                        // Accept defaults (enabled: true) and mark prompted
                        diag_prompted.set(true);
                        spawn(async move {
                            save_diagnostics_prompt(true).await;
                        });
                    },
                    on_opt_out: move |_| {
                        diag_enabled.set(false);
                        diag_prompted.set(true);
                        spawn(async move {
                            save_diagnostics_prompt(false).await;
                        });
                    },
                }
            }
        }
    }
}

/// Save the diagnostics prompt response to disk.
async fn save_diagnostics_prompt(enabled: bool) {
    let repo = match lib::Repo::open_or_create(lib::Repo::default_location()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to open repo for diagnostics prompt: {e:#}");
            return;
        }
    };
    let settings = lib::DiagnosticsSettings {
        enabled,
        prompted: true,
    };
    if let Err(e) = repo.write_diagnostics_settings(&settings).await {
        tracing::warn!("Failed to save diagnostics settings: {e:#}");
    }
}

/// One-time prompt shown on first launch for network diagnostics opt-in.
#[component]
fn DiagnosticsPrompt(
    on_continue: EventHandler<MouseEvent>,
    on_opt_out: EventHandler<MouseEvent>,
) -> Element {
    use crate::components::{Button, ButtonKind};

    rsx! {
        div {
            class: "mt-[32px] fixed inset-0 z-50 flex items-center justify-center animate-in fade-in duration-100",
            style: "background-color: rgba(0,0,0,0.2); -webkit-backdrop-filter: blur(1px); backdrop-filter: blur(1px);",
            onclick: move |evt| {
                evt.stop_propagation();
                on_continue.call(evt);
            },
            div {
                class: "w-full max-w-lg mx-auto p-8 bg-card-background rounded-lg border border-card-border shadow-card relative z-50",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "mb-4",
                    h2 { class: "text-sm font-semibold text-foreground",
                        "Network diagnostics are enabled"
                    }
                }
                p { class: "text-1xs text-foreground/80 leading-relaxed",
                    "We collect network diagnostic data to help us identify and resolve connectivity issues faster. Your desktop identifier will always be anonymous."
                }
                p { class: "text-1xs text-foreground/60 leading-relaxed mt-4",
                    "Not willing to help us improve reliability? No worries! You can always opt out at any time in Settings \u{2192} Privacy \u{2192} Network Diagnostics."
                }
                div { class: "flex gap-2 mt-6",
                    Button {
                        text: "Continue",
                        kind: ButtonKind::Primary,
                        onclick: move |evt| on_continue.call(evt),
                    }
                    Button {
                        text: "Opt Out",
                        kind: ButtonKind::Outline,
                        onclick: move |evt| on_opt_out.call(evt),
                    }
                }
            }
        }
    }
}

#[cfg(feature = "desktop")]
fn init_tray() -> TrayIcon {
    use n0_error::StdResultExt;

    let tray_menu = Menu::new();
    let about_item = MenuItem::with_id("about", "About Datum", true, None);
    let show_item = MenuItem::with_id("show", "Show Window", true, None);
    let hide_item = MenuItem::with_id("hide", "Hide", true, None);

    #[cfg(target_os = "macos")]
    let new_tunnel_item = IconMenuItem::with_id_and_native_icon(
        "new_tunnel",
        "New Tunnel",
        true,
        Some(NativeIcon::Add),
        None,
    );
    #[cfg(not(target_os = "macos"))]
    let new_tunnel_item = IconMenuItem::with_id("new_tunnel", "New Tunnel", true, None, None);

    let version_item = MenuItem::with_id(
        "version",
        format!("Version {}", env!("CARGO_PKG_VERSION")),
        false,
        None,
    );
    let sep = PredefinedMenuItem::separator();

    tray_menu
        .append_items(&[
            &new_tunnel_item,
            &sep,
            &about_item,
            &show_item,
            &hide_item,
            &sep,
            &version_item,
        ])
        .expect("Failed to build tray menu");

    TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Datum")
        .with_icon(icon())
        .build()
        .std_context("building tray icon")
        .expect("failed to build tray icon")
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
