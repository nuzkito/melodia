//! Messages between the UI thread and the backend.

use std::fmt;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use tokio::sync::mpsc as async_mpsc;

use crate::model::{
    Album, Artist, Device, Library, LibraryEdit, PlaybackSnapshot, Playlist, PlaylistEdit,
    PlaylistsStatus, QueueEdit, QueuePosition, QueueSnapshot, SearchPage, Track, UserProfile,
};
use crate::update::AvailableUpdate;

/// Requests from the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Starts the login flow. `client_id` replaces the Client ID in the configuration.
    Login {
        client_id: Option<String>,
    },
    CancelLogin,
    Logout,
    Search {
        query: String,
        offset: u32,
    },
    Resume,
    Pause,
    Next,
    Previous,
    Seek {
        position_ms: u64,
    },
    /// Plays `track` now. The upcoming queue stays.
    PlayTrack {
        track: Track,
    },
    /// Adds `track` at the end of the queue (or plays it, if nothing plays).
    AddToQueue {
        track: Track,
    },
    /// Changes the order of the queue or puts a track at a position (drag and drop).
    EditQueue(QueueEdit),
    /// Empties the full queue (previous, current and upcoming tracks) and stops the music.
    ClearQueue,
    /// Puts all the tracks of `playlist` in the queue, from `next[index]` (or plays them, if
    /// nothing plays).
    QueuePlaylist {
        playlist: Playlist,
        index: usize,
    },
    /// Changes the tracks of `playlist` (a playlist that the user can edit). It adds a track (at
    /// the end or at a given position), moves a track or removes a track.
    EditPlaylist {
        playlist: Playlist,
        edit: PlaylistEdit,
    },
    /// Asks if `playlist` already has `track` ([`Event::FoundInPlaylist`]), before the add at
    /// `index` (as [`PlaylistEdit::Insert`]). If it has the track, the UI asks the user first.
    /// The backend answers after the edits that it received before.
    FindInPlaylist {
        playlist: Playlist,
        index: Option<usize>,
        track: Track,
    },
    /// The user dropped the album `id` (with the name `name`) on `playlist`. The backend reads its
    /// tracks and the tracks that the playlist already has ([`Event::FoundAlbumInPlaylist`]).
    /// Thus the UI can ask the user before it adds repeated tracks. The backend answers after the
    /// edits that it received before.
    FindAlbumInPlaylist {
        playlist: Playlist,
        id: String,
        name: String,
    },
    /// Loads the tracks of `playlist`, to show them.
    LoadPlaylistTracks {
        playlist: Playlist,
    },
    /// Loads the album `id` with its tracks, to show it.
    LoadAlbum {
        id: String,
    },
    /// Loads the artist `id` with the list of their albums and singles ([`Event::Artist`]),
    /// without their tracks.
    LoadArtist {
        id: String,
    },
    /// Puts all the tracks of the album `id` (with the name `name`) in the queue, at `at` (or
    /// plays them, if nothing plays). The album comes from an artist view (dropped on the queue)
    /// or from the menu of an album.
    QueueAlbum {
        id: String,
        name: String,
        at: QueueAt,
    },
    /// Loads the user's playlists again.
    RefreshPlaylists,
    /// Creates an empty playlist with the name `name` in the user's library, in the folder
    /// `folder` of the sidebar (`None`: the first level).
    CreatePlaylist {
        name: String,
        folder: Option<String>,
    },
    /// Gives `playlist` (a playlist that the user can edit) the new name `name`.
    RenamePlaylist {
        playlist: Playlist,
        name: String,
    },
    /// The user changed the folders of the playlists or their order. Spotify does not have these,
    /// so the backend keeps them (see `playlist_library.rs`). The UI already applied the change.
    EditLibrary(LibraryEdit),
    /// Plays the queued track `uri`, which is at `position`.
    PlayFromQueue {
        position: QueuePosition,
        uri: String,
    },
    /// The volume of the active device, from 0 to 100.
    SetVolume {
        percent: u8,
    },
    TransferPlayback {
        device_id: String,
    },
    RefreshDevices,
    LoadImage {
        url: String,
        max_px: u32,
    },
    /// Asks if the tracks `uris` are in the user's «Liked Songs» ([`Event::LikedStates`]).
    CheckLiked {
        uris: Vec<String>,
    },
    /// Saves `track` to «Liked Songs» (`liked`) or removes it from there. The UI already shows the
    /// change, but not when it received [`Event::LikedNeedsReauth`]. In that case, the user first
    /// connects the account again, and [`Event::LikedChanged`] confirms the change.
    SetLiked {
        track: Track,
        liked: bool,
    },
    /// «Play in this app»: starts the embedded web player (if necessary) and moves the music to
    /// it.
    StartWebPlayer,
    /// Logs in again to get the permission that the web player needs, then starts it.
    ReauthorizeWebPlayer,
}

