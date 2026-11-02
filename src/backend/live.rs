//! Backend connected to the real Spotify Web API.
//!
//! One task owns the state. It processes the commands and the reports of the player page in
//! order, so two player commands never run at the same time. Slow or independent work runs in
//! separate tasks. `docs/backend.md` and `docs/queue.md` tell when the backend asks
//! Spotify what plays, how music on another device works, and how the app's own queue works.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{Mutex, OnceCell, Semaphore, mpsc, watch};
use tokio::task::JoinHandle;

use super::images::fetch_image;
use super::playlist_library::PlaylistLibrary;
use super::playlist_sync::{ApiStep, PlanError, PlaylistMirror};
use super::protocol::{
    AuthStatus, Command, CommandReceiver, Event, EventSink, QueueAt, ToastKind, WebPlayerStatus,
};
use super::response_cache::{self, ResponseCache};
use super::saved_queue::{self, SavedQueue};
use crate::auth::{self, AuthError, OAuthConfig, Session, SessionStore, TokenManager};
use crate::config::{self, AppConfig, ResumePoint};
use crate::model::{
    Device, LibraryEdit, PlaybackSnapshot, Playlist, PlaylistEdit, PlaylistsStatus, QueueEdit,
    QueuePosition, QueueSnapshot, Track, UserProfile,
};
use crate::playback::{
    ENQUEUE_LEAD_MS, Expectation, Follow, PlayHistory, PlayerAction, SpotifyPlan, apply_follow,
    follow, predict,
};
use crate::spotify::models::{parse_item, parse_search};
use crate::spotify::{ApiError, LIBRARY_BATCH, PlayOffset, PlayRequest, SpotifyClient};
use crate::util;
use crate::webplayer::{
    DEVICE_NAME, FromPage, HostRequest, PageControl, PageState, PlayerEvent, PlayerEvents,
};

/// The number of times the backend sends again a request for what plays, when it fails (or when
/// the answer is not yet useful).
const FETCH_RETRIES: u32 = 3;
/// A track that changes this near its end (or later) ended by itself. The user did not skip it.
const END_SLACK_MS: u64 = 5_000;
/// The interval between two saves of the answers from Spotify, if something changed. The backend
/// also saves them when the session ends and when the backend stops. Thus, if the app is killed,
/// it loses little.
const CACHE_SAVE_EVERY: Duration = Duration::from_secs(30);
const MAX_PARALLEL_IMAGES: usize = 6;
/// The maximum time for the web player to become a Spotify device.
const WEB_PLAYER_TIMEOUT: Duration = Duration::from_secs(40);
/// The number of times the backend sends again a request to a new device, while Spotify does not
/// know the device yet.
const NEW_DEVICE_RETRIES: u32 = 12;
/// The maximum number of tracks of a playlist that the playlist panel shows.
const MAX_SHOWN_PLAYLIST_TRACKS: usize = 500;
/// Nearer than this to the saved position, there is nothing to restore.
const RESUME_TOLERANCE_MS: u64 = 2_000;

type LoginResult = Result<Session, AuthError>;

/// What a report finds at the end of a track, when Spotify did not continue as Melodia's queue
/// tells.
enum TrackEnd {
    /// Melodia's queue has no more tracks, but Spotify started this track. It must not play.
    FinishQueue(String),
    /// Spotify's list has no more tracks, but Melodia's queue has more. Its next track must start.
    PlayNext,
}

/// Playlist work that runs in its own task. The backend applies the result.
enum PlaylistJob {
    /// The tracks of `playlist`, to put in the queue at `index`.
    Fetched {
        playlist: Playlist,
        index: usize,
        result: Result<Vec<Track>, ApiError>,
    },
    /// The tracks of the album with the name `name`, to put in the queue at `at`.
    AlbumFetched {
        name: String,
        at: QueueAt,
        result: Result<Vec<Track>, ApiError>,
    },
    /// The tracks of the album with the name `name`, which the user dropped on `playlist`, and the
    /// URIs of the tracks that the playlist already has.
    AlbumForPlaylist {
        playlist: Playlist,
        name: String,
        result: Result<(Vec<Track>, Vec<String>), ApiError>,
    },
    /// The result of the creation in Spotify of a playlist with the name `name`, to show in the
    /// folder `folder`.
    Created {
        name: String,
        folder: Option<String>,
        result: Result<Playlist, ApiError>,
    },
    /// The result of the change of the name of `playlist` to `name` in Spotify.
    Renamed {
        playlist: Playlist,
        name: String,
        result: Result<(), ApiError>,
    },
    /// The result in Spotify of an edit of the tracks of `playlist`.
    Edited {
        playlist: Playlist,
        edit: PlaylistEdit,
        result: Result<(), EditError>,
    },
    /// The result of the save of `track` to «Liked Songs» (`liked`) or its removal from there.
    /// With `confirm`, the UI waits for the result to show it.
    Liked {
        track: Track,
        liked: bool,
        confirm: bool,
        result: Result<(), ApiError>,
    },
}

/// Why an edit of a playlist failed.
#[derive(Debug)]
enum EditError {
    Api(ApiError),
    Plan(PlanError),
}

impl From<ApiError> for EditError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

/// The playlists with known entries (id → entries), to change the indices of the UI into Spotify
/// positions.
type Mirrors = Arc<Mutex<HashMap<String, PlaylistMirror>>>;

pub async fn run(config: AppConfig, mut commands: CommandReceiver, sink: EventSink) {
    let (login_tx, mut login_rx) = mpsc::unbounded_channel();
    let (web_tx, mut web_rx) = mpsc::unbounded_channel();
    let (playlist_tx, mut playlist_rx) = mpsc::unbounded_channel();
    let mut backend = LiveBackend::new(config, sink, login_tx, web_tx, playlist_tx);
    util::startup_mark("backend running");
    backend.restore_session();
    util::startup_mark("backend: session restored");

    loop {
        let fetch_at = backend.state_fetch_at.map(tokio::time::Instant::from_std);
        let check_at = backend.queue_check_at.map(tokio::time::Instant::from_std);
        let tick_at = tokio::time::Instant::from_std(backend.tick_at);
        let cache_save_at = tokio::time::Instant::from_std(backend.cache_save_at);
        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => backend.handle(command).await,
                None => break,
            },
            Some(result) = login_rx.recv() => backend.finish_login(result).await,
            Some(job) = playlist_rx.recv() => backend.finish_playlist_job(job).await,
            Some((generation, event)) = web_rx.recv() => {
                backend.on_web_player_event(generation, event).await;
            }
            () = tokio::time::sleep_until(fetch_at.unwrap_or(tick_at)),
                if fetch_at.is_some() && backend.api.is_some() =>
            {
                backend.fetch_state().await;
            }
            () = tokio::time::sleep_until(check_at.unwrap_or(tick_at)), if check_at.is_some() => {
                backend.check_queue().await;
            }
            () = tokio::time::sleep_until(tick_at) => backend.tick(),
            () = tokio::time::sleep_until(cache_save_at) => {
                backend.cache_save_at = Instant::now() + CACHE_SAVE_EVERY;
                backend.save_cache();
            }
        }
        backend.save_queue();
    }
    backend.abort_tasks();
    backend.save_cache();
    // The last position, queue and answers can be not yet on the disk.
    let writers = [
        backend.resume_task.take(),
        backend.queue_task.take(),
        backend.cache_task.take(),
    ];
    drop(backend);
    for task in writers.into_iter().flatten() {
        let _ = task.await;
    }
}

/// True if a session with `scope` (permissions with spaces between them) has all of `needed`.
fn has_scopes(scope: &str, needed: &[&str]) -> bool {
    needed
        .iter()
        .all(|needed| scope.split_whitespace().any(|granted| granted == *needed))
}

// Sessions from older versions of the app can lack the permissions of newer features: «Play in
// this app», the playlists panel, «Liked Songs».
fn has_streaming_scope(scope: &str) -> bool {
    has_scopes(scope, &["streaming"])
}

fn has_playlist_scopes(scope: &str) -> bool {
    has_scopes(scope, &["playlist-read-private", "playlist-modify-private"])
}

fn has_library_scopes(scope: &str) -> bool {
    has_scopes(scope, &["user-library-read", "user-library-modify"])
}

struct LiveBackend {
    config: AppConfig,
    sink: EventSink,
    http: reqwest::Client,
    store: SessionStore,
    api: Option<SpotifyClient>,
    login_tx: mpsc::UnboundedSender<LoginResult>,
    login_task: Option<JoinHandle<()>>,
    search_task: Option<JoinHandle<()>>,
    /// Reads the album that the user opens. A newer request replaces it.
    album_task: Option<JoinHandle<()>>,
    /// Reads the artist that the user opens. A newer request replaces it.
    artist_task: Option<JoinHandle<()>>,
    /// Searches, albums, artists and playlist tracks that the backend read recently. The tasks
    /// that read them share it.
    cache: Arc<std::sync::Mutex<ResponseCache>>,
    /// The contents of `cache.json` to write. The cache task writes them, not this task.
    cache_tx: watch::Sender<Option<Arc<Vec<u8>>>>,
    cache_task: Option<JoinHandle<()>>,
    /// The time of the next save of the cache, if something changed.
    cache_save_at: Instant,
    volume_task: Option<JoinHandle<()>>,
    playlists_task: Option<JoinHandle<()>>,
    playlist_tx: mpsc::UnboundedSender<PlaylistJob>,
    /// The last task that reads or edits the tracks of a playlist. Each task waits for the task
    /// before it. Thus edits go to Spotify in order, and the backend reads the lists after the
    /// edits that it sent before.
    playlist_chain: Option<JoinHandle<()>>,
    mirrors: Mirrors,
    /// The reads of the tracks that are in «Liked Songs».
    liked_checks: tokio::task::JoinSet<()>,
    /// The last change to «Liked Songs». Each change waits for the change before it. Thus quick
    /// likes and unlikes go to Spotify in order.
    liked_chain: Option<JoinHandle<()>>,
    /// A change to «Liked Songs» that the user asked for without the permission. The backend
    /// makes it when the user connects the account again.
    liked_after_login: Option<(Track, bool)>,
    /// The number of [`Command::EditPlaylist`]s received (see `Event::PlaylistTracks`).
    playlist_edits: u64,
    /// The user's folders and order of the playlists. Each load of the playlists applies them when
    /// it ends. Thus a load that runs while the user changes the order also shows the new order.
    /// Its disk I/O runs on blocking threads.
    playlist_order: Arc<std::sync::Mutex<PlaylistLibrary>>,
    /// The track count and name that each playlist must have after the edits that succeeded (see
    /// [`fetch_playlists`]).
    expected: ExpectedChanges,
    /// The profile of the session's user. The backend reads it one time for each session.
    profile: SessionProfile,
    image_permits: Arc<Semaphore>,

    history: PlayHistory,
    playback: PlaybackSnapshot,
    upcoming: Vec<Track>,
    expectation: Option<Expectation>,

    /// Melodia decides the order of `upcoming`, and Spotify follows it.
    queue_owned: bool,
    /// What Spotify plays next by itself (while Melodia owns the queue).
    plan: SpotifyPlan,
    /// The time to make sure that Spotify's next track is Melodia's.
    queue_check_at: Option<Instant>,
    /// The user cleared the queue, but Spotify keeps this track loaded (paused). The UI does not
    /// show it until something plays again.
    dismissed: Option<String>,
    /// The number of edits applied, which each [`QueueSnapshot`] includes.
    queue_edits: u64,
    /// The queue that the queue task last received to save, for the next time the app opens (see
    /// [`LiveBackend::save_queue`]).
    saved_queue: Option<SavedQueue>,
    queue_tx: watch::Sender<Option<SavedQueue>>,
    queue_task: Option<JoinHandle<()>>,
    /// The volume that the user set. The UI shows it until Spotify reports it (or for a moment).
    volume_hold: Option<(u8, Instant)>,
    /// The device that the music moved to, until the given time. Spotify can show the previous
    /// device as active for a moment (see `hold_device`).
    moved_to: Option<(Device, Instant)>,
    /// The device list as the backend last read it, to show the result of a transfer immediately.
    devices: Vec<Device>,
    /// The latest volume that the user wants. The volume task sends it (or only saves it).
    volume_tx: watch::Sender<Option<VolumeChange>>,
    /// The volume that the user last set in Melodia (the backend also saves it for next time).
    volume: Option<u8>,

    /// The time to ask Spotify what plays (`GET /me/player`), one time (see
    /// [`LiveBackend::fetch_state`]).
    state_fetch_at: Option<Instant>,
    /// The next request for what plays is the first request of the session.
    startup_fetch: bool,
    /// The page said that the music left this app. The next request for what plays finds where
    /// the music went.
    where_check: bool,
    /// The number of times that the backend sends again a request for what plays. It does this if
    /// the request fails, or if Spotify still shows the music here after the page said it left.
    fetch_retries: u32,
    /// The backend must read Spotify's queue again. The UI can show a queue that is not what
    /// Spotify plays next.
    queue_stale: bool,
    /// The time of the next save of the position of the music that plays here, and of the next
    /// check of the web player's start (see [`LiveBackend::tick`]).
    tick_at: Instant,
    rate_limited_until: Option<Instant>,
    /// The music plays on this other device, not in this app. Melodia shows where, with no queue
    /// and no player, and controls nothing there. The queue and the player come back when the
    /// music plays here again.
    elsewhere: Option<Device>,

    /// «Play in this app».
    web_tx: PlayerEvents,
    /// The player page is open (or opens) in the window.
    web_open: bool,
    /// The backend increments it for each page, to ignore messages from previous pages.
    web_generation: u64,
    web_status: WebPlayerStatus,
    web_started_at: Option<Instant>,
    web_last_error: Option<String>,
    /// Start the web player when the user logs in again.
    web_after_login: bool,
    /// When the app opens: start the web player when Spotify tells what plays, and become the
    /// device if nothing plays on a different device.
    web_autostart: bool,
    /// The music moved to this app, until the given time (or until the page tells that it plays
    /// here). Before that time, reports that it plays on a different device can be from before
    /// the change, and the backend ignores them. If the page tells nothing, the backend asks
    /// Spotify at `Timings::here_check_after`.
    moving_here_until: Option<Instant>,
    /// The device picker is open until this time (see `Timings::devices_shown_for`).
    devices_shown_until: Option<Instant>,
    /// What Spotify plays after the track with this URI, as the page last reported it (the first
    /// two or three tracks). The backend clears it when it sends a command, until the page
    /// reports again.
    page_next: Option<(String, Vec<String>)>,
    /// The page's next tracks that last did not agree with the queue shown, and caused Melodia to
    /// read the queue. If they still do not agree, Melodia does not read it again for them (see
    /// [`Self::follow_queue_from_page`]).
    page_next_mismatch: Option<Vec<String>>,
    /// The backend starts it when the app opens. When it is ready, it becomes the device, paused
    /// and without a notice.
    web_quiet: bool,
    /// A play request that found no device while the web player started. It plays there
    /// immediately when the web player is ready.
    web_pending: Option<Request>,
    /// The position of the music the last time it played in this app (as saved in
    /// `settings.json`), and the time of the save.
    resume: Option<ResumePoint>,
    resume_saved_at: Option<Instant>,
    /// The position to save. The resume task writes it, not this task.
    resume_tx: watch::Sender<Option<ResumePoint>>,
    resume_task: Option<JoinHandle<()>>,
    /// This app became the device when the app opened. The saved position to go back to, and the
    /// time limit for it.
    resume_pending: Option<(ResumePoint, Instant)>,
}

