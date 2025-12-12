//! The views module contains the components for all Layouts and Routes for our app. Each layout and route in our [`Route`]
//! enum will render one of these components.
//!
//!
//! The [`Home`] and [`Blog`] components will be rendered when the current route is [`Route::Home`] or [`Route::Blog`] respectively.
//!
//!
//! The [`Navbar`] component will be rendered on all pages of our app since every page is under the layout. The layout defines
//! a common wrapper around all child routes.

mod create_proxy;
mod join_proxy;
mod login;
mod navbar;
mod proxies_list;
mod signup;

pub use create_proxy::CreateProxy;
pub use join_proxy::JoinProxy;
pub use login::Login;
pub use navbar::Navbar;
pub use proxies_list::TempProxies;
pub use signup::Signup;