/// Where [`Command::QueueAlbum`] puts the tracks of an album.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAt {
    /// From `next[index]` (the user dropped the album there).
    Next(usize),
    /// At the end of the queue («Play last»).
    End,
    /// The first track plays now, and the other tracks go first in the queue, before the tracks
    /// that were there («Play»).
    Now,
}

/// The state of the embedded web player («Play in this app»).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WebPlayerStatus {
    #[default]
    Off,
    /// Not possible here (for example, not Windows). The text tells why.
    Unavailable(String),
    /// The saved session does not have the `streaming` permission.
    NeedsReauth,
    Starting,
    /// Connected to Spotify as the Connect device `device_id`.
    Ready {
        device_id: String,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthStatus {
    /// The UI waits for the backend to report the session state.
    Starting,
    /// A login is necessary. Without a Client ID, the UI first asks for one.
    LoggedOut {
        client_id: Option<String>,
        redirect_uri: String,
        error: Option<String>,
    },
    /// The browser is open. The app waits for the user to authorize it.
    WaitingForBrowser {
        auth_url: String,
        redirect_uri: String,
    },
    LoggedIn {
        demo: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub kind: ToastKind,
    pub message: String,
}

/// A decoded RGBA image, ready to upload as a texture.
#[derive(Clone, PartialEq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The dominant vivid color (it tints the UI).
    pub accent: Option<[u8; 3]>,
}

impl fmt::Debug for DecodedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("accent", &self.accent)
            .finish_non_exhaustive()
    }
}

/// Notifications from the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Auth(AuthStatus),
    Profile(UserProfile),
    Playback(PlaybackSnapshot),
    Queue(QueueSnapshot),
    /// The music plays on this other device (`Some`), not in this app. The UI shows where, in
    /// place of the player and the queue, which are empty. Melodia controls nothing there. The
    /// value is `None` when the music plays (or can play) here again.
    PlayingElsewhere(Option<Device>),
    Devices(Vec<Device>),
    Playlists(PlaylistsStatus),
    /// The folders and order of the playlists, after `edits` [`Command::EditLibrary`]s. The
    /// backend sends this immediately before each [`Event::Playlists`]. A value from before the
    /// UI's last edit can undo that edit on the screen.
    Library {
        library: Library,
        edits: u64,
    },
    /// The backend created the playlist `name` of [`Command::CreatePlaylist`], with the ID `id`.
    /// An [`Event::Playlists`] that shows it follows. `None`: the creation failed (a toast tells
    /// why).
    PlaylistCreated {
        name: String,
        id: Option<String>,
    },
    /// The tracks of the playlist `id`, after [`Command::LoadPlaylistTracks`] (or to undo an edit
    /// that Spotify refused).
    PlaylistTracks {
        id: String,
        tracks: Vec<Track>,
        /// The number of entries that the playlist has now. It can be different from
        /// [`Playlist::total`] if a different app changed the playlist.
        total: u32,
        /// The number of [`Command::EditPlaylist`]s that the backend received when it read the
        /// tracks. The UI applies its edits immediately and ignores older lists. Thus an edited
        /// playlist never jumps back.
        edits: u64,
    },
    PlaylistTracksFailed {
        id: String,
        message: String,
    },
    /// True if `playlist` has `track` (`found`), after [`Command::FindInPlaylist`], which this
    /// event repeats. If the backend could not read the playlist, `found` is false: the edit
    /// itself reports why.
    FoundInPlaylist {
        playlist: Playlist,
        index: Option<usize>,
        track: Track,
        found: bool,
    },
    /// The tracks of the album with the name `name`, and the URIs of the tracks that `playlist`
    /// already has (`found`), after [`Command::FindAlbumInPlaylist`]. If the backend could not
    /// read the playlist, `found` is empty: the edit itself reports why.
    FoundAlbumInPlaylist {
        playlist: Playlist,
        name: String,
        tracks: Vec<Track>,
        found: Vec<String>,
    },
    /// The album that [`Command::LoadAlbum`] requested.
    Album(Album),
    AlbumFailed {
        id: String,
        message: String,
    },
    /// The artist that [`Command::LoadArtist`] requested.
    Artist(Artist),
    ArtistFailed {
        id: String,
        message: String,
    },
    SearchResults(SearchPage),
    SearchFailed {
        query: String,
        offset: u32,
        message: String,
    },
    ImageLoaded {
        url: String,
        max_px: u32,
        image: DecodedImage,
    },
    ImageFailed {
        url: String,
        max_px: u32,
    },
    Toast(Toast),
    WebPlayer(WebPlayerStatus),
    /// True for each track (URI) that is in «Liked Songs», after [`Command::CheckLiked`]. The data
    /// can be older than a change that the user made here. Thus the UI keeps its own value for
    /// the tracks that the user changed.
    LikedStates(Vec<(String, bool)>),
    /// The track `uri` is (`liked`) or is not in «Liked Songs», whatever the UI shows. Spotify
    /// refused a [`Command::SetLiked`], or the backend applied it after the user connected the
    /// account again.
    LikedChanged {
        uri: String,
        liked: bool,
    },
    /// The saved session does not have the permissions for «Liked Songs». When the user saves a
    /// song, the app first connects the account again.
    LikedNeedsReauth,
    /// The update server tells about a version newer than this one. The check that runs beside
    /// each backend when the app opens sends it one time, at most.
    UpdateAvailable(AvailableUpdate),
}

