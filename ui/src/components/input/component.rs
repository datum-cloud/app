use dioxus::prelude::*;

use crate::components::icon::{Icon, IconSource};

#[component]
pub fn Input(
    /// Optional label text shown above the input. Use `id` to associate the label with the input for accessibility.
    #[props(default = None)]
    label: Option<String>,
    /// Optional helper or description text shown below the input.
    #[props(default = None)]
    description: Option<String>,
    /// Optional error message. When set, the input shows error styling and the message below.
    #[props(default = None)]
    error: Option<String>,
    /// Optional id for the input element. When set, the label's `for` attribute is set so clicking the label focuses the input.
    #[props(default = None)]
    id: Option<String>,
    /// Autocomplete attribute (e.g. "off", "nope"). Set to "nope" to disable browser autocomplete when "off" is ignored.
    #[props(default = None)]
    autocomplete: Option<String>,
    /// Autocapitalize attribute (e.g. "off", "none"). Disables mobile keyboard auto-capitalization.
    #[props(default = None)]
    autocapitalize: Option<String>,
    /// Optional icon shown at the start of the input (e.g. search).
    #[props(default = None)]
    leading_icon: Option<IconSource>,
    oninput: Option<EventHandler<FormEvent>>,
    onchange: Option<EventHandler<FormEvent>>,
    oninvalid: Option<EventHandler<FormEvent>>,
    onselect: Option<EventHandler<SelectionEvent>>,
    onselectionchange: Option<EventHandler<SelectionEvent>>,
    onfocus: Option<EventHandler<FocusEvent>>,
    onblur: Option<EventHandler<FocusEvent>>,
    onfocusin: Option<EventHandler<FocusEvent>>,
    onfocusout: Option<EventHandler<FocusEvent>>,
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    onkeypress: Option<EventHandler<KeyboardEvent>>,
    onkeyup: Option<EventHandler<KeyboardEvent>>,
    oncompositionstart: Option<EventHandler<CompositionEvent>>,
    oncompositionupdate: Option<EventHandler<CompositionEvent>>,
    oncompositionend: Option<EventHandler<CompositionEvent>>,
    oncopy: Option<EventHandler<ClipboardEvent>>,
    oncut: Option<EventHandler<ClipboardEvent>>,
    onpaste: Option<EventHandler<ClipboardEvent>>,
    #[props(extends=GlobalAttributes)]
    #[props(extends=input)]
    attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    let has_error = error.is_some();
    let border_class = if has_error {
        "border-alert-red-dark focus:ring-alert-red-dark focus:ring-2"
    } else {
        "border-app-border focus:ring-app-border focus:ring-2"
    };
    let input_class = match &leading_icon {
        None => format!("w-full rounded-lg border bg-white px-2 h-9 text-foreground placeholder:text-form-description focus:outline-none focus:ring-1 {border_class} text-xs placeholder:text-xs"),
        Some(_) => format!("flex-1 min-w-0 border-0 bg-transparent py-0 px-2 h-9 text-foreground placeholder:text-form-description focus:outline-none focus:ring-0 text-xs placeholder:text-xs rounded-none"),
    };
    let wrapper_class = if leading_icon.is_some() && has_error {
        "flex items-center rounded-lg border border-red-500 bg-white h-9 focus-within:ring-1 focus-within:ring-red-500"
    } else if leading_icon.is_some() {
        "flex items-center rounded-lg border border-app-border bg-white h-9 focus-within:ring-1 focus-within:ring-app-border"
    } else {
        ""
    };

    rsx! {
        div { class: "flex flex-col gap-2",
            if let Some(ref label_text) = label {
                label {
                    r#for: id.as_deref().unwrap_or(""),
                    class: "text-xs text-form-label/80",
                    {label_text.clone()}
                }
            }
            if let Some(ref icon_source) = leading_icon {
                div { class: "{wrapper_class}",
                    div { class: "pl-2.5 text-form-description shrink-0 flex items-center",
                        Icon { source: icon_source.clone(), size: 14 }
                    }
                    input {
                        id: id.as_deref(),
                        class: "{input_class}",
                        autocomplete: autocomplete.as_deref().unwrap_or(""),
                        autocapitalize: autocapitalize.as_deref().unwrap_or(""),
                        oninput: move |e| _ = oninput.map(|callback| callback(e)),
                        onchange: move |e| _ = onchange.map(|callback| callback(e)),
                        oninvalid: move |e| _ = oninvalid.map(|callback| callback(e)),
                        onselect: move |e| _ = onselect.map(|callback| callback(e)),
                        onselectionchange: move |e| _ = onselectionchange.map(|callback| callback(e)),
                        onfocus: move |e| _ = onfocus.map(|callback| callback(e)),
                        onblur: move |e| _ = onblur.map(|callback| callback(e)),
                        onfocusin: move |e| _ = onfocusin.map(|callback| callback(e)),
                        onfocusout: move |e| _ = onfocusout.map(|callback| callback(e)),
                        onkeydown: move |e| _ = onkeydown.map(|callback| callback(e)),
                        onkeypress: move |e| _ = onkeypress.map(|callback| callback(e)),
                        onkeyup: move |e| _ = onkeyup.map(|callback| callback(e)),
                        oncompositionstart: move |e| _ = oncompositionstart.map(|callback| callback(e)),
                        oncompositionupdate: move |e| _ = oncompositionupdate.map(|callback| callback(e)),
                        oncompositionend: move |e| _ = oncompositionend.map(|callback| callback(e)),
                        oncopy: move |e| _ = oncopy.map(|callback| callback(e)),
                        oncut: move |e| _ = oncut.map(|callback| callback(e)),
                        onpaste: move |e| _ = onpaste.map(|callback| callback(e)),
                        ..attributes,
                        {children}
                    }
                }
            } else {
                input {
                    id: id.as_deref(),
                    class: "{input_class}",
                    oninput: move |e| _ = oninput.map(|callback| callback(e)),
                    onchange: move |e| _ = onchange.map(|callback| callback(e)),
                    oninvalid: move |e| _ = oninvalid.map(|callback| callback(e)),
                    onselect: move |e| _ = onselect.map(|callback| callback(e)),
                    onselectionchange: move |e| _ = onselectionchange.map(|callback| callback(e)),
                    onfocus: move |e| _ = onfocus.map(|callback| callback(e)),
                    onblur: move |e| _ = onblur.map(|callback| callback(e)),
                    onfocusin: move |e| _ = onfocusin.map(|callback| callback(e)),
                    onfocusout: move |e| _ = onfocusout.map(|callback| callback(e)),
                    onkeydown: move |e| _ = onkeydown.map(|callback| callback(e)),
                    onkeypress: move |e| _ = onkeypress.map(|callback| callback(e)),
                    onkeyup: move |e| _ = onkeyup.map(|callback| callback(e)),
                    oncompositionstart: move |e| _ = oncompositionstart.map(|callback| callback(e)),
                    oncompositionupdate: move |e| _ = oncompositionupdate.map(|callback| callback(e)),
                    oncompositionend: move |e| _ = oncompositionend.map(|callback| callback(e)),
                    oncopy: move |e| _ = oncopy.map(|callback| callback(e)),
                    oncut: move |e| _ = oncut.map(|callback| callback(e)),
                    onpaste: move |e| _ = onpaste.map(|callback| callback(e)),
                    ..attributes,
                    {children}
                }
            }
            if let Some(ref desc) = description {
                div { class: "text-1xs text-form-description", {desc.clone()} }
            }
            if let Some(ref err) = error {
                div { class: "text-1xs text-alert-red-dark", {err.clone()} }
            }
        }
    }
}
