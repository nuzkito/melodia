//! Domain types shared by the backend and the UI.
//!
//! These types do not depend on the Spotify JSON format (see [`crate::spotify::models`]). Thus
//! the UI never uses optional API fields.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::util::plural;

mod library;

pub use library::{Folder, Library, LibraryEdit, LibraryItem, LibraryKey};

/// A song. The app does not support podcast episodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    /// Spotify URI (`spotify:track:…`, or `spotify:local:…` for local files). It is the identity.
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    /// The Spotify ID of each of `artists` (same order), to open them. It is `None` (or missing)
    /// when it is not known, for example for local files.
    pub artist_ids: Vec<Option<String>>,
    pub album: String,
    /// The Spotify ID of the album, to open it. It is `None` for local files.
    pub album_id: Option<String>,
    pub duration_ms: u64,
    /// Small cover, for list thumbnails.
    pub cover_small: Option<String>,
    /// The largest available cover, for the "now playing" view.
    pub cover_large: Option<String>,
    /// Spotify marks the track as explicit content. An «E» badge shows next to its title. The
    /// field is missing in caches from before it existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub explicit: bool,
}

impl Track {
    pub fn artist_line(&self) -> String {
        if self.artists.is_empty() {
            "Unknown artist".to_owned()
        } else {
            self.artists.join(", ")
        }
    }

    /// The Spotify ID of `artists[index]`, if it is known.
    pub fn artist_id(&self, index: usize) -> Option<&str> {
        self.artist_ids
            .get(index)?
            .as_deref()
            .filter(|id| !id.is_empty())
    }

    /// "Artist · Album", as the lists show it under the title.
    pub fn subtitle(&self) -> String {
        if self.album.is_empty() {
            self.artist_line()
        } else {
            format!("{} · {}", self.artist_line(), self.album)
        }
    }
}

/// A Spotify Connect device (desktop app, phone, speaker…).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Spotify device type: "Computer", "Smartphone", "Speaker"…
    pub kind: String,
    pub is_active: bool,
    /// Volume from 0 to 100, when the device reports it.
    pub volume_percent: Option<u8>,
    /// True when the app can change its volume. Some phones refuse.
    pub supports_volume: bool,
}

/// What plays now, as the backend last reported (or predicted) it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSnapshot {
    pub track: Option<Track>,
    pub is_playing: bool,
    pub progress_ms: u64,
    /// The time of the `progress_ms` measurement. The UI uses it to calculate the position.
    pub measured_at: Instant,
    pub device: Option<Device>,
    pub shuffle: bool,
}

impl PlaybackSnapshot {
    pub fn idle() -> Self {
        Self {
            track: None,
            is_playing: false,
            progress_ms: 0,
            measured_at: Instant::now(),
            device: None,
            shuffle: false,
        }
    }

    /// The playback position at `now`. While the track plays, it adds the time since `at`.
    pub fn progress_at(&self, now: Instant) -> u64 {
        let mut progress = self.progress_ms;
        if self.is_playing {
            let elapsed = now.saturating_duration_since(self.measured_at);
            progress = progress.saturating_add(elapsed.as_millis() as u64);
        }
        match &self.track {
            Some(track) if track.duration_ms > 0 => progress.min(track.duration_ms),
            _ => progress,
        }
    }

    pub fn track_uri(&self) -> Option<&str> {
        self.track.as_ref().map(|t| t.uri.as_str())
    }
}

/// The play queue around the current track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueueSnapshot {
    /// The tracks that played before, oldest first.
    pub previous: Vec<Track>,
    pub current: Option<Track>,
    pub next: Vec<Track>,
    /// The number of [`QueueEdit`]s that the backend applied. The UI applies its edits
    /// immediately and ignores older snapshots. Thus a dropped track does not jump back to its
    /// old position.
    pub edits: u64,
    /// The app already sent `next[0]` to Spotify (shortly before the current track ends). The user
    /// cannot move it or put a track before it.
    pub next_locked: bool,
}

impl QueueSnapshot {
    pub const MAX_NEXT: usize = 100;

    /// The number of upcoming tracks at the front that cannot move (0 or 1).
    pub fn fixed(&self) -> usize {
        usize::from(self.next_locked && !self.next.is_empty())
    }
}

/// A change to the queue that the user makes (drag and drop, or the remove button).
#[derive(Debug, Clone, PartialEq)]
pub enum QueueEdit {
    /// Moves a track of the full queue (`previous`, then the current track, then `next`). The
    /// track at `from` is `uri`, and it goes to `to`. A track that moves before the current one
    /// becomes a previous track, and the opposite. When the current track moves, it continues to
    /// play, and the tracks that it passes change sides.
    Move { from: usize, to: usize, uri: String },
    /// Inserts `track` at `next[index]`.
    Insert { index: usize, track: Track },
    /// Inserts `tracks`, in order, from `next[index]` (a full playlist). It inserts only the
    /// tracks that fit in the queue.
    InsertMany { index: usize, tracks: Vec<Track> },
    /// Removes the track at `next[index]`, which is `uri`, from the queue.
    Remove { index: usize, uri: String },
    /// Removes the track at `previous[index]`, which is `uri`, from the previous tracks.
    RemovePrevious { index: usize, uri: String },
}

