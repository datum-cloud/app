use dioxus::prelude::*;
use dioxus_primitives::select::{
    self, SelectGroupLabelProps, SelectGroupProps, SelectOptionProps, SelectProps,
    SelectTriggerProps, SelectValueProps,
};
use crate::components::icon::{Icon, IconSource};

/// Alignment of the select list relative to the trigger (start = left, end = right, center = centered).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectAlign {
    Start,
    Center,
    End,
}

#[component]
pub fn Select<T: Clone + PartialEq + 'static>(props: SelectProps<T>) -> Element {
    rsx! {
        select::Select {
            class: "relative w-full",
            value: props.value,
            default_value: props.default_value,
            on_value_change: props.on_value_change,
            disabled: props.disabled,
            name: props.name,
            placeholder: props.placeholder,
            roving_loop: props.roving_loop,
            typeahead_timeout: props.typeahead_timeout,
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn SelectTrigger(props: SelectTriggerProps) -> Element {
    rsx! {
        select::SelectTrigger {
            class: "w-full h-6 min-w-0 rounded-md border border-app-border bg-white px-2 text-left text-xs text-foreground hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-app-border inline-flex items-center justify-between gap-2 cursor-default data-disabled:opacity-50 data-disabled:cursor-not-allowed",
            attributes: props.attributes,
            {props.children}
            Icon {
                source: IconSource::Named("chevron-down".into()),
                size: 9,
                class: "shrink-0 flex items-center justify-center mb-0.5 text-icon-select",
            }
        }
    }
}

#[component]
pub fn SelectValue(props: SelectValueProps) -> Element {
    rsx! {
        select::SelectValue { attributes: props.attributes }
    }
}

/// SelectList positioning: content-aware so the list doesn't render off the view.
/// - Width follows longest item (w-max min-w-max), won't shrink below content.
/// - Constrains max-height to viewport so the list scrolls instead of overflowing.
/// - Pass `align` to position the list (start/center/end) relative to the trigger.
const SELECT_LIST_BASE_CLASS: &str = "absolute z-50 w-max min-w-max max-w-[min(calc(100vw-2rem),40rem)] max-h-[min(20rem,calc(100vh-2rem))] overflow-y-auto overflow-x-auto rounded-md border border-app-border bg-white shadow-card p-1 animate-in fade-in duration-300 outline-none focus:outline-none top-full mt-1 data-[side=top]:top-auto data-[side=top]:bottom-full data-[side=top]:mt-0 data-[side=top]:mb-1 data-[side=bottom]:bottom-auto data-[side=bottom]:top-full data-[side=bottom]:mb-0 data-[side=bottom]:mt-1";

fn align_class(align: Option<SelectAlign>) -> &'static str {
    match align {
        None | Some(SelectAlign::Start) => "left-0 right-auto",
        Some(SelectAlign::Center) => "left-1/2 right-auto -translate-x-1/2",
        Some(SelectAlign::End) => "left-auto right-0",
    }
}

#[component]
pub fn SelectList(
    #[props(default = None)] align: Option<SelectAlign>,
    #[props(default = None)] id: Option<String>,
    children: Element,
) -> Element {
    let class = format!("{} {}", SELECT_LIST_BASE_CLASS, align_class(align));
    rsx! {
        select::SelectList { class, id, attributes: vec![], {children} }
    }
}

#[component]
pub fn SelectGroup(props: SelectGroupProps) -> Element {
    rsx! {
        select::SelectGroup {
            class: "",
            disabled: props.disabled,
            id: props.id,
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn SelectGroupLabel(props: SelectGroupLabelProps) -> Element {
    rsx! {
        select::SelectGroupLabel { class: "", id: props.id, attributes: props.attributes, {props.children} }
    }
}

#[component]
pub fn SelectOption<T: Clone + PartialEq + 'static>(props: SelectOptionProps<T>) -> Element {
    rsx! {
        select::SelectOption::<T> {
            class: "w-full text-left px-2 py-2 text-xs hover:bg-content-background text-foreground rounded-md cursor-default data-highlighted:bg-content-background flex items-center justify-between gap-2 whitespace-nowrap",
            value: props.value,
            text_value: props.text_value,
            disabled: props.disabled,
            id: props.id,
            index: props.index,
            aria_label: props.aria_label,
            aria_roledescription: props.aria_roledescription,
            attributes: props.attributes,
            {props.children}
        }
    }
}

/// Convenience wrapper when you have a plain value (e.g. String) and need to pass ReadSignal<T> to SelectOption.
#[component]
pub fn SelectOptionItem(
    value: String,
    text_value: String,
    index: usize,
    #[props(default = false)] disabled: bool,
    children: Element,
) -> Element {
    let value_signal = use_signal(move || value);
    rsx! {
        SelectOption::<String> {
            value: value_signal,
            text_value,
            index,
            disabled,
            {children}
        }
    }
}

#[component]
pub fn SelectItemIndicator() -> Element {
    rsx! {
        select::SelectItemIndicator {
            svg {
                class: "shrink-0 flex items-center size-4 justify-center fill-none stroke-icon-select",
                view_box: "0 0 24 24",
                xmlns: "http://www.w3.org/2000/svg",
                path { d: "M5 13l4 4L19 7" }
            }
        }
    }
}
