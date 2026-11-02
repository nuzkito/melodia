//! Successful answers from Spotify, kept for some time. Thus, when the user goes back to a
//! search, an album, an artist or a playlist, the app does not ask again. The cache is saved in
//! `cache.json` with the account that read the answers (a different account can see a different
//! catalog). See «Cache» in `docs/backend.md`.

use std::collections::HashMap;
use std::hash::Hash;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config;
use crate::model::{Album, Artist, Playlist, Track};

/// The time that the cache uses a search page again.
const SEARCH_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// The time that the cache uses an album again. Its songs almost never change.
const ALBUM_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// The time that the cache uses an artist again. A new release can appear.
const ARTIST_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The time that the cache uses the tracks of a playlist again. They change more frequently, from
/// here (see [`ResponseCache::playlist_edited`]) or from a different app. Its `snapshot_id`
/// shows changes from a different app sooner (see [`ResponseCache::playlist_tracks`]).
const PLAYLIST_TTL: Duration = Duration::from_secs(10 * 60);

const MAX_SEARCH_PAGES: usize = 100;
const MAX_ALBUMS: usize = 100;
const MAX_ARTISTS: usize = 50;
const MAX_PLAYLISTS: usize = 50;

const CACHE_FILE: &str = "cache.json";
/// The version of the file's format. The cache ignores a file with a different version.
const FORMAT: u32 = 2;

/// True if a value stored at `stored` is still good at `now`. If the clock went back, the
/// difference counts in the same way.
fn fresh(stored: SystemTime, now: SystemTime, ttl: Duration) -> bool {
    let age = now
        .duration_since(stored)
        .unwrap_or_else(|error| error.duration());
    age < ttl
}

/// One value as saved, with the time of its read.
#[derive(Serialize, Deserialize)]
struct Saved<K, V> {
    key: K,
    value: V,
    stored: SystemTime,
}

/// Values that expire `ttl` after the store. When the map is full, the oldest value makes space
/// for the new value.
struct Expiring<K, V> {
    entries: HashMap<K, (V, SystemTime)>,
    ttl: Duration,
    capacity: usize,
}

impl<K: Eq + Hash + Clone, V: Clone> Expiring<K, V> {
    fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            ttl,
            capacity,
        }
    }

    fn get(&mut self, key: &K, now: SystemTime) -> Option<V> {
        match self.entries.get(key) {
            Some((value, stored)) if fresh(*stored, now, self.ttl) => Some(value.clone()),
            Some(_) => {
                self.entries.remove(key);
                None
            }
            None => None,
        }
    }

    fn insert(&mut self, key: K, value: V, now: SystemTime) {
        self.insert_read_at(key, value, now, now);
    }

    /// Keeps `value`, read at `stored`.
    fn insert_read_at(&mut self, key: K, value: V, stored: SystemTime, now: SystemTime) {
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            let ttl = self.ttl;
            self.entries
                .retain(|_, (_, stored)| fresh(*stored, now, ttl));
            if self.entries.len() >= self.capacity
                && let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (_, stored))| *stored)
                    .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(key, (value, stored));
    }

    fn remove(&mut self, key: &K) -> bool {
        self.entries.remove(key).is_some()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }

    /// The values that are still good at `now`, to save them.
    fn saved(&self, now: SystemTime) -> Vec<Saved<&K, &V>> {
        self.entries
            .iter()
            .filter(|(_, (_, stored))| fresh(*stored, now, self.ttl))
            .map(|(key, (value, stored))| Saved {
                key,
                value,
                stored: *stored,
            })
            .collect()
    }

    /// Uses the saved values that are still good at `now`. The data read in this session is
    /// newer, so it stays.
    fn take_in(&mut self, saved: Vec<Saved<K, V>>, now: SystemTime) {
        for Saved { key, value, stored } in saved {
            if fresh(stored, now, self.ttl) && !self.entries.contains_key(&key) {
                self.insert_read_at(key, value, stored, now);
            }
        }
    }
}

/// The tracks of a playlist as read, after the first `edits` playlist edits that the backend
/// received.
#[derive(Clone, Serialize, Deserialize)]
struct PlaylistTracks {
    tracks: Vec<Track>,
    total: u32,
    /// Spotify's `snapshot_id` at the time of the read, if known.
    snapshot: Option<String>,
    /// The cache does not save it. The next session counts its edits from 0, and a saved read
    /// came after the edits of its own session.
    #[serde(skip)]
    edits: u64,
}

