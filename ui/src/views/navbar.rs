use crate::{
    components::{Button, ButtonKind},
    state::AppState,
    Route,
};
use dioxus::events::MouseEvent;
use dioxus::prelude::*;
use dioxus_desktop::DesktopContext;

#[component]
pub fn Chrome() -> Element {
    rsx! {
        div { class: "h-screen overflow-hidden flex flex-col bg-[#f4f4f1] text-gray-900 rounded-[12px] border border-black/10 shadow-[0_18px_60px_rgba(0,0,0,0.18)]",
            HeaderBar {}
            Outlet::<Route> {}
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    let sidebar = rsx! {
        // Sidebar
        div { class: "w-52 min-w-[208px] max-w-[208px] shrink-0 flex-none bg-[#f2f2ee] border-r border-[#e3e3dc] pt-6 pb-6 px-6 flex flex-col",
            // Full-width content with equal left/right padding
            div { class: "w-full",
                Button {
                    to: Some(Route::CreateProxy { }),
                    leading: Some("+".to_string()),
                    text: "Add tunnel",
                    kind: ButtonKind::Primary,
                    // Keep label on one line in narrower sidebar
                    class: Some("w-full font-normal whitespace-nowrap px-6 gap-2".to_string()),
                }
            }

            // Bottom nav (visual-only for now)
            div { class: "w-full mt-auto space-y-4 text-gray-600 pl-2",
                div { class: "flex items-center gap-3 cursor-pointer hover:text-gray-900",
                    NavIconBook {}
                    span { class: "text-base font-medium", "Docs" }
                }
                div { class: "flex items-center gap-3 cursor-pointer hover:text-gray-900",
                    NavIconUsers {}
                    span { class: "text-base font-medium", "Invite" }
                }
                div { class: "flex items-center gap-3 cursor-pointer hover:text-gray-900",
                    NavIconGear {}
                    span { class: "text-base font-medium", "Settings" }
                }
            }
        }
    };

    rsx! {
        // Content row
        div { class: "flex flex-1 min-h-0",
            {sidebar}

            // Main content
            // Important: `min-h-0` allows the scroll container to shrink within the flex layout.
            div { class: "flex-1 min-h-0 overflow-y-auto pt-6 pb-8 px-8",
                Outlet::<Route> {}
            }
        }
    }
}

#[component]
pub fn HeaderBar() -> Element {
    let window = || consume_context::<DesktopContext>();
    let state = consume_context::<AppState>();
    let auth_state = state.datum().auth_state();
    let nav = use_navigator();
    let mut menu_open = use_signal(|| false);
    let user_name = match auth_state.get() {
        Ok(auth) => auth.profile.display_name(),
        Err(_) => "Not logged in".to_string(),
    };

    let mut logout = use_action(move |_: ()| async move {
        let state = consume_context::<AppState>();
        state.datum().auth().logout().await?;
        nav.push(Route::Login {});
        n0_error::Ok(())
    });

    rsx! {
        // Custom titlebar (color + height)
        div {
            class: "h-12 bg-[#f2f2ee] border-b border-[#e3e3dc] flex items-center select-none cursor-grab active:cursor-grabbing",
            onmousedown: move |_| window().drag(),
            // macOS-ish window controls
            div {
                class: "flex items-center gap-2 px-4 cursor-default",
                onmousedown: move |evt: MouseEvent| evt.stop_propagation(),
                button {
                    class: "w-3.5 h-3.5 rounded-full bg-[#ff5f57] border border-black/10 hover:brightness-95 cursor-pointer",
                    onclick: move |_| window().set_visible(false),
                }
                button {
                    class: "w-3.5 h-3.5 rounded-full bg-[#febc2e] border border-black/10 hover:brightness-95 cursor-pointer",
                    onclick: move |_| window().set_minimized(true),
                }
                button {
                    class: "w-3.5 h-3.5 rounded-full bg-[#28c840] border border-black/10 hover:brightness-95 cursor-pointer",
                    onclick: move |_| window().toggle_maximized(),
                }
            }
            div { class: "flex-1 text-center text-sm font-medium text-slate-600", "Datum Connect" }
            // Profile icon (top-right)
            div {
                class: "px-4",
                onmousedown: move |evt: MouseEvent| evt.stop_propagation(),
                button {
                    class: "w-8 h-8 rounded-full border border-[#dfe3ea] bg-white flex items-center justify-center text-slate-600 hover:text-slate-800 hover:bg-gray-50 shadow-sm cursor-pointer",
                    // TODO: wire to profile menu/settings
                    onclick: move |evt: MouseEvent| {
                        evt.stop_propagation();
                        menu_open.set(!menu_open());
                    },
                    svg {
                        width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
                        path { d: "M12 12a4 4 0 1 0-4-4 4 4 0 0 0 4 4Z", stroke: "currentColor", stroke_width: "1.6" }
                        path { d: "M4 21c1.6-3.5 4.6-5 8-5s6.4 1.5 8 5", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round" }
                    }
                }

                if menu_open() {
                    // Full-screen click-catcher so any click outside closes the menu.
                    // This also prevents the card click handler from triggering.
                    div {
                        class: "fixed inset-0 z-40",
                        onclick: move |evt: MouseEvent| {
                            evt.stop_propagation();
                            menu_open.set(false);
                        }
                    }
                    div {
                        class: "absolute right-0 mt-2 w-44 rounded-xl border border-[#dfe3ea] bg-white shadow-[0_12px_30px_rgba(17,24,39,0.14)] overflow-hidden z-50",
                        onclick: move |evt: MouseEvent| evt.stop_propagation(),
                        button {
                            class: "w-full text-left px-4 py-3 text-sm text-slate-800 hover:bg-gray-50",
                            {user_name}
                        }
                        if auth_state.get().is_ok() {
                            button {
                                class: "w-full text-left px-4 py-3 text-sm text-red-600 hover:bg-red-50",
                                onclick: move |evt: MouseEvent| {
                                    evt.stop_propagation();
                                    menu_open.set(false);
                                    logout.call(());
                                },
                                "Logout"
                            }
                        }
                    }
                }
            }
        }

    }
}

#[component]
fn NavIconBook() -> Element {
    rsx! {
        svg {
            width: "20", height: "20", view_box: "0 0 24 24", fill: "none",
            class: "text-gray-500",
            path { d: "M4 5.5C4 4.12 5.12 3 6.5 3H20v17.5a2.5 2.5 0 0 1-2.5 2.5H6.5A2.5 2.5 0 0 1 4 20.5v-15Z", stroke: "currentColor", stroke_width: "1.6" }
            path { d: "M8 3v18", stroke: "currentColor", stroke_width: "1.6" }
        }
    }
}

#[component]
fn NavIconUsers() -> Element {
    rsx! {
        svg {
            width: "20", height: "20", view_box: "0 0 24 24", fill: "none",
            class: "text-gray-500",
            path { d: "M16 11a4 4 0 1 0-8 0 4 4 0 0 0 8 0Z", stroke: "currentColor", stroke_width: "1.6" }
            path { d: "M4 20c1.2-3.5 4-5 8-5s6.8 1.5 8 5", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round" }
        }
    }
}

#[component]
fn NavIconGear() -> Element {
    rsx! {
        svg {
            width: "20", height: "20", view_box: "0 0 24 24", fill: "none",
            class: "text-gray-500",
            // Use a standard "settings" gear path (lucide/feather-style) to avoid skew/squish.
            circle { cx: "12", cy: "12", r: "3", stroke: "currentColor", stroke_width: "1.6" }
            path {
                d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1Z",
                stroke: "currentColor",
                stroke_width: "1.6",
                stroke_linejoin: "round",
                stroke_linecap: "round",
            }
        }
    }
}
