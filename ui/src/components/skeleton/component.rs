use dioxus::prelude::*;

#[component]
pub fn Skeleton(
    /// Additional classes to merge with the base skeleton classes
    #[props(default = None)]
    class: Option<String>,
    #[props(extends=GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let base_class = "rounded-md bg-foreground/10 animate-pulse";
    let merged_class = match class.as_deref() {
        Some(extra) if !extra.is_empty() => format!("{base_class} {extra}"),
        _ => base_class.to_string(),
    };

    rsx! {
        div { class: "{merged_class}", ..attributes }
    }
}