impl LiveBackend {
    fn new(
        config: AppConfig,
        sink: EventSink,
        login_tx: mpsc::UnboundedSender<LoginResult>,
        web_tx: PlayerEvents,
        playlist_tx: mpsc::UnboundedSender<PlaylistJob>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("melodia/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("the HTTP client configuration is valid");
        let store = SessionStore::new(config.session_path());
        let volume = config.volume;
        let resume = config::resume_point(&config.config_dir);
        let playlist_order = PlaylistLibrary::new(Some(config.config_dir.clone()));
        let (resume_tx, resume_rx) = watch::channel(resume.clone());
        let resume_task = spawn_save_task(
            resume_rx,
            config.config_dir.clone(),
            "the playback position",
            config::save_resume_point,
        );
        let saved_queue = saved_queue::read(&config.config_dir);
        let (queue_tx, queue_rx) = watch::channel(saved_queue.clone());
        let queue_task = spawn_save_task(
            queue_rx,
            config.config_dir.clone(),
            "the queue",
            saved_queue::write,
        );
        let (cache_tx, cache_rx) = watch::channel(None);
        let cache_task = spawn_cache_task(cache_rx, config.config_dir.clone());
        let tick_at = Instant::now() + config.timings.tick;
        Self {
            config,
            sink,
            http,
            store,
            api: None,
            login_tx,
            login_task: None,
            search_task: None,
            album_task: None,
            artist_task: None,
            cache: Arc::default(),
            cache_tx,
            cache_task: Some(cache_task),
            cache_save_at: Instant::now() + CACHE_SAVE_EVERY,
            volume_task: None,
            playlists_task: None,
            playlist_tx,
            playlist_chain: None,
            mirrors: Mirrors::default(),
            liked_checks: tokio::task::JoinSet::new(),
            liked_chain: None,
            liked_after_login: None,
            expected: ExpectedChanges::default(),
            profile: SessionProfile::default(),
            playlist_edits: 0,
            playlist_order: Arc::new(std::sync::Mutex::new(playlist_order)),
            image_permits: Arc::new(Semaphore::new(MAX_PARALLEL_IMAGES)),
            history: PlayHistory::new(),
            playback: PlaybackSnapshot::idle(),
            upcoming: Vec::new(),
            expectation: None,
            queue_owned: false,
            plan: SpotifyPlan::default(),
            queue_check_at: None,
            dismissed: None,
            queue_edits: 0,
            saved_queue,
            queue_tx,
            queue_task: Some(queue_task),
            volume_hold: None,
            moved_to: None,
            devices: Vec::new(),
            volume_tx: watch::channel(None).0,
            volume,
            state_fetch_at: None,
            startup_fetch: false,
            where_check: false,
            fetch_retries: 0,
            queue_stale: false,
            tick_at,
            rate_limited_until: None,
            elsewhere: None,
            web_tx,
            web_open: false,
            web_generation: 0,
            web_status: WebPlayerStatus::Off,
            web_started_at: None,
            web_last_error: None,
            web_after_login: false,
            web_autostart: false,
            moving_here_until: None,
            devices_shown_until: None,
            page_next: None,
            page_next_mismatch: None,
            web_quiet: false,
            web_pending: None,
            resume,
            resume_saved_at: None,
            resume_tx,
            resume_task: Some(resume_task),
            resume_pending: None,
        }
    }

    fn restore_session(&mut self) {
        match (self.config.client_id.clone(), self.store.load()) {
            (Some(client_id), Some(session)) => {
                log::info!("Restoring saved Spotify session");
                self.begin_session(session, client_id);
                self.restore_queue();
            }
            _ => self.emit_logged_out(None),
        }
    }

    fn emit_logged_out(&self, error: Option<String>) {
        self.sink.send(Event::Auth(AuthStatus::LoggedOut {
            client_id: self.config.client_id.clone(),
            redirect_uri: self.config.redirect_uri(),
            error,
        }));
    }

    fn begin_session(&mut self, session: Session, client_id: String) {
        let tokens = TokenManager::new(
            self.http.clone(),
            &self.config.accounts_base,
            client_id,
            session,
            Some(self.store.clone()),
        );
        let api = SpotifyClient::new(self.http.clone(), &self.config.api_base, Arc::new(tokens));
        self.api = Some(api.clone());
        // Data that a different account read can be incorrect for this account. The backend first
        // saves the data read until now, for the account that read it.
        self.save_cache();
        let cache_generation = {
            let mut cache = util::lock(&self.cache);
            cache.clear();
            cache.generation()
        };
        self.profile = SessionProfile::default();
        // Ask one time what plays now. After that, the player page reports it.
        self.startup_fetch = true;
        self.where_check = false;
        self.fetch_retries = FETCH_RETRIES;
        self.state_fetch_at = Some(Instant::now());
        self.queue_stale = true;
        self.set_elsewhere(None);
        // «Play in this app» is the default device, if nothing plays on a different device. The
        // backend decides this when Spotify tells what plays.
        self.web_autostart = self.config.web_player.is_some();
        let (volume_tx, volume_rx) = watch::channel(None);
        self.volume_tx = volume_tx;
        let volume_task = spawn_volume_task(
            api.clone(),
            self.sink.clone(),
            volume_rx,
            self.config.config_dir.clone(),
        );
        if let Some(previous) = self.volume_task.replace(volume_task) {
            previous.abort();
        }
        self.sink
            .send(Event::Auth(AuthStatus::LoggedIn { demo: false }));
        self.load_playlists();

        let sink = self.sink.clone();
        let profile = self.profile.clone();
        let cache = Arc::clone(&self.cache);
        let config_dir = self.config.config_dir.clone();
        tokio::spawn(async move {
            match user_profile(&api, &profile).await {
                Ok(profile) => {
                    // The answers that earlier sessions of this account saved.
                    let saved = tokio::task::spawn_blocking(move || {
                        response_cache::read_saved(&config_dir)
                    })
                    .await
                    .ok()
                    .flatten();
                    util::lock(&cache).adopt(
                        &profile.id,
                        cache_generation,
                        saved,
                        SystemTime::now(),
                    );
                    sink.send(Event::Profile(profile.clone()));
                }
                Err(error) => log::warn!("Could not load the user profile: {error}"),
            }
        });
    }

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Login { client_id } => self.start_login(client_id),
            Command::CancelLogin => {
                if let Some(task) = self.login_task.take() {
                    task.abort();
                }
                self.web_after_login = false;
                self.liked_after_login = None;
                if self.api.is_some() {
                    // The user logs in again (for a new permission): keep the session.
                    self.sink
                        .send(Event::Auth(AuthStatus::LoggedIn { demo: false }));
                } else {
                    self.emit_logged_out(None);
                }
            }
            Command::Logout => self.logout(None),
            Command::Search { query, offset } => self.search(query, offset),
            Command::LoadImage { url, max_px } => self.load_image(url, max_px),
            Command::CheckLiked { uris } => self.check_liked(uris).await,
            Command::SetLiked { track, liked } => self.set_liked(track, liked).await,
            Command::LoadAlbum { id } => self.load_album(id),
            Command::LoadArtist { id } => self.load_artist(id),
            Command::QueueAlbum { id, name, at } => self.fetch_album(id, name, at),
            Command::RefreshDevices => {
                self.devices_shown_until =
                    Some(Instant::now() + self.config.timings.devices_shown_for);
                self.refresh_devices().await;
            }
            Command::TransferPlayback { device_id } => {
                // From a different device, the backend does not know if the music plays there.
                // The user chose to listen where it goes.
                let play = self.playback.is_playing || self.elsewhere.is_some();
                self.transfer(device_id, play).await;
            }
            Command::StartWebPlayer => {
                // The user chose this: the music comes here (and plays).
                self.web_quiet = false;
                self.start_web_player().await;
            }
            Command::ReauthorizeWebPlayer => {
                self.web_after_login = true;
                self.start_login(None);
            }
            Command::AddToQueue { track } => self.add_to_queue(track).await,
            Command::EditQueue(edit) => self.edit_queue(edit).await,
            Command::ClearQueue => self.clear_queue().await,
            Command::QueuePlaylist { playlist, index } => self.fetch_playlist(playlist, index),
            Command::EditPlaylist { playlist, edit } => self.edit_playlist(playlist, edit),
            Command::FindInPlaylist {
                playlist,
                index,
                track,
            } => self.find_in_playlist(playlist, index, track),
            Command::FindAlbumInPlaylist { playlist, id, name } => {
                self.find_album_in_playlist(playlist, id, name);
            }
            Command::LoadPlaylistTracks { playlist } => self.load_playlist_tracks(playlist),
            Command::RefreshPlaylists => self.load_playlists(),
            Command::CreatePlaylist { name, folder } => self.create_playlist(name, folder),
            Command::RenamePlaylist { playlist, name } => self.rename_playlist(playlist, name),
            Command::EditLibrary(edit) => self.edit_library(&edit),
            Command::PlayFromQueue { position, uri } => self.play_from_queue(position, &uri).await,
            Command::SetVolume { percent } => self.set_volume(percent),
            Command::Resume => self.run_player_action(PlayerAction::Resume).await,
            Command::Pause => self.run_player_action(PlayerAction::Pause).await,
            Command::Next => self.run_player_action(PlayerAction::Next).await,
            Command::Previous => self.run_player_action(PlayerAction::Previous).await,
            Command::Seek { position_ms } => {
                self.run_player_action(PlayerAction::Seek(position_ms))
                    .await;
            }
            Command::PlayTrack { track } => {
                self.run_player_action(PlayerAction::Play {
                    tracks: vec![track],
                    index: 0,
                })
                .await;
            }
        }
    }

    // --- Session -------------------------------------------------------------

    fn start_login(&mut self, client_id: Option<String>) {
        if let Some(client_id) = client_id
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
            && self.config.client_id.as_deref() != Some(client_id.as_str())
            && let Err(error) = self.config.save_client_id(&client_id)
        {
            log::warn!("Could not save the Client ID: {error}");
            self.config.client_id = Some(client_id);
        }
        let Some(client_id) = self.config.client_id.clone() else {
            self.emit_logged_out(Some("Enter your Spotify app's Client ID.".into()));
            return;
        };
        if let Some(task) = self.login_task.take() {
            task.abort();
        }

        let oauth = OAuthConfig {
            client_id,
            accounts_base: self.config.accounts_base.clone(),
            redirect_port: self.config.redirect_port,
            bind_addr: self.config.callback_bind,
        };
        let http = self.http.clone();
        let sink = self.sink.clone();
        let results = self.login_tx.clone();
        self.login_task = Some(tokio::spawn(async move {
            let result = auth::login(&http, &oauth, |auth_url| {
                let redirect_uri = url::Url::parse(auth_url)
                    .ok()
                    .and_then(|url| {
                        url.query_pairs()
                            .find(|(key, _)| key == "redirect_uri")
                            .map(|(_, value)| value.into_owned())
                    })
                    .unwrap_or_default();
                sink.send(Event::Auth(AuthStatus::WaitingForBrowser {
                    auth_url: auth_url.to_owned(),
                    redirect_uri,
                }));
            })
            .await;
            let _ = results.send(result);
        }));
    }

    async fn finish_login(&mut self, result: LoginResult) {
        self.login_task = None;
        match result {
            Ok(session) => {
                if let Err(error) = self.store.save(&session) {
                    log::warn!("Could not save the session: {error}");
                    self.sink.toast(
                        ToastKind::Error,
                        "Could not save the session: you'll have to connect your account again next time.",
                    );
                }
                let client_id = self.config.client_id.clone().unwrap_or_default();
                self.begin_session(session, client_id);
                if std::mem::take(&mut self.web_after_login) {
                    self.start_web_player().await;
                }
                if let Some((track, liked)) = self.liked_after_login.take() {
                    self.change_liked(track, liked, true).await;
                }
            }
            Err(error) => {
                log::warn!("Login failed: {error}");
                self.web_after_login = false;
                self.liked_after_login = None;
                let message = capitalize(&error.to_string());
                if self.api.is_some() {
                    // The new login failed: the previous session still works.
                    self.sink
                        .send(Event::Auth(AuthStatus::LoggedIn { demo: false }));
                    self.sink.toast(ToastKind::Error, message);
                } else {
                    self.emit_logged_out(Some(message));
                }
            }
        }
    }

    fn logout(&mut self, error: Option<String>) {
        self.abort_tasks();
        self.stop_web_player();
        self.api = None;
        self.save_cache();
        util::lock(&self.cache).clear();
        if let Err(error) = self.store.clear() {
            log::warn!("Could not delete the session file: {error}");
        }
        self.history.clear();
        self.playback = PlaybackSnapshot::idle();
        self.upcoming.clear();
        self.expectation = None;
        self.dismissed = None;
        self.give_queue_back();
        self.queue_stale = false;
        self.state_fetch_at = None;
        self.startup_fetch = false;
        self.where_check = false;
        self.set_elsewhere(None);
        self.volume_hold = None;
        self.moved_to = None;
        self.devices.clear();
        self.web_autostart = false;
        self.web_pending = None;
        self.resume_pending = None;
        self.store_resume(None);
        self.mirrors = Mirrors::default();
        self.liked_after_login = None;
        self.sink.send(Event::Playback(self.playback.clone()));
        self.emit_queue();
        self.sink.send(Event::Devices(Vec::new()));
        self.emit_logged_out(error);
    }

    fn abort_tasks(&mut self) {
        for task in [
            self.login_task.take(),
            self.search_task.take(),
            self.album_task.take(),
            self.artist_task.take(),
            self.volume_task.take(),
            self.playlists_task.take(),
            self.playlist_chain.take(),
            self.liked_chain.take(),
        ]
        .into_iter()
        .flatten()
        {
            task.abort();
        }
        self.liked_checks.abort_all();
    }

    // --- What plays ------------------------------------------------------------

    /// Asks Spotify what plays, one time (see `state_fetch_at`), and uses the answer. Then it
    /// reads Spotify's queue if necessary.
    async fn fetch_state(&mut self) {
        self.state_fetch_at = None;
        let Some(api) = self.api.clone() else {
            return;
        };
        let now = Instant::now();
        if let Some(until) = self.rate_limited_until {
            if now < until {
                self.state_fetch_at = Some(until);
                return;
            }
            self.rate_limited_until = None;
        }
        let startup = self.startup_fetch;
        match api.playback_state().await {
            Ok(state) => {
                self.on_fetched(&api, state.unwrap_or_else(PlaybackSnapshot::idle))
                    .await;
            }
            Err(error) => self.on_fetch_error(error),
        }
        if startup
            && std::mem::take(&mut self.web_autostart)
            && self.web_status == WebPlayerStatus::Off
        {
            // Start it also when the music plays on a different device. Spotify lets the user
            // send the music here only after this app is a device.
            util::startup_mark("backend: preparing «Play in this app»");
            log::info!("Preparing «Play in this app»");
            self.web_quiet = true;
            self.start_web_player().await;
            if self.web_status != WebPlayerStatus::Starting {
                // This is not possible now (no permission yet…): there is nothing to wait for.
                self.web_quiet = false;
            }
        }
        self.read_queue_if_stale(&api).await;
    }

    /// Uses what Spotify tells about what plays. Spotify can show a change of device late. Thus,
    /// when an answer does not agree with the page, the backend asks again a moment later.
    async fn on_fetched(&mut self, api: &SpotifyClient, state: PlaybackSnapshot) {
        let startup = std::mem::take(&mut self.startup_fetch);
        if startup {
            util::startup_mark("backend: Spotify answered what plays");
        }
        let where_check = std::mem::take(&mut self.where_check);
        let here = self.is_web_player(state.device.as_ref());
        let other = state.device.clone().filter(|_| !here);
        let not_yet = (here && where_check) || (other.is_some() && self.moving_here());
        if not_yet && self.fetch_retries > 0 {
            self.fetch_retries -= 1;
            self.where_check = where_check;
            self.state_fetch_at = Some(Instant::now() + self.config.timings.where_retry_after);
            return;
        }
        if let Some(device) = other
            && (state.is_playing || !startup)
        {
            self.go_elsewhere(device);
            return;
        }
        if here {
            self.come_back_here();
        }
        self.take_in(api, state).await;
    }

    /// A request for what plays failed. The backend sends it again a few times (see
    /// [`FETCH_RETRIES`]), or one time when the wait for a 429 ends.
    fn on_fetch_error(&mut self, error: ApiError) {
        let now = Instant::now();
        match error {
            error if error.is_auth_failure() => self.report(error),
            ApiError::RateLimited(wait) => {
                log::warn!("Could not ask Spotify what plays: {error}");
                self.rate_limited_until = Some(now + wait);
                self.state_fetch_at = Some(now + wait);
            }
            error => {
                log::warn!("Could not ask Spotify what plays: {error}");
                if self.fetch_retries > 0 {
                    self.fetch_retries -= 1;
                    self.state_fetch_at = Some(now + self.config.timings.fetch_retry_after);
                }
            }
        }
    }

    /// Reads Spotify's queue when it can be out of date (see `queue_stale`), if the music does
    /// not play on a different device.
    async fn read_queue_if_stale(&mut self, api: &SpotifyClient) {
        if self.elsewhere.is_some() {
            return;
        }
        if self.queue_owned {
            // Melodia knows its own list, so it does not read it.
            self.queue_stale = false;
        } else if self.queue_stale && self.playback.track.is_some() {
            // When nothing plays, Spotify has no queue to read.
            self.queue_stale = false;
            self.refresh_queue(api).await;
        }
    }

    /// Uses the state of the music (from the player page, or from Spotify) and does what is
    /// necessary.
    async fn take_in(&mut self, api: &SpotifyClient, state: PlaybackSnapshot) {
        match self.on_playback(state) {
            Some(TrackEnd::FinishQueue(started)) => self.finish_queue(api, started).await,
            Some(TrackEnd::PlayNext) => self.run_player_action(PlayerAction::Next).await,
            None => {}
        }
        self.go_back_to_saved_position().await;
        self.remember_position(Instant::now());
    }

    /// The music plays on `device`, not in this app. The backend clears this app's queue, and
    /// the UI hides the player until the music plays here again. Melodia does not control the
    /// music on that device.
    fn go_elsewhere(&mut self, device: Device) {
        if self
            .elsewhere
            .as_ref()
            .is_some_and(|known| *known == device)
        {
            return;
        }
        log::info!(
            "The music plays on «{}»: this app's queue is dropped",
            device.name
        );
        self.expectation = None;
        self.dismissed = None;
        self.moving_here_until = None;
        self.resume_pending = None;
        self.give_queue_back();
        self.queue_stale = false;
        self.history.clear();
        self.upcoming.clear();
        self.page_next = None;
        self.page_next_mismatch = None;
        self.playback = PlaybackSnapshot::idle();
        // That device tells Spotify the position of the music.
        self.store_resume(None);
        self.set_elsewhere(Some(device));
        self.sink.send(Event::Playback(self.playback.clone()));
        self.emit_queue();
    }

    /// The music plays in this app again (it was on a different device). The backend reads
    /// Spotify's queue again, and the UI shows the player again.
    fn come_back_here(&mut self) {
        if self.elsewhere.is_none() {
            return;
        }
        log::info!("The music plays in this app again: reading the queue");
        self.set_elsewhere(None);
        self.give_queue_back();
    }

    fn set_elsewhere(&mut self, device: Option<Device>) {
        if self.elsewhere != device {
            self.elsewhere.clone_from(&device);
            self.sink.send(Event::PlayingElsewhere(device));
        }
    }

    /// The page tells that the music does not play here. The backend asks Spotify where it plays.
    /// It does not ask if the music just moved here (the report can be from before), or if it
    /// already knows that the music is on a different device.
    fn music_not_here(&mut self) {
        if self.elsewhere.is_some() || self.moving_here() {
            return;
        }
        self.where_check = true;
        self.fetch_state_at(Instant::now() + self.config.timings.state_after_page_report);
    }

    /// Runs at each `Timings::tick`. It saves the position of the music that plays here, and
    /// stops a web player that does not start. It asks Spotify nothing.
    fn tick(&mut self) {
        let now = Instant::now();
        self.tick_at = now + self.config.timings.tick;
        self.check_web_player_timeout();
        self.remember_position(now);
    }

    /// The music plays in this app. Its player page reports each play, pause, seek and change of
    /// track when it occurs (`FromPage::State`). Thus the backend does not ask Spotify.
    fn page_reports_playback(&self) -> bool {
        self.web_open && self.is_web_player(self.playback.device.as_ref())
    }

    /// The music just moved to this app, and Spotify possibly does not show it yet.
    fn moving_here(&self) -> bool {
        self.moving_here_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// The music just moved to this app. The page should tell this soon. If it does not, the
    /// backend asks Spotify at `Timings::here_check_after`.
    fn start_moving_here(&mut self) {
        let now = Instant::now();
        self.moving_here_until = Some(now + self.config.timings.moving_here_window);
        self.fetch_state_at(now + self.config.timings.here_check_after);
    }

    /// Asks Spotify what plays at `at` (or before), one time.
    fn fetch_state_at(&mut self, at: Instant) {
        self.fetch_retries = FETCH_RETRIES;
        self.state_fetch_at = Some(self.state_fetch_at.map_or(at, |t| t.min(at)));
    }

    /// After a command, the backend soon asks Spotify if it applied the command. While the player
    /// page reports the music, the page confirms it. The backend asks Spotify only if the page
    /// did not confirm it when the prediction expires.
    fn confirm_command(&mut self) {
        let now = Instant::now();
        let at = if self.page_reports_playback() || self.moving_here() {
            self.expectation
                .as_ref()
                .map_or(now + self.config.timings.state_after_command, |e| {
                    e.deadline
                })
        } else {
            now + self.config.timings.state_after_command
        };
        self.fetch_state_at(at);
    }

    /// Uses what Spotify reports. It returns what to do if a track just ended and Spotify did not
    /// continue as Melodia's queue tells.
    fn on_playback(&mut self, mut state: PlaybackSnapshot) -> Option<TrackEnd> {
        let now = Instant::now();
        if let Some(expectation) = &self.expectation {
            if !expectation.accepts(&state, now) {
                // Spotify did not apply the last command yet: keep the prediction.
                return None;
            }
            self.expectation = None;
        }
        self.hold_device(&mut state, now);
        self.hold_volume(&mut state, now);
        if let Some(uri) = &self.dismissed {
            if !state.is_playing && state.track_uri() == Some(uri.as_str()) {
                state.track = None;
                state.progress_ms = 0;
            } else {
                // Something plays again (possibly from a different device).
                self.dismissed = None;
            }
        }

        let track_changed = state.track_uri() != self.playback.track_uri();
        if self.list_ended_before_the_queue(&state, track_changed, now) {
            log::info!(
                "Spotify's list ended: Melodia starts «{}» itself",
                self.upcoming[0].name
            );
            return Some(TrackEnd::PlayNext);
        }
        // After a moment with nothing playing (the device went away), the same track that comes
        // back is not a change of position in the queue.
        let same_as_before = self.history.current().map(|t| t.uri.as_str()) == state.track_uri();
        let mut history_changed = false;
        let mut queue_moved = false;
        match state.track.clone() {
            Some(reported)
                if track_changed
                    && !same_as_before
                    && self.queue_owned
                    && self.upcoming.is_empty()
                    && self.ran_to_the_end(now) =>
            {
                // The last track of the queue ended and Spotify continued by itself (a track left
                // in its list, autoplay…).
                log::info!("The queue ended: «{}» will not play", reported.name);
                self.playback = state;
                return Some(TrackEnd::FinishQueue(reported.uri));
            }
            Some(reported) if track_changed && !same_as_before && self.queue_owned => {
                let step = follow(&self.history, &self.upcoming, &mut self.plan, &reported.uri);
                if step == Follow::Lost {
                    log::info!("Something else plays now: Spotify's queue is shown again");
                    self.give_queue_back();
                } else {
                    queue_moved = true;
                }
                apply_follow(step, &mut self.history, &mut self.upcoming, &reported, now);
            }
            _ => {
                history_changed = self.history.observe(state.track.as_ref(), now);
                // The track of the queue shows again (after a restart, or after a moment with
                // nothing playing): the queue also shows it.
                queue_moved = track_changed && same_as_before;
            }
        }
        self.playback = state;
        if !self.queue_owned && self.playback.track.is_none() && !self.upcoming.is_empty() {
            // Nothing plays: Spotify's queue is gone, and the backend does not read it again until
            // something plays.
            self.upcoming.clear();
            queue_moved = true;
        }
        self.sink.send(Event::Playback(self.playback.clone()));
        if queue_moved {
            self.emit_queue();
        }

        if self.queue_owned {
            self.schedule_queue_check();
        } else if track_changed || history_changed {
            // The backend reads Spotify's queue again, if the page does not tell what comes next
            // (see `follow_queue_from_page`).
            self.queue_stale = true;
        }
        None
    }

    /// The track that played was the last track of Spotify's list, and it ended. But Melodia's
    /// queue has more tracks. Spotify stopped (or continued with its own music, for example
    /// autoplay) and did not play Melodia's next track. A track added to Spotify's queue does not
    /// play after the end of its list.
    fn list_ended_before_the_queue(
        &self,
        state: &PlaybackSnapshot,
        track_changed: bool,
        now: Instant,
    ) -> bool {
        let Some(next) = self.upcoming.first() else {
            return false;
        };
        let reported = state.track_uri();
        self.queue_owned
            && self.plan.next().is_none()
            && self.ran_to_the_end(now)
            && (!state.is_playing || track_changed)
            && reported != Some(next.uri.as_str())
                // The user pushed "previous" on a different device just before the end.
            && reported != self.history.last().map(|t| t.uri.as_str())
    }

    /// The track that showed as playing got to its end (the user did not skip it).
    fn ran_to_the_end(&self, now: Instant) -> bool {
        self.playback.is_playing
            && self.playback.track.as_ref().is_some_and(|track| {
                self.playback.progress_at(now) + END_SLACK_MS >= track.duration_ms
            })
    }

    /// Melodia's queue has no more tracks: Spotify must not continue with other music. The
    /// backend pauses Spotify on `loaded` (its current track), which stays hidden. The last track
    /// of the queue becomes a previous track.
    async fn finish_queue(&mut self, api: &SpotifyClient, loaded: String) {
        let now = Instant::now();
        // Reports can tell for a moment that it still plays: wait for the pause.
        self.expectation = Some(Expectation::for_action(
            &PlayerAction::Pause,
            &self.playback,
            now,
            self.config.timings.expectation_ttl,
        ));
        if !self.control_on_page(&Request::Pause)
            && let Err(error) = api.pause().await
        {
            log::warn!("Could not pause Spotify at the end of the queue: {error}");
            self.expectation = None;
        }
        self.dismissed = Some(loaded);
        self.plan = SpotifyPlan::default();
        self.queue_check_at = None;
        self.history.finish();
        self.upcoming.clear();
        self.playback.track = None;
        self.playback.is_playing = false;
        self.playback.progress_ms = 0;
        self.playback.measured_at = now;
        self.confirm_command();
        self.sink.send(Event::Playback(self.playback.clone()));
        self.emit_queue();
    }

    /// Immediately after a transfer, Spotify can still show the previous device. The UI shows the
    /// new device until Spotify also shows it.
    fn hold_device(&mut self, state: &mut PlaybackSnapshot, now: Instant) {
        let Some((target, until)) = &self.moved_to else {
            return;
        };
        match &mut state.device {
            Some(device) if device.id == target.id => self.moved_to = None,
            Some(device) if now < *until => *device = target.clone(),
            Some(_) => self.moved_to = None,
            None => {}
        }
    }

    /// The device list, with only the new device of the music marked as active. Spotify can still
    /// show the previous device.
    fn mark_moved_to(&self, devices: &mut [Device]) {
        let Some((target, until)) = &self.moved_to else {
            return;
        };
        if Instant::now() < *until && devices.iter().any(|d| d.id == target.id) {
            for device in devices {
                device.is_active = device.id == target.id;
            }
        }
    }

    /// While a new volume goes to Spotify, its answers can still report the old volume. Keep the
    /// new value on the screen for a moment.
    fn hold_volume(&mut self, state: &mut PlaybackSnapshot, now: Instant) {
        if self.web_open
            && self.is_web_player(state.device.as_ref())
            && let (Some(percent), Some(device)) = (self.volume, &mut state.device)
        {
            // Only Melodia sets the page's volume, and the SDK applies it. Spotify can know it
            // late, or never report it.
            device.volume_percent = Some(percent);
            self.volume_hold = None;
            return;
        }
        let Some((percent, until)) = self.volume_hold else {
            return;
        };
        let reported = state.device.as_ref().and_then(|d| d.volume_percent);
        if now < until && reported != Some(percent) {
            if let Some(device) = &mut state.device {
                device.volume_percent = Some(percent);
            }
        } else {
            self.volume_hold = None;
        }
    }

    async fn refresh_queue(&mut self, api: &SpotifyClient) {
        match api.queue().await {
            Ok(next) => {
                self.upcoming = without_padding(next, self.playback.track_uri());
                self.emit_queue();
            }
            Err(error) => log::warn!("Could not load the queue: {error}"),
        }
    }

    /// The page's report of what Spotify plays after the current track, if the report is about
    /// that track and the page reports the music.
    fn page_next_now(&self) -> Option<&[String]> {
        let (current, next) = self.page_next.as_ref()?;
        (self.page_reports_playback() && self.playback.track_uri() == Some(current.as_str()))
            .then_some(next.as_slice())
    }

    /// While Melodia shows Spotify's queue, the backend reads the queue again only if the page's
    /// next tracks are not the tracks shown. `before` is the track shown before this report.
    fn follow_queue_from_page(&mut self, before: Option<Track>) {
        if self.queue_owned {
            return;
        }
        let Some(next) = self.page_next_now().map(<[String]>::to_vec) else {
            return; // the page did not say, or a prediction is still shown
        };
        let current = self.playback.track_uri().unwrap_or_default().to_owned();
        let mut upcoming = self.upcoming.clone();
        if let Some(before) = before.filter(|before| before.uri != current) {
            if upcoming.first().is_some_and(|track| track.uri == current) {
                upcoming.remove(0);
            } else if next.first() == Some(&before.uri) {
                upcoming.insert(0, before);
            }
        }
        if agrees_with_page(&upcoming, &next) {
            self.page_next_mismatch = None;
            self.queue_stale = false;
            if upcoming != self.upcoming {
                self.upcoming = upcoming;
                self.emit_queue();
            }
        } else if self.page_next_mismatch.as_ref() != Some(&next) {
            // The queue changed on a different device: read it now. If Spotify's queue and the
            // page still do not agree, the backend does not compare them again for these tracks.
            self.page_next_mismatch = Some(next);
            self.queue_stale = true;
        }
    }

    fn emit_queue(&self) {
        self.sink.send(Event::Queue(QueueSnapshot {
            previous: self.history.entries().to_vec(),
            current: self.playback.track.clone(),
            next: self.upcoming.clone(),
            edits: self.queue_edits,
            next_locked: self.next_locked(),
        }));
    }

    /// Shows the queue that the app saved when it last closed (see `docs/queue.md`, «Saved
    /// queue»), until Spotify tells what plays.
    fn restore_queue(&mut self) {
        let Some(saved) = self.saved_queue.clone() else {
            return;
        };
        log::info!("Restoring the queue saved when the app closed");
        self.history = PlayHistory::restored(saved.previous, saved.current);
        self.upcoming = saved.next;
        if saved.owned {
            self.queue_owned = true;
            self.plan = SpotifyPlan::restored();
        }
        self.emit_queue();
    }

    /// Gives the queue to the queue task to save it, if it changed. When the user is logged out,
    /// there is no queue: the task deletes the file.
    fn save_queue(&mut self) {
        let queue = self.api.as_ref().and_then(|_| {
            SavedQueue::new(
                self.history.entries(),
                self.history.current(),
                &self.upcoming,
                self.queue_owned,
            )
        });
        if queue != self.saved_queue {
            self.queue_tx.send_replace(queue.clone());
            self.saved_queue = queue;
        }
    }

    /// The app already sent the first upcoming track to Spotify's queue.
    fn next_locked(&self) -> bool {
        self.queue_owned
            && self.plan.queued().is_some()
            && self.plan.queued() == self.upcoming.first().map(|t| t.uri.as_str())
    }

    /// After a transfer, the backend reads the device list only if the picker shows it. If not,
    /// it reads the list when the picker opens.
    async fn refresh_shown_devices(&mut self) {
        if self
            .devices_shown_until
            .is_some_and(|until| Instant::now() < until)
        {
            self.refresh_devices().await;
        }
    }

    async fn refresh_devices(&mut self) -> Vec<Device> {
        let Some(api) = self.api.clone() else {
            return Vec::new();
        };
        match api.devices().await {
            Ok(mut devices) => {
                self.mark_moved_to(&mut devices);
                self.devices.clone_from(&devices);
                self.sink.send(Event::Devices(devices.clone()));
                devices
            }
            Err(error) => {
                log::warn!("Could not load devices: {error}");
                Vec::new()
            }
        }
    }

    // --- Player commands ---------------------------------------------------------

    async fn run_player_action(&mut self, action: PlayerAction) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let starts_here = matches!(action, PlayerAction::Play { .. } | PlayerAction::Jump(_));
        if self.elsewhere.is_some() && !starts_here {
            // Melodia does not control the music of another device.
            log::info!("The music plays elsewhere: {action:?} is not sent");
            return;
        }
        if self.dismissed.is_some() {
            if !matches!(action, PlayerAction::Play { .. } | PlayerAction::Jump(_)) {
                // The user cleared the queue: the hidden track must not resume.
                return;
            }
            self.dismissed = None;
        }
        if action == PlayerAction::Next && self.queue_owned && self.upcoming.is_empty() {
            // Nothing comes after the last track: the queue stops here.
            if let Some(current) = self.playback.track.as_ref().map(|t| t.uri.clone()) {
                self.finish_queue(&api, current).await;
            }
            return;
        }
        let now = Instant::now();
        let expectation = Expectation::for_action(
            &action,
            &self.playback,
            now,
            self.config.timings.expectation_ttl,
        );
        let prediction = predict(&self.playback, &self.history, &self.upcoming, &action, now);
        if prediction.is_none()
            && matches!(action, PlayerAction::Play { .. } | PlayerAction::Jump(_))
        {
            return; // that track is not there any more
        }
        // Spotify plays Melodia's next track by itself (the next track of its list, or a track
        // already in its queue). Thus "next" can continue normally.
        let spotify_agrees =
            self.queue_owned && self.upcoming.first().map(|t| t.uri.as_str()) == self.plan.next();
        let goes_on = spotify_agrees
            && matches!(
                action,
                PlayerAction::Next | PlayerAction::Jump(QueuePosition::Next(0))
            );
        // A resume is a good moment to give Spotify the correct list again. Nothing stops (if
        // Spotify loses nothing that it knows).
        let resume_with_order = action == PlayerAction::Resume
            && self.queue_owned
            && self.plan.is_complete()
            && self.plan.queued().is_none()
            && !self.plan.matches(&self.upcoming);
        let resume_at = self.playback.progress_at(now);

        let changes_track = prediction.as_ref().is_some_and(|p| p.queue.is_some());
        if let Some(prediction) = prediction {
            if let Some(change) = prediction.queue {
                self.history = change.history;
                self.upcoming = change.next;
            }
            self.playback = prediction.playback;
            self.sink.send(Event::Playback(self.playback.clone()));
        } else if action == PlayerAction::Previous {
            self.history.hint_back(now);
        }

        let request = if goes_on {
            if let Some(current) = &self.playback.track {
                self.plan.started(&current.uri);
            }
            Request::Next
        } else {
            let send_order = resume_with_order
                || (changes_track && (self.queue_owned || native_request(&action).is_none()));
            match (native_request(&action), self.playback.track.clone()) {
                (Some(native), _) if !send_order => native,
                (_, Some(current)) => {
                    self.order_from(&current, resume_with_order.then_some(resume_at))
                }
                (_, None) => return,
            }
        };
        if changes_track {
            self.emit_queue();
        }
        if matches!(request, Request::Order { .. }) {
            self.turn_off_shuffle(&api).await;
        }
        self.expectation = Some(expectation);
        // What comes next can change: wait for the page to tell it again.
        self.page_next = None;

        if resume_with_order && self.page_reports_playback() {
            // The command restarts the track through the Web API. The page fades it in when it
            // plays, as for a resume.
            if let Some(host) = &self.config.web_player {
                host.send(HostRequest::Control(PageControl::FadeIn));
            }
        }

        let mut sent_here = false;
        let mut result = if self.control_on_page(&request) {
            Ok(())
        } else {
            let device = self.web_device_to_start(&request);
            sent_here = device.is_some();
            if self.elsewhere.is_some() {
                // The user played it from here: the music comes back to this app.
                self.set_elsewhere(None);
            }
            send_request(&api, &request, device.as_deref()).await
        };
        if result == Err(ApiError::NoActiveDevice)
            && matches!(request, Request::Resume | Request::Order { .. })
        {
            // No device is active: start the music on an available device.
            if self.web_status == WebPlayerStatus::Starting {
                // This app becomes a device soon: play there when it is ready.
                self.web_pending = Some(request.clone());
                result = Ok(());
            } else if let Some((id, _)) = self.fallback_device().await {
                result = send_request(&api, &request, Some(&id)).await;
            }
        }

        match result {
            Ok(()) => {
                if sent_here {
                    self.start_moving_here();
                }
                self.confirm_command();
                self.schedule_queue_check();
            }
            Err(error) => {
                // Remove the prediction and show the real state again.
                self.expectation = None;
                self.fetch_state_at(Instant::now());
                if matches!(request, Request::Order { .. }) {
                    self.give_queue_back();
                }
                self.queue_stale = true;
                self.report(error);
            }
        }
    }

    /// The request that plays `current` (from `position_ms`, or from the start), followed by the
    /// upcoming tracks, in Melodia's order. After this request, Melodia owns the queue.
    ///
    /// The app cannot remove a track that it already sent to Spotify's queue. That track plays
    /// immediately after `current`, so the queue shows (and keeps) it as the next track.
    fn order_from(&mut self, current: &Track, position_ms: Option<u64>) -> Request {
        let queued = self.plan.queued().map(str::to_owned);
        if let Some(uri) = queued.as_ref().filter(|uri| **uri != current.uri) {
            let index = self.upcoming.iter().position(|t| t.uri == *uri);
            let track = match index {
                Some(index) => Some(self.upcoming.remove(index)),
                None => self.history.forget_last(uri),
            };
            if let Some(track) = track {
                self.upcoming.insert(0, track);
            }
        }
        // Spotify plays the queued track by itself, so the request does not send it again.
        let skip = usize::from(
            queued.is_some() && self.upcoming.first().map(|t| &t.uri) == queued.as_ref(),
        );
        let uris = queue_uris(current, &self.upcoming[skip..]);
        self.queue_owned = true;
        self.plan = SpotifyPlan::sent(&uris, queued);
        Request::Order { uris, position_ms }
    }

    /// The web player's device, for a request that starts the music while the music is not in
    /// this app and this app's player is ready. Melodia never starts or resumes the music on a
    /// different device.
    fn web_device_to_start(&self, request: &Request) -> Option<String> {
        if self.is_web_player(self.playback.device.as_ref())
            || !matches!(request, Request::Resume | Request::Order { .. })
        {
            return None;
        }
        match &self.web_status {
            WebPlayerStatus::Ready { device_id } => Some(device_id.clone()),
            _ => None,
        }
    }

    /// A device for the music when no device is active: this app if it is ready, or else the best
    /// device that Spotify lists.
    async fn fallback_device(&mut self) -> Option<(String, String)> {
        if let WebPlayerStatus::Ready { device_id } = &self.web_status {
            return Some((device_id.clone(), "this app".to_owned()));
        }
        let devices = self.refresh_devices().await;
        preferred_device(&devices).map(|device| (device.id.clone(), device.name.clone()))
    }

    // --- Queue -------------------------------------------------------------------

    /// Adds `track` at the end of the queue, or plays it if nothing plays.
    async fn add_to_queue(&mut self, track: Track) {
        if self.api.is_none() {
            return;
        }
        if self.playback.track.is_none() {
            self.run_player_action(PlayerAction::Play {
                tracks: vec![track],
                index: 0,
            })
            .await;
            return;
        }
        let edit = QueueEdit::Insert {
            index: self.upcoming.len(),
            track,
        };
        if !self.change_queue(&edit).await {
            self.toast_queue_full();
        }
        self.emit_queue();
    }

    async fn edit_queue(&mut self, edit: QueueEdit) {
        // Count it also when the backend cannot apply it, so the UI removes its optimistic
        // version of the edit.
        self.queue_edits += 1;
        if self.api.is_none() {
            self.emit_queue();
            return;
        }
        if let QueueEdit::Insert { track, .. } = &edit
            && self.playback.track.is_none()
        {
            // Nothing plays: a track that the user drops on the empty queue starts.
            self.run_player_action(PlayerAction::Play {
                tracks: vec![track.clone()],
                index: 0,
            })
            .await;
        } else if !self.change_queue(&edit).await && matches!(edit, QueueEdit::Insert { .. }) {
            self.toast_queue_full();
        }
        self.emit_queue();
    }

    /// Empties the queue and stops the music. The Web API cannot stop Spotify or empty its queue.
    /// Thus the backend pauses Spotify, and hides its track until something plays again.
    async fn clear_queue(&mut self) {
        self.queue_edits += 1;
        if self.api.is_some() && self.playback.is_playing {
            self.run_player_action(PlayerAction::Pause).await;
        }
        if !self.queue_owned {
            // Spotify's list must not come back with the next refresh.
            self.queue_owned = true;
            self.plan = SpotifyPlan::observed(&self.upcoming);
        }
        self.queue_check_at = None;
        if let Some(track) = self.playback.track.take() {
            self.dismissed = Some(track.uri);
        }
        self.playback.is_playing = false;
        self.playback.progress_ms = 0;
        self.playback.measured_at = Instant::now();
        self.history.clear();
        self.upcoming.clear();
        self.sink.send(Event::Playback(self.playback.clone()));
        self.emit_queue();
    }

    /// Changes the order of the queue (also of the previous tracks, and which tracks are
    /// previous). The backend sends nothing to Spotify now, so the music does not stop. Spotify
    /// follows the new order through [`Self::check_queue`]. A track that the app already sent to
    /// Spotify's queue stays first.
    async fn change_queue(&mut self, edit: &QueueEdit) -> bool {
        if !self.queue_owned {
            // After this, Melodia decides. Spotify still plays its list.
            self.queue_owned = true;
            self.plan = SpotifyPlan::observed(&self.upcoming);
        }
        let fixed = usize::from(self.next_locked());
        let mut previous = self.history.entries().to_vec();
        let current = self.playback.track.clone();
        if !edit.apply(&mut previous, current.as_ref(), &mut self.upcoming, fixed) {
            return false;
        }
        self.history.set_entries(previous);
        self.schedule_queue_check();
        if self.queue_check_at.is_some_and(|at| at <= Instant::now()) {
            // The track is near its end: decide Spotify's next track now.
            self.check_queue().await;
        }
        true
    }

    /// The time to make sure that Spotify's next track is Melodia's: [`ENQUEUE_LEAD_MS`] before
    /// the current track ends.
    fn schedule_queue_check(&mut self) {
        self.queue_check_at = None;
        if !self.queue_owned || !self.playback.is_playing {
            return;
        }
        let Some(track) = &self.playback.track else {
            return;
        };
        let now = Instant::now();
        let remaining = track
            .duration_ms
            .saturating_sub(self.playback.progress_at(now));
        let wait = remaining.saturating_sub(ENQUEUE_LEAD_MS);
        self.queue_check_at = Some(now + Duration::from_millis(wait));
    }

    /// Runs shortly before the current track ends. If Spotify is about to play a track that is
    /// not Melodia's next track, it adds Melodia's next track to Spotify's queue. That track plays
    /// next, and the music never stops.
    async fn check_queue(&mut self) {
        self.queue_check_at = None;
        if !self.queue_owned || !self.playback.is_playing {
            return;
        }
        let Some(api) = self.api.clone() else {
            return;
        };
        if self.expectation.is_some() {
            // A command is not yet complete: look again in a moment.
            self.queue_check_at = Some(Instant::now() + self.config.timings.state_after_command);
            return;
        }
        let Some(next) = self.upcoming.first().cloned() else {
            // The queue stops here. The backend pauses the next track that Spotify starts (see
            // `Self::on_playback`).
            return;
        };
        if self.plan.queued().is_some() {
            return;
        }
        // If the page knows what Spotify plays next, that decides, whatever the plan expects. If
        // not, the plan decides.
        let spotify_next = self
            .page_next_now()
            .and_then(<[String]>::first)
            .map(String::as_str)
            .or(self.plan.next());
        if spotify_next == Some(next.uri.as_str()) {
            return;
        }
        if self.plan.next().is_none() {
            // Spotify's list ends with this track. Spotify stops there and does not play the
            // tracks added to its queue. Melodia starts the next track itself (see
            // `Self::on_playback`).
            return;
        }
        match api.add_to_queue(&next.uri).await {
            Ok(()) => {
                log::info!(
                    "Queued «{}» in Spotify to follow the queue's order",
                    next.name
                );
                self.plan.mark_queued(next.uri);
                self.emit_queue();
            }
            // Spotify continues with its own next track. The queue follows it.
            Err(error) => log::warn!("Could not queue the next track in Spotify: {error}"),
        }
    }

    async fn play_from_queue(&mut self, position: QueuePosition, uri: &str) {
        match position.resolve(self.history.entries(), &self.upcoming, uri) {
            Some(position) => self.run_player_action(PlayerAction::Jump(position)).await,
            // The track already played or left the queue: show the queue as it is.
            None => self.emit_queue(),
        }
    }

    fn toast_queue_full(&self) {
        self.sink.toast(ToastKind::Error, super::queue_full());
    }

    /// The UI shows Spotify's queue again.
    fn give_queue_back(&mut self) {
        self.queue_owned = false;
        self.plan = SpotifyPlan::default();
        self.queue_check_at = None;
        self.queue_stale = true;
    }

    /// In shuffle mode, Spotify plays Melodia's list in a random order.
    async fn turn_off_shuffle(&mut self, api: &SpotifyClient) {
        if !self.playback.shuffle {
            return;
        }
        match api.set_shuffle(false).await {
            Ok(()) => self.playback.shuffle = false,
            Err(error) => log::warn!("Could not turn shuffle off: {error}"),
        }
    }

    // --- Playlists ---------------------------------------------------------------

    /// Loads the user's playlists in the background.
    fn load_playlists(&mut self) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if let Some(task) = self.playlists_task.take() {
            task.abort();
        }
        let sink = self.sink.clone();
        let order = self.playlist_order.clone();
        let expected = self.expected.clone();
        let profile = self.profile.clone();
        self.playlists_task = Some(tokio::spawn(async move {
            let status = if has_playlist_scopes(&api.tokens().session().await.scope) {
                match fetch_playlists(&api, &profile, &expected).await {
                    Ok((user, playlists)) => {
                        let arranged = tokio::task::spawn_blocking(move || {
                            let mut order = util::lock(&order);
                            let playlists = order.arrange(&user, playlists);
                            (playlists, order.library().clone(), order.edits())
                        })
                        .await;
                        match arranged {
                            Ok((playlists, library, edits)) => {
                                sink.send(Event::Library { library, edits });
                                PlaylistsStatus::Loaded(playlists)
                            }
                            Err(error) => {
                                log::error!("Could not arrange the playlists: {error}");
                                PlaylistsStatus::Failed(
                                    "Could not reorder your playlists.".to_owned(),
                                )
                            }
                        }
                    }
                    Err(error) => {
                        log::warn!("Could not load the playlists: {error}");
                        PlaylistsStatus::Failed(error.user_message())
                    }
                }
            } else {
                PlaylistsStatus::NeedsReauth
            };
            sink.send(Event::Playlists(status));
        }));
    }

    /// Keeps the user's change to the folders or to the order of the playlists (Spotify cannot
    /// keep them). The UI already shows the change.
    fn edit_library(&self, edit: &LibraryEdit) {
        if !util::lock(&self.playlist_order).apply(edit) {
            return;
        }
        // Each save writes the latest order, so the order of the saves is not important.
        let order = self.playlist_order.clone();
        tokio::task::spawn_blocking(move || util::lock(&order).save());
    }

    /// Creates an empty playlist with the name `name`, to show in the folder `folder`. The
    /// backend loads the list again when the playlist exists.
    fn create_playlist(&mut self, name: String, folder: Option<String>) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let jobs = self.playlist_tx.clone();
        self.chain_playlist_task(async move {
            let result = api.create_playlist(&name).await;
            let _ = jobs.send(PlaylistJob::Created {
                name,
                folder,
                result,
            });
        });
    }

    /// Changes the name of `playlist` to `name`. The UI already shows the new name. If the change
    /// fails, the backend loads the playlists again.
    fn rename_playlist(&mut self, playlist: Playlist, name: String) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let jobs = self.playlist_tx.clone();
        self.chain_playlist_task(async move {
            let result = api.rename_playlist(&playlist.id, &name).await;
            let _ = jobs.send(PlaylistJob::Renamed {
                playlist,
                name,
                result,
            });
        });
    }

    /// Loads the tracks of `playlist` to put them in the queue at `index`.
    fn fetch_playlist(&self, playlist: Playlist, index: usize) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let jobs = self.playlist_tx.clone();
        tokio::spawn(async move {
            let result = api
                .playlist_tracks(&playlist.id, QueueSnapshot::MAX_NEXT)
                .await;
            let _ = jobs.send(PlaylistJob::Fetched {
                playlist,
                index,
                result,
            });
        });
    }

    /// Reads the album `id` (with the name `name`) to put its tracks in the queue at `at`.
    fn fetch_album(&self, id: String, name: String, at: QueueAt) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let jobs = self.playlist_tx.clone();
        if let Some(album) = util::lock(&self.cache).album(&id, SystemTime::now()) {
            let _ = jobs.send(PlaylistJob::AlbumFetched {
                name,
                at,
                result: Ok(album.tracks),
            });
            return;
        }
        let cache = Arc::clone(&self.cache);
        tokio::spawn(async move {
            let result = api.album(&id).await.map(|album| {
                let tracks = album.tracks.clone();
                util::lock(&cache).store_album(album, SystemTime::now());
                tracks
            });
            let _ = jobs.send(PlaylistJob::AlbumFetched { name, at, result });
        });
    }

    /// Runs `work` after the reads and edits of playlists that the backend sent before.
    fn chain_playlist_task(&mut self, work: impl Future<Output = ()> + Send + 'static) {
        let previous = self.playlist_chain.take();
        self.playlist_chain = Some(tokio::spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            work.await;
        }));
    }

    /// Loads the tracks of `playlist` to show them in the playlist panel, if the backend did not
    /// read them recently (see [`ResponseCache::playlist_tracks`]).
    fn load_playlist_tracks(&mut self, playlist: Playlist) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let edits = self.playlist_edits;
        let cached = util::lock(&self.cache).playlist_tracks(&playlist, SystemTime::now());
        if let Some((tracks, total)) = cached {
            self.sink.send(Event::PlaylistTracks {
                id: playlist.id,
                tracks,
                total,
                edits,
            });
            return;
        }
        let sink = self.sink.clone();
        let mirrors = self.mirrors.clone();
        let cache = Arc::clone(&self.cache);
        self.chain_playlist_task(async move {
            let id = playlist.id.clone();
            match read_playlist(&api, &id).await {
                Ok((mirror, tracks, total)) => {
                    let snapshot = mirror.snapshot.clone();
                    mirrors.lock().await.insert(id.clone(), mirror);
                    util::lock(&cache).store_playlist_tracks(
                        &id,
                        tracks.clone(),
                        total,
                        snapshot,
                        edits,
                        SystemTime::now(),
                    );
                    sink.send(Event::PlaylistTracks {
                        id,
                        tracks,
                        total,
                        edits,
                    });
                }
                Err(error) => {
                    log::warn!("Could not load the tracks of «{}»: {error}", playlist.name);
                    let message = match error {
                        ApiError::Status { status: 403, .. } => {
                            super::PLAYLIST_NOT_READABLE.to_owned()
                        }
                        error => error.user_message(),
                    };
                    sink.send(Event::PlaylistTracksFailed { id, message });
                }
            }
        });
    }

    /// Adds, moves or removes a track of `playlist` in Spotify. The UI already shows the change.
    /// If the edit fails, the backend reads the tracks again.
    fn edit_playlist(&mut self, playlist: Playlist, edit: PlaylistEdit) {
        self.playlist_edits += 1;
        util::lock(&self.cache).playlist_edited(&playlist.id, self.playlist_edits);
        let Some(api) = self.api.clone() else {
            return;
        };
        let mirrors = self.mirrors.clone();
        let jobs = self.playlist_tx.clone();
        self.chain_playlist_task(async move {
            let result = apply_playlist_edit(&api, &mirrors, &playlist.id, &edit).await;
            let _ = jobs.send(PlaylistJob::Edited {
                playlist,
                edit,
                result,
            });
        });
    }

    /// Tells if `playlist` has `track`, after the edits that the backend sent before.
    fn find_in_playlist(&mut self, playlist: Playlist, index: Option<usize>, track: Track) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let mirrors = self.mirrors.clone();
        let sink = self.sink.clone();
        self.chain_playlist_task(async move {
            let found = playlist_has(&api, &mirrors, &playlist.id, &track.uri)
                .await
                .unwrap_or_else(|error| {
                    log::warn!(
                        "Could not read «{}» to look for a track: {error}",
                        playlist.name
                    );
                    false
                });
            sink.send(Event::FoundInPlaylist {
                playlist,
                index,
                track,
                found,
            });
        });
    }

    /// Reads the album `id` (with the name `name`) and tells which of its tracks `playlist` has,
    /// after the edits that the backend sent before.
    fn find_album_in_playlist(&mut self, playlist: Playlist, id: String, name: String) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let cached = util::lock(&self.cache).album(&id, SystemTime::now());
        let cache = Arc::clone(&self.cache);
        let mirrors = self.mirrors.clone();
        let jobs = self.playlist_tx.clone();
        self.chain_playlist_task(async move {
            let tracks = match cached {
                Some(album) => Ok(album.tracks),
                None => api.album(&id).await.map(|album| {
                    let tracks = album.tracks.clone();
                    util::lock(&cache).store_album(album, SystemTime::now());
                    tracks
                }),
            };
            let result = match tracks {
                Ok(tracks) => {
                    let uris: Vec<String> = tracks.iter().map(|t| t.uri.clone()).collect();
                    let found = playlist_has_any(&api, &mirrors, &playlist.id, &uris)
                        .await
                        .unwrap_or_else(|error| {
                            log::warn!(
                                "Could not read «{}» to look for an album: {error}",
                                playlist.name
                            );
                            Vec::new()
                        });
                    Ok((tracks, found))
                }
                Err(error) => Err(error),
            };
            let _ = jobs.send(PlaylistJob::AlbumForPlaylist {
                playlist,
                name,
                result,
            });
        });
    }

    async fn finish_playlist_job(&mut self, job: PlaylistJob) {
        if self.api.is_none() {
            return; // logged out meanwhile
        }
        match job {
            PlaylistJob::AlbumForPlaylist {
                playlist,
                name,
                result: Ok((tracks, found)),
            } => self.sink.send(Event::FoundAlbumInPlaylist {
                playlist,
                name,
                tracks,
                found,
            }),
            PlaylistJob::AlbumForPlaylist {
                playlist,
                name,
                result: Err(error),
            } => {
                log::warn!(
                    "Could not read the album «{name}» to add it to «{}»: {error}",
                    playlist.name
                );
                self.report(error);
            }
            PlaylistJob::Created {
                name,
                folder,
                result: Ok(playlist),
            } => {
                if let Some(folder) = folder
                    && util::lock(&self.playlist_order).place_new(&playlist.id, &folder)
                {
                    let order = self.playlist_order.clone();
                    tokio::task::spawn_blocking(move || util::lock(&order).save());
                }
                self.sink.send(Event::PlaylistCreated {
                    name,
                    id: Some(playlist.id.clone()),
                });
                util::lock(&self.expected).created.push(playlist);
                self.load_playlists();
            }
            PlaylistJob::Created {
                name,
                result: Err(error),
                ..
            } => {
                log::warn!("Could not create the playlist «{name}»: {error}");
                self.sink.toast(
                    ToastKind::Error,
                    format!("Could not create “{name}”. {}", error.user_message()),
                );
                self.sink.send(Event::PlaylistCreated { name, id: None });
            }
            PlaylistJob::Renamed {
                playlist,
                name,
                result: Ok(()),
            } => {
                // Spotify can show the old name for some time.
                util::lock(&self.expected).names.insert(playlist.id, name);
            }
            PlaylistJob::Renamed {
                playlist,
                name,
                result: Err(error),
            } => {
                log::warn!(
                    "Could not rename the playlist «{}» to «{name}»: {error}",
                    playlist.name
                );
                let message = match &error {
                    // A collaborative playlist of a different user.
                    ApiError::Status { status: 403, .. } => {
                        format!("Only the creator of “{}” can rename it.", playlist.name)
                    }
                    error => format!(
                        "Could not rename “{}”. {}",
                        playlist.name,
                        error.user_message()
                    ),
                };
                self.sink.toast(ToastKind::Error, message);
                // The UI already showed the new name: go back to Spotify's name.
                util::lock(&self.expected).names.remove(&playlist.id);
                self.load_playlists();
            }
            PlaylistJob::Edited {
                playlist,
                edit,
                result: Ok(()),
            } => {
                // The new count (and possibly cover) of the list. Spotify can show the old count
                // for some time, so the backend keeps this count.
                let total = edit.total_after(playlist.total);
                util::lock(&self.expected).totals.insert(playlist.id, total);
                self.load_playlists();
            }
            PlaylistJob::Edited {
                playlist,
                edit,
                result: Err(error),
            } => {
                match error {
                    EditError::Api(error) => self.report_playlist_error(error, &playlist),
                    EditError::Plan(PlanError::LocalFile) => self.sink.toast(
                        ToastKind::Error,
                        if matches!(
                            edit,
                            PlaylistEdit::Insert { .. } | PlaylistEdit::Append { .. }
                        ) {
                            "Local files can't be added to a playlist from here."
                        } else {
                            "Spotify doesn't let you remove just one copy of a repeated local file from here."
                        },
                    ),
                    EditError::Plan(PlanError::Stale) => self.sink.toast(
                        ToastKind::Error,
                        format!(
                            "“{}” has changed in Spotify: check the playlist and try again.",
                            playlist.name
                        ),
                    ),
                }
                // The UI already showed the change: go back to what Spotify has.
                util::lock(&self.expected).totals.remove(&playlist.id);
                self.load_playlists();
                self.load_playlist_tracks(playlist);
            }
            PlaylistJob::Fetched {
                playlist,
                index,
                result: Ok(tracks),
            } => {
                self.queue_tracks(&playlist.name, QueueAt::Next(index), tracks)
                    .await;
            }
            PlaylistJob::Fetched {
                playlist,
                result: Err(error),
                ..
            } => self.report_playlist_error(error, &playlist),
            PlaylistJob::AlbumFetched {
                name,
                at,
                result: Ok(tracks),
            } => self.queue_tracks(&name, at, tracks).await,
            PlaylistJob::AlbumFetched {
                name,
                result: Err(error),
                ..
            } => {
                log::warn!("Could not read the album «{name}» to queue it: {error}");
                self.report(error);
            }
            PlaylistJob::Liked {
                track,
                liked,
                confirm,
                result: Ok(()),
            } => {
                if confirm {
                    self.sink.send(Event::LikedChanged {
                        uri: track.uri,
                        liked,
                    });
                }
            }
            PlaylistJob::Liked {
                track,
                liked,
                result: Err(error),
                ..
            } => {
                log::warn!("Could not change «{}» in Liked Songs: {error}", track.name);
                // The UI already showed the change: go back to the old value.
                self.sink.send(Event::LikedChanged {
                    uri: track.uri,
                    liked: !liked,
                });
                if error.is_auth_failure() || matches!(error, ApiError::RateLimited(_)) {
                    self.report(error);
                } else {
                    let message = if liked {
                        format!("Could not save “{}” to Liked Songs.", track.name)
                    } else {
                        format!("Could not remove “{}” from Liked Songs.", track.name)
                    };
                    self.sink.toast(
                        ToastKind::Error,
                        format!("{message} {}", error.user_message()),
                    );
                }
            }
        }
    }

    /// Puts the tracks of the playlist or album with the name `name` in the queue at `at`, or
    /// plays them if nothing plays.
    async fn queue_tracks(&mut self, name: &str, at: QueueAt, mut tracks: Vec<Track>) {
        if tracks.is_empty() {
            self.sink
                .toast(ToastKind::Error, super::nothing_to_queue(name));
            return;
        }
        if self.playback.track.is_none() {
            self.run_player_action(PlayerAction::Play { tracks, index: 0 })
                .await;
            return;
        }
        // «Play»: the other tracks go first in the queue, then the first track plays. The queue
        // stays, as with ▶ on a track.
        let play_now = (at == QueueAt::Now).then(|| tracks.remove(0));
        let index = match at {
            QueueAt::Next(index) => index,
            QueueAt::End => self.upcoming.len(),
            QueueAt::Now => 0,
        };
        if !tracks.is_empty() {
            let before = self.upcoming.len();
            let total = tracks.len() + usize::from(play_now.is_some());
            let edit = QueueEdit::InsertMany { index, tracks };
            if self.change_queue(&edit).await {
                let added = self.upcoming.len() - before + usize::from(play_now.is_some());
                if added < total {
                    self.sink.toast(
                        ToastKind::Error,
                        super::playlist_partly_queued(name, added, total),
                    );
                }
            } else {
                self.toast_queue_full();
            }
        }
        if let Some(track) = play_now {
            self.run_player_action(PlayerAction::Play {
                tracks: vec![track],
                index: 0,
            })
            .await;
        }
        self.emit_queue();
    }

    fn report_playlist_error(&mut self, error: ApiError, playlist: &Playlist) {
        log::warn!("Playlist request failed for «{}»: {error}", playlist.name);
        match error {
            ApiError::Status { status: 403, .. } => self
                .sink
                .toast(ToastKind::Error, super::playlist_not_usable(&playlist.name)),
            error => self.report(error),
        }
    }

    // --- Volume ------------------------------------------------------------------

    fn set_volume(&mut self, percent: u8) {
        if self.api.is_none() {
            return;
        }
        let percent = percent.min(100);
        self.volume = Some(percent);
        self.volume_hold = Some((percent, Instant::now() + self.config.timings.volume_hold));
        if let Some(device) = &mut self.playback.device {
            device.volume_percent = Some(percent);
        }
        self.sink.send(Event::Playback(self.playback.clone()));
        // The SDK changes the volume on the page. There is no request, so a drag of the slider
        // never uses the Web API's quota. Without the page, the volume goes to Spotify, but never
        // to the music of a different device.
        let on_page = self.web_open
            && self
                .config
                .web_player
                .as_ref()
                .is_some_and(|host| host.send(HostRequest::Volume(percent)));
        self.volume_tx.send_replace(Some(VolumeChange {
            percent,
            to_spotify: !on_page && self.elsewhere.is_none(),
        }));
    }

    async fn transfer(&mut self, device_id: String, play: bool) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let to_web_player = matches!(&self.web_status, WebPlayerStatus::Ready { device_id: id } if *id == device_id);
        let retries = if to_web_player { NEW_DEVICE_RETRIES } else { 0 };
        let transferred =
            retry_while_device_is_new(retries, self.config.timings.new_device_retry_every, || {
                api.transfer_playback(&device_id, play)
            })
            .await;
        if let Err(error) = transferred {
            self.report(error);
            return;
        }
        let target = self.show_moved_to(&device_id, to_web_player);
        self.refresh_shown_devices().await;
        if to_web_player {
            // The page tells when the music plays here. Then the backend reads the queue.
            self.start_moving_here();
        } else if let Some(target) = target {
            self.go_elsewhere(target);
        } else {
            // Melodia does not know the device: Spotify tells its name.
            self.where_check = true;
            self.fetch_state_at(Instant::now() + self.config.timings.state_after_command);
        }
    }

    /// Spotify accepted the move of the music to `device_id`. The UI shows it as the active device
    /// immediately, and the backend does not ask Spotify again. Returns that device, if known.
    fn show_moved_to(&mut self, device_id: &str, to_web_player: bool) -> Option<Device> {
        let known = self.devices.iter().find(|d| d.id == device_id).cloned();
        let mut target = known.or_else(|| self.web_device().filter(|_| to_web_player))?;
        target.is_active = true;
        self.moved_to = Some((
            target.clone(),
            Instant::now() + self.config.timings.transfer_hold,
        ));
        let mut devices = std::mem::take(&mut self.devices);
        self.mark_moved_to(&mut devices);
        self.devices = devices;
        self.sink.send(Event::Devices(self.devices.clone()));
        if to_web_player && self.elsewhere.is_none() {
            let mut state = self.playback.clone();
            state.device = Some(target.clone());
            self.hold_volume(&mut state, Instant::now());
            self.playback = state;
            self.sink.send(Event::Playback(self.playback.clone()));
        }
        Some(target)
    }

    // --- Web player («Play in this app») ---------------------------------

    /// Runs on the web player a play request that waited for it.
    async fn play_on_web_player(&mut self, device_id: &str, request: &Request) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let played = retry_while_device_is_new(
            NEW_DEVICE_RETRIES,
            self.config.timings.new_device_retry_every,
            || send_request(&api, request, Some(device_id)),
        )
        .await;
        if let Err(error) = played {
            self.report(error);
            self.fetch_state_at(Instant::now() + self.config.timings.state_after_command);
        } else {
            self.start_moving_here();
        }
    }

    /// Makes the web player the active device, but does not start the music. The backend does
    /// this when the app opens, so it fails silently. If there is no previous playback, there is
    /// nothing to take, and the backend uses the web player when something plays.
    async fn adopt_quietly(&mut self, device_id: &str) {
        let Some(api) = self.api.clone() else {
            return;
        };
        // Restore the saved position only if the last music was in this app. For any other
        // device, Spotify knows the position.
        let last_here = self
            .playback
            .device
            .as_ref()
            .is_none_or(|device| device.name == DEVICE_NAME);
        let restore = self.resume.clone().filter(|_| last_here);
        match retry_while_device_is_new(
            NEW_DEVICE_RETRIES,
            self.config.timings.new_device_retry_every,
            || api.transfer_playback(device_id, false),
        )
        .await
        {
            Ok(()) => {
                log::info!("This app is the Spotify device now (paused)");
                self.start_moving_here();
                self.resume_pending = restore
                    .map(|point| (point, Instant::now() + self.config.timings.resume_window));
                self.refresh_shown_devices().await;
            }
            Err(error) => log::info!("This app could not take over the playback: {error}"),
        }
    }

    fn is_web_player(&self, device: Option<&Device>) -> bool {
        matches!(
            (&self.web_status, device),
            (WebPlayerStatus::Ready { device_id }, Some(device)) if device.id == *device_id
        )
    }

    /// Immediately after this app becomes the device when it opens, goes back to the position of
    /// the music when the app closed. Spotify only knows a position that the player page told it
    /// some time before.
    async fn go_back_to_saved_position(&mut self) {
        let Some((saved, until)) = self.resume_pending.clone() else {
            return;
        };
        if !self.is_web_player(self.playback.device.as_ref()) {
            // Spotify can show the change of device late.
            if Instant::now() >= until {
                self.resume_pending = None;
            }
            return;
        }
        self.resume_pending = None;
        let state = &self.playback;
        if state.is_playing
            || state.track_uri() != Some(saved.uri.as_str())
            || state.progress_ms.abs_diff(saved.position_ms) < RESUME_TOLERANCE_MS
        {
            return;
        }
        log::info!(
            "Going back to {} s, where the music was when the app closed",
            saved.position_ms / 1000
        );
        self.run_player_action(PlayerAction::Seek(saved.position_ms))
            .await;
    }

    /// Saves the position of the music while it plays in this app (every few seconds, and at each
    /// pause or seek), for the next time the app opens.
    fn remember_position(&mut self, now: Instant) {
        if self.resume_pending.is_some() {
            return; // the saved position has not been restored yet
        }
        if !self.is_web_player(self.playback.device.as_ref()) {
            if self.playback.is_playing && self.resume.is_some() {
                // The music plays on a different device: that device tells Spotify the position.
                self.store_resume(None);
            }
            return;
        }
        let Some(track) = &self.playback.track else {
            return;
        };
        let point = ResumePoint {
            uri: track.uri.clone(),
            position_ms: self.playback.progress_at(now),
        };
        let up_to_date = match (&self.resume, self.resume_saved_at) {
            (Some(saved), Some(at)) if saved.uri == point.uri => {
                if self.playback.is_playing {
                    now.saturating_duration_since(at) < self.config.timings.resume_save_every
                } else {
                    saved.position_ms.abs_diff(point.position_ms) < 1000
                }
            }
            _ => false,
        };
        if !up_to_date {
            self.store_resume(Some(point));
        }
    }

    /// Gives the cache to the cache task to write it, if something changed after the last time.
    fn save_cache(&self) {
        let contents = util::lock(&self.cache).take_changes(SystemTime::now());
        if let Some(contents) = contents {
            self.cache_tx.send_replace(Some(Arc::new(contents)));
        }
    }

    fn store_resume(&mut self, point: Option<ResumePoint>) {
        if self.resume == point {
            return;
        }
        self.resume_tx.send_replace(point.clone());
        self.resume = point;
        self.resume_saved_at = Some(Instant::now());
    }

    fn set_web_status(&mut self, status: WebPlayerStatus) {
        if self.web_status != status {
            self.web_status = status.clone();
            self.sink.send(Event::WebPlayer(status));
        }
    }

    /// Closes the player page and ignores its remaining messages.
    fn kill_web_player(&mut self) {
        if std::mem::take(&mut self.web_open)
            && let Some(host) = &self.config.web_player
        {
            host.send(HostRequest::Stop);
        }
        self.web_generation += 1;
        self.web_started_at = None;
        self.web_quiet = false;
        self.web_pending = None;
        self.moving_here_until = None;
        self.page_next = None;
    }

    fn stop_web_player(&mut self) {
        self.kill_web_player();
        self.set_web_status(WebPlayerStatus::Off);
    }

    fn fail_web_player(&mut self, message: String) {
        log::warn!("Web player failed: {message}");
        self.drop_pending_play();
        self.kill_web_player();
        self.set_web_status(WebPlayerStatus::Failed(message));
    }

    /// Cancels a play request that waits for the web player.
    fn drop_pending_play(&mut self) {
        if self.web_pending.take().is_some() {
            self.sink.toast(
                ToastKind::Error,
                "This app's player couldn't start. Choose another device.",
            );
        }
    }

    async fn start_web_player(&mut self) {
        let Some(api) = self.api.clone() else {
            return;
        };
        match self.web_status.clone() {
            WebPlayerStatus::Starting => return,
            // The web player is already connected: only move the music here.
            WebPlayerStatus::Ready { device_id } if self.web_open => {
                let play = self.playback.track.is_some() || self.elsewhere.is_some();
                self.transfer(device_id, play).await;
                return;
            }
            _ => {}
        }
        let Some(host) = self.config.web_player.clone() else {
            self.set_web_status(WebPlayerStatus::Unavailable(
                "Only available on Windows.".into(),
            ));
            return;
        };
        if !has_streaming_scope(&api.tokens().session().await.scope) {
            self.set_web_status(WebPlayerStatus::NeedsReauth);
            return;
        }

        self.web_generation += 1;
        let start = HostRequest::Start {
            generation: self.web_generation,
            // The SDK starts at the volume that the user set last time.
            volume: self.volume,
            events: self.web_tx.clone(),
        };
        if host.send(start) {
            log::info!("Web player starting");
            self.web_open = true;
            self.web_started_at = Some(Instant::now());
            self.web_last_error = None;
            self.set_web_status(WebPlayerStatus::Starting);
        } else {
            self.set_web_status(WebPlayerStatus::Failed(
                "Could not start the web player: the app window isn't responding.".into(),
            ));
        }
    }

    async fn on_web_player_event(&mut self, generation: u64, event: PlayerEvent) {
        if generation != self.web_generation {
            return;
        }
        match event {
            PlayerEvent::Closed => {
                let was_here = self.page_reports_playback();
                self.web_open = false;
                self.page_next = None;
                self.web_started_at = None;
                self.web_quiet = false;
                self.moving_here_until = None;
                self.drop_pending_play();
                if was_here {
                    // The page does not report the music now: Spotify tells the remaining state.
                    self.fetch_state_at(Instant::now());
                }
                if matches!(
                    self.web_status,
                    WebPlayerStatus::Starting | WebPlayerStatus::Ready { .. }
                ) {
                    let message = self
                        .web_last_error
                        .take()
                        .unwrap_or_else(|| "The web player closed unexpectedly.".to_owned());
                    self.set_web_status(WebPlayerStatus::Failed(message));
                }
            }
            PlayerEvent::Message(message) => self.on_web_player_message(message).await,
        }
    }

    async fn on_web_player_message(&mut self, message: FromPage) {
        match message {
            FromPage::Log { message } => log::info!("[web player] {message}"),
            FromPage::NeedToken => self.send_web_player_token().await,
            FromPage::Ready { device_id } => {
                log::info!("Web player ready as Spotify device {device_id}");
                self.web_started_at = None;
                self.set_web_status(WebPlayerStatus::Ready {
                    device_id: device_id.clone(),
                });
                // Spotify possibly does not know the new device yet. The backend sends the
                // requests below again while Spotify tells this (`retry_while_device_is_new`).
                let quiet = std::mem::take(&mut self.web_quiet);
                if let Some(request) = self.web_pending.take() {
                    // A user pushed play while the web player started.
                    self.play_on_web_player(&device_id, &request).await;
                } else if quiet {
                    // The page opened (or connected again) with nothing playing. This app
                    // becomes the device, paused, if no music plays here or on a different device.
                    if !self.playback.is_playing && self.elsewhere.is_none() {
                        self.adopt_quietly(&device_id).await;
                    }
                } else {
                    // The user chose this: the music comes here.
                    let play = self.playback.track.is_some() || self.elsewhere.is_some();
                    self.transfer(device_id, play).await;
                }
            }
            FromPage::NotReady { device_id } => {
                log::warn!("Web player device {device_id} went offline; the SDK reconnects");
                self.web_started_at = Some(Instant::now());
                // When it connects again, it must not take the music from a different device.
                self.web_quiet = true;
                self.set_web_status(WebPlayerStatus::Starting);
            }
            FromPage::State(report) => self.on_page_state(report).await,
            FromPage::ControlFailed {
                action,
                position,
                message,
            } => {
                log::warn!("[web player] could not {action}: {message}");
                if let Some(control) = PageControl::parse(&action, position) {
                    self.control_through_api(control).await;
                }
            }
            // The music went to a different device: only Spotify knows which.
            FromPage::Inactive => self.music_not_here(),
            FromPage::Error { kind, message } => self.on_web_player_error(&kind, message),
            // Only the window waits for it, to close.
            FromPage::ReadyToClose => {}
        }
    }

    /// What the SDK reports. The backend uses only reports of music that plays here, as if
    /// Spotify told them. A report that the music plays on a different device makes Melodia ask
    /// Spotify where (`music_not_here`).
    async fn on_page_state(&mut self, report: PageState) {
        let now = Instant::now();
        let report_here = report.here == Some(true);
        match report.here {
            Some(true) => self.come_back_here(),
            Some(false) => {
                self.music_not_here();
                return;
            }
            // The page could not tell. Trust it only about music that is already here.
            None if !self.page_reports_playback() => return,
            None => {}
        }
        let PageState {
            paused,
            position,
            track_uri,
            shuffle,
            track: described,
            linked_from,
            next_uris,
            here: _,
        } = report;
        let known = track_uri
            .iter()
            .chain(&linked_from)
            .find_map(|uri| self.known_track(uri));
        // For a track that Melodia does not know, use the description from the SDK.
        let track = known.or_else(|| {
            described
                .and_then(parse_item)
                .filter(|track| track_uri.as_deref() == Some(track.uri.as_str()))
        });
        self.page_next = track
            .as_ref()
            .zip(next_uris)
            .map(|(track, next)| (track.uri.clone(), next));
        let (Some(api), Some(track), Some(device)) = (self.api.clone(), track, self.web_device())
        else {
            if report_here {
                // Spotify can still show the previous device of the music.
                self.moving_here_until = Some(now + self.config.timings.moving_here_window);
            }
            self.fetch_state_at(now + self.config.timings.state_after_page_report);
            return;
        };
        if report_here && self.moving_here_until.take().is_some() {
            log::info!("The web player says the music arrived here");
        }
        let state = PlaybackSnapshot {
            track: Some(track),
            is_playing: !paused,
            progress_ms: position,
            measured_at: now,
            device: Some(device),
            shuffle: shuffle.unwrap_or(self.playback.shuffle),
        };
        let before = self.playback.track.clone();
        self.take_in(&api, state).await;
        if self.expectation.is_none() {
            // The page confirmed the last command and where the music plays: Spotify does not
            // have to.
            self.state_fetch_at = None;
            self.startup_fetch = false;
            self.where_check = false;
        }
        self.follow_queue_from_page(before);
        self.read_queue_if_stale(&api).await;
    }

    /// While the music plays here, the SDK does `request` on the page, with no Web API request
    /// (the page confirms it with its report). Returns true if the page did it.
    fn control_on_page(&self, request: &Request) -> bool {
        let control = match request {
            Request::Resume => PageControl::Resume,
            Request::Pause => PageControl::Pause,
            Request::Next => PageControl::Next,
            Request::Previous => PageControl::Previous,
            Request::Seek(position_ms) => PageControl::Seek(*position_ms),
            Request::Order { .. } => return false,
        };
        self.page_reports_playback()
            && self
                .config
                .web_player
                .as_ref()
                .is_some_and(|host| host.send(HostRequest::Control(control)))
    }

    /// The SDK could not do `control`, so the Web API does it, if its prediction still waits. If
    /// not, newer commands came after it, and the backend only asks Spotify what plays.
    async fn control_through_api(&mut self, control: PageControl) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if self.expectation.is_none() {
            self.fetch_state_at(Instant::now());
            return;
        }
        let request = match control {
            PageControl::Resume => Request::Resume,
            PageControl::Pause => Request::Pause,
            PageControl::FadeIn => return,
            PageControl::Next => Request::Next,
            PageControl::Previous => Request::Previous,
            PageControl::Seek(position_ms) => Request::Seek(position_ms),
        };
        match send_request(&api, &request, None).await {
            Ok(()) => self.confirm_command(),
            Err(error) => {
                self.expectation = None;
                self.fetch_state_at(Instant::now());
                self.report(error);
            }
        }
    }

    /// This app as a Spotify device: as Spotify last showed it, or as the SDK created it if
    /// Spotify did not show it yet.
    fn web_device(&self) -> Option<Device> {
        let WebPlayerStatus::Ready { device_id } = &self.web_status else {
            return None;
        };
        let shown = self
            .playback
            .device
            .as_ref()
            .filter(|device| device.id == *device_id);
        Some(shown.cloned().unwrap_or_else(|| Device {
            id: device_id.clone(),
            name: DEVICE_NAME.to_owned(),
            kind: "Computer".to_owned(),
            is_active: true,
            volume_percent: self.volume,
            supports_volume: true,
        }))
    }

    /// The track with `uri` in the current, upcoming and previous tracks.
    fn known_track(&self, uri: &str) -> Option<Track> {
        self.playback
            .track
            .iter()
            .chain(&self.upcoming)
            .chain(self.history.entries())
            .find(|track| track.uri == uri)
            .cloned()
    }

    async fn send_web_player_token(&mut self) {
        let Some(api) = self.api.clone() else {
            return;
        };
        let token = match api.tokens().access_token().await {
            Ok(token) => token,
            Err(error) => {
                self.report(error);
                return;
            }
        };
        if self.web_open
            && let Some(host) = &self.config.web_player
            && !host.send(HostRequest::Token(token))
        {
            log::warn!("Could not send the token to the web player: the window is gone");
        }
    }

    fn on_web_player_error(&mut self, kind: &str, message: String) {
        log::warn!("[web player] {kind}: {message}");
        match kind {
            "authentication_error" if message.to_lowercase().contains("scope") => {
                self.kill_web_player();
                self.set_web_status(WebPlayerStatus::NeedsReauth);
            }
            "initialization_error" => self.fail_web_player(format!(
                "Spotify can't play in this WebView (DRM): {message}"
            )),
            "authentication_error" => {
                self.fail_web_player(format!("Spotify rejected the player's session: {message}"));
            }
            "account_error" => self.fail_web_player(format!(
                "Spotify doesn't allow playback with this account (is it Premium?): {message}"
            )),
            "webview" | "network" => self.fail_web_player(message),
            // These errors alone do not stop the player: the SDK can still connect, or report a
            // more specific error. The backend keeps them for the timeout message.
            "connect" | "drm" | "script" => {
                self.web_last_error = Some(message);
            }
            // Problems with a track, while the player continues to work.
            _ => self
                .sink
                .toast(ToastKind::Error, format!("Web player: {message}")),
        }
    }

    /// Stops if the SDK does not become a Spotify device.
    fn check_web_player_timeout(&mut self) {
        if self
            .web_started_at
            .is_some_and(|started| started.elapsed() > WEB_PLAYER_TIMEOUT)
        {
            let reason = self.web_last_error.take().map_or_else(
                || "Spotify didn't respond.".to_owned(),
                |error| format!("Last error: {error}"),
            );
            self.fail_web_player(format!(
                "The web player never connected to Spotify. {reason}"
            ));
        }
    }

    // --- Liked Songs -----------------------------------------------------------

    /// Reads which of `uris` are in «Liked Songs», in batches, in its own task.
    async fn check_liked(&mut self, uris: Vec<String>) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if !has_library_scopes(&api.tokens().session().await.scope) {
            self.sink.send(Event::LikedNeedsReauth);
            return;
        }
        let sink = self.sink.clone();
        self.liked_checks.spawn(async move {
            for batch in uris.chunks(LIBRARY_BATCH) {
                match api.library_contains(batch).await {
                    Ok(saved) => {
                        let states = batch.iter().cloned().zip(saved).collect();
                        sink.send(Event::LikedStates(states));
                    }
                    // This does not need a notice: the hearts stay empty.
                    Err(error) => {
                        log::warn!("Could not check Liked Songs: {error}");
                        return;
                    }
                }
            }
        });
        // Forget the reads that ended.
        while self.liked_checks.try_join_next().is_some() {}
    }

    /// Saves `track` to «Liked Songs» or removes it. Without the permission, the user first
    /// connects the account again.
    async fn set_liked(&mut self, track: Track, liked: bool) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if has_library_scopes(&api.tokens().session().await.scope) {
            self.change_liked(track, liked, false).await;
            return;
        }
        self.sink.toast(
            ToastKind::Info,
            "Spotify needs a new permission to save songs. Reconnect your account in the browser.",
        );
        self.liked_after_login = Some((track, liked));
        self.start_login(None);
    }

    /// Sends the change after the changes sent before. With `confirm`, the UI gets the result (it
    /// did not show the change).
    async fn change_liked(&mut self, track: Track, liked: bool, confirm: bool) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if !has_library_scopes(&api.tokens().session().await.scope) {
            self.sink.toast(
                ToastKind::Error,
                "Spotify didn't give the permission to save songs.",
            );
            return;
        }
        let jobs = self.playlist_tx.clone();
        let previous = self.liked_chain.take();
        self.liked_chain = Some(tokio::spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            let uris = [track.uri.clone()];
            let result = if liked {
                api.save_to_library(&uris).await
            } else {
                api.remove_from_library(&uris).await
            };
            let _ = jobs.send(PlaylistJob::Liked {
                track,
                liked,
                confirm,
                result,
            });
        }));
    }

    // --- Search and images -----------------------------------------------------

    fn search(&mut self, query: String, offset: u32) {
        let Some(api) = self.api.clone() else {
            self.sink.send(Event::SearchFailed {
                query,
                offset,
                message: "Connect your Spotify account to search.".into(),
            });
            return;
        };
        if let Some(task) = self.search_task.take() {
            task.abort();
        }
        if let Some(raw) = util::lock(&self.cache).search(&query, offset, SystemTime::now()) {
            self.sink
                .send(Event::SearchResults(parse_search(&query, offset, raw)));
            return;
        }
        let sink = self.sink.clone();
        let cache = Arc::clone(&self.cache);
        self.search_task = Some(tokio::spawn(async move {
            match api.search_raw(&query, offset).await {
                Ok(raw) => {
                    util::lock(&cache).store_search(&query, offset, raw.clone(), SystemTime::now());
                    sink.send(Event::SearchResults(parse_search(&query, offset, raw)));
                }
                Err(error) => sink.send(Event::SearchFailed {
                    query,
                    offset,
                    message: error.user_message(),
                }),
            }
        }));
    }

    /// Reads the album `id` in its own task, if the backend did not read it recently. Only the
    /// last album requested is important: the backend cancels an earlier request that did not
    /// end.
    fn load_album(&mut self, id: String) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if let Some(task) = self.album_task.take() {
            task.abort();
        }
        if let Some(album) = util::lock(&self.cache).album(&id, SystemTime::now()) {
            self.sink.send(Event::Album(album));
            return;
        }
        let sink = self.sink.clone();
        let cache = Arc::clone(&self.cache);
        self.album_task = Some(tokio::spawn(async move {
            match api.album(&id).await {
                Ok(album) => {
                    util::lock(&cache).store_album(album.clone(), SystemTime::now());
                    sink.send(Event::Album(album));
                }
                Err(error) => {
                    log::warn!("Could not load album {id}: {error}");
                    sink.send(Event::AlbumFailed {
                        id,
                        message: error.user_message(),
                    });
                }
            }
        }));
    }

    /// Reads the artist `id` and the list of their releases in its own task, if the backend did
    /// not read them recently. Only the last artist requested is important: the backend cancels
    /// an earlier request that did not end.
    fn load_artist(&mut self, id: String) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if let Some(task) = self.artist_task.take() {
            task.abort();
        }
        if let Some(artist) = util::lock(&self.cache).artist(&id, SystemTime::now()) {
            self.sink.send(Event::Artist(artist));
            return;
        }
        let sink = self.sink.clone();
        let cache = Arc::clone(&self.cache);
        self.artist_task = Some(tokio::spawn(async move {
            match api.artist(&id).await {
                Ok(artist) => {
                    util::lock(&cache).store_artist(artist.clone(), SystemTime::now());
                    sink.send(Event::Artist(artist));
                }
                Err(error) => {
                    log::warn!("Could not load artist {id}: {error}");
                    sink.send(Event::ArtistFailed {
                        id,
                        message: error.user_message(),
                    });
                }
            }
        }));
    }

    fn load_image(&self, url: String, max_px: u32) {
        let http = self.http.clone();
        let sink = self.sink.clone();
        let permits = Arc::clone(&self.image_permits);
        tokio::spawn(async move {
            let _permit = permits.acquire_owned().await;
            match fetch_image(&http, &url, max_px).await {
                Ok(image) => sink.send(Event::ImageLoaded { url, max_px, image }),
                Err(error) => {
                    log::debug!("Could not load image {url}: {error}");
                    sink.send(Event::ImageFailed { url, max_px });
                }
            }
        });
    }

    // --- Errors ----------------------------------------------------------------

    fn report(&mut self, error: ApiError) {
        log::warn!("Spotify request failed: {error}");
        match error {
            error if error.is_auth_failure() => self.logout(Some(error.user_message())),
            ApiError::RateLimited(wait) => {
                self.rate_limited_until = Some(Instant::now() + wait);
                self.sink.toast(ToastKind::Error, error.user_message());
            }
            error => self.sink.toast(ToastKind::Error, error.user_message()),
        }
    }
}

