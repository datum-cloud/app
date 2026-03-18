//! Raw wry/tao auth window that loads the OAuth URL directly.
//! Bypasses Dioxus so http/https navigation stays in-window (Dioxus opens them in the system browser).
//! The redirect page closes the window when auth completes.
//!
//! Uses the Dioxus custom_event_handler to create the window on the main thread, which is
//! required on macOS (tao EventLoop must be on main thread) and keeps the implementation unified.
//!
//! Note: We don't use transparent title bar + fullsize content (like the main app) because
//! the WebView would extend under the traffic lights and capture clicks, breaking the close button.

#[cfg(feature = "desktop")]
pub mod desktop_state {
    use std::cell::RefCell;
    use std::sync::{atomic::AtomicBool, Arc, Mutex};

    use dioxus_desktop::tao::window::Window;
    use dioxus_desktop::wry::WebView;
    use tokio_util::sync::CancellationToken;

    /// URL to open when the next event is processed.
    pub static PENDING_URL: Mutex<Option<String>> = Mutex::new(None);
    /// Cancellation token to signal auth flow when window is closed by user.
    pub static CANCEL_TOKEN: Mutex<Option<CancellationToken>> = Mutex::new(None);
    /// Set when the close handle is called; handler will drop the window.
    pub static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);
    // Keep the auth window alive until close is requested.
    // Thread-local because WebView is not Send on macOS (main-thread-only types).
    thread_local! {
        pub static WINDOW_HOLDER: RefCell<Option<(Arc<Window>, WebView)>> = RefCell::new(None);
    }
}

#[cfg(feature = "desktop")]
/// Process auth window creation/close on the main thread. Call from custom_event_handler.
pub fn process_auth_window<T>(
    event: &dioxus_desktop::tao::event::Event<'_, T>,
    target: &dioxus_desktop::tao::event_loop::EventLoopWindowTarget<T>,
) {
    use std::sync::{atomic::Ordering, Arc};

    use dioxus_desktop::tao::event::WindowEvent;
    use dioxus_desktop::tao::dpi::LogicalSize;
    use dioxus_desktop::tao::window::WindowBuilder;
    use dioxus_desktop::wry::WebViewBuilder;

    use crate::auth_window::desktop_state;

    // Handle CloseRequested for the auth window - drop it so the close button works
    if let dioxus_desktop::tao::event::Event::WindowEvent {
        window_id,
        event: WindowEvent::CloseRequested,
        ..
    } = event
    {
        let is_auth_window = desktop_state::WINDOW_HOLDER.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|(win, _)| win.id() == *window_id)
                .unwrap_or(false)
        });
        if is_auth_window {
            // Signal auth flow that user cancelled (closed window without completing OAuth)
            if let Ok(mut guard) = desktop_state::CANCEL_TOKEN.lock() {
                if let Some(token) = guard.take() {
                    token.cancel();
                }
            }
            desktop_state::WINDOW_HOLDER.with(|cell| {
                *cell.borrow_mut() = None;
            });
            return;
        }
    }

    // Process close request from our close handle (e.g. after OAuth completes)
    if desktop_state::CLOSE_REQUESTED.swap(false, Ordering::SeqCst) {
        desktop_state::WINDOW_HOLDER.with(|cell| {
            *cell.borrow_mut() = None;
        });
        return;
    }

    // Check for pending URL to open
    let url: Option<String> = match desktop_state::PENDING_URL.lock() {
        Ok(mut pending) => pending.take(),
        Err(_) => return,
    };

    let Some(auth_url) = url else {
        return;
    };

    const REDIRECT_PORT: u16 = 7076;
    let redirect_prefix = format!("http://localhost:{REDIRECT_PORT}/oauth/redirect");

    let window = match WindowBuilder::new()
        .with_title("Log in to Datum")
        .with_inner_size(LogicalSize::new(750.0, 800.0))
        .with_decorations(true)
        .with_closable(true)
        .build(target)
    {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("Failed to create auth window: {e}");
            return;
        }
    };

    let webview_builder = WebViewBuilder::new()
        .with_url(&auth_url)
        .with_navigation_handler(move |url| {
            if url.starts_with(&redirect_prefix) {
                // Redirect page has window.close(); allow it to load
            }
            true
        });

    let webview = {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            webview_builder.build(&window)
        }
        #[cfg(all(
            not(target_os = "windows"),
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        {
            use dioxus_desktop::tao::platform::unix::WindowExtUnix;
            use dioxus_desktop::wry::WebViewBuilderExtUnix;
            let vbox = window.default_vbox().expect("Failed to get vbox");
            webview_builder.build_gtk(vbox)
        }
    };

    let webview = match webview {
        Ok(wv) => wv,
        Err(e) => {
            tracing::error!("Failed to create auth webview: {e}");
            return;
        }
    };

    desktop_state::WINDOW_HOLDER.with(|cell| {
        *cell.borrow_mut() = Some((Arc::new(window), webview));
    });
}

#[cfg(feature = "desktop")]
pub fn open_auth_window(
    auth_url: String,
    cancel_token: tokio_util::sync::CancellationToken,
) -> Option<Box<dyn FnOnce() + Send>> {
    use std::sync::atomic::Ordering;

    use crate::auth_window::desktop_state;

    if let Ok(mut guard) = desktop_state::CANCEL_TOKEN.lock() {
        *guard = Some(cancel_token);
    }
    if let Ok(mut pending) = desktop_state::PENDING_URL.lock() {
        *pending = Some(auth_url);
    }
    Some(Box::new(|| {
        desktop_state::CLOSE_REQUESTED.store(true, Ordering::SeqCst);
    }))
}

#[cfg(not(feature = "desktop"))]
pub fn open_auth_window(
    _auth_url: String,
    _cancel_token: tokio_util::sync::CancellationToken,
) -> Option<Box<dyn FnOnce() + Send>> {
    None
}
