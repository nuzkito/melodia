//! The egui user interface.
//!
//! The painter draws all items on top of a few egui widgets (text fields, scroll areas). This
//! gives full control of the look and the animations. See [`anim`] for the animation helpers.

mod account_menu;
mod album;
pub mod anim;
mod app;
mod artist;
mod backdrop;
mod changelog;
mod context_menu;
mod detail;
mod devices;
mod dnd;
mod guide;
mod icons;
mod idle_report;
mod liked;
mod login;
mod navigation;
mod player_controls;
mod playlist_adds;
mod playlist_tracks;
mod playlists;
mod queue;
mod queue_keys;
mod search;
mod spotify_logo;
mod textures;
pub mod theme;
mod toasts;
mod track_row;
mod widgets;
mod window_frame;

pub use app::MusicApp;
pub use idle_report::IdleReport;
pub use search::SEARCH_FIELD_ID;
