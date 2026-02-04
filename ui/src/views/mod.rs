//! The views module contains the components for all Layouts and Routes for our app. Each layout and route in our [`Route`]
//! enum will render one of these components.
//!
//! The [`Navbar`] component will be rendered on all pages of our app since every page is under the layout. The layout defines
//! a common wrapper around all child routes.

mod join_proxy;
mod login;
mod navbar;
mod proxies_list;
mod select_project;
mod settings;
mod signup;
mod tunnel_bandwidth;

pub use join_proxy::JoinProxy;
pub use login::Login;
pub use navbar::*;
pub use proxies_list::{ProxiesList, TunnelCard};
pub use select_project::SelectProject;
pub use settings::Settings;
pub use signup::Signup;
pub use tunnel_bandwidth::TunnelBandwidth;
