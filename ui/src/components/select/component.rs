use crate::components::icon::{Icon, IconSource};
use dioxus::prelude::*;
use dioxus_primitives::select::{
    self, SelectGroupLabelProps, SelectGroupProps, SelectOptionProps, SelectProps, SelectValueProps,
};

/// Alignment of the select list relative to the trigger (start = left, end = right, center = centered).
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(unused)]
pub enum SelectAlign {
    Start,
    Center,
    End,
}

/// Size variant for select components
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectSize {
    Default,
    Small,
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

#[derive(Props, PartialEq)]
pub struct SelectTriggerPropsWithSize {
    #[props(default = Vec::new())]
    attributes: Vec<Attribute>,
    children: Element,
    #[props(default = SelectSize::Default)]
    size: SelectSize,
}

impl Clone for SelectTriggerPropsWithSize {
    fn clone(&self) -> Self {
        Self {
            attributes: self.attributes.clone(),
            children: self.children.clone(),
            size: self.size,
        }
    }
}

#[component]
pub fn SelectTrigger(props: SelectTriggerPropsWithSize) -> Element {
    let class = match props.size {
        SelectSize::Default => "w-full h-9 min-w-0 rounded-md border border-app-border bg-white px-2 text-left text-xs text-foreground hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-app-border inline-flex items-center justify-between gap-2 cursor-default data-disabled:opacity-50 data-disabled:cursor-not-allowed",
        SelectSize::Small => "w-full h-6 min-w-0 rounded-md border border-app-border bg-white px-2 text-left text-xs text-foreground hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-app-border inline-flex items-center justify-between gap-2 cursor-default data-disabled:opacity-50 data-disabled:cursor-not-allowed",
    };

    rsx! {
        select::SelectTrigger { class, attributes: props.attributes,
            {props.children}
            Icon {
                source: IconSource::Named("chevron-down".into()),
                size: match props.size {
                    SelectSize::Default => 14,
                    SelectSize::Small => 12,
                },
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
    #[props(default = SelectSize::Default)] size: SelectSize,
    children: Element,
) -> Element {
    let base_class = match size {
        SelectSize::Default => "absolute z-[60] w-full min-w-full max-h-[min(20rem,calc(100vh-2rem))] overflow-y-auto overflow-x-auto rounded-md border border-app-border bg-white shadow-card p-1 animate-in fade-in duration-300 outline-none focus:outline-none top-full mt-1 data-[side=top]:top-auto data-[side=top]:bottom-full data-[side=top]:mt-0 data-[side=top]:mb-1 data-[side=bottom]:bottom-auto data-[side=bottom]:top-full data-[side=bottom]:mb-0 data-[side=bottom]:mt-1",
        SelectSize::Small => "absolute z-[60] w-max min-w-max max-w-[20rem,calc(100vw-2rem)] max-h-[min(20rem,calc(100vh-2rem))] overflow-y-auto overflow-x-auto rounded-md border border-app-border bg-white shadow-card p-0.5 animate-in fade-in duration-300 outline-none focus:outline-none top-full mt-1 data-[side=top]:top-auto data-[side=top]:bottom-full data-[side=top]:mt-0 data-[side=top]:mb-1 data-[side=bottom]:bottom-auto data-[side=bottom]:top-full data-[side=bottom]:mb-0 data-[side=bottom]:mt-1",
    };
    let class = format!("{} {}", base_class, align_class(align));
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
    let initial_value = value.clone();
    let mut value_signal = use_signal(move || initial_value);
    // Update the signal when the value prop changes
    use_effect(move || {
        value_signal.set(value.clone());
    });
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
