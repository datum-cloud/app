use dioxus::prelude::*;

#[component]
pub fn JoinProxy() -> Element {
    rsx! {
        div {
            "unimplemented"
        }
    }
    // let mut local_address = use_signal(|| "127.0.0.1:9000".to_string());
    // let mut label = use_signal(|| "".to_string());
    // let mut ticket_str = use_signal(|| "".to_string());
    // // let mut validation_error = use_signal(|| "".to_string());

    // rsx! {
    //     div {
    //         id: "create-domain",
    //         class: "flex flex-col",
    //         h1 { "join proxy" },
    //         // p {
    //         //     class: "text-red-500",
    //         //     "{validation_error}"
    //         // }
    //         Subhead { text: "Local Address" }
    //         input {
    //             class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
    //             value: "{local_address}",
    //             onchange: move |e| local_address.set(e.value()),
    //         }
    //         Subhead { text: "Label" }
    //         input {
    //             class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
    //             placeholder: "Label",
    //             value: "{label}",
    //             onchange: move |e| label.set(e.value()),
    //         }
    //         Subhead { text: "Ticket" }
    //         textarea {
    //             class: "border border-gray-300 rounded-md px-3 py-2 my-1 mr-4",
    //             value: "{ticket_str}",
    //             onchange: move |e| ticket_str.set(e.value()),
    //         },
    //         button {
    //             class: "cursor-pointer",
    //             onclick: move |_| async move {
    //                 let state = consume_context::<AppState>();
    //                 // let ticket = match TcpProxyTicket::from_str(&ticket_str()) {
    //                 //     Ok(ticket) => ticket,
    //                 //     Err(err) => {
    //                 //         validation_error.set(format!("Invalid ticket: {}", err));
    //                 //         return;
    //                 //     }
    //                 // };
    //                 state.clone().node().outbound.connect(label()).await.unwrap();
    //             },
    //             "Join"
    //         }

    //     }
    // }
}