/// The contents of `cache.json`, as [`ResponseCache::take_changes`] writes them.
#[derive(Deserialize)]
pub struct SavedCache {
    format: u32,
    /// The Spotify ID of the account that read it.
    user: String,
    #[serde(default)]
    searches: Vec<Saved<(String, u32), Value>>,
    #[serde(default)]
    albums: Vec<Saved<String, Album>>,
    #[serde(default)]
    artists: Vec<Saved<String, Artist>>,
    #[serde(default)]
    playlists: Vec<Saved<String, PlaylistTracks>>,
}

/// [`SavedCache`] with borrowed values, to write it.
#[derive(Serialize)]
struct SavedCacheRef<'a> {
    format: u32,
    user: &'a str,
    searches: Vec<Saved<&'a (String, u32), &'a Value>>,
    albums: Vec<Saved<&'a String, &'a Album>>,
    artists: Vec<Saved<&'a String, &'a Artist>>,
    playlists: Vec<Saved<&'a String, &'a PlaylistTracks>>,
}

/// Reads `cache.json` from `config_dir`. A file that is missing, that it cannot read, or that has
/// a different format gives nothing: the backend asks for the answers again.
pub fn read_saved(config_dir: &Path) -> Option<SavedCache> {
    let bytes = std::fs::read(config_dir.join(CACHE_FILE)).ok()?;
    match serde_json::from_slice::<SavedCache>(&bytes) {
        Ok(saved) if saved.format == FORMAT => Some(saved),
        Ok(_) => None,
        Err(error) => {
            log::warn!("Could not read the saved answers from Spotify: {error}");
            None
        }
    }
}

/// Writes `contents` (from [`ResponseCache::take_changes`]) as `cache.json` in
/// `config_dir`.
pub fn write_saved(config_dir: &Path, contents: &[u8]) -> io::Result<()> {
    config::write_private_file(&config_dir.join(CACHE_FILE), contents)
}

/// The searches, albums, artists and playlist tracks that the backend read from Spotify recently.
pub struct ResponseCache {
    searches: Expiring<(String, u32), Value>,
    albums: Expiring<String, Album>,
    artists: Expiring<String, Artist>,
    playlists: Expiring<String, PlaylistTracks>,
    /// The playlist edit count immediately after the last edit of each playlist (id → count). A
    /// read from before that edit does not count.
    edited: HashMap<String, u64>,
    /// The Spotify ID of the session's user, when known (see [`ResponseCache::adopt`]). Until
    /// then, the cache saves nothing.
    user: Option<String>,
    /// Changes with each [`ResponseCache::clear`]. Thus the cache does not use the data that it
    /// read from the disk for an earlier session.
    generation: u64,
    /// Something changed after the last save.
    changed: bool,
}

impl Default for ResponseCache {
    fn default() -> Self {
        Self {
            searches: Expiring::new(SEARCH_TTL, MAX_SEARCH_PAGES),
            albums: Expiring::new(ALBUM_TTL, MAX_ALBUMS),
            artists: Expiring::new(ARTIST_TTL, MAX_ARTISTS),
            playlists: Expiring::new(PLAYLIST_TTL, MAX_PLAYLISTS),
            edited: HashMap::new(),
            user: None,
            generation: 0,
            changed: false,
        }
    }
}

/// Spotify's search ignores case and spaces at the start and end. The cache does the same.
fn search_key(query: &str, offset: u32) -> (String, u32) {
    (query.trim().to_lowercase(), offset)
}

impl ResponseCache {
    /// The answer of `GET /search` for `query` from `offset`, as Spotify sent it.
    pub fn search(&mut self, query: &str, offset: u32, now: SystemTime) -> Option<Value> {
        self.searches.get(&search_key(query, offset), now)
    }

    /// Keeps the full answer of `GET /search`.
    pub fn store_search(&mut self, query: &str, offset: u32, raw: Value, now: SystemTime) {
        self.searches.insert(search_key(query, offset), raw, now);
        self.changed = true;
    }

    pub fn album(&mut self, id: &str, now: SystemTime) -> Option<Album> {
        self.albums.get(&id.to_owned(), now)
    }