impl QueueEdit {
    /// Applies the edit to the queue and returns true if the queue changed. The first `fixed`
    /// upcoming tracks stay immediately after the current track, which also does not move. No
    /// edit moves them or puts a track before them. The queue can move while the user drags, so
    /// the function finds the moved track by its URI near `from`.
    pub fn apply(
        &self,
        previous: &mut Vec<Track>,
        current: Option<&Track>,
        next: &mut Vec<Track>,
        fixed: usize,
    ) -> bool {
        match self {
            Self::Move { from, to, uri } => {
                move_in_queue(previous, current, next, fixed, *from, *to, uri)
            }
            Self::Insert { index, track } => {
                if next.len() >= QueueSnapshot::MAX_NEXT {
                    return false;
                }
                next.insert(
                    (*index).clamp(fixed.min(next.len()), next.len()),
                    track.clone(),
                );
                true
            }
            Self::InsertMany { index, tracks } => {
                let room = QueueSnapshot::MAX_NEXT.saturating_sub(next.len());
                let at = (*index).clamp(fixed.min(next.len()), next.len());
                let count = tracks.len().min(room);
                next.splice(at..at, tracks[..count].iter().cloned());
                count > 0
            }
            Self::Remove { index, uri } => {
                let Some(index) = find_near(next, uri, *index).filter(|index| *index >= fixed)
                else {
                    return false;
                };
                next.remove(index);
                true
            }
            Self::RemovePrevious { index, uri } => {
                let Some(index) = find_near(previous, uri, *index) else {
                    return false;
                };
                previous.remove(index);
                true
            }
        }
    }
}

/// See [`QueueEdit::Move`].
fn move_in_queue(
    previous: &mut Vec<Track>,
    current: Option<&Track>,
    next: &mut Vec<Track>,
    fixed: usize,
    from: usize,
    to: usize,
    uri: &str,
) -> bool {
    let fixed = if current.is_some() {
        fixed.min(next.len())
    } else {
        0
    };
    let previous_count = previous.len();
    let mut all: Vec<Track> = previous
        .iter()
        .chain(current)
        .chain(next.iter())
        .cloned()
        .collect();
    let Some(from) = find_near(&all, uri, from) else {
        return false;
    };
    if fixed > 0 && (previous_count..=previous_count + fixed).contains(&from) {
        return false;
    }
    let track = all.remove(from);
    let mut to = to.min(all.len());
    if fixed > 0 {
        // Never between the current track and the fixed tracks.
        let current_at = previous_count - usize::from(from < previous_count);
        if to > current_at && to <= current_at + fixed {
            to = current_at + fixed + 1;
        }
    }
    all.insert(to, track);
    let previous_count = previous_count_after_move(previous_count, current.is_some(), from, to);
    let after = previous_count + usize::from(current.is_some());
    let next_len = all.len() - after;
    if next_len > QueueSnapshot::MAX_NEXT && next_len > next.len() {
        return false;
    }
    if all[..previous_count] == previous[..] && all[after..] == next[..] {
        return false;
    }
    *next = all.split_off(after);
    all.truncate(previous_count);
    *previous = all;
    true
}

/// The number of previous tracks after the item at `from` of the full queue moves to `to` (as
/// `Vec::remove`, then `Vec::insert`). A track that the user drops immediately before the current
/// track is a previous track. When there is no current track, it is an upcoming track.
pub fn previous_count_after_move(
    previous_count: usize,
    has_current: bool,
    from: usize,
    to: usize,
) -> usize {
    if has_current && from == previous_count {
        // This is the current track: the tracks that it passed change sides.
        return to;
    }
    let remaining = previous_count - usize::from(from < previous_count);
    remaining + usize::from(to < remaining || (has_current && to == remaining))
}

/// A playlist of the user's library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    /// The display name of its owner.
    pub owner: String,
    /// The number of entries, as Spotify counts them (with episodes).
    pub total: u32,
    pub cover: Option<String>,
    /// The user owns the playlist or collaborates on it. Thus the user can add tracks, and Spotify
    /// lets the app read them.
    pub editable: bool,
    /// Its version (Spotify's `snapshot_id`) when the app read the list, if known. Each change to
    /// the playlist changes it.
    pub snapshot: Option<String>,
}

impl Playlist {
    /// "12 songs", as the sidebar shows it under the name.
    pub fn count_line(&self) -> String {
        plural(self.total as usize, "song", "songs")
    }
}

