//! The backend owns all I/O (network, disk, decoding) on a Tokio runtime in a background thread.
//! The UI sends [`Command`]s and receives [`Event`]s, so the interface never blocks.

pub mod demo;
mod demo_covers;
mod images;
mod live;
mod playlist_library;
mod playlist_sync;
mod protocol;
mod response_cache;
mod saved_queue;
mod timings;
mod updates;

pub use images::{accent_color, decode_image, hsl_to_rgb, rgb_to_hsl};
pub use protocol::{
    AuthStatus, BackendHandle, Command, CommandReceiver, DecodedImage, Event, EventSink, QueueAt,
    Toast, ToastKind, WebPlayerStatus, channel,
};
pub use timings::Timings;

use crate::config::AppConfig;
use crate::model::QueueSnapshot;

/// Starts the backend (real or demo, as the configuration tells) in its own thread, with the check
/// for a new version beside it. The backend calls `wake` after each event to repaint the UI.
pub fn spawn(config: AppConfig, wake: impl Fn() + Send + Sync + 'static) -> BackendHandle {
    let (mut handle, sink, commands) = channel(wake);
    let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
    handle.on_stopped(stopped_rx);
    std::thread::Builder::new()
        .name("backend".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("backend-worker")
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(async move {
                    // It stops with the runtime, when the backend ends.
                    tokio::spawn(updates::check(
                        config.update_url.clone(),
                        sink.clone(),
                        updates::CHECK_DELAY,
                    ));
                    if config.demo {
                        demo::run(commands, sink, config.timings).await;
                    } else {
                        live::run(config, commands, sink).await;
                    }
                }),
                Err(error) => log::error!("Could not start the async runtime: {error}"),
            }
            let _ = stopped_tx.send(());
        })
        .expect("failed to spawn the backend thread");
    handle
}

/// The reason why the app cannot show the tracks of the playlist of a different user.
pub const PLAYLIST_NOT_READABLE: &str = "From here, Spotify only shows the songs of your own playlists and the ones you collaborate on.";

// The two backends use the same notices below.

/// The error when only `added` of the `total` tracks of a playlist fit in the queue.
fn playlist_partly_queued(name: &str, added: usize, total: usize) -> String {
    format!(
        "Only {added} of the {total} songs in “{name}” fit: the queue holds {}.",
        QueueSnapshot::MAX_NEXT
    )
}

/// The error when the queue cannot accept one more track.
fn queue_full() -> String {
    format!(
        "The queue is full: it holds {} songs.",
        QueueSnapshot::MAX_NEXT
    )
}

/// The error when the playlist or album with the name `name`, for the queue, has no tracks.
fn nothing_to_queue(name: &str) -> String {
    format!("“{name}” has no songs to play")
}

/// The error when Spotify does not let the app use the playlist with the name `name` (the playlist
/// of a different user).
fn playlist_not_usable(name: &str) -> String {
    format!(
        "Spotify doesn't let you use “{name}” from here: only your own playlists and the ones you collaborate on."
    )
}

/// Runs the live backend on the current Tokio runtime (the tests use it).
pub async fn run_live(config: AppConfig, commands: CommandReceiver, sink: EventSink) {
    live::run(config, commands, sink).await;
}
