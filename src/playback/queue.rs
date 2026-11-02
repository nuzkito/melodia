//! Melodia's own queue on top of Spotify's queue: what Spotify plays next by itself
//! ([`SpotifyPlan`]) and where it went at each change of track ([`follow`]). The reasons and the
//! full flow are in `docs/queue.md`.

use std::time::Instant;

use super::PlayHistory;
use crate::model::Track;

/// Melodia's next track goes into Spotify's queue this long before the current track ends. Thus
/// Spotify has time to load it without a gap. After that, it is fixed
/// (`QueueSnapshot::next_locked`).
pub const ENQUEUE_LEAD_MS: u64 = 3_000;

/// What Spotify plays after the current track if nobody changes it: the track that Melodia put in
/// Spotify's queue, if there is one, then the remaining tracks of the list that Spotify plays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpotifyPlan {
    /// Melodia added it to Spotify's queue, and it did not start yet.
    queued: Option<String>,
    rest: Vec<String>,
    /// `rest` is all that Spotify has (Melodia sent the list). When the plan comes from a read of
    /// Spotify's queue, more tracks can come after it.
    complete: bool,
}

impl SpotifyPlan {
    /// Spotify got `uris` (the first one plays now). `queued` is still in Spotify's own queue, so
    /// it plays immediately after it.
    pub fn sent(uris: &[String], queued: Option<String>) -> Self {
        Self {
            queued,
            rest: uris.iter().skip(1).cloned().collect(),
            complete: true,
        }
    }

    /// The app opened again with its own queue. What Spotify plays after the current track is not
    /// known. Thus the plan tells that nothing comes and does not match the queue, and the next
    /// resume sends the full list.
    pub fn restored() -> Self {
        Self {
            complete: true,
            ..Self::default()
        }
    }

    /// Spotify's upcoming tracks as read from its queue.
    pub fn observed(next: &[Track]) -> Self {
        Self {
            queued: None,
            rest: next.iter().map(|track| track.uri.clone()).collect(),
            complete: false,
        }
    }

    pub fn next(&self) -> Option<&str> {
        self.queued
            .as_deref()
            .or(self.rest.first().map(String::as_str))
    }

