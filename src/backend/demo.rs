//! Demo backend: the full app with a fictional catalog and simulated playback. It needs no
//! Spotify account. The UI tests, the GUI smoke test and the `--demo` mode use it.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::Timings;
use super::demo_covers::render_cover;
#[cfg(test)]
use super::demo_covers::style_of;
use super::images::from_rgba;
use super::playlist_library::PlaylistLibrary;
use super::protocol::{
    AuthStatus, Command, CommandReceiver, Event, EventSink, QueueAt, ToastKind, WebPlayerStatus,
};
use crate::config::{DEFAULT_REDIRECT_PORT, redirect_uri_for_port};
use crate::model::{
    Album, AlbumHit, Artist, ArtistHit, Device, Folder, Library, LibraryItem, PlaybackSnapshot,
    Playlist, PlaylistEdit, PlaylistsStatus, QueueEdit, QueueSnapshot, Release, ReleaseKind,
    SearchPage, Track, UserProfile,
};
use crate::playback::{ENQUEUE_LEAD_MS, PlayHistory, PlayerAction, RESTART_THRESHOLD_MS, predict};
use crate::spotify::SEARCH_PAGE_SIZE;
use crate::util::normalize_for_search;

/// Cover URLs of the demo catalog: `demo:cover/<album>`.
pub const DEMO_COVER_PREFIX: &str = "demo:cover/";

/// The songs of [`CATALOG`] marked as explicit content, so the demo shows the «E» badge.
const EXPLICIT: &[&str] = &["Salt and Copper", "Neon"];

/// (title, artist, album, seconds). All names are fictional.
const CATALOG: &[(&str, &str, &str, u64)] = &[
    ("Tides", "Amber Moon", "Tides", 214),
    ("Southern Lighthouse", "Amber Moon", "Tides", 198),
    ("Salt and Copper", "Amber Moon", "Tides", 241),
    ("August Nights", "Amber Moon", "Tides", 263),
    ("Low Orbit", "The Satellites", "Low Orbit", 187),
    ("Zero Gravity", "The Satellites", "Low Orbit", 225),
    ("Lost Signal", "The Satellites", "Low Orbit", 202),
    ("Stardust", "The Satellites", "Low Orbit", 276),
    ("Glass City", "Nora Vega", "Glass City", 233),
    ("Neon", "Nora Vega", "Glass City", 191),
    ("Last Train", "Nora Vega", "Glass City", 248),
    ("Rooftops", "Nora Vega", "Glass City", 205),
    ("Roots", "North Woods", "Roots", 289),
    ("Fog", "North Woods", "Roots", 237),
    ("Pine Heart", "North Woods", "Roots", 254),
    ("Bonfire", "North Woods", "Roots", 219),
    ("Gold in the Cracks", "Kintsugi", "Gold in the Cracks", 268),
    ("Green Tea", "Kintsugi", "Gold in the Cracks", 183),
    ("April Rain", "Kintsugi", "Gold in the Cracks", 242),
    ("Mirage", "South Delta", "Desert", 229),
    ("Dunes", "South Delta", "Desert", 251),
    ("Caravan", "South Delta", "Desert", 216),
    ("Oasis", "South Delta", "Desert", 274),
    ("Endless Summer", "High Tide", "Endless Summer", 196),
    ("Bicycles", "High Tide", "Endless Summer", 178),
    ("Lemon Ice Cream", "High Tide", "Endless Summer", 207),
    ("Aurora", "Glacier", "North", 301),
    ("Slow Burn", "Glacier", "North", 244),
    ("Horizon", "Glacier", "North", 262),
    // More releases of some artists, for their page. New tracks go at the end, because the
    // playlists and the queue refer to the tracks above by index.
    ("First Light", "Amber Moon", "First Light", 226),
    ("Harbor Song", "Amber Moon", "First Light", 248),
    ("Paper Boats", "Amber Moon", "Paper Boats", 201),
    ("Paper Boats (Acoustic)", "Amber Moon", "Paper Boats", 189),
    ("Comet", "The Satellites", "Comet", 214),
    ("Neon (Night Mix)", "Nora Vega", "Neon (Night Mix)", 226),
    ("Morning Tide", "Amber Moon", "First Light", 212),
    ("Lantern", "Amber Moon", "First Light", 195),
    ("Quiet Bay", "Amber Moon", "First Light", 271),
    ("Driftwood", "Amber Moon", "First Light", 233),
    ("Saltwater Letters", "Amber Moon", "First Light", 258),
    ("Blue Hour", "Amber Moon", "First Light", 204),
    ("Seagulls", "Amber Moon", "First Light", 186),
    ("Northern Pier", "Amber Moon", "First Light", 297),
    ("Rust", "Amber Moon", "Rust", 231),
    ("Iron Bridge", "Amber Moon", "Rust", 247),
    ("Red Lanterns", "Amber Moon", "Rust", 219),
];

