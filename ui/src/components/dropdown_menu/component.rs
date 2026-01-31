use dioxus::prelude::*;
use dioxus_primitives::dropdown_menu::{
    self, DropdownMenuContentProps, DropdownMenuProps, DropdownMenuTriggerProps,
};

use crate::components::icon::{Icon, IconSource};

/// Dark backdrop when dropdown is open (same style as dialog). Only visible when using controlled `open` state.
const BACKDROP_CLASS: &str = "fixed inset-0 bg-foreground/30 z-40 mt-10 rounded-b-md animate-in fade-in duration-100";

#[component]
pub fn DropdownMenu(props: DropdownMenuProps) -> Element {
    let is_open = move || (props.open)() == Some(true);
    rsx! {
        if is_open() {
            div {
                class: BACKDROP_CLASS,
                onclick: move |_| props.on_open_change.call(false),
            }
        }
        dropdown_menu::DropdownMenu {
            open: props.open,
            default_open: props.default_open,
            on_open_change: props.on_open_change,
            disabled: props.disabled,
            roving_loop: props.roving_loop,
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn DropdownMenuTrigger(props: DropdownMenuTriggerProps) -> Element {
    rsx! {
        dropdown_menu::DropdownMenuTrigger { attributes: props.attributes, {props.children} }
    }
}

#[component]
pub fn DropdownMenuContent(props: DropdownMenuContentProps) -> Element {
    rsx! {
        dropdown_menu::DropdownMenuContent {
            id: props.id,
            attributes: props.attributes,
            class: "absolute right-0 min-w-36 top-0 rounded-md border-[#dfe3ea] bg-white shadow-card overflow-hidden z-50 p-1 animate-in fade-in duration-300",
            {props.children}
        }
    }
}

/// A 1px horizontal line separator for use inside [`DropdownMenuContent`].
/// Uses negative margins so the line spans the full width of the dropdown (parent has p-1).
#[component]
pub fn DropdownMenuSeparator() -> Element {
    rsx! {
        div {
            class: "h-px w-[calc(100%+0.5rem)] -mx-1 bg-app-border my-1",
            role: "separator",
        }
    }
}

const ITEM_CLASS: &str = "w-full text-left px-2 py-2 text-xs hover:bg-content-background text-foreground rounded-md cursor-default";
const ITEM_DESTRUCTIVE_CLASS: &str = "w-full text-left px-2 py-2 text-xs hover:bg-content-background text-alert-red-dark rounded-md cursor-default";

/// Props for our DropdownMenuItem wrapper (adds `destructive` and optional `icon` over the primitive).
#[derive(Props, Clone, PartialEq)]
pub struct DropdownMenuItemProps<T: Clone + PartialEq + 'static> {
    pub value: ReadSignal<T>,
    pub index: ReadSignal<usize>,
    #[props(default)]
    pub disabled: ReadSignal<bool>,
    #[props(default)]
    pub on_select: Callback<T>,
    /// When true, applies destructive styling (e.g. red text for delete actions).
    #[props(default = false)]
    pub destructive: bool,
    /// Optional icon shown on the right; text and icon are laid out with justify-between.
    #[props(default = None)]
    pub icon: Option<IconSource>,
    #[props(extends = GlobalAttributes)]
    pub attributes: Vec<Attribute>,
    pub children: Element,
}

#[component]
pub fn DropdownMenuItem<T: Clone + PartialEq + 'static>(
    props: DropdownMenuItemProps<T>,
) -> Element {
    let item_class = if props.destructive {
        ITEM_DESTRUCTIVE_CLASS
    } else {
        ITEM_CLASS
    };
    let mut attrs = vec![Attribute::new("class", item_class, None, false)];
    attrs.extend(props.attributes);
    let content = match &props.icon {
        Some(icon_source) => rsx! {
            div { class: "flex items-center justify-between w-full gap-2",
                {props.children}
                Icon { source: icon_source.clone(), size: 14 }
            }
        },
        None => props.children,
    };
    rsx! {
        dropdown_menu::DropdownMenuItem {
            value: props.value,
            index: props.index,
            disabled: props.disabled,
            on_select: props.on_select,
            attributes: attrs,
            {content}
        }
    }
}