/// The values that the playlists edited with success must show (id → value).
#[derive(Default)]
struct Expected {
    totals: HashMap<String, u32>,
    names: HashMap<String, String>,
    /// New playlists, which Spotify possibly does not list yet.
    created: Vec<Playlist>,
}

type ExpectedChanges = Arc<std::sync::Mutex<Expected>>;

/// The profile of the session's user. Its ID does not change during the session, so the backend
/// sends `GET /me` one time, not at each load of the playlists. When a read fails, the next load
/// tries again.
type SessionProfile = Arc<OnceCell<UserProfile>>;

/// The session's user, which the backend reads from Spotify the first time it is necessary. The
/// login reads it two times at the same time: the second read waits for the first.
async fn user_profile<'a>(
    api: &SpotifyClient,
    profile: &'a SessionProfile,
) -> Result<&'a UserProfile, ApiError> {
    profile.get_or_try_init(|| api.current_user()).await
}

/// The user's playlists. Immediately after an edit, `GET /me/playlists` can show the count (or
/// name) from before the edit. That can undo on the screen what the UI already shows (or not show
/// a new playlist). The edit succeeded, so the function keeps the value of `expected` one time.
/// Later loads show Spotify's value.
async fn fetch_playlists(
    api: &SpotifyClient,
    profile: &SessionProfile,
    expected: &ExpectedChanges,
) -> Result<(String, Vec<Playlist>), ApiError> {
    let user = user_profile(api, profile).await?.id.clone();
    let mut playlists = api.playlists(&user).await?;
    let mut expected = util::lock(expected);
    for playlist in &mut playlists {
        if let Some(total) = expected.totals.remove(&playlist.id) {
            playlist.total = total;
        }
        if let Some(name) = expected.names.remove(&playlist.id) {
            playlist.name = name;
        }
    }
    // Spotify lists new playlists first.
    for created in std::mem::take(&mut expected.created) {
        if !playlists.iter().any(|p| p.id == created.id) {
            playlists.insert(0, created);
        }
    }
    Ok((user, playlists))
}

