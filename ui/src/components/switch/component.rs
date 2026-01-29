use dioxus::prelude::*;
use dioxus_primitives::switch::{self, SwitchProps, SwitchThumbProps};

#[component]
pub fn Switch(props: SwitchProps) -> Element {
    rsx! {
        switch::Switch {
            class: "group relative w-10 h-5.5 rounded-full bg-switch-disabled 
                    transition-colors duration-150 data-[state=checked]:bg-switch-checked 
                    data-[disabled=true]:cursor-not-allowed data-[disabled=true]:opacity-50 px-[1.70px]",
            checked: props.checked,
            default_checked: props.default_checked,
            disabled: props.disabled,
            required: props.required,
            name: props.name,
            value: props.value,
            on_checked_change: props.on_checked_change,
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn SwitchThumb(props: SwitchThumbProps) -> Element {
    rsx! {
        switch::SwitchThumb {
            class: "block w-4 h-4 rounded-full
        bg-switch-thumb translate-x-[1px] transition-transform duration-150 [will-change:transform] group-data-[state=checked]:translate-x-5",
            attributes: props.attributes,
            {props.children}
        }
    }
}
