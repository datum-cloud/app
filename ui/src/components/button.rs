use dioxus::prelude::*;

use crate::Route;

#[derive(PartialEq, Clone, Copy)]
pub enum ButtonKind {
    Primary,
    Secondary,
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
    /// Additional classes appended to the base button classes
    #[props(default = None)]
    class: Option<String>,
}

fn class_for(kind: ButtonKind) -> &'static str {
    match kind {
        // Figma-ish: dark slate pill button
        ButtonKind::Primary => "inline-flex items-center justify-center gap-3 rounded-xl px-8 py-4 bg-[#4b5664] text-white font-medium shadow-sm hover:bg-[#3f4854] transition-colors",
        // Figma-ish: white pill button with soft border + shadow
        ButtonKind::Secondary => "inline-flex items-center justify-center gap-3 rounded-xl px-8 py-4 bg-white text-gray-900 font-semibold border border-[#dfe3ea] shadow-sm hover:bg-[#fafafa] transition-colors",
        ButtonKind::Ghost => "inline-flex items-center justify-center gap-2 rounded-xl px-4 py-2 bg-transparent text-gray-700 font-medium hover:bg-gray-100 transition-colors",
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
    match (props.to.is_some(), props.onclick.is_some()) {
        (true, false) => {
            rsx! {
                Link {
                    to: to_route.unwrap(),
                    class: "{class}",
                    if let Some(leading) = props.leading.clone() {
                        span { class: "text-xl leading-none", "{leading}" }
                    }
                    span { class: "leading-none", "{props.text}" }
                }
            }
        }
        (false, true) => {
            rsx! {
                button {
                    class: "{class}",
                    onclick: move |evt| props.onclick.unwrap().call(evt),
                    if let Some(leading) = props.leading.clone() {
                        span { class: "text-xl leading-none", "{leading}" }
                    }
                    span { class: "leading-none", "{props.text}" }
                }
            }
        }
        _ => {
            rsx! {
                button {
                    class: "{class}",
                    if let Some(leading) = props.leading.clone() {
                        span { class: "text-xl leading-none", "{leading}" }
                    }
                    span { class: "leading-none", "{props.text}" }
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
