//! Project-less local tunnels: shares of a local service that devices pair
//! with using a ticket, and connections to services other devices share.
//! None of this needs a Datum account, so it is also reachable logged out.

use std::net::SocketAddr;

use dioxus::events::FormEvent;
use dioxus::prelude::*;
use lib::{
    local_tunnels::{parse_ticket, LocalPath},
    LocalTunnel, LocalTunnelKind, LocalTunnelStatus,
};
use open::that;

use crate::{
    components::{
        dialog::{DialogContent, DialogRoot, DialogTitle},
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator,
            DropdownMenuTrigger,
        },
        input::Input,
        Button, ButtonKind, Icon, IconSource, Switch, SwitchThumb,
    },
    state::AppState,
    util::copy_to_clipboard,
    views::OpenEditTunnelDialog,
};

/// The local share whose ticket dialog is open. Provided by Chrome so a new
/// share can show its ticket as soon as the add dialog closes.
#[derive(Clone, Copy)]
pub struct LocalTicketTarget(pub Signal<Option<String>>);

/// The tunnels page for people who are not logged in: local tunnels only.
#[component]
pub fn LocalTunnelsPage() -> Element {
    let state = consume_context::<AppState>();
    let tunnels = state.local_tunnel_list();
    let mut add_dialog_open = consume_context::<OpenEditTunnelDialog>().dialog_open;

    const EMPTY_MOON: Asset = asset!("/assets/images/empty-card-moon.png");
    const EMPTY_ROCKS: Asset = asset!("/assets/images/empty-card-rocks.png");

    rsx! {
        div { class: "max-w-5xl mx-auto",
            if tunnels().is_empty() {
                div { class: "space-y-5",
                    div { class: "relative rounded-lg border border-card-border bg-card-background h-48 p-10 text-center shadow-card text-foreground flex flex-col items-center justify-center gap-6 overflow-hidden",
                        img {
                            class: "absolute right-0 top-0 h-20 w-auto object-contain pointer-events-none",
                            src: "{EMPTY_MOON}",
                            alt: "",
                        }
                        img {
                            class: "absolute left-0 bottom-0 h-28 w-auto object-contain pointer-events-none",
                            src: "{EMPTY_ROCKS}",
                            alt: "",
                        }
                        div { class: "text-sm mt-2 max-w-sm",
                            "Share a local service with your phone or another computer. No account needed."
                        }
                        Button {
                            kind: ButtonKind::Outline,
                            class: "w-fit text-foreground",
                            text: "Add New",
                            leading_icon: Some(IconSource::Named("plus".into())),
                            onclick: move |_| add_dialog_open.set(true),
                        }
                    }
                    div { class: "rounded-lg bg-background h-48" }
                    div { class: "rounded-lg bg-background h-48" }
                }
            } else {
                LocalTunnelList { show_heading: false }
            }
            p { class: "mt-6 text-1xs text-foreground/60 leading-relaxed",
                "Local tunnels connect your devices directly, or through a relay when they can't reach each other. "
                "Log in to Datum to publish a service on a public HTTPS URL."
            }
        }
    }
}

/// Every local tunnel as a card. With `show_heading`, the list is titled and
/// hidden while empty, for use below the project tunnels.
#[component]
pub fn LocalTunnelList(show_heading: bool) -> Element {
    let state = consume_context::<AppState>();
    let tunnels = state.local_tunnel_list();
    let mut renaming = use_signal(|| None::<LocalTunnel>);
    let mut deleting = use_signal(|| None::<LocalTunnel>);
    let mut rotating = use_signal(|| None::<LocalTunnel>);
    let mut porting = use_signal(|| None::<LocalTunnel>);

    if show_heading && tunnels().is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: if show_heading { "mt-8" } else { "" },
            if show_heading {
                div { class: "mb-3",
                    h2 { class: "text-sm font-medium text-foreground", "Local tunnels" }
                    p { class: "text-1xs text-foreground/60 mt-0.5",
                        "Peer-to-peer and paired with a ticket. No Datum account needed."
                    }
                }
            }
            div { class: "space-y-5",
                for tunnel in tunnels() {
                    LocalTunnelCard {
                        key: "{tunnel.id}",
                        tunnel,
                        on_rename: move |t| renaming.set(Some(t)),
                        on_delete: move |t| deleting.set(Some(t)),
                        on_new_ticket: move |t| rotating.set(Some(t)),
                        on_port: move |t| porting.set(Some(t)),
                    }
                }
            }
        }
        RenameLocalTunnelDialog { tunnel: renaming, on_close: move |_| renaming.set(None) }
        DeleteLocalTunnelDialog { tunnel: deleting, on_close: move |_| deleting.set(None) }
        NewTicketDialog { tunnel: rotating, on_close: move |_| rotating.set(None) }
        LocalPortDialog { tunnel: porting, on_close: move |_| porting.set(None) }
    }
}

