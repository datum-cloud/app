//! The views module contains the components for all Layouts and Routes for our app. Each layout and route in our [`Route`]
//! enum will render one of these components.
//!
//! The [`Navbar`] component will be rendered on all pages of our app since every page is under the layout. The layout defines
//! a common wrapper around all child routes.

mod create_proxy;
mod edit_proxy;
mod join_proxy;
mod login;
mod navbar;
mod proxies_list;
mod select_project;
mod signup;
mod tunnel_bandwidth;

pub use create_proxy::CreateProxy;
pub use edit_proxy::EditProxy;
pub use join_proxy::JoinProxy;
pub use login::Login;
pub use navbar::*;
pub use proxies_list::{ProxiesList, TunnelCard};
pub use select_project::SelectProject;
pub use signup::Signup;
pub use tunnel_bandwidth::TunnelBandwidth;
