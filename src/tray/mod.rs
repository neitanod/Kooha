pub mod model;
mod service;

pub use self::{
    model::{TrayCommand, TrayModel},
    service::{TrayRequest, TrayService},
};