/// Reads the entries of the playlist `id` (and its version): the data that edits need, and the
/// tracks to show.
async fn read_playlist(
    api: &SpotifyClient,
    id: &str,
) -> Result<(PlaylistMirror, Vec<Track>, u32), ApiError> {
    // Read the version first: thus the entries are as new as the version, or newer.
    let snapshot = api.playlist_snapshot(id).await.unwrap_or_else(|error| {
        log::debug!("No snapshot for playlist {id}: {error}");
        None
    });
    let read = api
        .playlist_entries(id, 0, MAX_SHOWN_PLAYLIST_TRACKS)
        .await?;
    let uris = read
        .entries
        .iter()
        .map(|entry| entry.as_ref().map(|track| track.uri.clone()))
        .collect();
    let mirror = PlaylistMirror::new(uris, read.complete, snapshot);
    let tracks = read.entries.into_iter().flatten().collect();
    Ok((mirror, tracks, read.total))
}

/// True if the playlist `id` has `uri`. It reads the entries of the playlist if they are not
/// known (all entries, if necessary).
async fn playlist_has(
    api: &SpotifyClient,
    mirrors: &Mirrors,
    id: &str,
    uri: &str,
) -> Result<bool, ApiError> {
    let found = playlist_has_any(api, mirrors, id, std::slice::from_ref(&uri.to_owned())).await?;
    Ok(!found.is_empty())
}