/// The releases of the catalog that are singles (the other releases are albums).
const SINGLES: &[&str] = &["Paper Boats", "Comet", "Neon (Night Mix)"];

/// The release date of each album of the catalog.
const ALBUM_YEARS: &[(&str, u16)] = &[
    ("Tides", 2019),
    ("Low Orbit", 2021),
    ("Glass City", 2023),
    ("Roots", 2016),
    ("Gold in the Cracks", 2024),
    ("Desert", 2018),
    ("Endless Summer", 2022),
    ("North", 2020),
    ("First Light", 2015),
    ("Paper Boats", 2024),
    ("Comet", 2023),
    ("Neon (Night Mix)", 2023),
    ("Rust", 2021),
];

/// `name` as a part of an ID or a URL: "Glass City" → `glass-city`.
fn slug(name: &str) -> String {
    normalize_for_search(name).replace(' ', "-")
}

/// Album IDs of the demo catalog: `demo-<album>`.
fn album_id(album: &str) -> String {
    format!("demo-{}", slug(album))
}

/// Artist IDs of the demo catalog: `demo-artist-<artist>`.
fn artist_id(artist: &str) -> String {
    format!("demo-artist-{}", slug(artist))
}

/// The albums and singles of the `tracks`, with their track count.
fn releases_of(tracks: &[&Track]) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    for track in tracks {
        let Some(album_id) = &track.album_id else {
            continue;
        };
        if let Some(release) = releases.iter_mut().find(|r| &r.id == album_id) {
            release.total_tracks += 1;
            continue;
        }
        let year = ALBUM_YEARS
            .iter()
            .find(|(album, _)| *album == track.album)
            .map(|(_, year)| year.to_string())
            .unwrap_or_default();
        releases.push(Release {
            id: album_id.clone(),
            name: track.album.clone(),
            kind: if SINGLES.contains(&track.album.as_str()) {
                ReleaseKind::Single
            } else {
                ReleaseKind::Album
            },
            release_date: year,
            total_tracks: 1,
            cover: track.cover_small.clone(),
        });
    }
    releases
}

/// Runs the demo until the interface drops its end of the channel. Its simulated delays come from
/// `timings` (`demo_*`).
pub async fn run(mut commands: CommandReceiver, sink: EventSink, timings: Timings) {
    let mut demo = Demo::new(sink, timings);
    demo.emit_logged_out();
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => demo.handle(command).await,
                None => break,
            },
            _ = ticker.tick() => demo.tick(),
        }
    }
}

/// The fictional catalog of the demo.
pub fn catalog() -> Vec<Track> {
    CATALOG
        .iter()
        .enumerate()
        .map(|(i, (name, artist, album, seconds))| {
            let cover = format!("{DEMO_COVER_PREFIX}{}", slug(album));
            Track {
                uri: format!("spotify:track:demo{i:02}"),
                name: (*name).to_owned(),
                artists: vec![(*artist).to_owned()],
                artist_ids: vec![Some(artist_id(artist))],
                album: (*album).to_owned(),
                album_id: Some(album_id(album)),
                duration_ms: seconds * 1000,
                cover_small: Some(cover.clone()),
                cover_large: Some(cover),
                explicit: EXPLICIT.contains(name),
            }
        })
        .collect()
}

struct Demo {
    sink: EventSink,
    timings: Timings,
    catalog: Vec<Track>,
    logged_in: bool,
    history: PlayHistory,
    current: Option<Track>,
    next: Vec<Track>,
    is_playing: bool,
    progress_ms: u64,
    clock: Instant,
    volume: u8,
    /// The number of queue edits applied (see `QueueSnapshot::edits`). It never goes back to 0.
    queue_edits: u64,
    /// As with Spotify, the next track becomes fixed in the last seconds of the current track
    /// (`QueueSnapshot::next_locked`).
    next_locked: bool,
    /// «Play in this app» started (simulated: the demo has no audio).
    web_player: bool,
    active_device: &'static str,
    /// What the phone plays while the music is there (see [`Away`]).
    away: Option<Away>,
    /// The user's playlists and their tracks.
    playlists: Vec<(Playlist, Vec<Track>)>,
    /// The number of playlist edits received (see `Event::PlaylistTracks`). It never goes back
    /// to 0.
    playlist_edits: u64,
    /// The folders and order that the user gave the playlists, as the live backend keeps them
    /// (here only in memory).
    playlist_order: PlaylistLibrary,
    /// The URIs of the tracks in «Liked Songs». They stay until logout, as the other data.
    liked: HashSet<String>,
}

/// The music went to the phone, as to a different Spotify device. The app shows where it is, with
/// no player and no queue. The demo keeps what the phone plays, and it comes back as Spotify's
/// queue when the music comes back.
struct Away {
    current: Option<Track>,
    next: Vec<Track>,
    progress_ms: u64,
}

/// Catalog indices of the tracks in «Liked Songs» at the start.
const LIKED: &[usize] = &[0, 4, 9, 16, 26, 43];