/// Puts `playlists`, in Spotify's order, in the user's own `order` (playlist IDs, as the user last
/// arranged them in the sidebar). Spotify cannot reorder a library, so the app keeps that order.
/// Playlists that are not in `order` (new playlists) keep their position relative to Spotify's
/// order. They go immediately after the playlist before them in Spotify, or at the top if there is
/// none (Spotify puts new playlists there). The result does not include IDs of deleted playlists.
/// An empty `order` keeps Spotify's order.
pub fn arrange_playlists(playlists: Vec<Playlist>, order: &[String]) -> Vec<Playlist> {
    if order.is_empty() {
        return playlists;
    }
    // The position of each ID in `order` (the first position, if the ID repeats).
    let mut ranks: HashMap<&str, usize> = HashMap::with_capacity(order.len());
    for (rank, id) in order.iter().enumerate() {
        ranks.entry(id).or_insert(rank);
    }
    // New playlists, grouped by the known playlist before them in Spotify's order (`None`: before
    // all known playlists).
    let mut known = Vec::new();
    let mut new: HashMap<Option<String>, Vec<Playlist>> = HashMap::new();
    let mut anchor = None;
    for playlist in playlists {
        match ranks.get(playlist.id.as_str()).copied() {
            Some(rank) => {
                anchor = Some(playlist.id.clone());
                known.push((rank, playlist));
            }
            None => new.entry(anchor.clone()).or_default().push(playlist),
        }
    }
    known.sort_by_key(|(rank, _)| *rank);

    let mut arranged = new.remove(&None).unwrap_or_default();
    for (_, playlist) in known {
        let followers = new.remove(&Some(playlist.id.clone()));
        arranged.push(playlist);
        arranged.extend(followers.into_iter().flatten());
    }
    arranged
}

/// A change to the tracks of a playlist that the user makes (drag and drop, the remove button).
/// Indices count the tracks that the playlist panel shows, as in
/// [`crate::backend::Event::PlaylistTracks`]. A playlist can have the same track more than one
/// time. The URI only makes sure that the index still points to that track.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaylistEdit {
    /// Moves the track at `from`, which is `uri`, to `to` (as `Vec::remove`, then
    /// `Vec::insert`).
    Move { from: usize, to: usize, uri: String },
    /// Inserts `track` at `index`, or at the end of the playlist with `None`.
    Insert { index: Option<usize>, track: Track },
    /// Removes the track at `index`, which is `uri`, from the playlist (only that copy).
    Remove { index: usize, uri: String },
    /// Adds `tracks`, in order, at the end of the playlist (an album that the user drops on it).
    Append { tracks: Vec<Track> },
}

impl PlaylistEdit {
    /// Applies the edit to the tracks of the playlist and returns true if they changed. The
    /// function finds the track to move or remove by its URI near its index. The list can reload
    /// during the edit.
    pub fn apply(&self, tracks: &mut Vec<Track>) -> bool {
        match self {
            Self::Move { from, to, uri } => {
                let Some(from) = find_near(tracks, uri, *from) else {
                    return false;
                };
                let to = (*to).min(tracks.len() - 1);
                if from == to {
                    return false;
                }
                let track = tracks.remove(from);
                tracks.insert(to, track);
                true
            }
            Self::Insert { index, track } => {
                let at = index.unwrap_or(tracks.len()).min(tracks.len());
                tracks.insert(at, track.clone());
                true
            }
            Self::Remove { index, uri } => match find_near(tracks, uri, *index) {
                Some(index) => {
                    tracks.remove(index);
                    true
                }
                None => false,
            },
            Self::Append { tracks: added } => {
                tracks.extend(added.iter().cloned());
                !added.is_empty()
            }
        }
    }

    /// The number of tracks after the edit, for a playlist that had `total` tracks.
    pub fn total_after(&self, total: u32) -> u32 {
        match self {
            Self::Move { .. } => total,
            Self::Insert { .. } => total.saturating_add(1),
            Self::Remove { .. } => total.saturating_sub(1),
            Self::Append { tracks } => {
                total.saturating_add(u32::try_from(tracks.len()).unwrap_or(u32::MAX))
            }
        }
    }
}

/// The playlists of the user, as the backend knows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PlaylistsStatus {
    #[default]
    Loading,
    Loaded(Vec<Playlist>),
    /// The saved session does not have the playlist permissions.
    NeedsReauth,
    Failed(String),
}

/// A track of the queue, for «play this one».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePosition {
    /// `QueueSnapshot::previous[index]`.
    Previous(usize),
    /// `QueueSnapshot::next[index]`.
    Next(usize),
}

impl QueuePosition {
    /// The current position of `uri`, near this position. The queue can move after the user
    /// clicks. It is `None` if the track is not in the queue now.
    pub fn resolve(self, previous: &[Track], next: &[Track], uri: &str) -> Option<Self> {
        match self {
            Self::Previous(index) => find_near(previous, uri, index).map(Self::Previous),
            Self::Next(index) => find_near(next, uri, index).map(Self::Next),
        }
    }
}