/// The items of `uris` that the playlist `id` has. It reads the entries of the playlist if they
/// are not known (all entries, if necessary).
async fn playlist_has_any(
    api: &SpotifyClient,
    mirrors: &Mirrors,
    id: &str,
    uris: &[String],
) -> Result<Vec<String>, ApiError> {
    let mut mirrors = mirrors.lock().await;
    let mirror = match mirrors.entry(id.to_owned()) {
        Entry::Occupied(occupied) => occupied.into_mut(),
        Entry::Vacant(vacant) => vacant.insert(read_playlist(api, id).await?.0),
    };
    if uris.iter().any(|uri| mirror.has(uri).is_none()) {
        read_the_rest(api, id, mirror).await?;
    }
    Ok(uris
        .iter()
        .filter(|uri| mirror.has(uri) == Some(true))
        .cloned()
        .collect())
}

/// Reads the entries of the playlist `id` that `mirror` does not have yet. After this, `mirror`
/// has all of them.
async fn read_the_rest(
    api: &SpotifyClient,
    id: &str,
    mirror: &mut PlaylistMirror,
) -> Result<(), ApiError> {
    let rest = api
        .playlist_entries(id, mirror.entries_read(), usize::MAX)
        .await?;
    mirror.complete_with(
        rest.entries
            .into_iter()
            .map(|entry| entry.map(|track| track.uri)),
    );
    Ok(())
}

