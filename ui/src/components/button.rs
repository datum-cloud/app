use dioxus::prelude::*;

use crate::components::icon::{Icon, IconSource};
use crate::Route;

#[derive(PartialEq, Clone, Copy)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Outline,
    #[allow(unused)]
    Ghost,
}

#[derive(PartialEq, Clone, Props)]
pub struct ButtonProps {
    text: String,
    to: Option<Route>,
    onclick: Option<EventHandler<MouseEvent>>,
    #[props(default = ButtonKind::Primary)]
    kind: ButtonKind,
    #[props(default = None)]
    leading: Option<String>,
    /// Leading icon from assets/icons or IconKind (shown before text)
    #[props(default = None)]
    leading_icon: Option<IconSource>,
    /// Trailing icon from assets/icons or IconKind (shown after text)
    #[props(default = None)]
    trailing_icon: Option<IconSource>,
    /// Additional classes appended to the base button classes
    #[props(default = None)]
    class: Option<String>,
}

fn class_for(kind: ButtonKind) -> &'static str {
    // [transform:translateZ(0)] keeps the button on its own compositing layer so opacity hover doesn't cause subpixel shift
    match kind {
        ButtonKind::Primary => "inline-flex items-center justify-center gap-2 rounded-md px-3.5 py-2.5 bg-button-primary-background/90 text-button-primary-foreground hover:opacity-80 transition-opacity duration-300 border border-1 border-button-primary-background [transform:translateZ(0)] text-xs",
        ButtonKind::Secondary => "inline-flex items-center justify-center gap-2 rounded-md px-3.5 py-2.5 bg-button-secondary-background/90 text-button-secondary-foreground border border-1 border-button-secondary-background hover:opacity-80 transition-opacity duration-300 text-xs [transform:translateZ(0)]",
        ButtonKind::Ghost => "inline-flex items-center justify-center gap-2 rounded-md px-3.5 py-2.5 bg-transparent text-button-outline-foreground border border-1 border-button-outline-background hover:opacity-80 transition-opacity duration-300 [transform:translateZ(0)] text-xs",
        ButtonKind::Outline => "inline-flex items-center justify-center gap-2 rounded-md px-3.5 py-2.5 bg-transparent text-foreground border border-1 border-foreground hover:opacity-80 transition-opacity duration-300 [transform:translateZ(0)] text-xs",
    }
}

#[component]
pub fn Button(props: ButtonProps) -> Element {
    let base = class_for(props.kind);
    let class = match props.class.as_deref() {
        Some(extra) if !extra.is_empty() => format!("{base} {extra}"),
        _ => base.to_string(),
    };
    let to_route = props.to.clone();
    let leading_content = rsx! {
        if let Some(ref icon) = props.leading_icon {
            span { class: "flex shrink-0 items-center justify-start size-5 min-w-5 min-h-5",
                Icon { source: icon.clone(), size: 16 }
            }
        } else if let Some(leading) = props.leading.clone() {
            span { class: "leading-none shrink-0", "{leading}" }
        }
    };

    let trailing_content = rsx! {
        if let Some(ref icon) = props.trailing_icon {
            span { class: "flex shrink-0 items-center justify-center size-5 min-w-5 min-h-5",
                Icon { source: icon.clone(), size: 16 }
            }
        }
    };
    

    match (props.to.is_some(), props.onclick.is_some()) {
        (true, false) => {
            rsx! {
                Link { to: to_route.unwrap(), class: "{class}",
                    {leading_content}
                    span { class: "leading-none", "{props.text}" }
                    {trailing_content}
                }
            }
        }
        (false, true) => {
            rsx! {
                button {
                    class: "{class}",
                    onclick: move |evt| props.onclick.unwrap().call(evt),
                    {leading_content}
                    span { class: "leading-none", "{props.text}" }
                    {trailing_content}
                }
            }
        }
        _ => {
            rsx! {
                button { class: "{class}",
                    {leading_content}
                    span { class: "leading-none", "{props.text}" }
                    {trailing_content}
                }
            }
        }
    }
}

#[derive(PartialEq, Clone, Props)]
pub struct CloseButtonProps {
    onclick: EventHandler<MouseEvent>,
}

#[component]
pub fn CloseButton(props: CloseButtonProps) -> Element {
    rsx! {
        button {
            class: "w-9 h-9 rounded-full border border-gray-300 text-gray-500 hover:text-gray-700 hover:bg-gray-50 cursor-pointer flex items-center justify-center",
            onclick: move |evt| {
                evt.stop_propagation();
                props.onclick.call(evt)
            },
            "×"
        }
    }
}