    pub fn store_album(&mut self, album: Album, now: SystemTime) {
        self.albums.insert(album.id.clone(), album, now);
        self.changed = true;
    }

    pub fn artist(&mut self, id: &str, now: SystemTime) -> Option<Artist> {
        self.artists.get(&id.to_owned(), now)
    }

    pub fn store_artist(&mut self, artist: Artist, now: SystemTime) {
        self.artists.insert(artist.id.clone(), artist, now);
        self.changed = true;
    }

    /// The tracks of `playlist` and their total, if the backend read them after its last edit
    /// from here, and no different app changed it after that. Spotify must still show the same
    /// count and, when both are known, the same `snapshot_id` (each change of the playlist
    /// changes it).
    pub fn playlist_tracks(
        &mut self,
        playlist: &Playlist,
        now: SystemTime,
    ) -> Option<(Vec<Track>, u32)> {
        let read = self.playlists.get(&playlist.id, now)?;
        let edited = self.edited.get(&playlist.id).copied().unwrap_or(0);
        let other_version = matches!(
            (&read.snapshot, &playlist.snapshot),
            (Some(read), Some(listed)) if read != listed
        );
        if read.edits < edited || read.total != playlist.total || other_version {
            self.playlists.remove(&playlist.id);
            self.changed = true;
            return None;
        }
        Some((read.tracks, read.total))
    }

    /// Keeps the tracks of the playlist `id` (version `snapshot`), read after the first `edits`
    /// playlist edits. It does not keep them if the playlist got an edit after those edits.
    pub fn store_playlist_tracks(
        &mut self,
        id: &str,
        tracks: Vec<Track>,
        total: u32,
        snapshot: Option<String>,
        edits: u64,
        now: SystemTime,
    ) {
        if edits < self.edited.get(id).copied().unwrap_or(0) {
            return;
        }
        let read = PlaylistTracks {
            tracks,
            total,
            snapshot,
            edits,
        };
        self.playlists.insert(id.to_owned(), read, now);
        self.changed = true;
    }

    /// The playlist `id` got an edit, which makes the playlist edit count `edits`. The cache does
    /// not use the data read before.
    pub fn playlist_edited(&mut self, id: &str, edits: u64) {
        self.edited.insert(id.to_owned(), edits);
        if self.playlists.remove(&id.to_owned()) {
            self.changed = true;
        }
    }

    /// Changes with each [`ResponseCache::clear`]. Give it to [`ResponseCache::adopt`].
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The session's user is `user`. After this, the cache saves the data that it reads for that
    /// user, and uses `saved` (read from the disk) if it is of that user. If the session changed
    /// after `generation`, nothing occurs.
    pub fn adopt(
        &mut self,
        user: &str,
        generation: u64,
        saved: Option<SavedCache>,
        now: SystemTime,
    ) {
        if generation != self.generation {
            return;
        }
        self.user = Some(user.to_owned());
        let Some(saved) = saved.filter(|saved| saved.user == user) else {
            return;
        };
        self.searches.take_in(saved.searches, now);
        self.albums.take_in(saved.albums, now);
        self.artists.take_in(saved.artists, now);
        self.playlists.take_in(saved.playlists, now);
    }

    /// The data to write in `cache.json` (see [`write_saved`]), if something changed after the
    /// last time and the user is known.
    pub fn take_changes(&mut self, now: SystemTime) -> Option<Vec<u8>> {
        let user = self.user.as_deref().filter(|_| self.changed)?;
        let saved = SavedCacheRef {
            format: FORMAT,
            user,
            searches: self.searches.saved(now),
            albums: self.albums.saved(now),
            artists: self.artists.saved(now),
            playlists: self.playlists.saved(now),
        };
        match serde_json::to_vec(&saved) {
            Ok(contents) => {
                self.changed = false;
                Some(contents)
            }
            Err(error) => {
                log::warn!("Could not save the answers from Spotify: {error}");
                None
            }
        }
    }