/// Index of the track with `uri` closest to `hint`.
pub fn find_near(tracks: &[Track], uri: &str, hint: usize) -> Option<usize> {
    if tracks.get(hint).is_some_and(|track| track.uri == uri) {
        return Some(hint);
    }
    tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.uri == uri)
        .min_by_key(|(index, _)| index.abs_diff(hint))
        .map(|(index, _)| index)
}

/// An album, which the user opens from the card of the current track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    /// The release year, when Spotify knows it.
    pub year: Option<u16>,
    pub tracks: Vec<Track>,
}

impl Album {
    /// The maximum number of tracks shown. The app cuts longer albums (box sets) here.
    pub const MAX_TRACKS: usize = 500;

    /// The year of a Spotify release date (`1975-10-31`, `1975-10` or `1975`).
    pub fn year_of(release_date: &str) -> Option<u16> {
        let year = release_date.get(..4)?;
        year.parse().ok().filter(|year| *year > 0)
    }
}

/// The type of a release: album or single. For Spotify, EPs are singles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReleaseKind {
    Album,
    Single,
}

impl ReleaseKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Album => "Album",
            Self::Single => "Single",
        }
    }
}

/// An album or single of an artist, without its tracks (they come as an [`Album`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// Spotify ID of the album.
    pub id: String,
    pub name: String,
    pub kind: ReleaseKind,
    /// `1975-10-31`, `1975-10` or `1975`, or empty if it is not known.
    pub release_date: String,
    /// The number of its tracks, to keep their space while they load.
    pub total_tracks: u32,
    pub cover: Option<String>,
}

impl Release {
    pub fn year(&self) -> Option<u16> {
        Album::year_of(&self.release_date)
    }

    /// "Album · 1975 · 12 songs", as the artist view shows it under its name.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.kind.label().to_owned()];
        if let Some(year) = self.year() {
            parts.push(year.to_string());
        }
        if self.total_tracks > 0 {
            parts.push(plural(self.total_tracks as usize, "song", "songs"));
        }
        parts.join(" · ")
    }
}

/// An artist with their albums and singles. The user opens it with a click on their name under a
/// track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    /// Albums and singles together, the newest first.
    pub releases: Vec<Release>,
}

impl Artist {
    /// The artist with `releases` in this order: albums and singles together, the newest first.
    /// When a release shows two times in the list, the function keeps it one time.
    pub fn new(id: String, name: String, mut releases: Vec<Release>) -> Self {
        let mut seen = std::collections::HashSet::new();
        releases.retain(|release| seen.insert(release.id.clone()));
        // ISO dates sort as text. A stable sort keeps Spotify's order for releases of the same
        // day.
        releases.sort_by(|a, b| b.release_date.cmp(&a.release_date));
        Self { id, name, releases }
    }
}

/// An artist that a search found. When the user opens it, the app shows their releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistHit {
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
}

/// An album or single that a search found, with its artists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumHit {
    pub release: Release,
    pub artists: Vec<String>,
}

impl AlbumHit {
    /// "Album · Queen", as the search shows it under its name.
    pub fn subtitle(&self) -> String {
        let kind = self.release.kind.label();
        if self.artists.is_empty() {
            kind.to_owned()
        } else {
            format!("{kind} · {}", self.artists.join(", "))
        }
    }
}

/// One page of search results. Only the first page (offset 0) has albums and artists. The next
/// pages continue with the tracks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub query: String,
    pub offset: u32,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub albums: Vec<AlbumHit>,
    #[serde(default)]
    pub artists: Vec<ArtistHit>,
    /// The total number of results that Spotify can return for this query.
    pub total: u32,
}

impl SearchPage {
    /// Spotify does not return results after this offset.
    pub const MAX_OFFSET: u32 = 1000;

    /// The search does not show artists and albums that are less similar to the query than this.
    pub const MIN_SIMILARITY: f64 = 0.5;

    /// The order of the first page's results, as indices into "artists, then albums, then songs".
    /// Spotify gives no relevance across types, so the function compares names with the query
    /// (it ignores accents and case). The order is: exact match, then prefix, then similarity. It
    /// removes artists and albums below [`Self::MIN_SIMILARITY`] that are not a prefix match. When
    /// two results have the same score, the type order applies, then Spotify's order.
    pub fn relevance_order(&self) -> Vec<usize> {
        let count = self.artists.len() + self.albums.len() + self.tracks.len();
        let mut order: Vec<usize> = (0..count).collect();
        let query = crate::util::normalize_for_search(self.query.trim());
        if self.offset > 0 || query.is_empty() {
            return order;
        }
        let score = |name: &str| {
            let name = crate::util::normalize_for_search(name.trim());
            if name == query {
                2.0
            } else if name.starts_with(&query) {
                1.5
            } else {
                let similarity = crate::util::similarity(&query, &name);
                if similarity < Self::MIN_SIMILARITY {
                    0.0
                } else {
                    similarity
                }
            }
        };
        let scores: Vec<f64> = self
            .artists
            .iter()
            .map(|a| score(&a.name))
            .chain(self.albums.iter().map(|a| score(&a.release.name)))
            .chain(self.tracks.iter().map(|t| score(&t.name)))
            .collect();
        let filterable = self.artists.len() + self.albums.len();
        order.retain(|&i| i >= filterable || scores[i] > 0.0);
        order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
        order
    }

