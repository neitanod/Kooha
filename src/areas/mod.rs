//! Areas marked on a monitor, and the saved list of them.

// The window, the recording and the tray use these once they are wired in.
#![allow(dead_code)]

pub mod file;
pub mod model;
pub mod monitors;
pub mod store;

#[allow(unused_imports)]
pub use self::{
    file::Selected,
    model::*,
    store::{AreaStore, NameTaken},
};