/// (name, owner, editable, catalog indices). «Summer Hits» is of a different user. As with
/// Spotify, the user can see it but cannot use it.
const PLAYLISTS: &[(&str, &str, bool, &[usize])] = &[
    ("Favorites", "Guest", true, &[0, 4, 8, 12, 16, 19]),
    ("Running", "Guest", true, &[5, 9, 23, 24, 25, 27]),
    ("Calm", "Guest", true, &[13, 14, 17, 18, 26]),
    ("Saved for later", "Guest", true, &[]),
    ("Summer Hits", "Spotify", false, &[23, 24, 25, 21]),
    ("Coast to Coast", "Guest", true, &[23, 30, 9, 20, 5, 26]),
    ("Main Stage", "Guest", true, &[10, 23, 4, 33, 7, 24]),
    ("Sunset Sessions", "Guest", true, &[20, 19, 14, 26, 31]),
    ("Night Drive", "Guest", true, &[10, 6, 34, 28, 11]),
    ("Headliners", "Guest", true, &[33, 9, 24, 4, 16, 22]),
];

/// The folders of the library at the start: their names, their positions in the first level and
/// the indices in [`PLAYLISTS`] of their playlists. The cover of the first playlist gives the
/// folder its color.
const FOLDERS: &[(&str, usize, &[usize])] =
    &[("Festivals", 0, &[9, 6, 7]), ("Road Trips", 6, &[5, 8])];

/// The catalog index of the track that plays at the start («Harbor Song»).
const START_TRACK: usize = 30;

/// Catalog indices of the upcoming tracks at the start. Each track is from a different album than
/// the track before it, so that the covers of the queue change at each position.
const START_QUEUE: &[usize] = &[43, 9, 13, 5, 20, 17, 1, 26, 24, 10, 33];

fn playlist_id(index: usize) -> String {
    format!("demo-playlist-{index}")
}

/// The playlists of [`PLAYLISTS`] in their order, with [`FOLDERS`] at their positions.
fn start_library() -> Library {
    let in_folder = |i: &usize| FOLDERS.iter().any(|(_, _, inside)| inside.contains(i));
    let playlists = |indices: &mut dyn Iterator<Item = usize>| -> Vec<LibraryItem> {
        indices
            .map(|i| LibraryItem::Playlist(playlist_id(i)))
            .collect()
    };
    let mut items = playlists(&mut (0..PLAYLISTS.len()).filter(|i| !in_folder(i)));
    for (n, (name, position, inside)) in FOLDERS.iter().enumerate() {
        let folder = LibraryItem::Folder(Folder {
            id: format!("folder-{}", n + 1),
            name: (*name).to_owned(),
            items: playlists(&mut inside.iter().copied()),
        });
        items.insert((*position).min(items.len()), folder);
    }
    Library::new(items)
}

/// The demo account, for the order of its playlists.
const DEMO_USER: &str = "demo";
const LOCAL_DEVICE: &str = "demo-device";
const WEB_PLAYER_DEVICE: &str = "demo-web-player";
/// A different device of the account: music sent there plays on that device.
const PHONE_DEVICE: &str = "demo-phone";

impl Demo {
    fn new(sink: EventSink, timings: Timings) -> Self {
        Self {
            sink,
            timings,
            catalog: catalog(),
            logged_in: false,
            history: PlayHistory::new(),
            current: None,
            next: Vec::new(),
            is_playing: false,
            progress_ms: 0,
            clock: Instant::now(),
            volume: 70,
            queue_edits: 0,
            next_locked: false,
            web_player: false,
            active_device: LOCAL_DEVICE,
            away: None,
            playlists: Vec::new(),
            playlist_edits: 0,
            playlist_order: PlaylistLibrary::in_memory(DEMO_USER, start_library()),
            liked: LIKED
                .iter()
                .map(|&i| format!("spotify:track:demo{i:02}"))
                .collect(),
        }
    }

    fn create_playlists(&mut self) {
        self.playlists = PLAYLISTS
            .iter()
            .enumerate()
            .map(|(i, (name, owner, editable, indices))| {
                let playlist = Playlist {
                    id: playlist_id(i),
                    name: (*name).to_owned(),
                    owner: (*owner).to_owned(),
                    total: indices.len() as u32,
                    cover: Some(format!("{DEMO_COVER_PREFIX}playlist-{}", slug(name))),
                    editable: *editable,
                    snapshot: None,
                };
                let tracks = indices.iter().map(|&i| self.catalog[i].clone()).collect();
                (playlist, tracks)
            })
            .collect();
    }

    /// A new, empty playlist. As in Spotify, it goes at the top (of the folder `folder`, if
    /// there is one).
    fn create_playlist(&mut self, name: String, folder: Option<&str>) {
        let id = playlist_id(self.playlists.len());
        if let Some(folder) = folder {
            let _ = self.playlist_order.place_new(&id, folder);
        }
        self.sink.send(Event::PlaylistCreated {
            name: name.clone(),
            id: Some(id.clone()),
        });
        let playlist = Playlist {
            id,
            name,
            owner: "Guest".to_owned(),
            total: 0,
            cover: None,
            editable: true,
            snapshot: None,
        };
        self.playlists.insert(0, (playlist, Vec::new()));
        self.emit_playlists();
    }