/// Makes `edit` in the playlist `id`, in Spotify.
async fn apply_playlist_edit(
    api: &SpotifyClient,
    mirrors: &Mirrors,
    id: &str,
    edit: &PlaylistEdit,
) -> Result<(), EditError> {
    let mut mirrors = mirrors.lock().await;
    // An add at the end needs no positions (and no read of the playlist).
    if !matches!(
        edit,
        PlaylistEdit::Insert { index: None, .. } | PlaylistEdit::Append { .. }
    ) && let Entry::Vacant(vacant) = mirrors.entry(id.to_owned())
    {
        vacant.insert(read_playlist(api, id).await?.0);
    }
    let mut unknown = PlaylistMirror::default();
    let mirror = mirrors.get_mut(id).unwrap_or(&mut unknown);
    if mirror.needs_all_for(edit) {
        // Other copies of the track can be in the entries that the backend did not read.
        read_the_rest(api, id, mirror).await?;
    }
    let result = run_playlist_steps(api, mirror, id, edit).await;
    if result.is_err() {
        // The state is not known: read it again before the next edit.
        mirrors.remove(id);
    }
    result
}

async fn run_playlist_steps(
    api: &SpotifyClient,
    mirror: &mut PlaylistMirror,
    id: &str,
    edit: &PlaylistEdit,
) -> Result<(), EditError> {
    let steps = mirror.plan(edit).map_err(EditError::Plan)?;
    for step in steps {
        let snapshot = mirror.snapshot.as_deref();
        let new_snapshot = match &step {
            ApiStep::Reorder {
                range_start,
                insert_before,
            } => {
                api.reorder_playlist(id, *range_start, *insert_before, snapshot)
                    .await?
            }
            ApiStep::RemoveAll { uri } => api.remove_from_playlist(id, uri).await?,
            ApiStep::Add { uri, position } => {
                api.add_to_playlist(id, std::slice::from_ref(uri), *position)
                    .await?
            }
            ApiStep::Append { uris } => api.add_to_playlist(id, uris, None).await?,
        };
        if new_snapshot.is_some() {
            mirror.snapshot = new_snapshot;
        }
    }
    Ok(())
}

