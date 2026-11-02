//! Melodia: a desktop player for Spotify on Windows.
//!
//! The crate has layers, so that tests can check all code except the drawing code without a
//! window or a network connection (the code map is in `docs/code-map.md`).

pub mod auth;
pub mod backend;
pub mod config;
pub mod icon;
pub mod model;
pub mod playback;
pub mod spotify;
pub mod ui;
pub mod update;
pub mod util;
pub mod webplayer;
