//! The queue as it was when the app closed (previous tracks, the current track and the upcoming
//! tracks). It is saved in `queue.json`, so that it comes back the next time the session is
//! restored. See «Saved queue» in `docs/queue.md`.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::model::Track;

const QUEUE_FILE: &str = "queue.json";
/// The version of the file's format. The app ignores a file with a different version.
const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQueue {
    format: u32,
    /// Oldest first.
    pub previous: Vec<Track>,
    /// The track that played last, also if Spotify does not show it now.
    pub current: Option<Track>,
    pub next: Vec<Track>,
    /// Melodia decided the order of `next` (`queue_owned`). If false, it was Spotify's queue,
    /// which the app reads again.
    pub owned: bool,
}

impl SavedQueue {
    /// `None` when there is nothing to save.
    pub fn new(
        previous: &[Track],
        current: Option<&Track>,
        next: &[Track],
        owned: bool,
    ) -> Option<Self> {
        if previous.is_empty() && current.is_none() && next.is_empty() {
            return None;
        }
        Some(Self {
            format: FORMAT,
            previous: previous.to_vec(),
            current: current.cloned(),
            next: next.to_vec(),
            owned,
        })
    }
}

/// The queue saved in `config_dir`, if there is one that this version can read.
pub fn read(config_dir: &Path) -> Option<SavedQueue> {
    let bytes = std::fs::read(config_dir.join(QUEUE_FILE)).ok()?;
    match serde_json::from_slice::<SavedQueue>(&bytes) {
        Ok(saved) if saved.format == FORMAT => Some(saved),
        Ok(_) => None,
        Err(error) => {
            log::warn!("Could not read the saved queue: {error}");
            None
        }
    }
}

/// Saves `queue` in `config_dir`, or deletes the file with `None`.
pub fn write(config_dir: &Path, queue: Option<&SavedQueue>) -> io::Result<()> {
    let path = config_dir.join(QUEUE_FILE);
    match queue {
        Some(queue) => {
            let json = serde_json::to_vec(queue).map_err(io::Error::other)?;
            config::write_private_file(&path, &json)
        }
        None => match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, name: &str) -> Track {
        Track {
            uri: format!("spotify:track:{id}"),
            name: name.into(),
            artists: vec!["Queen".into()],
            album: "Greatest Hits".into(),
            duration_ms: 240_000,
            cover_small: None,
            cover_large: None,
            explicit: false,
            album_id: None,
            artist_ids: vec![],
        }
    }

    #[test]
    fn the_queue_is_read_back_as_it_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()), None);

        let queue = SavedQueue::new(
            &[track("a", "Under Pressure")],
            Some(&track("b", "Bohemian Rhapsody")),
            &[track("c", "Radio Ga Ga"), track("d", "Innuendo")],
            true,
        );
        write(dir.path(), queue.as_ref()).unwrap();
        assert_eq!(read(dir.path()), queue);

        write(dir.path(), None).unwrap();
        assert_eq!(read(dir.path()), None);
        write(dir.path(), None).unwrap();
    }

    #[test]
    fn an_empty_queue_is_not_saved() {
        assert_eq!(SavedQueue::new(&[], None, &[], true), None);
    }

    #[test]
    fn a_file_of_another_format_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut queue =
            SavedQueue::new(&[], Some(&track("a", "Under Pressure")), &[], false).unwrap();
        queue.format = FORMAT + 1;
        write(dir.path(), Some(&queue)).unwrap();
        assert_eq!(read(dir.path()), None);

        std::fs::write(dir.path().join(QUEUE_FILE), "not json").unwrap();
        assert_eq!(read(dir.path()), None);
    }
}
