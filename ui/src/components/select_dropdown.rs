use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub struct SelectItem {
    pub id: String,
    pub label: String,
    pub subtitle: Option<String>,
}

#[derive(Props, Clone, PartialEq)]
pub struct SelectDropdownProps {
    label: String,
    placeholder: String,
    items: Vec<SelectItem>,
    selected: Option<String>,
    on_select: EventHandler<String>,
    #[props(default = false)]
    disabled: bool,
    #[props(default = true)]
    searchable: bool,
    #[props(default = "Search…".to_string())]
    search_placeholder: String,
    #[props(default = true)]
    show_label: bool,
    #[props(default = true)]
    show_selected_subtitle: bool,
    #[props(default = false)]
    dense: bool,
    #[props(default = None)]
    expanded_min_width: Option<String>,
    #[props(default = false)]
    stacked: bool,
    #[props(default = false)]
    stack_list_items: bool,
    #[props(default = false)]
    align_right: bool,
    #[props(default = None)]
    dense_height: Option<String>,
}

#[component]
pub fn SelectDropdown(props: SelectDropdownProps) -> Element {
    let mut open = use_signal(|| false);
    let mut query = use_signal(String::new);
    let selected_item = props
        .selected
        .as_ref()
        .and_then(|id| props.items.iter().find(|item| &item.id == id));
    let selected_title = selected_item.map(|item| {
        if props.show_selected_subtitle {
            match &item.subtitle {
                Some(subtitle) => format!("{} ({})", item.label, subtitle),
                None => item.label.clone(),
            }
        } else {
            item.label.clone()
        }
    });

    let filtered: Vec<SelectItem> = if !props.searchable || query.read().is_empty() {
        props.items.clone()
    } else {
        let q = query.read().to_lowercase();
        props
            .items
            .iter()
            .filter(|item| {
                item.label.to_lowercase().contains(&q)
                    || item
                        .subtitle
                        .as_ref()
                        .map(|s| s.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .cloned()
            .collect()
    };

    let button_class = if props.disabled {
        if props.dense {
            "w-full h-9 rounded-xl border border-[#dfe3ea] bg-white px-3 text-left text-sm text-slate-900 shadow-sm opacity-50 cursor-not-allowed"
        } else {
            "w-full h-12 rounded-xl border border-[#dfe3ea] bg-white px-4 text-left text-sm text-slate-900 shadow-sm opacity-50 cursor-not-allowed"
        }
    } else if props.dense {
        "w-full h-9 rounded-xl border border-[#dfe3ea] bg-white px-3 text-left text-sm text-slate-900 shadow-sm hover:bg-gray-50"
    } else {
        "w-full h-12 rounded-xl border border-[#dfe3ea] bg-white px-4 text-left text-sm text-slate-900 shadow-sm hover:bg-gray-50"
    };
    let button_style = if props.dense {
        props
            .dense_height
            .as_ref()
            .map(|height| format!("height: {height};"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    // Cap the list to a consistent height with viewport padding at the bottom.
    let list_max_height = "max-height: min(200px, calc(100vh - 320px));";
    let is_selected = |item: &SelectItem| {
        props
            .selected
            .as_ref()
            .map(|id| id == &item.id)
            .unwrap_or(false)
    };
    let list_items: Vec<Element> = {
        let mut open = open.clone();
        let mut query = query.clone();
        let on_select = props.on_select.clone();
        filtered
            .iter()
            .cloned()
            .map(|item| {
                let selected = is_selected(&item);
                let item_id = item.id.clone();
                rsx! {
                    button {
                        class: if selected {
                            "w-full text-left px-4 py-3 text-sm border-l-4 border-[#4a6fa1] cursor-pointer"
                        } else {
                            "w-full text-left px-4 py-3 text-sm text-slate-900 hover:bg-slate-50 cursor-pointer"
                        },
                        style: if selected {
                            "background-color: #edf3ff; color: #0f172a;"
                        } else {
                            ""
                        },
                        onclick: move |_| {
                            open.set(false);
                            query.set(String::new());
                            on_select.call(item_id.clone());
                        },
                        if props.stack_list_items {
                            div { class: "flex flex-col leading-tight",
                                span {
                                    class: if selected {
                                        "text-sm text-slate-900 font-semibold truncate whitespace-nowrap"
                                    } else {
                                        "text-sm text-slate-900 truncate whitespace-nowrap"
                                    },
                                    title: "{item.label}",
                                    "{item.label}"
                                }
                                if let Some(subtitle) = &item.subtitle {
                                    span {
                                        class: if selected {
                                            "text-xs text-slate-600 truncate whitespace-nowrap"
                                        } else {
                                            "text-xs text-slate-500 truncate whitespace-nowrap"
                                        },
                                        title: "{subtitle}",
                                        "{subtitle}"
                                    }
                                }
                            }
                        } else {
                            div { class: "flex items-center gap-3 min-w-0",
                                span {
                                    class: if selected {
                                        "text-sm text-slate-900 font-semibold truncate flex-1 min-w-0 whitespace-nowrap"
                                    } else {
                                        "text-sm text-slate-900 truncate flex-1 min-w-0 whitespace-nowrap"
                                    },
                                    title: "{item.label}",
                                    "{item.label}"
                                }
                                if let Some(subtitle) = &item.subtitle {
                                    span {
                                        class: if selected {
                                            "text-xs text-slate-600 truncate ml-auto whitespace-nowrap"
                                        } else {
                                            "text-xs text-slate-500 truncate ml-auto whitespace-nowrap"
                                        },
                                        title: "{subtitle}",
                                        "{subtitle}"
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .collect()
    };

    let expanded_style = props
        .expanded_min_width
        .as_ref()
        .map(|width| format!("min-width: {width};"))
        .unwrap_or_default();

    rsx! {
        div { class: "space-y-2",
            if props.show_label {
                h2 { class: "text-sm font-semibold text-slate-700", "{props.label}" }
            }
            div { class: "relative",
                if open() {
                    div {
                        class: "fixed inset-0 z-40",
                        onclick: move |_| open.set(false),
                    }
                    div {
                        class: if props.dense {
                            if props.align_right {
                                "absolute right-0 top-0 z-50 w-full rounded-xl border border-[#dfe3ea] bg-white shadow-[0_14px_34px_rgba(17,24,39,0.12)] overflow-hidden"
                            } else {
                                "absolute inset-x-0 top-0 z-50 w-full rounded-xl border border-[#dfe3ea] bg-white shadow-[0_14px_34px_rgba(17,24,39,0.12)] overflow-hidden"
                            }
                        } else if props.align_right {
                            "absolute right-0 top-0 z-50 w-full rounded-xl border border-[#dfe3ea] bg-white shadow-[0_14px_34px_rgba(17,24,39,0.12)] overflow-hidden"
                        } else {
                            "absolute inset-x-0 top-0 z-50 w-full rounded-xl border border-[#dfe3ea] bg-white shadow-[0_14px_34px_rgba(17,24,39,0.12)] overflow-hidden"
                        },
                        style: "{expanded_style}",
                        if props.searchable {
                            button {
                                class: "w-full h-12 rounded-none border-0 bg-white px-4 text-left text-sm text-slate-500 focus:outline-none focus:ring-2 focus:ring-slate-400/30 cursor-text",
                                onclick: move |_| {},
                                input {
                                    class: "w-full h-12 bg-transparent text-sm text-slate-800 focus:outline-none cursor-text",
                                    placeholder: "{props.search_placeholder}",
                                    value: "{query()}",
                                    oninput: move |evt| query.set(evt.value()),
                                }
                            }
                        }
                        div { class: "overflow-y-auto",
                            style: "{list_max_height}",
                            if filtered.is_empty() {
                                div { class: "px-4 py-3 text-sm text-slate-500", "No results" }
                            } else {
                                for item in list_items {
                                    {item}
                                }
                            }
                        }
                    }
                }
                button {
                    class: if open() {
                        format!("{button_class} invisible")
                    } else {
                        format!("{button_class} cursor-pointer")
                    },
                    style: "{button_style}",
                    title: "{selected_title.clone().unwrap_or_default()}",
                    disabled: props.disabled,
                    onclick: move |_| {
                        if props.disabled {
                            return;
                        }
                        open.set(true);
                        query.set(String::new());
                    },
                    if let Some(item) = selected_item {
                        if props.show_selected_subtitle {
                            div { class: "flex h-full items-center gap-3 min-w-0",
                                span { class: "text-sm text-slate-900 truncate flex-1 min-w-0 whitespace-nowrap", title: "{item.label}", "{item.label}" }
                                if let Some(subtitle) = &item.subtitle {
                                    span { class: "text-xs text-slate-500 truncate ml-auto max-w-[45%] whitespace-nowrap", title: "{subtitle}", "{subtitle}" }
                                }
                            }
                        } else if props.stacked {
                            div { class: "flex h-full flex-col justify-center leading-tight min-w-0",
                                span { class: "text-sm text-slate-900 truncate min-w-0 whitespace-nowrap", title: "{item.label}", "{item.label}" }
                                if let Some(subtitle) = &item.subtitle {
                                    span { class: "text-xs text-slate-500 truncate min-w-0 whitespace-nowrap", title: "{subtitle}", "{subtitle}" }
                                }
                            }
                        } else {
                            div { class: "flex h-full items-center gap-2 min-w-0 w-full overflow-hidden",
                                span { class: "text-sm text-slate-900 truncate flex-1 min-w-0 whitespace-nowrap overflow-hidden", title: "{item.label}", "{item.label}" }
                            }
                        }
                    } else {
                        span { class: "text-sm text-slate-500", "{props.placeholder}" }
                    }
                }
            }
        }
    }
}