    fn rename_playlist(&mut self, id: &str, name: String) {
        if let Some((playlist, _)) = self.usable_playlist(id) {
            playlist.name = name;
        }
        // Also when the name cannot change: the UI shows its name again.
        self.emit_playlists();
    }

    fn emit_playlists(&mut self) {
        let playlists = self.playlists.iter().map(|(p, _)| p.clone()).collect();
        let playlists = self.playlist_order.arrange(DEMO_USER, playlists);
        self.sink.send(Event::Library {
            library: self.playlist_order.library().clone(),
            edits: self.playlist_order.edits(),
        });
        self.sink
            .send(Event::Playlists(PlaylistsStatus::Loaded(playlists)));
    }

    /// The playlist `id`, if Spotify lets the app use it.
    fn usable_playlist(&mut self, id: &str) -> Option<&mut (Playlist, Vec<Track>)> {
        let entry = self.playlists.iter_mut().find(|(p, _)| p.id == id)?;
        if !entry.0.editable {
            self.sink
                .toast(ToastKind::Error, super::playlist_not_usable(&entry.0.name));
            return None;
        }
        Some(entry)
    }

    fn queue_playlist(&mut self, id: &str, index: usize) {
        let Some((playlist, tracks)) = self.usable_playlist(id).cloned() else {
            return;
        };
        self.queue_tracks(&playlist.name, QueueAt::Next(index), tracks);
    }

    /// Puts the tracks of the album `id` (with the name `name`) in the queue.
    fn queue_album(&mut self, id: &str, name: &str, at: QueueAt) {
        let tracks = self.album_tracks(id);
        self.queue_tracks(name, at, tracks);
    }

    /// Puts the tracks of the playlist or album with the name `name` in the queue at `at`, or
    /// plays them if nothing plays.
    fn queue_tracks(&mut self, name: &str, at: QueueAt, mut tracks: Vec<Track>) {
        if tracks.is_empty() {
            self.sink
                .toast(ToastKind::Error, super::nothing_to_queue(name));
        } else if self.current.is_none() {
            self.play(PlayerAction::Play { tracks, index: 0 });
        } else {
            // «Play»: the other tracks go first in the queue, then the first track plays, as in
            // the real backend.
            let play_now = (at == QueueAt::Now).then(|| tracks.remove(0));
            let index = match at {
                QueueAt::Next(index) => index,
                QueueAt::End => self.next.len(),
                QueueAt::Now => 0,
            };
            if !tracks.is_empty() {
                let before = self.next.len();
                let total = tracks.len() + usize::from(play_now.is_some());
                if self.apply_edit(&QueueEdit::InsertMany { index, tracks }) {
                    let added = self.next.len() - before + usize::from(play_now.is_some());
                    if added < total {
                        self.sink.toast(
                            ToastKind::Error,
                            super::playlist_partly_queued(name, added, total),
                        );
                    }
                } else {
                    self.sink.toast(ToastKind::Error, super::queue_full());
                }
            }
            if let Some(track) = play_now {
                self.play(PlayerAction::Play {
                    tracks: vec![track],
                    index: 0,
                });
            }
        }
        self.emit_all();
    }

    /// The tracks of the playlist `id`, for the playlist panel.
    fn send_playlist_tracks(&self, id: &str) {
        let Some((playlist, tracks)) = self.playlists.iter().find(|(p, _)| p.id == id) else {
            return;
        };
        let id = id.to_owned();
        self.sink.send(if playlist.editable {
            Event::PlaylistTracks {
                id,
                tracks: tracks.clone(),
                total: playlist.total,
                edits: self.playlist_edits,
            }
        } else {
            Event::PlaylistTracksFailed {
                id,
                message: super::PLAYLIST_NOT_READABLE.to_owned(),
            }
        });
    }

    /// The album `id` of the catalog, with its tracks.
    fn send_album(&self, id: String) {
        let tracks = self.album_tracks(&id);
        let Some(first) = tracks.first() else {
            self.sink.send(Event::AlbumFailed {
                id,
                message: "This album isn't available.".into(),
            });
            return;
        };
        let name = first.album.clone();
        let year = ALBUM_YEARS
            .iter()
            .find(|(album, _)| *album == name)
            .map(|(_, year)| *year);
        self.sink.send(Event::Album(Album {
            id,
            name,
            year,
            tracks,
        }));
    }

    /// The tracks of the album `id` of the catalog.
    fn album_tracks(&self, id: &str) -> Vec<Track> {
        self.catalog
            .iter()
            .filter(|track| track.album_id.as_deref() == Some(id))
            .cloned()
            .collect()
    }

