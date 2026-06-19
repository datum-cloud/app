//! Minimal layout that handles tray "New tunnel" navigation. Must be inside Router
//! (as a route layout) so use_navigator works. Used for Login which has no Chrome.

use crate::Route;
use dioxus::prelude::*;

#[component]
pub fn TrayNavHandler() -> Element {
    let nav = use_navigator();
    let open_add_tunnel_from_tray = consume_context::<Signal<bool>>();
    let mut open_tunnel_from_tray = consume_context::<Signal<Option<String>>>();

    use_effect(move || {
        if open_add_tunnel_from_tray() {
            nav.push(Route::ProxiesList {});
            // Don't reset here - Chrome will open the dialog and reset when it mounts
        }
    });

    use_effect(move || {
        if let Some(id) = open_tunnel_from_tray() {
            nav.push(Route::TunnelBandwidth { id });
            open_tunnel_from_tray.set(None);
        }
    });

    rsx! {
        Outlet::<Route> {}
    }
}