#[component]
fn LocalTunnelCard(
    tunnel: LocalTunnel,
    on_rename: EventHandler<LocalTunnel>,
    on_delete: EventHandler<LocalTunnel>,
    on_new_ticket: EventHandler<LocalTunnel>,
    on_port: EventHandler<LocalTunnel>,
) -> Element {
    let state = consume_context::<AppState>();
    let LocalTicketTarget(mut ticket_target) = consume_context::<LocalTicketTarget>();
    let mut menu_open = use_signal(|| None::<bool>);
    let is_share = matches!(tunnel.kind, LocalTunnelKind::Share { .. });
    let active = tunnel.status.is_active();
    let starting = tunnel.status == LocalTunnelStatus::Starting;

    let id_for_toggle = tunnel.id.clone();
    let state_for_toggle = state.clone();
    let on_toggle = move |next: bool| {
        if next {
            state_for_toggle.local_tunnels().start(&id_for_toggle);
        } else {
            state_for_toggle.local_tunnels().stop(&id_for_toggle);
        }
    };

    let body = match (&tunnel.kind, &tunnel.status) {
        (LocalTunnelKind::Share { address }, status) => {
            let id = tunnel.id.clone();
            rsx! {
                Row { icon: "down-right-arrow", text: "{address}" }
                match status {
                    LocalTunnelStatus::Starting => rsx! {
                        Row { icon: "loader-circle", spin: true, text: "Getting a ticket…" }
                    },
                    LocalTunnelStatus::Sharing { .. } => rsx! {
                        Row {
                            icon: "users",
                            text: match tunnel.devices.len() {
                                0 => "Waiting for a device to pair".to_string(),
                                1 => "1 device paired".to_string(),
                                n => format!("{n} devices paired"),
                            },
                        }
                        for device in tunnel.devices.iter() {
                            div { class: "pl-6 text-1xs text-foreground/60",
                                "{device_label(&device.name, &device.endpoint_id)} · {path_label(&device.path)}"
                            }
                        }
                        div { class: "pl-6",
                            button {
                                class: "text-xs text-button-link-foreground hover:underline cursor-pointer",
                                r#type: "button",
                                onclick: move |_| ticket_target.set(Some(id.clone())),
                                "Show ticket and QR code"
                            }
                        }
                    },
                    LocalTunnelStatus::Failed(error) => rsx! {
                        ErrorRow { text: "Couldn't share: {error}" }
                    },
                    _ => rsx! {
                        Row {
                            icon: "power-cable",
                            text: "Stopped. Turn on to share again.",
                            muted: true,
                        }
                    },
                }
            }
        }
        (LocalTunnelKind::Connect { ticket, .. }, status) => {
            let source = match parse_ticket(ticket) {
                Ok(info) => format!(
                    "Port {} on {}…",
                    info.port,
                    info.endpoint_id.chars().take(12).collect::<String>()
                ),
                Err(_) => "Unreadable ticket".to_string(),
            };
            rsx! {
                Row { icon: "globe", text: "{source}" }
                match status {
                    LocalTunnelStatus::Starting => rsx! {
                        Row { icon: "loader-circle", spin: true, text: "Pairing…" }
                    },
                    LocalTunnelStatus::Open { local_addr } => {
                        let url = format!("http://{local_addr}");
                        let url_for_click = url.clone();
                        rsx! {
                            div { class: "flex items-center gap-2.5 text-icon-tunnel",
                                Icon {
                                    source: IconSource::Named("external-link".into()),
                                    size: 14,
                                }
                                a {
                                    class: "text-xs text-foreground hover:underline cursor-pointer",
                                    onclick: move |_| {
                                        let _ = that(&url_for_click);
                                    },
                                    "{url}"
                                }
                            }
                            div { class: "pl-6 text-1xs text-foreground/60",
                                {
                                    tunnel
                                        .path
                                        .as_ref()
                                        .map(LocalPath::summary)
                                        .unwrap_or_else(|| "Path appears once traffic flows".to_string())
                                }
                            }
                        }
                    }
                    LocalTunnelStatus::Failed(error) => rsx! {
                        ErrorRow { text: "Couldn't connect: {error}" }
                    },
                    _ => rsx! {
                        Row { icon: "power-cable", text: "Stopped", muted: true }
                    },
                }
            }
        }
    };

    // Menu items, numbered in display order for keyboard navigation.
    let show_ticket = is_share && matches!(tunnel.status, LocalTunnelStatus::Sharing { .. });
    let open_url = match &tunnel.status {
        LocalTunnelStatus::Open { local_addr } => Some(format!("http://{local_addr}")),
        _ => None,
    };
    let connect_ticket = match &tunnel.kind {
        LocalTunnelKind::Connect { ticket, .. } => Some(ticket.clone()),
        _ => None,
    };
    let open_index = usize::from(show_ticket);
    let copy_index = open_index + usize::from(open_url.is_some());
    let port_index = copy_index + usize::from(connect_ticket.is_some());
    let new_ticket_index = port_index + usize::from(connect_ticket.is_some());
    let rename_index = new_ticket_index + usize::from(is_share);
    let id_for_ticket = tunnel.id.clone();
    let tunnel_for_rename = tunnel.clone();
    let tunnel_for_delete = tunnel.clone();
    let tunnel_for_new_ticket = tunnel.clone();
    let tunnel_for_port = tunnel.clone();

    rsx! {
        div { class: "bg-tunnel-card-background rounded-lg border border-app-border shadow-card relative",
            div { class: "px-4 py-2.5 flex items-center justify-between bg-card-background rounded-t-lg",
                div { class: "flex items-center gap-2 min-w-0",
                    h2 { class: "text-md font-normal text-foreground truncate", "{tunnel.name}" }
                    span { class: "shrink-0 rounded-full border border-app-border px-2 py-0.5 text-[10px] text-foreground/60",
                        if is_share { "Shared from this computer" } else { "From a ticket" }
                    }
                }
                if starting {
                    Icon {
                        source: IconSource::Named("loader-circle".into()),
                        size: 16,
                        class: "animate-spin text-icon-tunnel",
                    }
                } else {
                    Switch {
                        checked: active,
                        on_checked_change: on_toggle,
                        SwitchThumb {}
                    }
                }
            }
            div { class: "border-t border-tunnel-card-border" }
            div { class: "p-4 flex items-start justify-between bg-tunnel-card-background rounded-b-lg gap-4",
                div { class: "flex flex-col gap-1.5 min-w-0", {body} }
                div { class: "relative",
                    DropdownMenu {
                        open: menu_open,
                        default_open: false,
                        on_open_change: move |v| menu_open.set(Some(v)),
                        disabled: use_signal(|| false),
                        DropdownMenuTrigger { class: "w-8 h-8 rounded-lg border border-app-border text-foreground/60 flex items-center justify-center bg-transparent focus:outline-2 focus:outline-app-border/50 cursor-pointer",
                            Icon {
                                source: IconSource::Named("ellipsis".into()),
                                size: 16,
                            }
                        }
                        DropdownMenuContent { id: use_signal(|| None::<String>), class: "",
                            if show_ticket {
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "ticket".to_string()),
                                    index: use_signal(|| 0),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| ticket_target.set(Some(id_for_ticket.clone())),
                                    "Show ticket"
                                }
                            }
                            if let Some(url) = open_url {
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "open".to_string()),
                                    index: use_signal(move || open_index),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| {
                                        let _ = that(&url);
                                    },
                                    "Open in browser"
                                }
                            }
                            if let Some(ticket) = connect_ticket {
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "copy".to_string()),
                                    index: use_signal(move || copy_index),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| {
                                        if let Err(error) = copy_to_clipboard(&ticket) {
                                            tracing::warn!("copy ticket failed: {error}");
                                        }
                                    },
                                    "Copy ticket"
                                }
                            }
                            if !is_share {
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "port".to_string()),
                                    index: use_signal(move || port_index),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| on_port.call(tunnel_for_port.clone()),
                                    "Local port…"
                                }
                            }
                            if is_share {
                                DropdownMenuItem::<String> {
                                    value: use_signal(|| "new-ticket".to_string()),
                                    index: use_signal(move || new_ticket_index),
                                    disabled: use_signal(|| false),
                                    on_select: move |_| on_new_ticket.call(tunnel_for_new_ticket.clone()),
                                    "New ticket…"
                                }
                            }
                            DropdownMenuItem::<String> {
                                value: use_signal(|| "rename".to_string()),
                                index: use_signal(move || rename_index),
                                disabled: use_signal(|| false),
                                on_select: move |_| on_rename.call(tunnel_for_rename.clone()),
                                "Rename"
                            }
                            DropdownMenuSeparator {}
                            DropdownMenuItem::<String> {
                                value: use_signal(|| "delete".to_string()),
                                index: use_signal(move || rename_index + 1),
                                disabled: use_signal(|| false),
                                on_select: move |_| on_delete.call(tunnel_for_delete.clone()),
                                destructive: true,
                                "Delete"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn Row(
    icon: &'static str,
    text: String,
    #[props(default)] spin: bool,
    #[props(default)] muted: bool,
) -> Element {
    rsx! {
        div { class: "flex items-center gap-2.5 text-icon-tunnel min-w-0",
            Icon {
                source: IconSource::Named(icon.into()),
                size: 14,
                class: if spin { "animate-spin shrink-0" } else { "shrink-0" },
            }
            span { class: if muted { "text-xs text-foreground/60 truncate" } else { "text-xs text-foreground truncate" },
                "{text}"
            }
        }
    }
}

#[component]
fn ErrorRow(text: String) -> Element {
    rsx! {
        div { class: "text-xs text-alert-red-dark break-words max-w-md", "{text}" }
    }
}

fn device_label(name: &Option<String>, endpoint_id: &str) -> String {
    match name {
        Some(name) if !name.trim().is_empty() => name.clone(),
        _ => format!(
            "Device {}…",
            endpoint_id.chars().take(8).collect::<String>()
        ),
    }
}

fn path_label(path: &Option<LocalPath>) -> String {
    path.as_ref()
        .map(LocalPath::summary)
        .unwrap_or_else(|| "Idle".to_string())
}

/// QR code for `ticket` as an SVG document. Always dark on light, whatever
/// the theme, so phone cameras can read it.
fn ticket_qr_svg(ticket: &str) -> Option<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(ticket.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(168, 168)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .quiet_zone(true)
            .build(),
    )
}

