use dioxus::prelude::*;
use dioxus_primitives::dialog::{
    self, DialogContentProps, DialogDescriptionProps, DialogRootProps, DialogTitleProps,
};

#[component]
pub fn DialogRoot(props: DialogRootProps) -> Element {
    rsx! {
        dialog::DialogRoot {
            class: "bg-foreground/30 absolute top-0 left-0 w-full h-full inset-0 z-50 flex items-center justify-center animate-in fade-in duration-100",
            id: props.id,
            is_modal: props.is_modal,
            open: props.open,
            default_open: props.default_open,
            on_open_change: props.on_open_change,
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn DialogContent(props: DialogContentProps) -> Element {
    rsx! {
        dialog::DialogContent {
            class: "bg-white rounded-md p-6.5 py-7 shadow-dialog animate-in fade-in duration-300",
            id: props.id,
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn DialogTitle(props: DialogTitleProps) -> Element {
    rsx! {
        dialog::DialogTitle {
            class: "text-md font-medium text-foreground",
            id: props.id,
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
pub fn DialogDescription(props: DialogDescriptionProps) -> Element {
    rsx! {
        dialog::DialogDescription {
            class: "text-sm text-foreground/80",
            id: props.id,
            attributes: props.attributes,
            {props.children}
        }
    }
}