/// The maximum time that [`BackendHandle::shut_down`] waits for the backend.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

/// The UI side of the channel.
pub struct BackendHandle {
    commands: async_mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    /// Tells when the backend's thread ended, if it has a thread (see
    /// [`BackendHandle::on_stopped`]).
    stopped: Option<mpsc::Receiver<()>>,
}

impl BackendHandle {
    /// `stopped` gets a message (or closes) when the backend ends.
    pub(crate) fn on_stopped(&mut self, stopped: mpsc::Receiver<()>) {
        self.stopped = Some(stopped);
    }

    /// Stops the backend and waits (up to [`SHUTDOWN_WAIT`]) for it to end. Thus the data that it
    /// must save (the cache, the playback position) gets to the disk before the process ends. The
    /// handle drops later commands.
    pub fn shut_down(&mut self) {
        // When the channel closes, the backend stops.
        let (closed, _) = async_mpsc::unbounded_channel();
        drop(std::mem::replace(&mut self.commands, closed));
        if let Some(stopped) = self.stopped.take() {
            let _ = stopped.recv_timeout(SHUTDOWN_WAIT);
        }
    }

    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            log::warn!("The backend has stopped; command dropped");
        }
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

/// The backend side of the channel. It wakes the UI up after each event.
#[derive(Clone)]
pub struct EventSink {
    events: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Drop for BackendHandle {
    fn drop(&mut self) {
        self.shut_down();
    }
}

impl EventSink {
    pub fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            (self.wake)();
        }
    }

    pub fn toast(&self, kind: ToastKind, message: impl Into<String>) {
        self.send(Event::Toast(Toast {
            kind,
            message: message.into(),
        }));
    }
}

pub type CommandReceiver = async_mpsc::UnboundedReceiver<Command>;

/// Creates a connected UI handle, event sink and command receiver.
pub fn channel(
    wake: impl Fn() + Send + Sync + 'static,
) -> (BackendHandle, EventSink, CommandReceiver) {
    let (command_tx, command_rx) = async_mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    (
        BackendHandle {
            commands: command_tx,
            events: event_rx,
            stopped: None,
        },
        EventSink {
            events: event_tx,
            wake: Arc::new(wake),
        },
        command_rx,
    )
}
