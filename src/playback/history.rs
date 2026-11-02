//! The tracks that played before the current track.
//!
//! The Web API gives the upcoming queue, but not the tracks that played before the current track
//! in this session. Thus the history comes from the changes of the current track. It continues
//! from its state when the app last closed (see «Saved queue» in `docs/queue.md`). The app
//! does not use Spotify's recently played tracks.

use std::time::{Duration, Instant};

use crate::model::Track;

pub const MAX_HISTORY: usize = 50;

/// The time during which a "previous" command applies when the track changes.
const BACK_HINT_TTL: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Default)]
pub struct PlayHistory {
    /// Oldest first, at most [`MAX_HISTORY`].
    entries: Vec<Track>,
    current: Option<Track>,
    back_hint_until: Option<Instant>,
}

impl PlayHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// The history as saved (oldest first, with only the latest [`MAX_HISTORY`]), with `current`
    /// as the track that plays.
    pub fn restored(mut entries: Vec<Track>, current: Option<Track>) -> Self {
        keep_latest(&mut entries);
        Self {
            entries,
            current,
            back_hint_until: None,
        }
    }

    /// The tracks that played before, oldest first.
    pub fn entries(&self) -> &[Track] {
        &self.entries
    }

    pub fn current(&self) -> Option<&Track> {
        self.current.as_ref()
    }

    /// The most recent track that played before the current track.
    pub fn last(&self) -> Option<&Track> {
        self.entries.last()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Tells the history that the user pushed "previous". Thus, when the music goes back to the
    /// last track, the history removes it and does not add a new entry.
    pub fn hint_back(&mut self, now: Instant) {
        self.back_hint_until = Some(now + BACK_HINT_TTL);
    }

    /// Records the track that plays now. Returns `true` if it changed.
    ///
    /// The history ignores `None` (nothing plays, for example Spotify closed), so that it stays
    /// during a pause in playback.
    pub fn observe(&mut self, track: Option<&Track>, now: Instant) -> bool {
        let Some(track) = track else {
            return false;
        };
        if let Some(current) = &mut self.current
            && current.uri == track.uri
        {
            // The same item: update its metadata (for example covers), nothing more.
            *current = track.clone();
            return false;
        }

        let hinted = self.back_hint_until.is_some_and(|until| now <= until);
        let went_back = hinted && self.last().is_some_and(|last| last.uri == track.uri);
        if went_back {
            self.entries.pop();
            self.back_hint_until = None;
        } else if let Some(previous) = self.current.take() {
            self.push(previous);
        }
        self.current = Some(track.clone());
        true
    }

    /// Moves forward to `track`. The current track and `skipped` (the skipped tracks, in order)
    /// become the latest entries.
    pub fn advance_to(&mut self, skipped: &[Track], track: &Track) {
        let same = self.current.as_ref().is_some_and(|c| c.uri == track.uri);
        if !(same && skipped.is_empty()) {
            if let Some(current) = self.current.take() {
                self.push(current);
            }
            for track in skipped {
                self.push(track.clone());
            }
        }
        self.current = Some(track.clone());
        self.back_hint_until = None;
    }

    /// The queue ended. The current track becomes the latest entry, and there is no current track.
    pub fn finish(&mut self) {
        if let Some(current) = self.current.take() {
            self.push(current);
        }
        self.back_hint_until = None;
    }

    /// Goes back to `entries[index]`, which becomes the current track. Returns the entries after it
    /// and the old current track, in play order: they come next again. `None` if that entry does
    /// not exist.
    pub fn rewind_to(&mut self, index: usize) -> Option<Vec<Track>> {
        if index >= self.entries.len() {
            return None;
        }
        let mut after = self.entries.split_off(index);
        let target = after.remove(0);
        after.extend(self.current.replace(target));
        self.back_hint_until = None;
        Some(after)
    }

    /// Removes the latest entry if it is `uri` (a skipped track that will play after all), and
    /// returns it.
    pub fn forget_last(&mut self, uri: &str) -> Option<Track> {
        if self.last().is_some_and(|last| last.uri == uri) {
            self.entries.pop()
        } else {
            None
        }
    }

    /// Replaces the entries (the user moved tracks in or out of them). It keeps only the latest
    /// [`MAX_HISTORY`].
    pub fn set_entries(&mut self, mut entries: Vec<Track>) {
        keep_latest(&mut entries);
        self.entries = entries;
        self.back_hint_until = None;
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn push(&mut self, track: Track) {
        if self.last().is_some_and(|last| last.uri == track.uri) {
            return;
        }
        self.entries.push(track);
        keep_latest(&mut self.entries);
    }
}

/// Removes the oldest `entries` after [`MAX_HISTORY`].
fn keep_latest(entries: &mut Vec<Track>) {
    let excess = entries.len().saturating_sub(MAX_HISTORY);
    entries.drain(..excess);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            uri: format!("spotify:track:{id}"),
            name: id.to_uppercase(),
            artists: vec!["Artist".into()],
            album: "Album".into(),
            duration_ms: 180_000,
            cover_small: None,
            cover_large: None,
            explicit: false,
            album_id: None,
            artist_ids: vec![],
        }
    }

    fn names(history: &PlayHistory) -> Vec<String> {
        history.entries().iter().map(|t| t.name.clone()).collect()
    }

    #[test]
    fn previous_track_is_recorded_when_the_track_changes() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        assert!(history.observe(Some(&track("a")), now));
        assert!(history.observe(Some(&track("b")), now));
        assert!(history.observe(Some(&track("c")), now));
        assert_eq!(names(&history), ["A", "B"]);
        assert_eq!(history.current().unwrap().name, "C");
    }

    #[test]
    fn same_track_only_updates_metadata() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        history.observe(Some(&track("a")), now);
        let mut updated = track("a");
        updated.cover_large = Some("cover".into());
        assert!(!history.observe(Some(&updated), now));
        assert!(history.is_empty());
        assert_eq!(
            history.current().unwrap().cover_large.as_deref(),
            Some("cover")
        );
    }

    #[test]
    fn going_back_pops_the_last_entry() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        for id in ["a", "b", "c"] {
            history.observe(Some(&track(id)), now);
        }
        history.hint_back(now);
        assert!(history.observe(Some(&track("b")), now));
        assert_eq!(names(&history), ["A"]);
        assert_eq!(history.current().unwrap().name, "B");
    }

    #[test]
    fn replaying_without_hint_is_a_new_entry() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        for id in ["a", "b", "a"] {
            history.observe(Some(&track(id)), now);
        }
        assert_eq!(names(&history), ["A", "B"]);
    }

    #[test]
    fn expired_hint_is_ignored() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        for id in ["a", "b"] {
            history.observe(Some(&track(id)), now);
        }
        history.hint_back(now);
        history.observe(Some(&track("a")), now + Duration::from_secs(30));
        assert_eq!(names(&history), ["A", "B"]);
    }

    #[test]
    fn nothing_playing_keeps_history() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        history.observe(Some(&track("a")), now);
        history.observe(Some(&track("b")), now);
        assert!(!history.observe(None, now));
        assert_eq!(names(&history), ["A"]);
        assert!(!history.observe(Some(&track("b")), now));
    }

    #[test]
    fn advancing_records_the_skipped_tracks() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        history.observe(Some(&track("a")), now);
        history.advance_to(&[track("b"), track("c")], &track("d"));
        assert_eq!(names(&history), ["A", "B", "C"]);
        assert_eq!(history.current().unwrap().name, "D");

        // When the current track plays again, it is not a new entry.
        history.advance_to(&[], &track("d"));
        assert_eq!(names(&history), ["A", "B", "C"]);
    }

    #[test]
    fn rewinding_returns_what_comes_next_again() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        for id in ["a", "b", "c", "d"] {
            history.observe(Some(&track(id)), now);
        }
        let next = history.rewind_to(1).unwrap();
        assert_eq!(names(&history), ["A"]);
        assert_eq!(history.current().unwrap().name, "B");
        let next: Vec<_> = next.into_iter().map(|t| t.name).collect();
        assert_eq!(next, ["C", "D"]);
        assert!(history.rewind_to(5).is_none());
    }

    #[test]
    fn entries_can_be_replaced() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        for id in ["a", "b", "c"] {
            history.observe(Some(&track(id)), now);
        }
        history.set_entries(vec![track("x"), track("a")]);
        assert_eq!(names(&history), ["X", "A"]);
        assert_eq!(history.current().unwrap().name, "C");

        let many = (0..MAX_HISTORY + 5)
            .map(|i| track(&i.to_string()))
            .collect();
        history.set_entries(many);
        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.entries()[0].name, "5", "the oldest go");
    }

    #[test]
    fn a_restored_history_goes_on_from_where_it_was() {
        let now = Instant::now();
        let mut history = PlayHistory::restored(vec![track("a"), track("b")], Some(track("c")));
        assert_eq!(names(&history), ["A", "B"]);
        assert!(!history.observe(Some(&track("c")), now), "the same track");
        assert!(history.observe(Some(&track("d")), now));
        assert_eq!(names(&history), ["A", "B", "C"]);

        let many = (0..MAX_HISTORY + 5)
            .map(|i| track(&i.to_string()))
            .collect();
        let history = PlayHistory::restored(many, None);
        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.entries()[0].name, "5", "the oldest go");
    }

    #[test]
    fn history_is_capped() {
        let now = Instant::now();
        let mut history = PlayHistory::new();
        for i in 0..(MAX_HISTORY + 10) {
            history.observe(Some(&track(&i.to_string())), now);
        }
        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.entries()[0].name, "9");
    }
}