    /// The artist `id` of the catalog with their releases.
    fn send_artist(&self, id: String) {
        let tracks: Vec<&Track> = self
            .catalog
            .iter()
            .filter(|track| track.artist_id(0) == Some(id.as_str()))
            .collect();
        let Some(first) = tracks.first() else {
            self.sink.send(Event::ArtistFailed {
                id,
                message: "This artist isn't available.".into(),
            });
            return;
        };
        let releases = releases_of(&tracks);
        let artist = Artist::new(id, first.artists[0].clone(), releases);
        self.sink.send(Event::Artist(artist));
    }

    /// Adds, moves or removes tracks of the playlist `id`.
    fn edit_playlist(&mut self, id: &str, edit: &PlaylistEdit) {
        self.playlist_edits += 1;
        let Some((playlist, tracks)) = self.usable_playlist(id) else {
            return;
        };
        if edit.apply(tracks) {
            playlist.total = tracks.len() as u32;
            self.emit_playlists();
        } else {
            // The UI showed a change that is not possible: undo it there.
            self.send_playlist_tracks(id);
        }
    }

    fn emit_logged_out(&self) {
        self.sink.send(Event::Auth(AuthStatus::LoggedOut {
            client_id: Some("demo".into()),
            redirect_uri: redirect_uri_for_port(DEFAULT_REDIRECT_PORT),
            error: None,
        }));
    }

