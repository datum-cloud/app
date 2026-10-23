//! Scanning a ticket's QR code with the camera.
//!
//! The webview drives the camera with `getUserMedia` and decodes frames
//! with jsQR, a vendored Apache-2.0 library that runs entirely in the
//! webview. The clipboard path ([`crate::util::read_ticket_from_clipboard`])
//! decodes QR images in Rust with rqrr instead.

use dioxus::prelude::*;

use crate::components::{
    dialog::{DialogContent, DialogRoot, DialogTitle},
    Button, ButtonKind, Icon, IconSource,
};
use crate::util::read_ticket_from_clipboard;

/// jsQR 1.4.0, UMD build. Sets `jsQR` on the window. Apache-2.0; see
/// `ui/vendor/jsQR/LICENSE`.
const JSQR: &str = include_str!("../../vendor/jsQR/jsQR.js");

/// Starts the camera and watches for a QR code, reporting back through
/// `dioxus.send`. The stream is kept in `window.__datumQr`; a watchdog
/// stops the camera once the video element is gone, so unmounting the
/// scanner is always enough to release the camera.
const SCAN: &str = r##"
    (async () => {
        const send = (message) => dioxus.send(message);
        const stop = () => {
            const qr = window.__datumQr;
            window.__datumQr = null;
            if (!qr) return;
            if (qr.watchdog !== null) clearInterval(qr.watchdog);
            if (qr.raf !== null) cancelAnimationFrame(qr.raf);
            qr.stream.getTracks().forEach((track) => track.stop());
            const video = document.getElementById("ticket-scanner-video");
            if (video) video.srcObject = null;
        };
        window.__datumQrStop = stop;
        const media = navigator.mediaDevices;
        if (!media || !media.getUserMedia) {
            send({ type: "unavailable" });
            return;
        }
        try {
            const stream = await media.getUserMedia({
                audio: false,
                video: { facingMode: "environment" },
            });
            const video = document.getElementById("ticket-scanner-video");
            if (!video) {
                stream.getTracks().forEach((track) => track.stop());
                send({ type: "error", reason: "The video surface is missing" });
                return;
            }
            window.__datumQr = { stream: stream, raf: null, watchdog: null };
            video.srcObject = stream;
            await video.play();
            const canvas = document.createElement("canvas");
            const ctx = canvas.getContext("2d", { willReadFrequently: true });
            const tick = () => {
                const qr = window.__datumQr;
                if (!qr) return;
                const width = video.videoWidth;
                const height = video.videoHeight;
                if (width > 0 && height > 0) {
                    canvas.width = width;
                    canvas.height = height;
                    ctx.drawImage(video, 0, 0, width, height);
                    const image = ctx.getImageData(0, 0, width, height);
                    const code = jsQR(image.data, width, height, {
                        inversionAttempts: "attemptBoth",
                    });
                    if (code && code.data) {
                        stop();
                        send({ type: "found", data: code.data });
                        return;
                    }
                }
                qr.raf = requestAnimationFrame(tick);
            };
            window.__datumQr.watchdog = setInterval(() => {
                if (window.__datumQr && !video.isConnected) stop();
            }, 1000);
            window.__datumQr.raf = requestAnimationFrame(tick);
        } catch (error) {
            stop();
            send({ type: "error", reason: (error && error.name) || String(error) });
        }
    })();
"##;

/// Stops the camera at once, when a button in the dialog is clicked.
const STOP: &str = r#"
    if (window.__datumQrStop) { window.__datumQrStop(); }
"#;

/// Where the scanner stands.
#[derive(Clone, PartialEq)]
enum ScanState {
    /// Waiting for the camera to find a QR code.
    Waiting,
    /// The webview has no camera support, so only pasting can work.
    Unavailable,
    /// The camera refused to start; the string names the browser error.
    Failed(String),
}

