use dioxus::prelude::*;

#[derive(PartialEq, Clone, Props)]
pub struct ChildProps {
    text: String,
}

#[component]
pub fn Subhead(props: ChildProps) -> Element {
    rsx! {
        h2 { class: "uppercase text-sm font-bold text-foreground", {props.text} }
    }
}
