//! Optimistic updates for player commands.
//!
//! The backend predicts the result of each command and shows it immediately. Until Spotify (or
//! the player page) reports the expected state, or a short time limit passes, the backend ignores
//! older states. Thus the UI does not flicker back.

use std::time::{Duration, Instant};

use super::PlayHistory;
use crate::model::{PlaybackSnapshot, QueuePosition, Track};

/// The default time during which answers that do not show the result of a command yet count as
/// old (the backend gets it from its `Timings`).
pub const EXPECTATION_TTL: Duration = Duration::from_millis(2500);

/// Spotify's "previous" restarts the track if it played longer than this.
pub const RESTART_THRESHOLD_MS: u64 = 3000;

/// A reported position this near to the target position means that the seek is complete.
const SEEK_TOLERANCE_MS: u64 = 3000;

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerAction {
    Resume,
    Pause,
    Next,
    Previous,
    Seek(u64),
    /// Play `tracks[index]` now. The upcoming queue stays. When it is empty, the other items of
    /// `tracks` follow.
    Play {
        tracks: Vec<Track>,
        index: usize,
    },
    /// Play a track of the queue. A jump forward puts the tracks between them in the history. A
    /// jump back puts the tracks after it in the queue again.
    Jump(QueuePosition),
}

/// The history and upcoming tracks after an action that changes the track.
#[derive(Debug, Clone)]
pub struct QueueChange {
    pub history: PlayHistory,
    pub next: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct Prediction {
    pub playback: PlaybackSnapshot,
    /// Only for actions that change the current track.
    pub queue: Option<QueueChange>,
}

/// Predicts the state immediately after `action`, or `None` if it cannot be known.
pub fn predict(
    state: &PlaybackSnapshot,
    history: &PlayHistory,
    upcoming: &[Track],
    action: &PlayerAction,
    now: Instant,
) -> Option<Prediction> {
    let position = state.progress_at(now);
    let mut playback = state.clone();
    playback.measured_at = now;

    let queue = match action {
        PlayerAction::Resume | PlayerAction::Pause => {
            state.track.as_ref()?;
            playback.is_playing = *action == PlayerAction::Resume;
            playback.progress_ms = position;
            None
        }
        PlayerAction::Seek(target) => {
            let duration = state.track.as_ref()?.duration_ms;
            playback.progress_ms = (*target).min(duration);
            None
        }
        PlayerAction::Next => {
            let next = upcoming.first()?;
            Some(change_to(
                &mut playback,
                history,
                next,
                &[],
                upcoming[1..].to_vec(),
            ))
        }
        PlayerAction::Previous => {
            let current = state.track.clone()?;
            playback.progress_ms = 0;
            playback.is_playing = true;
            match history.last() {
                Some(previous) if position <= RESTART_THRESHOLD_MS => {
                    let mut history = history.clone();
                    history.hint_back(now);
                    history.observe(Some(previous), now);
                    playback.track = Some(previous.clone());
                    let mut next = vec![current];
                    next.extend_from_slice(upcoming);
                    Some(QueueChange { history, next })
                }
                // Spotify restarts the current track.
                _ => None,
            }
        }
        PlayerAction::Play { tracks, index } => {
            let track = tracks.get(*index)?;
            let next = if upcoming.is_empty() {
                tracks[index + 1..].to_vec()
            } else {
                upcoming.to_vec()
            };
            Some(change_to(&mut playback, history, track, &[], next))
        }
        PlayerAction::Jump(QueuePosition::Next(index)) => {
            let track = upcoming.get(*index)?;
            Some(change_to(
                &mut playback,
                history,
                track,
                &upcoming[..*index],
                upcoming[index + 1..].to_vec(),
            ))
        }
        PlayerAction::Jump(QueuePosition::Previous(index)) => {
            let mut history = history.clone();
            let mut next = history.rewind_to(*index)?;
            next.extend_from_slice(upcoming);
            playback.track = history.current().cloned();
            playback.progress_ms = 0;
            playback.is_playing = true;
            Some(QueueChange { history, next })
        }
    };
    Some(Prediction { playback, queue })
}

/// `track` starts from the beginning. `skipped` go to the history with the current track.
fn change_to(
    playback: &mut PlaybackSnapshot,
    history: &PlayHistory,
    track: &Track,
    skipped: &[Track],
    next: Vec<Track>,
) -> QueueChange {
    let mut history = history.clone();
    history.advance_to(skipped, track);
    playback.track = Some(track.clone());
    playback.progress_ms = 0;
    playback.is_playing = true;
    QueueChange { history, next }
}

/// What the next reported states must show before they replace a prediction.
#[derive(Debug, Clone, PartialEq)]
pub enum Expected {
    Playing(bool),
    TrackChange { from: Option<String> },
    Track(String),
    RestartOrChange { from: Option<String> },
    Position(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expectation {
    pub expected: Expected,
    pub deadline: Instant,
}

impl Expectation {
    /// The expected result of `action`. Answers that do not show it count as old for `ttl`.
    pub fn for_action(
        action: &PlayerAction,
        state: &PlaybackSnapshot,
        now: Instant,
        ttl: Duration,
    ) -> Self {
        let from = state.track_uri().map(str::to_owned);
        let expected = match action {
            PlayerAction::Resume => Expected::Playing(true),
            PlayerAction::Pause => Expected::Playing(false),
            PlayerAction::Next | PlayerAction::Jump(_) => Expected::TrackChange { from },
            PlayerAction::Previous => Expected::RestartOrChange { from },
            PlayerAction::Seek(position) => Expected::Position(*position),
            PlayerAction::Play { tracks, index } => match tracks.get(*index) {
                Some(track) => Expected::Track(track.uri.clone()),
                None => Expected::TrackChange { from },
            },
        };
        Self {
            expected,
            deadline: now + ttl,
        }
    }

    pub fn is_met_by(&self, state: &PlaybackSnapshot) -> bool {
        let uri = state.track_uri();
        match &self.expected {
            Expected::Playing(playing) => state.is_playing == *playing,
            Expected::TrackChange { from } => uri != from.as_deref(),
            Expected::Track(expected) => uri == Some(expected.as_str()),
            Expected::RestartOrChange { from } => {
                uri != from.as_deref() || state.progress_ms < RESTART_THRESHOLD_MS
            }
            Expected::Position(position) => {
                state.progress_ms.abs_diff(*position) < SEEK_TOLERANCE_MS
            }
        }
    }

    /// True if a reported state should replace the prediction.
    pub fn accepts(&self, state: &PlaybackSnapshot, now: Instant) -> bool {
        now >= self.deadline || self.is_met_by(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            uri: format!("spotify:track:{id}"),
            name: id.to_uppercase(),
            artists: vec![],
            album: String::new(),
            duration_ms: 200_000,
            cover_small: None,
            cover_large: None,
            explicit: false,
            album_id: None,
            artist_ids: vec![],
        }
    }

    fn playing(id: &str, progress_ms: u64, now: Instant) -> PlaybackSnapshot {
        PlaybackSnapshot {
            track: Some(track(id)),
            is_playing: true,
            progress_ms,
            measured_at: now,
            device: None,
            shuffle: false,
        }
    }

    fn history_with(ids: &[&str], now: Instant) -> PlayHistory {
        let mut history = PlayHistory::new();
        for id in ids {
            history.observe(Some(&track(id)), now);
        }
        history
    }

    fn names(tracks: &[Track]) -> Vec<&str> {
        tracks.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn pause_freezes_the_extrapolated_position() {
        let now = Instant::now();
        let state = playing("a", 10_000, now);
        let later = now + Duration::from_secs(5);
        let prediction = predict(
            &state,
            &PlayHistory::new(),
            &[],
            &PlayerAction::Pause,
            later,
        )
        .unwrap();
        assert!(!prediction.playback.is_playing);
        assert_eq!(prediction.playback.progress_ms, 15_000);
        assert!(prediction.queue.is_none());
    }

    #[test]
    fn next_moves_the_queue_forward() {
        let now = Instant::now();
        let history = history_with(&["z", "a"], now);
        let state = playing("a", 50_000, now);
        let upcoming = [track("b"), track("c")];
        let prediction = predict(&state, &history, &upcoming, &PlayerAction::Next, now).unwrap();

        assert_eq!(prediction.playback.track_uri(), Some("spotify:track:b"));
        assert_eq!(prediction.playback.progress_ms, 0);
        let queue = prediction.queue.unwrap();
        assert_eq!(names(queue.history.entries()), ["Z", "A"]);
        assert_eq!(queue.next, vec![track("c")]);
    }

    #[test]
    fn next_without_known_queue_is_not_predicted() {
        let now = Instant::now();
        let state = playing("a", 0, now);
        assert!(predict(&state, &PlayHistory::new(), &[], &PlayerAction::Next, now).is_none());
    }

    #[test]
    fn previous_goes_back_early_in_the_track() {
        let now = Instant::now();
        let history = history_with(&["z", "a"], now);
        let state = playing("a", 1_000, now);
        let upcoming = [track("b")];
        let prediction =
            predict(&state, &history, &upcoming, &PlayerAction::Previous, now).unwrap();

        assert_eq!(prediction.playback.track_uri(), Some("spotify:track:z"));
        let queue = prediction.queue.unwrap();
        assert!(queue.history.is_empty());
        assert_eq!(queue.next, vec![track("a"), track("b")]);
    }

    #[test]
    fn previous_restarts_late_in_the_track() {
        let now = Instant::now();
        let history = history_with(&["z", "a"], now);
        let state = playing("a", 60_000, now);
        let prediction = predict(&state, &history, &[], &PlayerAction::Previous, now).unwrap();
        assert_eq!(prediction.playback.track_uri(), Some("spotify:track:a"));
        assert_eq!(prediction.playback.progress_ms, 0);
        assert!(prediction.queue.is_none());
    }

    #[test]
    fn play_uses_the_rest_of_the_list_as_queue() {
        let now = Instant::now();
        let state = PlaybackSnapshot::idle();
        let tracks = vec![track("a"), track("b"), track("c")];
        let action = PlayerAction::Play { tracks, index: 1 };
        let prediction = predict(&state, &PlayHistory::new(), &[], &action, now).unwrap();
        assert_eq!(prediction.playback.track_uri(), Some("spotify:track:b"));
        assert_eq!(prediction.queue.unwrap().next, vec![track("c")]);
    }

    #[test]
    fn play_keeps_a_queue_with_tracks() {
        let now = Instant::now();
        let history = history_with(&["a"], now);
        let state = playing("a", 10_000, now);
        let upcoming = [track("q")];
        let action = PlayerAction::Play {
            tracks: vec![track("x"), track("y")],
            index: 0,
        };
        let prediction = predict(&state, &history, &upcoming, &action, now).unwrap();
        assert_eq!(prediction.playback.track_uri(), Some("spotify:track:x"));
        let queue = prediction.queue.unwrap();
        assert_eq!(names(queue.history.entries()), ["A"]);
        assert_eq!(names(&queue.next), ["Q"]);
    }

    #[test]
    fn jumping_ahead_leaves_the_skipped_tracks_behind() {
        let now = Instant::now();
        let history = history_with(&["z", "a"], now);
        let state = playing("a", 10_000, now);
        let upcoming = [track("b"), track("c"), track("d")];
        let action = PlayerAction::Jump(QueuePosition::Next(1));
        let prediction = predict(&state, &history, &upcoming, &action, now).unwrap();
        assert_eq!(prediction.playback.track_uri(), Some("spotify:track:c"));
        assert_eq!(prediction.playback.progress_ms, 0);
        let queue = prediction.queue.unwrap();
        assert_eq!(names(queue.history.entries()), ["Z", "A", "B"]);
        assert_eq!(names(&queue.next), ["D"]);
    }

    #[test]
    fn jumping_back_queues_the_tracks_after_it_again() {
        let now = Instant::now();
        let history = history_with(&["x", "y", "z", "a"], now);
        let state = playing("a", 10_000, now);
        let upcoming = [track("b")];
        let action = PlayerAction::Jump(QueuePosition::Previous(1));
        let prediction = predict(&state, &history, &upcoming, &action, now).unwrap();
        assert_eq!(prediction.playback.track_uri(), Some("spotify:track:y"));
        let queue = prediction.queue.unwrap();
        assert_eq!(names(queue.history.entries()), ["X"]);
        assert_eq!(names(&queue.next), ["Z", "A", "B"]);

        let missing = PlayerAction::Jump(QueuePosition::Previous(9));
        assert!(predict(&state, &history, &upcoming, &missing, now).is_none());
    }

    #[test]
    fn seek_is_clamped_to_the_duration() {
        let now = Instant::now();
        let state = playing("a", 0, now);
        let prediction = predict(
            &state,
            &PlayHistory::new(),
            &[],
            &PlayerAction::Seek(999_999),
            now,
        )
        .unwrap();
        assert_eq!(prediction.playback.progress_ms, 200_000);
    }

    #[test]
    fn expectations_ignore_stale_polls_until_met_or_expired() {
        let now = Instant::now();
        let state = playing("a", 30_000, now);
        let expectation =
            Expectation::for_action(&PlayerAction::Next, &state, now, EXPECTATION_TTL);

        let stale = playing("a", 31_000, now);
        assert!(!expectation.accepts(&stale, now + Duration::from_millis(500)));
        assert!(expectation.accepts(&stale, now + EXPECTATION_TTL));
        assert!(expectation.accepts(&playing("b", 0, now), now));
    }

    #[test]
    fn expectations_for_each_action() {
        let now = Instant::now();
        let state = playing("a", 30_000, now);
        let mut paused = state.clone();
        paused.is_playing = false;

        let pause = Expectation::for_action(&PlayerAction::Pause, &state, now, EXPECTATION_TTL);
        assert!(pause.is_met_by(&paused) && !pause.is_met_by(&state));

        let previous =
            Expectation::for_action(&PlayerAction::Previous, &state, now, EXPECTATION_TTL);
        assert!(previous.is_met_by(&playing("a", 500, now)));
        assert!(!previous.is_met_by(&playing("a", 31_000, now)));

        let seek =
            Expectation::for_action(&PlayerAction::Seek(90_000), &state, now, EXPECTATION_TTL);
        assert!(seek.is_met_by(&playing("a", 91_000, now)));
        assert!(!seek.is_met_by(&state));

        let play = PlayerAction::Play {
            tracks: vec![track("x")],
            index: 0,
        };
        let play = Expectation::for_action(&play, &state, now, EXPECTATION_TTL);
        assert!(play.is_met_by(&playing("x", 0, now)) && !play.is_met_by(&state));
    }
}