/// What a player action sends to Spotify.
#[derive(Debug, Clone, PartialEq)]
enum Request {
    Resume,
    Pause,
    Next,
    Previous,
    Seek(u64),
    /// Plays `uris` from the first item: Melodia's queue in its order.
    Order {
        uris: Vec<String>,
        position_ms: Option<u64>,
    },
}

/// Spotify's own command for `action`. To play a given track, the backend always sends the full
/// queue.
fn native_request(action: &PlayerAction) -> Option<Request> {
    Some(match action {
        PlayerAction::Resume => Request::Resume,
        PlayerAction::Pause => Request::Pause,
        PlayerAction::Next => Request::Next,
        PlayerAction::Previous => Request::Previous,
        PlayerAction::Seek(position_ms) => Request::Seek(*position_ms),
        PlayerAction::Play { .. } | PlayerAction::Jump(_) => return None,
    })
}

/// The Web API can need a moment to know a new device. While Spotify does not find it, the
/// function sends `request` again after `every`, up to `retries` times.
async fn retry_while_device_is_new<F, Fut>(
    retries: u32,
    every: Duration,
    mut request: F,
) -> Result<(), ApiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), ApiError>>,
{
    let mut attempts = 0;
    loop {
        match request().await {
            Err(ApiError::NoActiveDevice | ApiError::Status { status: 404, .. })
                if attempts < retries =>
            {
                attempts += 1;
                tokio::time::sleep(every).await;
            }
            result => return result,
        }
    }
}