    async fn handle(&mut self, command: Command) {
        self.sync_clock();
        match command {
            Command::Login { .. } => {
                tokio::time::sleep(self.timings.demo_login).await;
                self.login();
            }
            Command::CancelLogin => self.emit_logged_out(),
            Command::LoadImage { url, max_px } => self.load_image(url, max_px),
            Command::Logout => {
                let queue_edits = self.queue_edits;
                *self = Self::new(self.sink.clone(), self.timings);
                self.queue_edits = queue_edits;
                self.sink.send(Event::Playback(PlaybackSnapshot::idle()));
                self.emit_queue();
                self.sink.send(Event::PlayingElsewhere(None));
                self.sink.send(Event::WebPlayer(WebPlayerStatus::Off));
                self.emit_logged_out();
            }
            _ if !self.logged_in => {}
            // As in the live backend, the app controls nothing on the phone.
            Command::Resume
            | Command::Pause
            | Command::Next
            | Command::Previous
            | Command::Seek { .. }
                if self.away.is_some() => {}
            Command::PlayTrack { .. }
            | Command::AddToQueue { .. }
            | Command::EditQueue(QueueEdit::Insert { .. })
            | Command::QueuePlaylist { .. }
            | Command::QueueAlbum { .. }
                if self.away.is_some() =>
            {
                // The user played it from here: the music comes to this app, from the start.
                self.away = None;
                self.active_device = self.here_device();
                self.sink.send(Event::PlayingElsewhere(None));
                self.emit_devices();
                Box::pin(self.handle(command)).await;
            }
            Command::Search { query, offset } => self.search(query, offset),
            Command::CheckLiked { uris } => {
                let states = uris
                    .into_iter()
                    .map(|uri| {
                        let liked = self.liked.contains(&uri);
                        (uri, liked)
                    })
                    .collect();
                self.sink.send(Event::LikedStates(states));
            }
            Command::SetLiked { track, liked } => {
                if liked {
                    self.liked.insert(track.uri);
                } else {
                    self.liked.remove(&track.uri);
                }
            }
            Command::Resume => {
                self.is_playing = self.current.is_some();
                self.emit_playback();
            }
            Command::Pause => {
                self.is_playing = false;
                self.emit_playback();
            }
            // The user cleared the queue: there is no track to skip to.
            Command::Next | Command::Previous if self.current.is_none() => {}
            Command::Next => {
                self.advance();
                self.emit_all();
            }
            Command::Previous => {
                self.go_back();
                self.emit_all();
            }
            Command::Seek { position_ms } => {
                let duration = self.current.as_ref().map_or(0, |t| t.duration_ms);
                self.progress_ms = position_ms.min(duration);
                self.emit_playback();
            }
            Command::PlayTrack { track } => {
                self.play(PlayerAction::Play {
                    tracks: vec![track],
                    index: 0,
                });
                self.emit_all();
            }
            Command::AddToQueue { track } if self.current.is_none() => {
                self.play(PlayerAction::Play {
                    tracks: vec![track],
                    index: 0,
                });
                self.emit_all();
            }
            Command::AddToQueue { track } => {
                let edit = QueueEdit::Insert {
                    index: self.next.len(),
                    track,
                };
                self.apply_edit(&edit);
                self.emit_queue();
            }
            Command::EditQueue(edit) => {
                self.queue_edits += 1;
                match edit {
                    QueueEdit::Insert { track, .. } if self.current.is_none() => {
                        self.play(PlayerAction::Play {
                            tracks: vec![track],
                            index: 0,
                        });
                    }
                    edit => {
                        self.apply_edit(&edit);
                    }
                }
                self.emit_all();
            }
            Command::ClearQueue => {
                self.queue_edits += 1;
                self.history.clear();
                self.current = None;
                self.next.clear();
                self.is_playing = false;
                self.progress_ms = 0;
                self.next_locked = false;
                self.emit_all();
            }
            Command::QueuePlaylist { playlist, index } => self.queue_playlist(&playlist.id, index),
            Command::EditPlaylist { playlist, edit } => self.edit_playlist(&playlist.id, &edit),
            Command::FindInPlaylist {
                playlist,
                index,
                track,
            } => {
                let found = self.playlists.iter().any(|(p, tracks)| {
                    p.id == playlist.id && tracks.iter().any(|t| t.uri == track.uri)
                });
                self.sink.send(Event::FoundInPlaylist {
                    playlist,
                    index,
                    track,
                    found,
                });
            }
            Command::FindAlbumInPlaylist { playlist, id, name } => {
                let tracks = self.album_tracks(&id);
                let found = self
                    .playlists
                    .iter()
                    .find(|(p, _)| p.id == playlist.id)
                    .map(|(_, listed)| {
                        tracks
                            .iter()
                            .filter(|track| listed.iter().any(|t| t.uri == track.uri))
                            .map(|track| track.uri.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                self.sink.send(Event::FoundAlbumInPlaylist {
                    playlist,
                    name,
                    tracks,
                    found,
                });
            }
            Command::LoadPlaylistTracks { playlist } => self.send_playlist_tracks(&playlist.id),
            Command::LoadAlbum { id } => self.send_album(id),
            Command::LoadArtist { id } => self.send_artist(id),
            Command::QueueAlbum { id, name, at } => self.queue_album(&id, &name, at),
            Command::RefreshPlaylists => self.emit_playlists(),
            Command::CreatePlaylist { name, folder } => {
                tokio::time::sleep(self.timings.demo_create_playlist).await;
                self.create_playlist(name, folder.as_deref());
            }
            Command::RenamePlaylist { playlist, name } => self.rename_playlist(&playlist.id, name),
            Command::EditLibrary(edit) => {
                // The UI already shows it. The demo keeps it for the next lists.
                let _ = self.playlist_order.apply(&edit);
            }
            Command::PlayFromQueue { position, uri } => {
                if let Some(position) = position.resolve(self.history.entries(), &self.next, &uri) {
                    self.play(PlayerAction::Jump(position));
                }
                self.emit_all();
            }
            Command::SetVolume { percent } => {
                self.volume = percent.min(100);
                self.emit_playback();
            }
            Command::TransferPlayback { device_id } => {
                match self.devices().into_iter().find(|d| d.id == device_id) {
                    Some(device) if device.id == PHONE_DEVICE => self.go_to_phone(),
                    Some(device) => {
                        self.active_device = if device.id == WEB_PLAYER_DEVICE {
                            WEB_PLAYER_DEVICE
                        } else {
                            LOCAL_DEVICE
                        };
                        self.come_back();
                        self.emit_devices();
                        self.emit_playback();
                    }
                    None => {}
                }
            }
            Command::RefreshDevices => self.emit_devices(),
            Command::StartWebPlayer => {
                if !self.web_player {
                    self.sink.send(Event::WebPlayer(WebPlayerStatus::Starting));
                    tokio::time::sleep(self.timings.demo_web_player).await;
                    self.web_player = true;
                    self.sink.send(Event::WebPlayer(WebPlayerStatus::Ready {
                        device_id: WEB_PLAYER_DEVICE.into(),
                    }));
                }
                self.active_device = WEB_PLAYER_DEVICE;
                self.come_back();
                self.emit_devices();
                self.emit_playback();
            }
            // Its web player needs no permission.
            Command::ReauthorizeWebPlayer => {}
        }
    }

    /// The device that is this app: its web player after it starts, or else the demo's own
    /// device.
    fn here_device(&self) -> &'static str {
        if self.web_player {
            WEB_PLAYER_DEVICE
        } else {
            LOCAL_DEVICE
        }
    }

    /// The music goes to the phone. The demo clears the app's queue and hides the player, as the
    /// live backend does.
    fn go_to_phone(&mut self) {
        if self.away.is_none() {
            self.away = Some(Away {
                current: self.current.take(),
                next: std::mem::take(&mut self.next),
                progress_ms: self.progress_ms,
            });
        }
        self.active_device = PHONE_DEVICE;
        self.history.clear();
        self.is_playing = false;
        self.progress_ms = 0;
        self.next_locked = false;
        let phone = self.devices().into_iter().find(|d| d.id == PHONE_DEVICE);
        self.sink.send(Event::PlayingElsewhere(phone));
        self.emit_devices();
        self.emit_all();
    }

    /// The music comes back from the phone (if it was there). It plays what the phone played, and
    /// the demo reads its queue again.
    fn come_back(&mut self) {
        let Some(away) = self.away.take() else {
            return;
        };
        let now = Instant::now();
        self.history.clear();
        self.history.observe(away.current.as_ref(), now);
        self.is_playing = away.current.is_some();
        self.current = away.current;
        self.next = away.next;
        self.progress_ms = away.progress_ms;
        self.clock = now;
        self.sink.send(Event::PlayingElsewhere(None));
        self.emit_all();
    }

    fn tick(&mut self) {
        self.sync_clock();
        if !self.logged_in || !self.is_playing {
            return;
        }
        let duration = self.current.as_ref().map_or(0, |t| t.duration_ms);
        if self.progress_ms >= duration {
            self.advance();
            self.emit_all();
        } else if !self.next_locked
            && !self.next.is_empty()
            && duration - self.progress_ms <= ENQUEUE_LEAD_MS
        {
            self.next_locked = true;
            self.emit_queue();
        }
    }

    /// Applies `edit` to the queue. Returns true if the queue changed.
    fn apply_edit(&mut self, edit: &QueueEdit) -> bool {
        let fixed = self.fixed();
        let mut previous = self.history.entries().to_vec();
        if !edit.apply(&mut previous, self.current.as_ref(), &mut self.next, fixed) {
            return false;
        }
        self.history.set_entries(previous);
        true
    }

    /// The number of upcoming tracks that cannot move (see `QueueEdit::apply`).
    fn fixed(&self) -> usize {
        usize::from(self.next_locked && !self.next.is_empty())
    }

    /// Moves the simulated playback position forward to now.
    fn sync_clock(&mut self) {
        let now = Instant::now();
        if self.is_playing {
            self.progress_ms += now.duration_since(self.clock).as_millis() as u64;
        }
        self.clock = now;
    }

    fn login(&mut self) {
        let now = Instant::now();
        self.logged_in = true;
        self.history.clear();
        self.current = Some(self.catalog[START_TRACK].clone());
        self.history.observe(self.current.as_ref(), now);
        self.next = START_QUEUE
            .iter()
            .map(|&i| self.catalog[i].clone())
            .collect();
        self.is_playing = true;
        self.progress_ms = 42_000;
        self.clock = now;

        self.sink
            .send(Event::Auth(AuthStatus::LoggedIn { demo: true }));
        self.sink.send(Event::Profile(UserProfile {
            id: "guest".into(),
            display_name: "Guest".into(),
        }));
        self.emit_devices();
        self.emit_all();
        self.create_playlists();
        self.emit_playlists();
    }

    fn advance(&mut self) {
        self.next_locked = false;
        self.progress_ms = 0;
        if self.next.is_empty() {
            // The queue stops here: the demo adds nothing more, and the music stops.
            self.history.finish();
            self.current = None;
            self.is_playing = false;
            return;
        }
        let next = self.next.remove(0);
        self.history.observe(Some(&next), Instant::now());
        self.current = Some(next);
    }

    fn go_back(&mut self) {
        let now = Instant::now();
        let previous = self.history.last().cloned();
        match previous {
            Some(previous) if self.progress_ms <= RESTART_THRESHOLD_MS => {
                if let Some(current) = self.current.take() {
                    self.next.insert(0, current);
                }
                self.history.hint_back(now);
                self.history.observe(Some(&previous), now);
                self.current = Some(previous);
            }
            _ => {}
        }
        self.progress_ms = 0;
        self.is_playing = self.current.is_some();
        self.next_locked = false;
    }

    /// Plays a track with the same queue rules as the live backend.
    fn play(&mut self, action: PlayerAction) {
        let state = self.snapshot();
        let prediction = predict(&state, &self.history, &self.next, &action, Instant::now());
        if let Some(change) = prediction.and_then(|p| p.queue) {
            self.current = change.history.current().cloned();
            self.history = change.history;
            self.next = change.next;
            self.progress_ms = 0;
            self.is_playing = true;
            self.next_locked = false;
        }
    }

    fn search(&self, query: String, offset: u32) {
        let needle = normalize_for_search(query.trim());
        let words: Vec<&str> = needle.split_whitespace().collect();
        let matches: Vec<Track> = self
            .catalog
            .iter()
            .filter(|track| {
                let haystack = normalize_for_search(&format!(
                    "{} {} {}",
                    track.name,
                    track.artist_line(),
                    track.album
                ));
                words.iter().all(|word| haystack.contains(word))
            })
            .cloned()
            .collect();
        // As in Spotify, albums and artists come only with the first page.
        let (albums, artists) = if offset == 0 {
            self.search_albums_and_artists(&words)
        } else {
            (Vec::new(), Vec::new())
        };
        let page = SearchPage {
            query,
            offset,
            albums,
            artists,
            total: matches.len() as u32,
            tracks: matches
                .into_iter()
                .skip(offset as usize)
                .take(SEARCH_PAGE_SIZE as usize)
                .collect(),
        };
        let sink = self.sink.clone();
        let latency = self.timings.demo_search;
        tokio::spawn(async move {
            tokio::time::sleep(latency).await;
            sink.send(Event::SearchResults(page));
        });
    }

    /// The artists with all the `words` in their name, and the albums with the words in their
    /// name or artist.
    fn search_albums_and_artists(&self, words: &[&str]) -> (Vec<AlbumHit>, Vec<ArtistHit>) {
        let matches_all = |text: &str| {
            let haystack = normalize_for_search(text);
            words.iter().all(|word| haystack.contains(word))
        };
        let mut artists: Vec<ArtistHit> = Vec::new();
        let mut albums: Vec<AlbumHit> = Vec::new();
        for track in &self.catalog {
            let Some(artist_id) = track.artist_id(0) else {
                continue;
            };
            let artist = track.artist_line();
            if matches_all(&artist) && !artists.iter().any(|hit| hit.id == artist_id) {
                artists.push(ArtistHit {
                    id: artist_id.to_owned(),
                    name: artist.clone(),
                    cover: track.cover_small.clone(),
                });
            }
            let Some(album_id) = &track.album_id else {
                continue;
            };
            if matches_all(&format!("{} {artist}", track.album))
                && !albums.iter().any(|hit| &hit.release.id == album_id)
            {
                let tracks: Vec<&Track> = self
                    .catalog
                    .iter()
                    .filter(|other| other.album_id.as_ref() == Some(album_id))
                    .collect();
                albums.extend(releases_of(&tracks).into_iter().map(|release| AlbumHit {
                    release,
                    artists: vec![artist.clone()],
                }));
            }
        }
        (albums, artists)
    }

    fn load_image(&self, url: String, max_px: u32) {
        let sink = self.sink.clone();
        tokio::task::spawn_blocking(move || match url.strip_prefix(DEMO_COVER_PREFIX) {
            Some(seed) => {
                let image = render_cover(seed, max_px.clamp(16, 640));
                let image = from_rgba(image);
                sink.send(Event::ImageLoaded { url, max_px, image });
            }
            None => sink.send(Event::ImageFailed { url, max_px }),
        });
    }

    fn devices(&self) -> Vec<Device> {
        let device = |id: &str, name: &str| Device {
            id: id.into(),
            name: name.into(),
            kind: "Computer".into(),
            is_active: self.active_device == id,
            volume_percent: Some(self.volume),
            supports_volume: true,
        };
        let mut devices = vec![device(LOCAL_DEVICE, "This computer")];
        if self.web_player {
            devices.push(device(WEB_PLAYER_DEVICE, crate::webplayer::DEVICE_NAME));
        }
        devices.push(Device {
            kind: "Smartphone".into(),
            ..device(PHONE_DEVICE, "Nuria's phone")
        });
        devices
    }

    fn emit_devices(&self) {
        self.sink.send(Event::Devices(self.devices()));
    }

    fn snapshot(&self) -> PlaybackSnapshot {
        PlaybackSnapshot {
            track: self.current.clone(),
            is_playing: self.is_playing,
            progress_ms: self.progress_ms,
            measured_at: self.clock,
            // When the music is on the phone, the UI shows nothing (see `go_to_phone`).
            device: self
                .devices()
                .into_iter()
                .find(|d| d.is_active && d.id != PHONE_DEVICE),
            shuffle: false,
        }
    }

    fn emit_playback(&self) {
        self.sink.send(Event::Playback(self.snapshot()));
    }

    fn emit_queue(&self) {
        self.sink.send(Event::Queue(QueueSnapshot {
            previous: self.history.entries().to_vec(),
            current: self.current.clone(),
            next: self.next.clone(),
            edits: self.queue_edits,
            next_locked: self.fixed() == 1,
        }));
    }

    fn emit_all(&self) {
        self.emit_playback();
        self.emit_queue();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let catalog = catalog();
        assert_eq!(catalog.len(), CATALOG.len());
        let uris: HashSet<_> = catalog.iter().map(|t| t.uri.as_str()).collect();
        assert_eq!(uris.len(), catalog.len(), "every URI is unique");
        assert!(catalog.iter().all(|t| {
            t.cover_large
                .as_deref()
                .unwrap()
                .starts_with(DEMO_COVER_PREFIX)
        }));
    }

    #[test]
    fn each_track_of_the_start_queue_has_a_different_album_than_the_track_before_it() {
        let catalog = catalog();
        let albums: Vec<&str> = std::iter::once(START_TRACK)
            .chain(START_QUEUE.iter().copied())
            .map(|i| catalog[i].album.as_str())
            .collect();
        assert!(
            albums.windows(2).all(|pair| pair[0] != pair[1]),
            "{albums:?}"
        );
    }

    #[test]
    fn the_covers_of_the_catalog_have_many_styles() {
        let seeds = CATALOG.iter().map(|(_, _, album, _)| slug(album)).chain(
            PLAYLISTS
                .iter()
                .map(|(name, ..)| format!("playlist-{}", slug(name))),
        );
        let styles: HashSet<_> = seeds.map(|seed| format!("{:?}", style_of(&seed))).collect();
        assert!(styles.len() >= 6, "{styles:?}");
    }
}
