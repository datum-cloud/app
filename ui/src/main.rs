// The dioxus prelude contains a ton of common items used in dioxus apps. It's a good idea to import wherever you
// need dioxus
use dioxus::prelude::*;

use state::AppState;
use views::{
    CreateDomain, CreateProxy, DomainsList, JoinProxy, Login, Navbar, Signup, TempProxies,
};

use crate::components::{Head, Splash};

/// Define a components module that contains all shared components for our app.
mod components;
/// Define a views module that contains the UI for all Layouts and Routes for our app.
mod views;
// App-wide state
mod state;

/// The Route enum is used to define the structure of internal routes in our app. All route enums need to derive
/// the [`Routable`] trait, which provides the necessary methods for the router to work.
///
/// Each variant represents a different URL pattern that can be matched by the router. If that pattern is matched,
/// the components for that route will be rendered.
#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[route("/login")]
    Login{},
    #[route("/signup")]
    Signup{},
    // The layout attribute defines a wrapper for all routes under the layout. Layouts are great for wrapping
    // many routes with a common UI like a navbar.
    #[layout(Navbar)]
        #[route("/")]
        TempProxies{},
        // The route attribute can include dynamic parameters that implement [`std::str::FromStr`] and [`std::fmt::Display`] with the `:` syntax.
        // In this case, id will match any integer like `/blog/123` or `/blog/-456`.
        #[route("/proxy/create")]
        CreateProxy {},
        #[route("/proxy/join")]
        JoinProxy {},
        // The route attribute defines the URL pattern that a specific route matches. If that pattern matches the URL,
        // the component for that route will be rendered. The component name that is rendered defaults to the variant name.
        #[route("/domains")]
        DomainsList {},
        #[route("/domain/create")]
        CreateDomain {},
}

fn main() {
    dotenv::dotenv().ok();
    // The `launch` function is the main entry point for a dioxus app. It takes a component and renders it with the platform feature
    // you have enabled
    dioxus::launch(App);
}

/// App is the main component of our app. Components are the building blocks of dioxus apps. Each component is a function
/// that takes some props and returns an Element. In this case, App takes no props because it is the root of our app.
///
/// Components should be annotated with `#[component]` to support props, better error messages, and autocomplete
#[component]
fn App() -> Element {
    let mut app_state_ready = use_signal(|| false);
    use_future(move || async move {
        let state = AppState::load().await.unwrap();
        provide_context(state);
        app_state_ready.set(true);
    });

    if !app_state_ready() {
        return rsx! {
            Head {  }
            Splash {}
        };
    }

    // The `rsx!` macro lets us define HTML inside of rust. It expands to an Element with all of our HTML inside.
    rsx! {
        Head {  }

        // // The router component renders the route enum we defined above. It will handle synchronization of the URL and render
        // // the layouts and components for the active route.
        Router::<Route> {}
    }
}