/// Shows a share's ticket and QR code. Opened through [`LocalTicketTarget`].
#[component]
pub fn LocalTicketDialog() -> Element {
    let state = consume_context::<AppState>();
    let LocalTicketTarget(mut target) = consume_context::<LocalTicketTarget>();
    let tunnels = state.local_tunnel_list();
    let mut copied = use_signal(|| None::<&'static str>);
    let tunnel = target().and_then(|id| tunnels().into_iter().find(|t| t.id == id));

    // A deleted share closes the dialog.
    use_effect(move || {
        if let Some(id) = target() {
            if !tunnels().iter().any(|t| t.id == id) {
                target.set(None);
            }
        }
    });
    use_effect(move || {
        let _ = target();
        copied.set(None);
    });

    let Some(tunnel) = tunnel else {
        return rsx! {};
    };
    let state_for_start = state.clone();
    let id_for_start = tunnel.id.clone();

    rsx! {
        DialogRoot {
            open: true,
            on_open_change: move |open: bool| {
                if !open {
                    target.set(None);
                }
            },
            is_modal: true,
            DialogContent {
                DialogTitle { "Share “{tunnel.name}”" }
                div { class: "mt-4 w-[452px] space-y-4",
                    match &tunnel.status {
                        LocalTunnelStatus::Sharing { ticket } => {
                            let qr = ticket_qr_svg(ticket);
                            let ticket_for_copy = ticket.clone();
                            let command = format!("datumctl connect dial {ticket}");
                            rsx! {
                                div { class: "flex gap-4 items-start",
                                    if let Some(svg) = qr {
                                        div {
                                            class: "shrink-0 rounded-lg border border-app-border bg-white p-1",
                                            dangerous_inner_html: "{svg}",
                                        }
                                    }
                                    div { class: "flex flex-col gap-3 min-w-0 flex-1",
                                        p { class: "text-xs text-foreground/80 leading-relaxed",
                                            "Scan the code with the Datum Connect app, or paste the ticket under "
                                            span { class: "font-medium", "Open a ticket" }
                                            " on another computer."
                                        }
                                        div { class: "rounded-lg border border-app-border bg-content-background p-2 font-mono text-[10px] leading-snug text-foreground break-all select-all max-h-20 overflow-y-auto",
                                            "{ticket}"
                                        }
                                        div { class: "flex flex-wrap items-center gap-2",
                                            Button {
                                                kind: ButtonKind::Primary,
                                                text: if copied() == Some("ticket") { "Copied" } else { "Copy ticket" },
                                                leading_icon: if copied() == Some("ticket") { Some(IconSource::Named("check".into())) } else { None },
                                                onclick: move |_| match copy_to_clipboard(&ticket_for_copy) {
                                                    Ok(()) => copied.set(Some("ticket")),
                                                    Err(error) => tracing::warn!("copy ticket failed: {error}"),
                                                },
                                            }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                text: if copied() == Some("command") { "Copied" } else { "Copy command" },
                                                onclick: move |_| match copy_to_clipboard(&command) {
                                                    Ok(()) => copied.set(Some("command")),
                                                    Err(error) => tracing::warn!("copy command failed: {error}"),
                                                },
                                            }
                                        }
                                    }
                                }
                                div { class: "text-1xs text-foreground/60 leading-relaxed",
                                    "Anyone with this ticket can pair while the share is on, up to four devices at a time. "
                                    "It keeps working when the share restarts; use New ticket to cut off every device that has it. "
                                    "It contains this computer's IP addresses."
                                }
                                if !tunnel.devices.is_empty() {
                                    div { class: "border-t border-tunnel-card-border pt-3 space-y-1",
                                        div { class: "text-xs font-medium text-foreground", "Paired devices" }
                                        for device in tunnel.devices.iter() {
                                            div { class: "text-1xs text-foreground/70",
                                                "{device_label(&device.name, &device.endpoint_id)} · {path_label(&device.path)}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        LocalTunnelStatus::Starting => rsx! {
                            div { class: "flex items-center gap-2 text-xs text-foreground/80 py-6 justify-center",
                                Icon {
                                    source: IconSource::Named("loader-circle".into()),
                                    size: 14,
                                    class: "animate-spin",
                                }
                                "Getting a ticket…"
                            }
                        },
                        LocalTunnelStatus::Failed(error) => rsx! {
                            div { class: "rounded-md border border-red-200 bg-red-50 p-4 text-red-800",
                                div { class: "text-sm font-semibold", "Couldn't share" }
                                div { class: "text-sm mt-1 break-words", "{error}" }
                            }
                        },
                        _ => rsx! {
                            p { class: "text-xs text-foreground/80", "This share is off." }
                        },
                    }
                    div { class: "flex items-center gap-2.5 justify-end pt-1",
                        if !tunnel.status.is_active() {
                            Button {
                                kind: ButtonKind::Primary,
                                text: "Start sharing",
                                onclick: move |_| state_for_start.local_tunnels().start(&id_for_start),
                            }
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            text: "Done",
                            onclick: move |_| target.set(None),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RenameLocalTunnelDialog(
    tunnel: ReadSignal<Option<LocalTunnel>>,
    on_close: EventHandler<()>,
) -> Element {
    let state = consume_context::<AppState>();
    let mut name = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        if let Some(t) = tunnel() {
            name.set(t.name.clone());
            error.set(None);
        }
    });

    let Some(current) = tunnel() else {
        return rsx! {};
    };
    let id = current.id.clone();
    let hint = match current.kind {
        LocalTunnelKind::Share { .. } => {
            Some("Devices see the new name the next time the share starts.".to_string())
        }
        LocalTunnelKind::Connect { .. } => None,
    };
    let mut save = move || match state.local_tunnels().rename(&id, &name()) {
        Ok(()) => on_close.call(()),
        Err(message) => error.set(Some(message)),
    };
    let mut save_on_enter = save.clone();

    rsx! {
        DialogRoot {
            open: true,
            on_open_change: move |open: bool| {
                if !open {
                    on_close.call(());
                }
            },
            is_modal: true,
            DialogContent {
                DialogTitle { "Rename local tunnel" }
                form {
                    class: "space-y-5 mt-5 w-[380px]",
                    autocomplete: "off",
                    onsubmit: move |e: FormEvent| {
                        e.prevent_default();
                        save_on_enter();
                    },
                    Input {
                        id: Some("local-tunnel-name".into()),
                        label: Some("Name".into()),
                        value: "{name}",
                        description: hint,
                        error: error(),
                        maxlength: "64",
                        oninput: move |e: FormEvent| {
                            name.set(e.value());
                            error.set(None);
                        },
                    }
                    div { class: "flex items-center gap-2.5 justify-start",
                        Button {
                            kind: ButtonKind::Primary,
                            text: "Save",
                            onclick: move |_| save(),
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            text: "Cancel",
                            onclick: move |_| on_close.call(()),
                        }
                    }
                }
            }
        }
    }
}

/// Changes the local port a connection serves on. A running connection
/// restarts on the new port; empty input lets the system pick. Prefills
/// the requested port, or the one it is bound to while it runs.
#[component]
fn LocalPortDialog(tunnel: ReadSignal<Option<LocalTunnel>>, on_close: EventHandler<()>) -> Element {
    let state = consume_context::<AppState>();
    let mut port = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        if let Some(t) = tunnel() {
            let initial = match &t.kind {
                LocalTunnelKind::Connect { port, .. } if *port != 0 => port.to_string(),
                LocalTunnelKind::Connect { .. } => match &t.status {
                    LocalTunnelStatus::Open { local_addr } => local_addr
                        .parse::<SocketAddr>()
                        .map(|addr| addr.port().to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                },
                LocalTunnelKind::Share { .. } => String::new(),
            };
            port.set(initial);
            error.set(None);
        }
    });

    let Some(current) = tunnel() else {
        return rsx! {};
    };
    let id = current.id.clone();
    let mut save = move || {
        let raw = port().trim().to_string();
        let chosen: u16 = if raw.is_empty() {
            0
        } else {
            match raw.parse() {
                Ok(port) => port,
                Err(_) => {
                    error.set(Some(
                        "Port must be a number from 1 to 65535, or empty.".to_string(),
                    ));
                    return;
                }
            }
        };
        match state.local_tunnels().set_port(&id, chosen) {
            Ok(()) => on_close.call(()),
            Err(message) => error.set(Some(message)),
        }
    };
    let mut save_on_enter = save.clone();

    rsx! {
        DialogRoot {
            open: true,
            on_open_change: move |open: bool| {
                if !open {
                    on_close.call(());
                }
            },
            is_modal: true,
            DialogContent {
                DialogTitle { "Change local port" }
                form {
                    class: "space-y-5 mt-5 w-[380px]",
                    autocomplete: "off",
                    onsubmit: move |e: FormEvent| {
                        e.prevent_default();
                        save_on_enter();
                    },
                    Input {
                        id: Some("local-tunnel-port".into()),
                        label: Some("Local port".into()),
                        value: "{port}",
                        placeholder: "Automatic",
                        description: Some(
                            "The connection restarts on the new port. Leave empty for the system to pick."
                                .to_string(),
                        ),
                        error: error(),
                        inputmode: "numeric",
                        oninput: move |e: FormEvent| {
                            port.set(e.value().chars().filter(char::is_ascii_digit).take(5).collect());
                            error.set(None);
                        },
                    }
                    div { class: "flex items-center gap-2.5 justify-start",
                        Button {
                            kind: ButtonKind::Primary,
                            text: "Save",
                            onclick: move |_| save(),
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            text: "Cancel",
                            onclick: move |_| on_close.call(()),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DeleteLocalTunnelDialog(
    tunnel: ReadSignal<Option<LocalTunnel>>,
    on_close: EventHandler<()>,
) -> Element {
    let state = consume_context::<AppState>();
    let Some(current) = tunnel() else {
        return rsx! {};
    };
    let message = match current.kind {
        LocalTunnelKind::Share { .. } => format!(
            "Delete \"{}\"? Paired devices lose access and its ticket stops working.",
            current.name
        ),
        LocalTunnelKind::Connect { .. } => format!(
            "Delete \"{}\"? This closes its local port and forgets the ticket.",
            current.name
        ),
    };
    let id = current.id.clone();

    rsx! {
        DialogRoot {
            open: true,
            on_open_change: move |open: bool| {
                if !open {
                    on_close.call(());
                }
            },
            is_modal: true,
            DialogContent {
                DialogTitle { "Delete local tunnel" }
                p { class: "mt-4 mb-6 text-sm text-foreground/80 max-w-md", "{message}" }
                div { class: "flex items-center gap-2.5 justify-end",
                    Button {
                        kind: ButtonKind::Ghost,
                        text: "Cancel",
                        onclick: move |_| on_close.call(()),
                    }
                    Button {
                        kind: ButtonKind::Primary,
                        text: "Delete",
                        onclick: move |_| {
                            state.local_tunnels().remove(&id);
                            on_close.call(());
                        },
                    }
                }
            }
        }
    }
}

/// Confirms giving a share a new ticket, which cuts off devices holding the
/// old one. A running share then shows its new ticket.
#[component]
fn NewTicketDialog(tunnel: ReadSignal<Option<LocalTunnel>>, on_close: EventHandler<()>) -> Element {
    let state = consume_context::<AppState>();
    let LocalTicketTarget(mut ticket_target) = consume_context::<LocalTicketTarget>();
    let Some(current) = tunnel() else {
        return rsx! {};
    };
    let message = format!(
        "Give \"{}\" a new ticket? Devices using the current ticket lose access and must scan the new one.",
        current.name
    );
    let id = current.id.clone();
    let running = current.status.is_active();

    rsx! {
        DialogRoot {
            open: true,
            on_open_change: move |open: bool| {
                if !open {
                    on_close.call(());
                }
            },
            is_modal: true,
            DialogContent {
                DialogTitle { "New ticket" }
                p { class: "mt-4 mb-6 text-sm text-foreground/80 max-w-md", "{message}" }
                div { class: "flex items-center gap-2.5 justify-end",
                    Button {
                        kind: ButtonKind::Ghost,
                        text: "Cancel",
                        onclick: move |_| on_close.call(()),
                    }
                    Button {
                        kind: ButtonKind::Primary,
                        text: "New ticket",
                        onclick: move |_| {
                            state.local_tunnels().new_ticket(&id);
                            on_close.call(());
                            if running {
                                ticket_target.set(Some(id.clone()));
                            }
                        },
                    }
                }
            }
        }
    }
}