    pub fn next_offset(&self) -> u32 {
        self.offset + self.tracks.len() as u32
    }

    pub fn has_more(&self) -> bool {
        !self.tracks.is_empty() && self.next_offset() < self.total.min(Self::MAX_OFFSET)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    /// The Spotify ID. It tells which playlists the user can edit, and which playlist order
    /// applies.
    pub id: String,
    pub display_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            uri: format!("spotify:track:{id}"),
            name: id.to_uppercase(),
            artists: vec![],
            artist_ids: vec![],
            album: String::new(),
            duration_ms: 1000,
            cover_small: None,
            cover_large: None,
            explicit: false,
            album_id: None,
        }
    }

    fn page(query: &str, artists: &[&str], albums: &[&str], songs: &[&str]) -> SearchPage {
        SearchPage {
            query: query.into(),
            artists: artists
                .iter()
                .map(|name| ArtistHit {
                    id: name.to_lowercase(),
                    name: (*name).into(),
                    cover: None,
                })
                .collect(),
            albums: albums
                .iter()
                .map(|name| AlbumHit {
                    release: Release {
                        id: name.to_lowercase(),
                        name: (*name).into(),
                        kind: ReleaseKind::Album,
                        release_date: String::new(),
                        total_tracks: 10,
                        cover: None,
                    },
                    artists: vec![],
                })
                .collect(),
            tracks: songs
                .iter()
                .map(|name| Track {
                    name: (*name).into(),
                    ..track("x")
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn first_page_results_are_mixed_and_sorted_by_how_well_their_name_matches() {
        // Indices: 0 Queen Latifah, 1 Queen (artists), 2 Killer Queen (album), 3 Queen, 4 Bohemian
        // Rhapsody (songs). The album is too different, so the order does not include it. The
        // unrelated song stays, last.
        let page = page(
            "queen",
            &["Queen Latifah", "Queen"],
            &["Killer Queen"],
            &["Queen", "Bohemian Rhapsody"],
        );
        assert_eq!(page.relevance_order(), vec![1, 3, 0, 4]);
    }

    #[test]
    fn matching_ignores_accents_and_case_and_exact_beats_prefix() {
        let page = page("CORAZON", &["Corazón Partío"], &["Corazón"], &[]);
        assert_eq!(page.relevance_order(), vec![1, 0]);
    }

    #[test]
    fn ties_keep_the_type_order_and_spotifys_order() {
        let tie = page("queen", &["Queen"], &["Queen"], &["Queen"]);
        assert_eq!(tie.relevance_order(), vec![0, 1, 2]);
        let prefixes = page("que", &[], &["Queen II", "Queens"], &[]);
        assert_eq!(prefixes.relevance_order(), vec![0, 1]);
    }

    #[test]
    fn similar_names_go_first_even_with_a_typo_and_unrelated_ones_are_dropped() {
        let page = page(
            "Tesserac",
            &["Jungle", "Tessel", "Tesseract"],
            &[],
            &["Jungle"],
        );
        assert_eq!(page.relevance_order(), vec![2, 1, 3]);
    }

    #[test]
    fn unrelated_artists_and_albums_are_dropped_but_songs_and_later_pages_are_not() {
        let none = page("freddie", &["Queen"], &[], &["Bohemian"]);
        assert_eq!(none.relevance_order(), vec![1]);
        let blank = page("  ", &["Queen"], &[], &[]);
        assert_eq!(blank.relevance_order(), vec![0]);
        let mut next = page("queen", &[], &[], &["Bohemian", "Queen"]);
        next.offset = 10;
        assert_eq!(next.relevance_order(), vec![0, 1]);
    }

    fn tracks(ids: &str) -> Vec<Track> {
        ids.chars().map(|id| track(&id.to_string())).collect()
    }

    fn ids(tracks: &[Track]) -> String {
        tracks.iter().map(|t| t.name.to_lowercase()).collect()
    }

    fn move_edit(from: usize, to: usize, id: &str) -> QueueEdit {
        QueueEdit::Move {
            from,
            to,
            uri: track(id).uri,
        }
    }

    /// Applies `edit` to the upcoming tracks only.
    fn apply(edit: &QueueEdit, next: &mut Vec<Track>, fixed: usize) -> bool {
        edit.apply(&mut Vec::new(), None, next, fixed)
    }

    /// Applies `edit` to the queue "previous|current|next" and returns the queue in the same
    /// form.
    fn edited(queue: &str, edit: &QueueEdit, fixed: usize) -> Option<String> {
        let parts: Vec<&str> = queue.split('|').collect();
        let (mut previous, current, mut next) =
            (tracks(parts[0]), tracks(parts[1]), tracks(parts[2]));
        edit.apply(&mut previous, current.first(), &mut next, fixed)
            .then(|| format!("{}|{}|{}", ids(&previous), ids(&current), ids(&next)))
    }

    #[test]
    fn moves_tracks_forward_and_backward() {
        let mut next = tracks("abcd");
        assert!(apply(&move_edit(0, 2, "a"), &mut next, 0));
        assert_eq!(ids(&next), "bcad");
        assert!(apply(&move_edit(3, 0, "d"), &mut next, 0));
        assert_eq!(ids(&next), "dbca");
        // An index after the end means "last".
        assert!(apply(&move_edit(0, 99, "d"), &mut next, 0));
        assert_eq!(ids(&next), "bcad");
        assert!(!apply(&move_edit(1, 1, "c"), &mut next, 0), "same place");
    }

    #[test]
    fn moves_are_counted_over_the_whole_queue() {
        // "ab" played, "c" plays, "de" come next.
        assert_eq!(
            edited("ab|c|de", &move_edit(4, 3, "e"), 0).as_deref(),
            Some("ab|c|ed")
        );
        // A previous track that moves after the current track plays again…
        assert_eq!(
            edited("ab|c|de", &move_edit(0, 2, "a"), 0).as_deref(),
            Some("b|c|ade")
        );
        // …and an upcoming track that moves before it counts as played.
        assert_eq!(
            edited("ab|c|de", &move_edit(4, 2, "e"), 0).as_deref(),
            Some("abe|c|d")
        );
        assert_eq!(
            edited("ab|c|de", &move_edit(3, 0, "d"), 0).as_deref(),
            Some("dab|c|e")
        );
        assert_eq!(edited("ab|c|de", &move_edit(1, 1, "b"), 0), None);
    }

    #[test]
    fn the_current_track_moves_past_the_others() {
        // Two back: the last two previous tracks come next again.
        assert_eq!(
            edited("abc|d|ef", &move_edit(3, 1, "d"), 0).as_deref(),
            Some("a|d|bcef")
        );
        // Two forward: the next two count as played.
        assert_eq!(
            edited("abc|d|ef", &move_edit(3, 5, "d"), 0).as_deref(),
            Some("abcef|d|")
        );
        assert_eq!(edited("abc|d|ef", &move_edit(3, 3, "d"), 0), None);
    }

    #[test]
    fn without_a_current_track_a_track_at_the_border_comes_next() {
        assert_eq!(
            edited("ab||c", &move_edit(0, 1, "a"), 0).as_deref(),
            Some("b||ac")
        );
        assert_eq!(
            edited("ab||c", &move_edit(2, 1, "c"), 0).as_deref(),
            Some("acb||")
        );
    }

    #[test]
    fn moved_tracks_are_found_by_uri() {
        // The queue moved: "c" is now at 1, not at 2.
        let mut next = tracks("bcd");
        assert!(apply(&move_edit(2, 0, "c"), &mut next, 0));
        assert_eq!(ids(&next), "cbd");
        assert!(
            !apply(&move_edit(0, 1, "z"), &mut next, 0),
            "no longer queued"
        );
        assert_eq!(ids(&next), "cbd");
    }

    #[test]
    fn inserts_tracks_at_a_position() {
        let mut next = tracks("ab");
        let insert = |index, id: &str| QueueEdit::Insert {
            index,
            track: track(id),
        };
        assert!(apply(&insert(1, "x"), &mut next, 0));
        assert!(apply(&insert(0, "y"), &mut next, 0));
        assert!(apply(&insert(50, "z"), &mut next, 0));
        assert_eq!(ids(&next), "yaxbz");

        let mut full = vec![track("a"); QueueSnapshot::MAX_NEXT];
        assert!(!apply(&insert(0, "x"), &mut full, 0));
    }

    #[test]
    fn inserts_whole_lists_while_they_fit() {
        let mut next = tracks("ab");
        let many = |index, ids: &str| QueueEdit::InsertMany {
            index,
            tracks: tracks(ids),
        };
        assert!(apply(&many(1, "xyz"), &mut next, 0));
        assert_eq!(ids(&next), "axyzb");
        assert!(apply(&many(0, "q"), &mut next, 1), "after the fixed one");
        assert_eq!(ids(&next), "aqxyzb");
        assert!(!apply(&many(0, ""), &mut next, 0));

        let mut almost_full = vec![track("a"); QueueSnapshot::MAX_NEXT - 2];
        assert!(apply(&many(0, "xyz"), &mut almost_full, 0));
        assert_eq!(almost_full.len(), QueueSnapshot::MAX_NEXT);
        assert_eq!(ids(&almost_full[..3]), "xya", "only the first ones fit");
        assert!(!apply(&many(0, "x"), &mut almost_full, 0));
    }

    #[test]
    fn a_fixed_first_track_is_not_moved_or_overtaken() {
        // "c" plays and the app already sent "d" to Spotify.
        assert_eq!(edited("ab|c|def", &move_edit(3, 5, "d"), 1), None);
        assert_eq!(
            edited("ab|c|def", &move_edit(2, 0, "c"), 1),
            None,
            "nor does the current one"
        );
        assert_eq!(
            edited("ab|c|def", &move_edit(5, 3, "f"), 1).as_deref(),
            Some("ab|c|dfe"),
            "it goes right after the fixed one"
        );
        assert_eq!(
            edited("ab|c|def", &move_edit(0, 2, "a"), 1).as_deref(),
            Some("b|c|daef"),
            "a previous one comes back after it"
        );
        assert_eq!(
            edited("ab|c|def", &move_edit(4, 1, "e"), 1).as_deref(),
            Some("aeb|c|df"),
            "the previous ones are free"
        );

        let mut next = tracks("acb");
        let insert = QueueEdit::Insert {
            index: 0,
            track: track("x"),
        };
        assert!(apply(&insert, &mut next, 1));
        assert_eq!(ids(&next), "axcb");
        let mut empty = Vec::new();
        assert!(apply(&insert, &mut empty, 1));
        assert_eq!(ids(&empty), "x");
    }

    #[test]
    fn a_full_queue_takes_no_previous_tracks_back() {
        let previous = "a";
        let next = "x".repeat(QueueSnapshot::MAX_NEXT);
        let queue = format!("{previous}|c|{next}");
        assert_eq!(edited(&queue, &move_edit(0, 2, "a"), 0), None);
        assert!(edited(&queue, &move_edit(2, 0, "x"), 0).is_some());
    }

    #[test]
    fn removes_tracks_found_by_uri() {
        let remove = |index, id: &str| QueueEdit::Remove {
            index,
            uri: track(id).uri,
        };
        let mut next = tracks("abcb");
        assert!(apply(&remove(3, "b"), &mut next, 0));
        assert_eq!(ids(&next), "abc", "the copy at the hint");
        // The queue moved: "c" is now at 1, not at 2.
        let mut next = tracks("bcd");
        assert!(apply(&remove(2, "c"), &mut next, 0));
        assert_eq!(ids(&next), "bd");
        assert!(!apply(&remove(0, "z"), &mut next, 0), "no longer queued");
        assert!(!apply(&remove(0, "b"), &mut next, 1), "fixed");
        assert_eq!(ids(&next), "bd");
    }

    #[test]
    fn removes_previous_tracks_found_by_uri() {
        let remove = |index, id: &str| QueueEdit::RemovePrevious {
            index,
            uri: track(id).uri,
        };
        assert_eq!(
            edited("abab|c|d", &remove(2, "a"), 1).as_deref(),
            Some("abb|c|d"),
            "the copy at the hint, even with a locked next track"
        );
        // The history moved: "b" is now at 0, not at 1.
        assert_eq!(edited("bd|c|", &remove(1, "b"), 0).as_deref(), Some("d|c|"));
        assert_eq!(edited("ab|c|d", &remove(0, "d"), 0), None, "not previous");
    }

    #[test]
    fn positions_are_resolved_near_the_hint() {
        let previous = tracks("aba");
        let next = tracks("cd");
        let uri = |id: &str| track(id).uri;
        assert_eq!(
            QueuePosition::Previous(2).resolve(&previous, &next, &uri("a")),
            Some(QueuePosition::Previous(2))
        );
        assert_eq!(
            QueuePosition::Previous(1).resolve(&previous, &next, &uri("a")),
            Some(QueuePosition::Previous(0))
        );
        assert_eq!(
            QueuePosition::Next(5).resolve(&previous, &next, &uri("d")),
            Some(QueuePosition::Next(1))
        );
        assert_eq!(
            QueuePosition::Next(0).resolve(&previous, &next, &uri("a")),
            None
        );
    }

    fn playlist_move(from: usize, to: usize, id: &str) -> PlaylistEdit {
        PlaylistEdit::Move {
            from,
            to,
            uri: track(id).uri,
        }
    }

    #[test]
    fn playlist_tracks_move_including_repeated_ones() {
        let mut list = tracks("abcb");
        assert!(playlist_move(3, 0, "b").apply(&mut list));
        assert_eq!(ids(&list), "babc", "the copy at the index moves");
        assert!(playlist_move(0, 99, "b").apply(&mut list));
        assert_eq!(ids(&list), "abcb", "past the end means last");
        assert!(!playlist_move(2, 2, "c").apply(&mut list), "same place");
        assert!(!playlist_move(0, 1, "z").apply(&mut list), "not there");
        // The index is old: the closest copy of the track moves.
        assert!(playlist_move(1, 0, "c").apply(&mut list));
        assert_eq!(ids(&list), "cabb");
    }

    #[test]
    fn playlist_tracks_are_inserted_and_removed_one_copy_at_a_time() {
        let mut list = tracks("abab");
        let remove = |index, id: &str| PlaylistEdit::Remove {
            index,
            uri: track(id).uri,
        };
        assert!(remove(2, "a").apply(&mut list));
        assert_eq!(ids(&list), "abb", "only that copy");
        assert!(!remove(0, "z").apply(&mut list));
        let insert = |index, id: &str| PlaylistEdit::Insert {
            index,
            track: track(id),
        };
        assert!(insert(Some(1), "c").apply(&mut list));
        assert_eq!(ids(&list), "acbb");
        assert!(insert(None, "a").apply(&mut list));
        assert!(insert(Some(99), "d").apply(&mut list));
        assert_eq!(ids(&list), "acbbad");
        assert_eq!(remove(0, "a").total_after(6), 5);
        assert_eq!(remove(0, "a").total_after(0), 0, "never below zero");
        assert_eq!(insert(None, "a").total_after(6), 7);
        assert_eq!(playlist_move(0, 1, "a").total_after(6), 6);
    }

    #[test]
    fn several_tracks_are_appended_in_order() {
        let mut list = tracks("ab");
        let append = |ids: &str| PlaylistEdit::Append {
            tracks: tracks(ids),
        };
        assert!(append("cad").apply(&mut list));
        assert_eq!(ids(&list), "abcad", "repeated ones too");
        assert!(!append("").apply(&mut list), "nothing to add");
        assert_eq!(append("cad").total_after(5), 8);
    }

    fn playlists(ids: &str) -> Vec<Playlist> {
        ids.chars()
            .map(|id| Playlist {
                id: id.to_string(),
                name: id.to_uppercase().to_string(),
                owner: String::new(),
                total: 0,
                cover: None,
                editable: true,
                snapshot: None,
            })
            .collect()
    }

    fn arranged(spotify: &str, order: &str) -> String {
        let order: Vec<String> = order.chars().map(String::from).collect();
        arrange_playlists(playlists(spotify), &order)
            .iter()
            .map(|p| p.id.as_str())
            .collect()
    }

    #[test]
    fn playlists_follow_the_users_order() {
        assert_eq!(arranged("abcd", ""), "abcd", "Spotify's, until reordered");
        assert_eq!(arranged("abcd", "cadb"), "cadb");
        // The playlist is not in Spotify now, so the result does not include it.
        assert_eq!(arranged("abd", "cadb"), "adb");
    }

    #[test]
    fn new_playlists_keep_their_place_in_spotifys_order() {
        // New playlists at the top in Spotify stay at the top…
        assert_eq!(arranged("xyabc", "cba"), "xycba");
        // …and the other new playlists follow the playlist before them in Spotify.
        assert_eq!(arranged("axbcy", "cba"), "cybax");
        assert_eq!(arranged("xaybz", "ba"), "xbzay");
    }

    #[test]
    fn the_year_of_an_album_comes_from_its_release_date_at_any_precision() {
        assert_eq!(Album::year_of("1975-10-31"), Some(1975));
        assert_eq!(Album::year_of("1991-02"), Some(1991));
        assert_eq!(Album::year_of("1986"), Some(1986));
        assert_eq!(Album::year_of(""), None);
        assert_eq!(Album::year_of("0000"), None);
        assert_eq!(Album::year_of("soon"), None);
    }

    fn release(id: &str, kind: ReleaseKind, date: &str) -> Release {
        Release {
            id: id.into(),
            name: id.to_uppercase(),
            kind,
            release_date: date.into(),
            total_tracks: 1,
            cover: None,
        }
    }

    #[test]
    fn an_artist_lists_albums_and_singles_together_the_newest_first() {
        use ReleaseKind::{Album, Single};
        let artist = Artist::new(
            "queen".into(),
            "Queen".into(),
            vec![
                release("a", Single, "1975-10-31"),
                release("b", Album, "1975-11"),
                release("c", Album, "1980"),
                release("d", Single, "1991-02-04"),
                release("b", Album, "1975-11"),
                release("e", Album, ""),
            ],
        );
        let ids: String = artist.releases.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, "dcbae");
    }

    #[test]
    fn a_track_knows_the_id_of_each_artist_it_has() {
        let track = Track {
            artists: vec!["Queen".into(), "David Bowie".into(), "Nadie".into()],
            artist_ids: vec![Some("queen".into()), Some(String::new())],
            ..track("a")
        };
        assert_eq!(track.artist_id(0), Some("queen"));
        assert_eq!(track.artist_id(1), None);
        assert_eq!(track.artist_id(2), None);
    }
}