    /// Forgets all data (the session ended). The saved data stays on the disk, for when the same
    /// account comes back.
    pub fn clear(&mut self) {
        self.searches.clear();
        self.albums.clear();
        self.artists.clear();
        self.playlists.clear();
        self.edited.clear();
        self.user = None;
        self.generation += 1;
        self.changed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(total: u32) -> Value {
        serde_json::json!({ "tracks": { "items": [], "total": total }, "popularity": 80 })
    }

    fn album(id: &str) -> Album {
        Album {
            id: id.to_owned(),
            name: "A Night at the Opera".to_owned(),
            year: Some(1975),
            tracks: Vec::new(),
        }
    }

    #[test]
    fn a_search_is_reused_ignoring_case_and_spaces() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_search("Queen", 0, raw(42), now);
        let found = cache.search(" queen ", 0, now).unwrap();
        // The full answer, with the data that the app does not read.
        assert_eq!(found, raw(42));
        assert!(cache.search("queen", 10, now).is_none());
    }

    #[test]
    fn answers_expire() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_search("queen", 0, raw(42), now);
        cache.store_album(album("opera"), now);
        assert!(cache.search("queen", 0, now + SEARCH_TTL).is_none());
        assert!(cache.album("opera", now + ALBUM_TTL / 2).is_some());
        assert!(cache.album("opera", now + ALBUM_TTL).is_none());
    }

    #[test]
    fn answers_from_the_future_expire_too() {
        // The clock went back after the read.
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_album(album("opera"), now + ALBUM_TTL);
        assert!(cache.album("opera", now).is_none());
    }

    #[test]
    fn the_oldest_answer_makes_room_when_full() {
        let now = SystemTime::now();
        let mut cache = Expiring::new(Duration::from_secs(60), 2);
        cache.insert("a", 1, now);
        cache.insert("b", 2, now + Duration::from_secs(1));
        cache.insert("c", 3, now + Duration::from_secs(2));
        let later = now + Duration::from_secs(3);
        assert_eq!(cache.get(&"a", later), None);
        assert_eq!(cache.get(&"b", later), Some(2));
        assert_eq!(cache.get(&"c", later), Some(3));
    }

    fn playlist(id: &str, total: u32) -> Playlist {
        Playlist {
            id: id.to_owned(),
            name: "Para correr".to_owned(),
            owner: "Nuria".to_owned(),
            total,
            cover: None,
            editable: true,
            snapshot: None,
        }
    }

    fn version(playlist: Playlist, snapshot: &str) -> Playlist {
        Playlist {
            snapshot: Some(snapshot.to_owned()),
            ..playlist
        }
    }

    #[test]
    fn playlist_tracks_are_reused_for_ten_minutes() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_playlist_tracks("run", Vec::new(), 3, None, 0, now);
        assert_eq!(
            cache.playlist_tracks(&playlist("run", 3), now + Duration::from_secs(599)),
            Some((Vec::new(), 3))
        );
        assert!(
            cache
                .playlist_tracks(&playlist("run", 3), now + Duration::from_secs(600))
                .is_none()
        );
    }

    #[test]
    fn playlist_tracks_from_before_an_edit_are_not_reused() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_playlist_tracks("run", Vec::new(), 3, None, 0, now);
        cache.store_playlist_tracks("gym", Vec::new(), 2, None, 0, now);
        cache.playlist_edited("run", 1);
        assert!(cache.playlist_tracks(&playlist("run", 3), now).is_none());
        // The cache also does not keep a read that was in progress during the edit.
        cache.store_playlist_tracks("run", Vec::new(), 3, None, 0, now);
        assert!(cache.playlist_tracks(&playlist("run", 3), now).is_none());
        // It keeps a read after the edit.
        cache.store_playlist_tracks("run", Vec::new(), 4, None, 1, now);
        assert!(cache.playlist_tracks(&playlist("run", 4), now).is_some());
        // The edit has no effect on other playlists.
        assert!(cache.playlist_tracks(&playlist("gym", 2), now).is_some());
    }

    #[test]
    fn playlist_tracks_are_read_again_when_their_count_changes_elsewhere() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_playlist_tracks("run", Vec::new(), 3, None, 0, now);
        assert!(cache.playlist_tracks(&playlist("run", 5), now).is_none());
    }

    #[test]
    fn playlist_tracks_are_read_again_when_spotify_lists_another_version() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        let read = Some("v1".to_owned());
        cache.store_playlist_tracks("run", Vec::new(), 3, read.clone(), 0, now);
        // The cache uses the data again for the same version, or for an unknown version.
        assert!(
            cache
                .playlist_tracks(&version(playlist("run", 3), "v1"), now)
                .is_some()
        );
        assert!(cache.playlist_tracks(&playlist("run", 3), now).is_some());
        // It does not use it for a different version (new order from a different app, same
        // count).
        assert!(
            cache
                .playlist_tracks(&version(playlist("run", 3), "v2"), now)
                .is_none()
        );
    }

    #[test]
    fn clearing_forgets_everything() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_search("queen", 0, raw(42), now);
        cache.store_album(album("opera"), now);
        cache.clear();
        assert!(cache.search("queen", 0, now).is_none());
        assert!(cache.album("opera", now).is_none());
    }

    /// The data that `cache` writes, read back as from the disk.
    fn written(cache: &mut ResponseCache, now: SystemTime) -> SavedCache {
        let contents = cache.take_changes(now).expect("something to save");
        serde_json::from_slice(&contents).unwrap()
    }

    #[test]
    fn answers_are_saved_for_the_same_account() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.adopt("nuria", cache.generation(), None, now);
        cache.store_search("queen", 0, raw(42), now);
        cache.store_album(album("opera"), now);
        cache.store_playlist_tracks("run", Vec::new(), 3, Some("v1".into()), 2, now);
        let saved = written(&mut cache, now);
        // There is nothing new, so there is nothing to write.
        assert!(cache.take_changes(now).is_none());

        let later = now + Duration::from_secs(60);
        let mut next = ResponseCache::default();
        next.adopt("nuria", next.generation(), Some(saved), later);
        assert!(next.search("Queen", 0, later).is_some());
        assert!(next.album("opera", later).is_some());
        // The read time stays `now`: it expires at the same time as before.
        assert!(next.album("opera", now + ALBUM_TTL).is_none());
        // The edit count starts again in each session.
        let run = version(playlist("run", 3), "v1");
        assert!(next.playlist_tracks(&run, later).is_some());
    }

    #[test]
    fn saved_answers_of_another_account_are_not_used() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.adopt("nuria", cache.generation(), None, now);
        cache.store_album(album("opera"), now);
        let saved = written(&mut cache, now);

        let mut next = ResponseCache::default();
        next.adopt("pablo", next.generation(), Some(saved), now);
        assert!(next.album("opera", now).is_none());
    }

    #[test]
    fn nothing_is_saved_until_the_user_is_known() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.store_album(album("opera"), now);
        assert!(cache.take_changes(now).is_none());
        // When the user is known, the cache saves the data that it read before.
        cache.adopt("nuria", cache.generation(), None, now);
        assert_eq!(written(&mut cache, now).albums.len(), 1);
    }

    #[test]
    fn a_file_read_for_an_earlier_session_is_not_taken_in() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.adopt("nuria", cache.generation(), None, now);
        cache.store_album(album("opera"), now);
        let saved = written(&mut cache, now);

        let generation = cache.generation();
        cache.clear();
        cache.adopt("nuria", generation, Some(saved), now);
        assert!(cache.album("opera", now).is_none());
        assert!(cache.take_changes(now).is_none());
    }

    #[test]
    fn what_was_read_in_this_session_wins_over_the_saved_answers() {
        let now = SystemTime::now();
        let mut cache = ResponseCache::default();
        cache.adopt("nuria", cache.generation(), None, now);
        cache.store_album(album("opera"), now);
        let saved = written(&mut cache, now);

        let mut next = ResponseCache::default();
        let newer = Album {
            name: "A Night at the Opera (Deluxe)".to_owned(),
            ..album("opera")
        };
        next.store_album(newer, now);
        next.adopt("nuria", next.generation(), Some(saved), now);
        assert_eq!(
            next.album("opera", now).unwrap().name,
            "A Night at the Opera (Deluxe)"
        );
    }

    #[test]
    fn the_saved_file_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        assert!(read_saved(dir.path()).is_none());
        let mut cache = ResponseCache::default();
        cache.adopt("nuria", cache.generation(), None, now);
        cache.store_album(album("opera"), now);
        write_saved(dir.path(), &cache.take_changes(now).unwrap()).unwrap();
        let mut next = ResponseCache::default();
        next.adopt("nuria", next.generation(), read_saved(dir.path()), now);
        assert!(next.album("opera", now).is_some());
        // The cache ignores a file that it cannot understand.
        std::fs::write(dir.path().join(CACHE_FILE), b"{\"format\": 99}").unwrap();
        assert!(read_saved(dir.path()).is_none());
    }
}
