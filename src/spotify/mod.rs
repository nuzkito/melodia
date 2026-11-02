//! Spotify Web API client.

mod client;
mod error;
pub mod models;
mod rate_limit;

pub use client::{LIBRARY_BATCH, PlaylistEntries, SEARCH_PAGE_SIZE, SpotifyClient};
pub use error::ApiError;
pub use models::{PlayOffset, PlayRequest};