async fn send_request(
    api: &SpotifyClient,
    request: &Request,
    device_id: Option<&str>,
) -> Result<(), ApiError> {
    match request {
        Request::Resume => api.play(device_id, None).await,
        Request::Pause => api.pause().await,
        Request::Next => api.next().await,
        Request::Previous => api.previous().await,
        Request::Seek(position_ms) => api.seek(*position_ms).await,
        Request::Order { uris, position_ms } => {
            let request = PlayRequest {
                uris: uris.clone(),
                offset: PlayOffset { position: 0 },
                position_ms: *position_ms,
            };
            api.play(device_id, Some(&request)).await
        }
    }
}

/// Spotify's real queue. When few tracks remain, Spotify fills its answer to approximately 20
/// tracks: it repeats them (or the current track), as if they played again. The function cuts the
/// list at the first repetition.
fn without_padding(next: Vec<Track>, current: Option<&str>) -> Vec<Track> {
    let mut seen: HashSet<String> = current.into_iter().map(str::to_owned).collect();
    next.into_iter()
        .take_while(|track| seen.insert(track.uri.clone()))
        .collect()
}

/// True if `next`, the tracks that the page tells Spotify plays next (only the first two or
/// three), are the first tracks of `upcoming`. An empty `next` agrees only with an empty queue.
fn agrees_with_page(upcoming: &[Track], next: &[String]) -> bool {
    if next.is_empty() {
        return upcoming.is_empty();
    }
    next.len() <= upcoming.len()
        && next
            .iter()
            .zip(upcoming)
            .all(|(uri, track)| *uri == track.uri)
}

/// The URIs of Melodia's queue: the current track and the upcoming tracks. The list does not
/// include local files, because the Web API cannot play them.
fn queue_uris(current: &Track, upcoming: &[Track]) -> Vec<String> {
    std::iter::once(current)
        .chain(
            upcoming
                .iter()
                .filter(|track| !track.uri.starts_with("spotify:local:")),
        )
        .map(|track| track.uri.clone())
        .collect()
}

/// A volume that the user selected, for the volume task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VolumeChange {
    percent: u8,
    /// `false` when the player page already applied it (the music plays here). The task only
    /// saves it.
    to_spotify: bool,
}

/// Sends volume changes one at a time. While a request goes to Spotify, newer values replace the
/// value that waits. Thus a drag of the slider does not make a large number of requests. The task
/// saves each volume that gets to Spotify (or to the player page) in `config_dir`, for the next
/// time the app opens.
fn spawn_volume_task(
    api: SpotifyClient,
    sink: EventSink,
    mut wanted: watch::Receiver<Option<VolumeChange>>,
    config_dir: PathBuf,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while wanted.changed().await.is_ok() {
            let Some(VolumeChange {
                percent,
                to_spotify,
            }) = *wanted.borrow_and_update()
            else {
                continue;
            };
            let sent = if to_spotify {
                api.set_volume(percent).await
            } else {
                Ok(())
            };
            match sent {
                Ok(()) => {
                    let dir = config_dir.clone();
                    let saved =
                        tokio::task::spawn_blocking(move || config::save_volume(&dir, percent))
                            .await;
                    if let Ok(Err(error)) = saved {
                        log::warn!("Could not save the volume: {error}");
                    }
                }
                Err(error) => {
                    log::warn!("Could not set the volume: {error}");
                    let message = match &error {
                        ApiError::Status {
                            reason: Some(reason),
                            ..
                        } if reason == "VOLUME_CONTROL_DISALLOW" => {
                            "This device doesn't allow changing the volume from here.".to_owned()
                        }
                        error => error.user_message(),
                    };
                    sink.toast(ToastKind::Error, message);
                }
            }
        }
    })
}

/// Writes the answers that [`ResponseCache`] keeps to `config_dir` (see
/// [`LiveBackend::save_cache`]). It does not run on the backend's main task, because the write
/// waits for the disk. Newer contents replace contents that wait. The task writes the last
/// contents before it ends, when `wanted` closes.
fn spawn_cache_task(
    mut wanted: watch::Receiver<Option<Arc<Vec<u8>>>>,
    config_dir: PathBuf,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut written: Option<Arc<Vec<u8>>> = None;
        loop {
            let closed = wanted.changed().await.is_err();
            let contents = wanted.borrow_and_update().clone();
            if let Some(contents) = contents
                && !written
                    .as_ref()
                    .is_some_and(|old| Arc::ptr_eq(old, &contents))
            {
                let dir = config_dir.clone();
                let to_write = Arc::clone(&contents);
                let result = tokio::task::spawn_blocking(move || {
                    response_cache::write_saved(&dir, &to_write)
                })
                .await;
                match result {
                    Ok(Ok(())) => written = Some(contents),
                    Ok(Err(error)) => log::warn!("Could not save the cache: {error}"),
                    Err(error) => log::warn!("Could not save the cache: {error}"),
                }
            }
            if closed {
                break;
            }
        }
    })
}

/// Saves `wanted` (`what`, for the logs) to `config_dir` with `save`. It does not run on the
/// backend's main task, because the write waits for the disk. A newer value replaces a value that
/// waits. The task writes the last value before it ends, when `wanted` closes.
fn spawn_save_task<T>(
    mut wanted: watch::Receiver<Option<T>>,
    config_dir: PathBuf,
    what: &'static str,
    save: fn(&Path, Option<&T>) -> std::io::Result<()>,
) -> JoinHandle<()>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut saved = wanted.borrow_and_update().clone();
        loop {
            let closed = wanted.changed().await.is_err();
            let value = wanted.borrow_and_update().clone();
            if value != saved {
                let dir = config_dir.clone();
                let to_save = value.clone();
                let result =
                    tokio::task::spawn_blocking(move || save(&dir, to_save.as_ref())).await;
                match result {
                    Ok(Ok(())) => saved = value,
                    Ok(Err(error)) => log::warn!("Could not save {what}: {error}"),
                    Err(error) => log::warn!("Could not save {what}: {error}"),
                }
            }
            if closed {
                break;
            }
        }
    })
}

/// The active device, or else a computer (probably this one), or else any device.
fn preferred_device(devices: &[Device]) -> Option<&Device> {
    devices
        .iter()
        .find(|device| device.is_active)
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.kind.eq_ignore_ascii_case("computer"))
        })
        .or_else(|| devices.first())
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, kind: &str, is_active: bool) -> Device {
        Device {
            id: id.into(),
            name: id.to_uppercase(),
            kind: kind.into(),
            is_active,
            volume_percent: None,
            supports_volume: true,
        }
    }

    #[test]
    fn spotify_queue_padding_is_cut() {
        let track = |id: &str| Track {
            uri: format!("spotify:track:{id}"),
            name: id.into(),
            artists: vec![],
            album: String::new(),
            duration_ms: 0,
            cover_small: None,
            cover_large: None,
            explicit: false,
            album_id: None,
            artist_ids: vec![],
        };
        let ids = |tracks: Vec<Track>| tracks.into_iter().map(|t| t.name).collect::<String>();
        let tracks = |ids: &str| {
            ids.chars()
                .map(|id| track(&id.to_string()))
                .collect::<Vec<_>>()
        };
        let current = Some("spotify:track:a");
        // Two tracks remain: Spotify repeats them to fill its answer.
        assert_eq!(ids(without_padding(tracks("bcbcbc"), current)), "bc");
        // No tracks remain: Spotify repeats the current track.
        assert_eq!(ids(without_padding(tracks("aaaa"), current)), "");
        assert_eq!(ids(without_padding(tracks("bcd"), current)), "bcd");
    }

    #[test]
    fn local_files_are_not_sent_to_spotify() {
        let track = |uri: &str| Track {
            uri: uri.into(),
            name: String::new(),
            artists: vec![],
            album: String::new(),
            duration_ms: 0,
            cover_small: None,
            cover_large: None,
            explicit: false,
            album_id: None,
            artist_ids: vec![],
        };
        let uris = queue_uris(
            &track("spotify:track:a"),
            &[track("spotify:local:x"), track("spotify:track:b")],
        );
        assert_eq!(uris, ["spotify:track:a", "spotify:track:b"]);
    }

    #[test]
    fn prefers_active_then_computer_devices() {
        let devices = [
            device("phone", "Smartphone", false),
            device("pc", "Computer", false),
            device("speaker", "Speaker", true),
        ];
        assert_eq!(preferred_device(&devices).unwrap().id, "speaker");
        assert_eq!(preferred_device(&devices[..2]).unwrap().id, "pc");
        assert_eq!(preferred_device(&devices[..1]).unwrap().id, "phone");
        assert!(preferred_device(&[]).is_none());
    }

    #[test]
    fn liked_songs_need_their_permissions() {
        assert!(!has_library_scopes("streaming playlist-read-private"));
        assert!(!has_library_scopes("user-library-read"));
        assert!(has_library_scopes(
            "streaming user-library-read user-library-modify"
        ));
    }

    #[test]
    fn playlists_need_their_permissions() {
        assert!(!has_playlist_scopes(""));
        assert!(!has_playlist_scopes("streaming playlist-read-private"));
        assert!(has_playlist_scopes(
            "streaming playlist-read-private playlist-modify-private playlist-modify-public"
        ));
    }

    #[test]
    fn capitalizes_messages() {
        assert_eq!(capitalize("you cancelled"), "You cancelled");
        assert_eq!(capitalize(""), "");
    }
}
