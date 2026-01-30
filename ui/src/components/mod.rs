//! The components module contains all shared components for our app. Components are the building blocks of dioxus apps.
//! They can be used to defined common UI elements like buttons, forms, and modals. In this template, we define a Hero
//! component  to be used in our app.



mod add_tunnel_dialog;
mod bandwidth_timeseries_chart;
mod button;
mod head;
mod icon;
mod splash;
mod typography;

pub use add_tunnel_dialog::AddTunnelDialog;
pub use button::Button;
pub use button::ButtonKind;
pub use head::Head;
pub use icon::{Icon, IconKind, IconSource};
pub use splash::Splash;
#[allow(unused)]
pub use typography::Subhead;
pub mod dialog;
pub mod input;
pub mod switch;
pub use switch::{Switch, SwitchThumb};
pub mod dropdown_menu;
pub mod select;
