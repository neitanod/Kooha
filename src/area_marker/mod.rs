//! The marker: a layer per monitor where the area to record is dragged.

// The window and the recording use the marker, wired in a later commit.
#![allow(dead_code)]

pub mod geometry;
mod monitor;
#[cfg(feature = "layer-shell")]
mod view;