    pub fn queued(&self) -> Option<&str> {
        self.queued.as_deref()
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn mark_queued(&mut self, uri: String) {
        self.queued = Some(uri);
    }

    /// Spotify started `uri`. Removes the tracks that it played or skipped before it, and returns
    /// true if `uri` was part of the plan.
    pub fn started(&mut self, uri: &str) -> bool {
        if self.queued.as_deref() == Some(uri) {
            self.queued = None;
            return true;
        }
        match self.rest.iter().position(|u| u == uri) {
            Some(index) => {
                // Spotify's own queue goes first: the queued track played (or Spotify skipped it).
                self.queued = None;
                self.rest.drain(..=index);
                true
            }
            None => false,
        }
    }

    /// Spotify went back one track. `uri`, which played, comes next again in its list.
    pub fn went_back(&mut self, uri: String) {
        self.rest.insert(0, uri);
    }

    /// True if Spotify plays exactly `upcoming` and then stops.
    pub fn matches(&self, upcoming: &[Track]) -> bool {
        let plan = self.queued.iter().chain(&self.rest);
        self.complete
            && plan.clone().count() == upcoming.len()
            && plan.zip(upcoming).all(|(uri, track)| *uri == track.uri)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// `upcoming[index]` plays now. Usually it is the first track. A different index means that
    /// Spotify played a track out of Melodia's order ("next" on a different device before Melodia
    /// queued its own track). Only that track leaves the queue.
    Upcoming(usize),
    /// A track that is still in Spotify's list but not in the queue (it already played out of
    /// order). It plays now, and the queue stays.
    Leftover,
    /// Spotify went back to the last track of the history.
    Back,
    /// Spotify plays a different track (a different device started an album, autoplay…). The UI
    /// shows Spotify's queue again.
    Lost,
}

/// Where Spotify went, now that it plays `uri`. Updates `plan`.
pub fn follow(
    history: &PlayHistory,
    upcoming: &[Track],
    plan: &mut SpotifyPlan,
    uri: &str,
) -> Follow {
    let planned = plan.started(uri);
    if let Some(index) = upcoming.iter().position(|track| track.uri == uri) {
        Follow::Upcoming(index)
    } else if history.last().is_some_and(|track| track.uri == uri) {
        if let Some(current) = history.current() {
            plan.went_back(current.uri.clone());
        }
        Follow::Back
    } else if planned {
        Follow::Leftover
    } else {
        Follow::Lost
    }
}

/// Moves the history and the upcoming tracks as `follow` tells.
pub fn apply_follow(
    follow: Follow,
    history: &mut PlayHistory,
    upcoming: &mut Vec<Track>,
    now_playing: &Track,
    now: Instant,
) {
    match follow {
        Follow::Upcoming(index) => {
            upcoming.remove(index);
            history.advance_to(&[], now_playing);
        }
        Follow::Leftover => history.advance_to(&[], now_playing),
        Follow::Back => {
            let old = history.current().cloned();
            history.hint_back(now);
            history.observe(Some(now_playing), now);
            if let Some(old) = old {
                upcoming.insert(0, old);
            }
        }
        Follow::Lost => {
            history.observe(Some(now_playing), now);
        }
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

    fn tracks(ids: &str) -> Vec<Track> {
        ids.chars().map(|id| track(&id.to_string())).collect()
    }

    fn uris(ids: &str) -> Vec<String> {
        tracks(ids).into_iter().map(|t| t.uri).collect()
    }

    fn names(tracks: &[Track]) -> String {
        tracks.iter().map(|t| t.name.to_lowercase()).collect()
    }

    /// "z" played, "a" plays now. The app sent a, b, c, d to Spotify.
    fn setup(now: Instant) -> (PlayHistory, Vec<Track>, SpotifyPlan) {
        let mut history = PlayHistory::new();
        for id in ["z", "a"] {
            history.observe(Some(&track(id)), now);
        }
        (
            history,
            tracks("bcd"),
            SpotifyPlan::sent(&uris("abcd"), None),
        )
    }

    /// Spotify starts `id`: the queue follows it.
    fn spotify_plays(
        id: &str,
        history: &mut PlayHistory,
        upcoming: &mut Vec<Track>,
        plan: &mut SpotifyPlan,
        now: Instant,
    ) -> Follow {
        let step = follow(history, upcoming, plan, &track(id).uri);
        apply_follow(step, history, upcoming, &track(id), now);
        step
    }

    #[test]
    fn the_plan_knows_what_spotify_plays_next() {
        let now = Instant::now();
        let (_, upcoming, mut plan) = setup(now);
        assert!(plan.matches(&upcoming));
        assert_eq!(plan.next(), Some(track("b").uri.as_str()));

        plan.mark_queued(track("d").uri);
        assert_eq!(plan.next(), Some(track("d").uri.as_str()));
        assert!(!plan.matches(&upcoming));
        assert!(plan.started(&track("d").uri));
        assert_eq!(
            plan.next(),
            Some(track("b").uri.as_str()),
            "the list goes on"
        );

        // Read from Spotify's queue: more tracks can follow, so it never "matches".
        let observed = SpotifyPlan::observed(&upcoming);
        assert!(!observed.is_complete() && !observed.matches(&upcoming));

        // After a restart, nothing is known: the resume sends the list.
        let restored = SpotifyPlan::restored();
        assert!(restored.is_complete() && restored.queued().is_none());
        assert!(!restored.matches(&upcoming));
        assert!(restored.matches(&[]), "an empty queue just ends");
    }

    #[test]
    fn a_reordered_queue_is_followed_without_resending_it() {
        let now = Instant::now();
        let (mut history, _, mut plan) = setup(now);
        // The user moves "d" first. Shortly before "a" ends, Melodia adds it to Spotify's queue,
        // and Spotify plays it next.
        let mut upcoming = tracks("dbc");
        assert_ne!(plan.next(), Some(upcoming[0].uri.as_str()));
        plan.mark_queued(upcoming[0].uri.clone());

        let plays = |id: &str, h: &mut PlayHistory, u: &mut Vec<Track>, p: &mut SpotifyPlan| {
            spotify_plays(id, h, u, p, now)
        };
        assert_eq!(
            plays("d", &mut history, &mut upcoming, &mut plan),
            Follow::Upcoming(0)
        );
        assert_eq!(names(&upcoming), "bc");
        // Spotify's list continues with b and c: they are already in the app's order.
        assert_eq!(plan.next(), Some(track("b").uri.as_str()));
        assert_eq!(
            plays("b", &mut history, &mut upcoming, &mut plan),
            Follow::Upcoming(0)
        );
        assert_eq!(
            plays("c", &mut history, &mut upcoming, &mut plan),
            Follow::Upcoming(0)
        );
        // "d" is still in Spotify's list: it would play again.
        assert_eq!(plan.next(), Some(track("d").uri.as_str()));
        assert_eq!(
            plays("d", &mut history, &mut upcoming, &mut plan),
            Follow::Leftover
        );
        assert!(upcoming.is_empty());
    }

    #[test]
    fn a_track_played_out_of_order_leaves_only_itself() {
        let now = Instant::now();
        let (mut history, _, mut plan) = setup(now);
        // The app's order is c, b, d. "next" on the phone plays Spotify's b.
        let mut upcoming = tracks("cbd");
        let step = spotify_plays("b", &mut history, &mut upcoming, &mut plan, now);
        assert_eq!(step, Follow::Upcoming(1));
        assert_eq!(names(&upcoming), "cd");
        assert_eq!(names(history.entries()), "za");
    }

    #[test]
    fn going_back_and_unknown_music() {
        let now = Instant::now();
        let (mut history, mut upcoming, mut plan) = setup(now);
        let step = spotify_plays("z", &mut history, &mut upcoming, &mut plan, now);
        assert_eq!(step, Follow::Back);
        assert_eq!(names(&upcoming), "abcd");
        assert_eq!(plan.next(), Some(track("a").uri.as_str()));

        let other = follow(&history, &upcoming, &mut plan, &track("new").uri);
        assert_eq!(other, Follow::Lost);
    }
}