/// Shows the camera and scans for a ticket's QR code, handing what it
/// finds to `on_found`. The parent stops showing this component on
/// `on_found` and `on_cancel`.
#[component]
pub fn TicketScanner(on_found: EventHandler<String>, on_cancel: EventHandler<()>) -> Element {
    let mut state = use_signal(|| ScanState::Waiting);
    let mut paste_error = use_signal(|| None::<String>);

    use_future(move || async move {
        let mut channel = document::eval(&format!("{}\n{}", JSQR, SCAN));
        loop {
            let Ok(message) = channel.recv::<serde_json::Value>().await else {
                return;
            };
            let reason = || {
                message
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("the camera failed")
                    .to_string()
            };
            match message.get("type").and_then(|t| t.as_str()) {
                Some("found") => {
                    if let Some(data) = message.get("data").and_then(|d| d.as_str()) {
                        // The script stops the camera itself when it finds one.
                        on_found.call(data.to_string());
                    }
                    return;
                }
                Some("unavailable") => {
                    state.set(ScanState::Unavailable);
                    return;
                }
                Some("error") => {
                    state.set(ScanState::Failed(reason()));
                    return;
                }
                _ => {}
            }
        }
    });

    let on_paste = move |_| match read_ticket_from_clipboard() {
        Ok(ticket) => on_found.call(ticket),
        Err(error) => paste_error.set(Some(error)),
    };
    let on_paste_scanning = move |_| match read_ticket_from_clipboard() {
        Ok(ticket) => on_found.call(ticket),
        Err(error) => paste_error.set(Some(error)),
    };

    let body = match state() {
        ScanState::Waiting => rsx! {
            video {
                id: "ticket-scanner-video",
                class: "w-full h-44 rounded-lg bg-black object-cover",
                muted: true,
                autoplay: true,
                playsinline: true,
            }
            p { class: "mt-3 text-1xs text-foreground/60",
                "Point the camera at the QR code the sharers show."
            }
        },
        ScanState::Unavailable => rsx! {
            UnavailableBody {
                icon: "camera",
                title: "The camera can't be used in this window.",
                detail: "Paste the ticket instead — copied text, or an image of its QR code, both work.",
                on_paste: on_paste_scanning,
                error: paste_error(),
            }
        },
        ScanState::Failed(reason) => {
            let (title, detail) = match reason.as_str() {
                "NotAllowedError" => (
                    "Camera access was denied.".to_string(),
                    "Allow the camera for Datum in System Settings, then try again.".to_string(),
                ),
                "NotFoundError" => (
                    "No camera was found.".to_string(),
                    "Connect a camera, or paste the ticket instead.".to_string(),
                ),
                other => (
                    "Couldn't start the camera.".to_string(),
                    format!("The camera reported \"{other}\". Paste the ticket instead."),
                ),
            };
            rsx! {
                UnavailableBody {
                    icon: "camera",
                    title: title,
                    detail: detail,
                    on_paste: on_paste,
                    error: paste_error(),
                }
            }
        }
    };

    rsx! {
        DialogRoot {
            open: true,
            on_open_change: move |open: bool| {
                if !open {
                    document::eval(STOP);
                    on_cancel.call(());
                }
            },
            is_modal: true,
            DialogContent {
                DialogTitle { "Scan a ticket" }
                div { class: "mt-4 w-[380px]", {body} }
                div { class: "flex items-center gap-2.5 justify-start mt-4",
                    Button {
                        kind: ButtonKind::Ghost,
                        text: if matches!(state(), ScanState::Waiting) {
                            "Cancel".to_string()
                        } else {
                            "Close".to_string()
                        },
                        onclick: move |_| {
                            document::eval(STOP);
                            on_cancel.call(());
                        },
                    }
                }
            }
        }
    }
}

/// Shown when the camera can't scan: explains why, offers the clipboard.
#[component]
fn UnavailableBody(
    icon: &'static str,
    title: String,
    detail: String,
    on_paste: EventHandler<MouseEvent>,
    error: Option<String>,
) -> Element {
    rsx! {
        div { class: "flex items-start gap-3",
            div { class: "text-foreground/60",
                Icon { source: IconSource::Named(icon.into()), size: 22 }
            }
            div { class: "text-sm text-foreground", "{title}" }
        }
        p { class: "mt-2 text-1xs text-foreground/60 leading-relaxed", "{detail}" }
        Button {
            class: Some("mt-4".to_string()),
            kind: ButtonKind::Primary,
            text: "Paste from clipboard",
            onclick: on_paste,
        }
        if let Some(error) = error {
            p { class: "mt-2 text-1xs text-alert-red-dark", "{error}" }
        }
    }
}
