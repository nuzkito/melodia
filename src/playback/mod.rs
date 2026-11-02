//! Pure playback logic shared by the Spotify and the demo backends.

mod history;
mod queue;
mod reconcile;

pub use history::{MAX_HISTORY, PlayHistory};
pub use queue::{ENQUEUE_LEAD_MS, Follow, SpotifyPlan, apply_follow, follow};
pub use reconcile::{
    EXPECTATION_TTL, Expectation, Expected, PlayerAction, Prediction, QueueChange,
    RESTART_THRESHOLD_MS, predict,
};
