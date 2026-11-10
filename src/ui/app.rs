use std::collections::HashSet;
use std::mem::take;
use std::time::{Duration, Instant};

use egui::{
    Color32, ColorImage, Context, Id, Key, Modifiers, PointerButton, Rect, TextureOptions, Ui,
    UiBuilder, ViewportCommand, pos2, vec2,
};

use super::account_menu::{self, AccountMenu};
use super::album::{AlbumData, AlbumView};
use super::anim::{self, ease_in_out_cubic};
use super::artist::ArtistView;
use super::backdrop;
use super::changelog::Changelog;
use super::devices::{DevicesData, DevicesView};
use super::dnd;
use super::guide;
use super::icons;
use super::liked::LikedSongs;
use super::login::{LoginView, LogoTextures, SPOTIFY_LOGO_PX};
use super::navigation::{History, Place};
use super::playlist_adds::{AlbumAdd, PlaylistAdd, PlaylistAdds};
use super::playlist_tracks::{PlaylistTracksData, PlaylistTracksView};
use super::playlists::{self, PlaylistsData, PlaylistsView};
use super::queue::{QueueData, QueueView};
use super::search::{SearchData, SearchView};
use super::spotify_logo;
use super::textures::{THUMB_PX, TextureCache};
use super::theme;
use super::toasts::Toasts;
use super::widgets;
use super::window_frame;
use crate::backend::{AuthStatus, BackendHandle, Command, Event, ToastKind, WebPlayerStatus};
use crate::config::APP_NAME;
use crate::icon;
use crate::model::{
    Device, Library, PlaybackSnapshot, Playlist, PlaylistEdit, PlaylistsStatus, QueueSnapshot,
    Track, UserProfile,
};
use crate::update::AvailableUpdate;

/// The volume step of the keyboard shortcuts, in percent.
const VOLUME_STEP: u8 = 10;
const SEARCH_HEIGHT: f32 = 44.0;
/// «Back» and «Forward», left of the search box.
const NAV_BUTTON: f32 = 32.0;
const NAV_GAP: f32 = 4.0;
/// The maximum width of the queue column. If it is wider, its rows (and the volume bar) look
/// stretched on maximized windows.
const QUEUE_MAX_WIDTH: f32 = 600.0;

/// The full application UI. It does no I/O: it shows the latest state from the backend and sends
/// [`Command`]s back.
pub struct MusicApp {
    backend: BackendHandle,
    demo: bool,
    auth: AuthStatus,
    profile: Option<UserProfile>,
    playback: PlaybackSnapshot,
    /// The different device where the music plays, while it is not in this app.
    elsewhere: Option<Device>,
    queue: QueueSnapshot,
    /// The number of queue edits sent. Snapshots from before them are old.
    queue_edits_sent: u64,
    /// The number of playlist edits sent. Track lists from before them are old.
    playlist_edits_sent: u64,
    devices: Vec<Device>,
    web_player: WebPlayerStatus,
    playlists: PlaylistsStatus,
    /// The folders and order of the playlists. Until the user (or the backend, with
    /// [`Event::Library`]) arranges them, it follows Spotify's order (`library_custom` false).
    library: Library,
    library_custom: bool,
    /// The number of library edits sent. Libraries from before them are old. The UI shows lists
    /// that load in the meantime with its own library.
    library_edits_sent: u64,
    textures: TextureCache,
    /// The songs that are in «Liked Songs» (the hearts of the rows).
    liked: LikedSongs,
    login: LoginView,
    account_menu: AccountMenu,
    /// The changelog dialog, which opens from the «☰» menu.
    changelog: Changelog,
    /// A newer version of the app, which the «☰» menu offers.
    update: Option<AvailableUpdate>,
    /// The device picker, left of the «☰» menu.
    devices_view: DevicesView,
    search: SearchView,
    queue_view: QueueView,
    playlists_view: PlaylistsView,
    playlist_tracks: PlaylistTracksView,
    /// Asks the user before it adds a track to a playlist that already has it.
    playlist_adds: PlaylistAdds,
    album: AlbumView,
    artist: ArtistView,
    /// The playlists, albums, artists and searches that the middle column showed, for «Back» and
    /// «Forward».
    history: History,
    toasts: Toasts,
    accent: AccentTransition,
    logo: Option<LogoTextures>,
    window_title: String,
    installed: bool,
    /// «/» opened the search. Its text can arrive one frame later, and the box must not type it.
    swallow_slash: bool,
}

/// The panels that alternate under the search box.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MiddlePanel {
    Search,
    Playlist,
    Album,
    Artist,
}

/// The accent color follows the cover of the current track, with a smooth blend from the previous
/// color.
struct AccentTransition {
    from: Color32,
    to: Color32,
    since: f64,
}

const ACCENT_TRANSITION: f32 = 1.0;

impl MusicApp {
    pub fn new(backend: BackendHandle, demo: bool) -> Self {
        Self {
            backend,
            demo,
            auth: AuthStatus::Starting,
            profile: None,
            playback: PlaybackSnapshot::idle(),
            elsewhere: None,
            queue: QueueSnapshot::default(),
            queue_edits_sent: 0,
            playlist_edits_sent: 0,
            devices: Vec::new(),
            web_player: WebPlayerStatus::Off,
            playlists: PlaylistsStatus::Loading,
            library: Library::default(),
            library_custom: false,
            library_edits_sent: 0,
            textures: TextureCache::default(),
            liked: LikedSongs::default(),
            login: LoginView::default(),
            account_menu: AccountMenu::default(),
            changelog: Changelog::default(),
            update: None,
            devices_view: DevicesView::default(),
            search: SearchView::default(),
            queue_view: QueueView::default(),
            playlists_view: PlaylistsView::default(),
            playlist_tracks: PlaylistTracksView::default(),
            playlist_adds: PlaylistAdds::default(),
            album: AlbumView::default(),
            artist: ArtistView::default(),
            history: History::default(),
            toasts: Toasts::default(),
            accent: AccentTransition {
                from: theme::DEFAULT_ACCENT,
                to: theme::DEFAULT_ACCENT,
                since: f64::NEG_INFINITY,
            },
            logo: None,
            window_title: APP_NAME.to_owned(),
            installed: false,
            swallow_slash: false,
        }
    }

    /// Stops the backend when the app closes. It waits a moment for the data that the backend must
    /// still save.
    pub fn shut_down(&mut self) {
        self.backend.shut_down();
    }

    /// Installs fonts, style and the logo textures. `main` calls it before the first frame. If
    /// not, [`Self::show`] installs the theme when necessary, and the logo when the login screen
    /// first needs it.
    pub fn install(&mut self, ctx: &Context) {
        theme::install(ctx);
        self.installed = true;
        self.load_logo(ctx);
    }

    /// The logo of the login screen, rendered the first time that it is necessary. The render is
    /// slow in debug builds, and most UI tests never show the login screen.
    fn load_logo(&mut self, ctx: &Context) {
        if self.logo.is_some() {
            return;
        }
        let texture = |name: &str, rgba: Vec<u8>, size: u32| {
            let image = ColorImage::from_rgba_unmultiplied([size as usize, size as usize], &rgba);
            ctx.load_texture(name, image, TextureOptions::LINEAR)
        };
        let layers = icon::render_layers(192);
        self.logo = Some(LogoTextures {
            circle: texture("logo-circle", layers.circle, 192),
            note: texture("logo-note", layers.note, 192),
            border: texture("logo-border", layers.border, 192),
            spotify: ctx.load_texture(
                "spotify-logo",
                spotify_logo::render(SPOTIFY_LOGO_PX),
                TextureOptions::LINEAR,
            ),
        });
    }

    pub fn auth(&self) -> &AuthStatus {
        &self.auth
    }

    pub fn toast_messages(&self) -> impl Iterator<Item = &str> {
        self.toasts.messages()
    }

    /// Draws one frame. `eframe::App::ui` and the UI tests use it.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.installed {
            theme::install(&ctx);
            self.installed = true;
        }
        let time = ctx.input(|i| i.time);
        self.textures.begin_frame();
        let mut actions = Vec::new();
        self.process_events(&ctx, time, &mut actions);
        self.liked.publish(&ctx);

        self.changelog.keyboard(&ctx);
        self.playlist_adds.keyboard(&ctx);
        self.shortcuts(&ctx, &mut actions);

        let rect = ui.max_rect();
        let accent = self.accent(&ctx, time);
        backdrop::paint(ui, rect, accent);
        theme::tint_scroll_bars(ui.style_mut(), accent);
        // Under all other items: the widgets in the top band keep their clicks.
        window_frame::drag_area(ui, rect);

        let logged_in = matches!(self.auth, AuthStatus::LoggedIn { .. });
        let screen = anim::tween_with(
            &ctx,
            Id::unique("screen"),
            if logged_in { 1.0 } else { 0.0 },
            0.6,
            ease_in_out_cubic,
        );
        if screen < 0.999 && self.auth != AuthStatus::Starting {
            let login_rect = rect.translate(vec2(0.0, -24.0 * screen));
            let mut login_ui = ui.new_child(
                UiBuilder::new()
                    .id_salt("login-screen")
                    .max_rect(login_rect),
            );
            login_ui.set_opacity(1.0 - screen);
            self.load_logo(&ctx);
            let logo = self.logo.as_ref();
            self.login.show(
                &mut login_ui,
                login_rect,
                &self.auth,
                logo,
                self.demo,
                &mut actions,
            );
        }
        let over_target = if screen > 0.001 {
            let main_rect = rect.translate(vec2(0.0, 24.0 * (1.0 - screen)));
            let mut main_ui =
                ui.new_child(UiBuilder::new().id_salt("main-screen").max_rect(main_rect));
            main_ui.set_opacity(screen);
            self.main_screen(&mut main_ui, main_rect, accent, &mut actions)
        } else {
            false
        };
        window_frame::controls(ui, rect);
        if screen > 0.001 {
            let mut menu_ui = ui.new_child(UiBuilder::new().id_salt("account-menu").max_rect(rect));
            menu_ui.set_opacity(screen);
            let menu_button = self.account_menu.show(
                &mut menu_ui,
                rect,
                self.profile.as_ref(),
                self.update.as_ref(),
                accent,
                &mut actions,
                &mut self.changelog,
            );
            let left = match &self.update {
                Some(update) => {
                    account_menu::update_button(&mut menu_ui, menu_button, update, accent).left()
                }
                None => menu_button.left(),
            };
            let devices_button = menu_button.translate(vec2(left - menu_button.right(), 0.0));
            let data = DevicesData {
                playback: &self.playback,
                elsewhere: self.elsewhere.as_ref(),
                devices: &self.devices,
                web_player: &self.web_player,
                accent,
            };
            let devices_button =
                self.devices_view
                    .show(&mut menu_ui, devices_button, &data, &mut actions);
            if self.demo {
                account_menu::demo_badge(&menu_ui, devices_button, accent);
            }
        }
        window_frame::resize_borders(ui, rect);
        window_frame::paint_border(ui, rect);
        dnd::paint_ghost(&ctx, &mut self.textures, accent, over_target);
        if logged_in {
            self.playlist_adds.show(&ctx, accent);
            self.changelog.show(&ctx, accent);
        }
        for (kind, message) in super::toasts::take_requests(&ctx) {
            self.toasts.push(kind, message, time);
        }
        self.toasts.show(&ctx, time, accent);

        for (url, max_px) in self.textures.take_requests() {
            self.backend.send(Command::LoadImage { url, max_px });
        }
        actions.extend(self.liked.collect(&ctx, time));
        for mut add in self.playlist_adds.take_confirmed() {
            self.refresh_playlist(add.playlist_mut());
            self.send(add.command(), time);
        }
        for action in actions {
            self.dispatch(action, time);
        }
        self.update_window_title(&ctx);
        if logged_in && self.playback.is_playing {
            // Keeps the progress bar and the equalizer in motion. Without animations, only the
            // elapsed time must change.
            let interval = match (anim::enabled(&ctx), ctx.input(|i| i.focused)) {
                (true, true) => 33,
                (true, false) => 250,
                (false, _) => 1000,
            };
            ctx.request_repaint_after(Duration::from_millis(interval));
        }
    }

    fn process_events(&mut self, ctx: &Context, time: f64, actions: &mut Vec<Command>) {
        while let Some(event) = self.backend.try_recv() {
            match event {
                Event::Auth(status) => {
                    self.login.on_status(&status, ctx);
                    // A different account, or new permissions: ask again.
                    self.liked.reset();
                    if !matches!(status, AuthStatus::LoggedIn { .. }) {
                        self.profile = None;
                        self.elsewhere = None;
                        self.search = SearchView::default();
                        self.queue_view = QueueView::default();
                        self.devices_view = DevicesView::default();
                        self.playlist_tracks = PlaylistTracksView::default();
                        self.playlist_adds = PlaylistAdds::default();
                        self.album = AlbumView::default();
                        self.artist = ArtistView::default();
                        self.history = History::default();
                    }
                    // A new login (for a new permission) keeps them.
                    if matches!(status, AuthStatus::LoggedOut { .. }) {
                        self.playlists = PlaylistsStatus::Loading;
                        self.library = Library::default();
                        self.library_custom = false;
                        self.library_edits_sent = 0;
                        self.playlists_view = PlaylistsView::default();
                    }
                    self.auth = status;
                }
                Event::Profile(profile) => self.profile = Some(profile),
                Event::Playback(playback) => self.playback = playback,
                Event::PlayingElsewhere(device) => self.elsewhere = device,
                // A snapshot from before the last edit would undo that edit on the screen.
                Event::Queue(queue) if queue.edits < self.queue_edits_sent => {}
                Event::Queue(queue) => self.queue = queue,
                Event::Devices(devices) => self.devices = devices,
                Event::WebPlayer(status) => self.web_player = status,
                // A library from before the last edit would undo that edit on the screen.
                Event::Library { edits, .. } if edits < self.library_edits_sent => {}
                Event::Library { library, .. } => {
                    self.library = library;
                    self.library_custom = true;
                }
                Event::Playlists(mut status) => {
                    if let PlaylistsStatus::Loaded(playlists) = &mut status {
                        if !self.library_custom {
                            self.library = Library::default();
                        }
                        *playlists = self.library.arrange(take(playlists));
                        self.playlist_tracks.on_playlists(playlists, actions);
                    }
                    self.playlists = status;
                }
                Event::PlaylistCreated { name, id } => {
                    self.playlists_view.on_created(&name, id);
                }
                // A list from before the last edit would undo that edit on the screen.
                Event::PlaylistTracks { id, edits, .. } if edits < self.playlist_edits_sent => {
                    self.playlist_tracks.on_outdated(&id);
                }
                Event::PlaylistTracks {
                    id, tracks, total, ..
                } => {
                    self.playlist_adds.on_read(&id);
                    // A different app can add or remove tracks.
                    if let PlaylistsStatus::Loaded(playlists) = &mut self.playlists
                        && let Some(playlist) = playlists.iter_mut().find(|p| p.id == id)
                    {
                        playlist.total = total;
                    }
                    self.playlist_tracks.on_tracks(&id, tracks, total, time);
                }
                Event::PlaylistTracksFailed { id, message } => {
                    self.playlist_tracks.on_failed(&id, message);
                }
                Event::FoundInPlaylist {
                    playlist,
                    index,
                    track,
                    found,
                } => {
                    let mut add = PlaylistAdd {
                        playlist,
                        index,
                        track,
                    };
                    self.refresh_playlist(&mut add.playlist);
                    // The user can add it from here while the backend looks.
                    if found || self.has_track(&add.playlist, &add.track.uri) == Some(true) {
                        self.playlist_adds.ask(add);
                    } else {
                        self.send(PlaylistAdds::command(add), time);
                    }
                }
                Event::FoundAlbumInPlaylist {
                    playlist,
                    name,
                    tracks,
                    found,
                } => self.on_album_for_playlist(playlist, name, tracks, found, time),
                Event::Album(album) => self.album.on_album(album, time),
                Event::AlbumFailed { id, message } => self.album.on_failed(&id, message),
                Event::Artist(artist) => self.artist.on_artist(artist, time),
                Event::ArtistFailed { id, message } => self.artist.on_failed(&id, message),
                Event::SearchResults(page) => self.search.on_results(page, time),
                Event::SearchFailed {
                    query,
                    offset,
                    message,
                } => {
                    self.search.on_search_failed(&query, offset, message);
                }
                Event::ImageLoaded { url, max_px, image } => {
                    self.textures.insert(ctx, url, max_px, &image, time);
                }
                Event::ImageFailed { url, max_px } => self.textures.mark_failed(url, max_px, time),
                Event::Toast(toast) => self.toasts.push(toast.kind, toast.message, time),
                Event::LikedStates(states) => self.liked.on_states(states),
                Event::LikedChanged { uri, liked } => self.liked.on_changed(uri, liked),
                Event::LikedNeedsReauth => self.liked.on_needs_reauth(),
                Event::UpdateAvailable(update) => {
                    let message = format!("Melodia {} is available", update.version);
                    self.toasts.push(ToastKind::Info, message, time);
                    self.update = Some(update);
                }
            }
        }
    }

    /// Sends a command from the views. To add a track to a playlist that possibly has it already,
    /// it first waits for that information.
    fn dispatch(&mut self, command: Command, time: f64) {
        match command {
            Command::EditPlaylist {
                playlist,
                edit: PlaylistEdit::Insert { index, track },
            } if playlist.editable => {
                let add = PlaylistAdd {
                    playlist,
                    index,
                    track,
                };
                match self.has_track(&add.playlist, &add.track.uri) {
                    Some(true) => self.playlist_adds.ask(add),
                    Some(false) => self.send(PlaylistAdds::command(add), time),
                    None => self.backend.send(Command::FindInPlaylist {
                        playlist: add.playlist,
                        index: add.index,
                        track: add.track,
                    }),
                }
            }
            command => self.send(command, time),
        }
    }

    /// True if `playlist` has the track `uri`, if this is known here.
    fn has_track(&self, playlist: &Playlist, uri: &str) -> Option<bool> {
        if self.playlist_adds.was_added(&playlist.id, uri) {
            return Some(true);
        }
        self.playlist_tracks.has(playlist, uri)
    }

    /// `playlist` as it is in the list now. Its count can change after the request.
    fn refresh_playlist(&self, playlist: &mut Playlist) {
        if let PlaylistsStatus::Loaded(playlists) = &self.playlists
            && let Some(listed) = playlists.iter().find(|p| p.id == playlist.id)
        {
            playlist.clone_from(listed);
        }
    }

    /// The tracks of the album with the name `name`, dropped on `playlist`, which has the tracks
    /// in `found`. The UI adds them, but if some tracks repeat, it first asks the user.
    fn on_album_for_playlist(
        &mut self,
        mut playlist: Playlist,
        name: String,
        tracks: Vec<Track>,
        found: Vec<String>,
        time: f64,
    ) {
        self.refresh_playlist(&mut playlist);
        if tracks.is_empty() {
            self.toasts.push(
                ToastKind::Error,
                format!("“{name}” has no songs to add."),
                time,
            );
            return;
        }
        // The user can add some from here while the backend looks.
        let repeated: HashSet<String> = tracks
            .iter()
            .filter(|track| {
                found.contains(&track.uri) || self.has_track(&playlist, &track.uri) == Some(true)
            })
            .map(|track| track.uri.clone())
            .collect();
        if repeated.is_empty() {
            self.send(
                Command::EditPlaylist {
                    playlist,
                    edit: PlaylistEdit::Append { tracks },
                },
                time,
            );
        } else {
            self.playlist_adds.ask_album(AlbumAdd {
                playlist,
                name,
                tracks,
                repeated,
            });
        }
    }

    /// Sends a command. When the result is clear, it first updates the local state, so that the
    /// interface changes in the same frame.
    fn send(&mut self, command: Command, time: f64) {
        match &command {
            Command::Pause => self.set_playing_locally(false),
            Command::Resume => self.set_playing_locally(true),
            // The queue tells it if the track goes out of view.
            Command::AddToQueue { .. } => self.queue_view.expect_arrival(),
            Command::EditQueue(edit) => {
                // A dropped track stays at the drop position.
                if self.queue.current.is_some() {
                    let fixed = self.queue.fixed();
                    let queue = &mut self.queue;
                    edit.apply(
                        &mut queue.previous,
                        queue.current.as_ref(),
                        &mut queue.next,
                        fixed,
                    );
                }
                self.queue_edits_sent += 1;
            }
            Command::EditPlaylist { playlist, edit } => {
                // The track shows in its new position (or goes away) immediately, and the count of
                // the playlist changes.
                self.playlist_tracks.on_edit(playlist, edit);
                if let PlaylistsStatus::Loaded(playlists) = &mut self.playlists
                    && let Some(listed) = playlists
                        .iter_mut()
                        .find(|p| p.id == playlist.id && p.editable)
                {
                    listed.total = edit.total_after(listed.total);
                }
                match edit {
                    PlaylistEdit::Insert { index, track } => {
                        self.playlist_adds.on_added(&playlist.id, &track.uri);
                        // Dropped on the sidebar: its row shows a ✓.
                        if index.is_none() && playlist.editable {
                            self.playlists_view.on_added(&playlist.id, time);
                        }
                    }
                    PlaylistEdit::Append { tracks } => {
                        for track in tracks {
                            self.playlist_adds.on_added(&playlist.id, &track.uri);
                        }
                        if playlist.editable {
                            self.playlists_view.on_added(&playlist.id, time);
                        }
                    }
                    PlaylistEdit::Remove { uri, .. } => {
                        self.playlist_adds.on_removed(&playlist.id, uri);
                    }
                    PlaylistEdit::Move { .. } => {}
                }
                self.playlist_edits_sent += 1;
            }
            Command::RenamePlaylist { playlist, name } => {
                // The new name shows immediately, in the sidebar and the panel.
                self.playlist_tracks.on_rename(&playlist.id, name);
                if let PlaylistsStatus::Loaded(playlists) = &mut self.playlists
                    && let Some(listed) = playlists.iter_mut().find(|p| p.id == playlist.id)
                {
                    listed.name.clone_from(name);
                }
            }
            Command::EditLibrary(edit) => {
                // It shows immediately.
                self.library.apply(edit);
                self.library_custom = true;
                self.library_edits_sent += 1;
                if let PlaylistsStatus::Loaded(playlists) = &mut self.playlists {
                    *playlists = self.library.arrange(take(playlists));
                }
            }
            Command::ClearQueue => {
                self.queue = QueueSnapshot {
                    edits: self.queue.edits,
                    ..QueueSnapshot::default()
                };
                self.playback.track = None;
                self.playback.is_playing = false;
                self.playback.progress_ms = 0;
                self.queue_edits_sent += 1;
            }
            // The queue card shows the new position immediately.
            Command::Seek { position_ms } if self.playback.track.is_some() => {
                self.playback.progress_ms = *position_ms;
                self.playback.measured_at = Instant::now();
            }
            // The heart fills (or empties) immediately.
            Command::SetLiked { track, liked } => self.liked.on_toggle(&track.uri, *liked),
            Command::SetVolume { percent } => {
                if let Some(device) = &mut self.playback.device {
                    device.volume_percent = Some(*percent);
                }
            }
            _ => {}
        }
        self.backend.send(command);
    }

    fn set_playing_locally(&mut self, playing: bool) {
        if self.playback.track.is_none() {
            return;
        }
        let now = Instant::now();
        self.playback.progress_ms = self.playback.progress_at(now);
        self.playback.measured_at = now;
        self.playback.is_playing = playing;
    }

    fn shortcuts(&mut self, ctx: &Context, actions: &mut Vec<Command>) {
        if !matches!(self.auth, AuthStatus::LoggedIn { .. })
            || self.playlist_adds.is_asking()
            || self.changelog.is_open()
        {
            return;
        }
        let typing = ctx.text_edit_focused();
        let pressed =
            |modifiers: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if !typing && pressed(Modifiers::NONE, Key::Space) && self.playback.track.is_some() {
            actions.push(if self.playback.is_playing {
                Command::Pause
            } else {
                Command::Resume
            });
        }
        let slash = !typing && pressed(Modifiers::NONE, Key::Slash);
        self.swallow_slash |= slash;
        if self.swallow_slash {
            // The key also arrives as text (in this frame or a later frame), which the box with the
            // new focus would type. Until the key is released, the UI ignores that text.
            ctx.input_mut(|i| {
                i.events.retain(|event| match event {
                    egui::Event::Text(text) if text == "/" => {
                        self.swallow_slash = false;
                        false
                    }
                    egui::Event::Key {
                        key: Key::Slash,
                        pressed: false,
                        ..
                    } => {
                        self.swallow_slash = false;
                        true
                    }
                    _ => true,
                });
            });
        }
        if pressed(Modifiers::COMMAND, Key::F) || slash {
            self.search.focus();
        }
        // Shift first: Ctrl alone would also match Ctrl+Shift.
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let has_track = self.playback.track.is_some();
        if pressed(ctrl_shift, Key::ArrowRight) && has_track {
            actions.push(Command::Next);
        }
        if pressed(ctrl_shift, Key::ArrowLeft) && has_track {
            actions.push(Command::Previous);
        }
        // As in a web browser: Ctrl+←/→ and the side buttons of the mouse.
        let time = ctx.input(|i| i.time);
        let mouse = |button| ctx.input(|i| i.pointer.button_pressed(button));
        if pressed(Modifiers::COMMAND, Key::ArrowLeft) || mouse(PointerButton::Extra1) {
            self.navigate(false, time, actions);
        }
        if pressed(Modifiers::COMMAND, Key::ArrowRight) || mouse(PointerButton::Extra2) {
            self.navigate(true, time, actions);
        }
        let volume = self
            .playback
            .device
            .as_ref()
            .filter(|device| device.supports_volume)
            .and_then(|device| device.volume_percent);
        if let Some(volume) = volume {
            if pressed(Modifiers::COMMAND, Key::ArrowUp) {
                actions.push(Command::SetVolume {
                    percent: volume.saturating_add(VOLUME_STEP).min(100),
                });
            }
            if pressed(Modifiers::COMMAND, Key::ArrowDown) {
                actions.push(Command::SetVolume {
                    percent: volume.saturating_sub(VOLUME_STEP),
                });
            }
        }
    }

    /// Goes back (or forward) to the playlist or search that showed before. It skips playlists
    /// that are gone.
    fn navigate(&mut self, forward: bool, time: f64, actions: &mut Vec<Command>) {
        let playlists = match &self.playlists {
            PlaylistsStatus::Loaded(playlists) => playlists.as_slice(),
            _ => &[],
        };
        let available = |place: &Place| match place {
            Place::Playlist(id) => playlists.iter().any(|p| &p.id == id),
            Place::Home | Place::Search(_) | Place::Album { .. } | Place::Artist { .. } => true,
        };
        let Some(place) = self.history.go(forward, available) else {
            return;
        };
        match place {
            Place::Home => self.close_middle_panels(None),
            Place::Playlist(id) => {
                if let Some(playlist) = playlists.iter().find(|p| p.id == id) {
                    let playlist = playlist.clone();
                    self.close_middle_panels(Some(MiddlePanel::Playlist));
                    self.playlist_tracks.open(playlist, time, actions);
                }
            }
            Place::Album { id, name } => self.open_album(id, name, time, actions),
            Place::Artist { id, name } => self.open_artist(id, name, time, actions),
            Place::Search(query) => {
                self.search.restore(query, actions);
                self.close_middle_panels(Some(MiddlePanel::Search));
            }
        }
    }

    /// Closes all the panels of the middle column, but not `keep`.
    fn close_middle_panels(&mut self, keep: Option<MiddlePanel>) {
        let closes = |panel| keep != Some(panel);
        if closes(MiddlePanel::Search) {
            self.search.clear();
        }
        if closes(MiddlePanel::Playlist) {
            self.playlist_tracks.close();
        }
        if closes(MiddlePanel::Album) {
            self.album.close();
        }
        if closes(MiddlePanel::Artist) {
            self.artist.close();
        }
    }

    /// Shows the album `id` (with the name `name`) in the middle column, in place of the search or
    /// the playlist.
    fn open_album(&mut self, id: String, name: String, time: f64, actions: &mut Vec<Command>) {
        self.close_middle_panels(Some(MiddlePanel::Album));
        self.album.open(id, name, time, actions);
    }

    /// Shows the artist `id` (with the name `name`) in the middle column, in place of the search,
    /// the playlist or the album.
    fn open_artist(&mut self, id: String, name: String, time: f64, actions: &mut Vec<Command>) {
        self.close_middle_panels(Some(MiddlePanel::Artist));
        self.artist.open(id, name, time, actions);
    }

    /// The contents of the middle column, for the history.
    fn place(&self) -> Place {
        if self.search.is_open() {
            Place::Search(self.search.query().to_owned())
        } else if let Some(id) = self.playlist_tracks.open_id() {
            Place::Playlist(id.to_owned())
        } else if let Some((id, name)) = self.album.open_id() {
            Place::Album {
                id: id.to_owned(),
                name: name.to_owned(),
            }
        } else if let Some((id, name)) = self.artist.open_id() {
            Place::Artist {
                id: id.to_owned(),
                name: name.to_owned(),
            }
        } else {
            Place::Home
        }
    }

    /// «Back» and «Forward», with their center on `y`, from `left`.
    fn navigation_buttons(
        &mut self,
        ui: &mut Ui,
        left: f32,
        y: f32,
        time: f64,
        actions: &mut Vec<Command>,
    ) {
        for (i, forward) in [false, true].into_iter().enumerate() {
            let (label, enabled) = if forward {
                ("Forward", self.history.can_go_forward())
            } else {
                ("Back", self.history.can_go_back())
            };
            let x = left + NAV_BUTTON / 2.0 + i as f32 * (NAV_BUTTON + NAV_GAP);
            let response = widgets::icon_button(
                ui,
                pos2(x, y),
                NAV_BUTTON,
                Id::unique(("navigation", forward)),
                label,
                enabled,
                |painter, center, color| icons::chevron(painter, center, 16.0, forward, color),
            );
            if response.clicked() {
                self.navigate(forward, time, actions);
            }
        }
    }

    fn accent(&mut self, ctx: &Context, time: f64) -> Color32 {
        // The thumbnail that the now-playing card already shows. The accent comes from a 32 px
        // version in all cases, and a large cover would use approximately 2 MB of texture for
        // each track.
        let target = self
            .playback
            .track
            .as_ref()
            .and_then(|track| track.cover_small.as_deref())
            .and_then(|url| self.textures.get(url, THUMB_PX, time))
            .and_then(|cover| cover.accent)
            .unwrap_or(theme::DEFAULT_ACCENT);
        let current = self.accent_value(ctx);
        if target != self.accent.to {
            self.accent = AccentTransition {
                from: current,
                to: target,
                since: time,
            };
        }
        self.accent_value(ctx)
    }

    fn accent_value(&self, ctx: &Context) -> Color32 {
        let elapsed = anim::elapsed_since(ctx, self.accent.since);
        anim::keep_animating(ctx, elapsed, ACCENT_TRANSITION);
        let t = ease_in_out_cubic(elapsed / ACCENT_TRANSITION);
        anim::lerp_color(self.accent.from, self.accent.to, t)
    }

    /// Draws the playlists, the search and the queue. Returns true if a dragged item is over a
    /// position where the user can drop it.
    fn main_screen(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        accent: Color32,
        actions: &mut Vec<Command>,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        // The content starts just under the window buttons.
        let top = rect.top() + window_frame::BUTTON_HEIGHT + 4.0;
        let body = Rect::from_min_max(pos2(rect.left() + 32.0, top), rect.max - vec2(32.0, 20.0));
        // Playlists on the left, then the search and the queue.
        let sidebar_width = (body.width() * 0.2).clamp(190.0, 250.0);
        let sidebar = Rect::from_min_size(body.min, vec2(sidebar_width, body.height()));
        let rest = Rect::from_min_max(pos2(sidebar.right() + 32.0, body.top()), body.max);
        // The queue stops at QUEUE_MAX_WIDTH, so on wide windows the middle column gets the
        // remaining width.
        let middle_width = (rest.width() * 0.42)
            .clamp(300.0, 450.0)
            .max(rest.width() - 40.0 - QUEUE_MAX_WIDTH);
        let middle = Rect::from_min_size(rest.min, vec2(middle_width, rest.height()));
        let queue_column = Rect::from_min_max(pos2(middle.right() + 40.0, rest.top()), rest.max);

        let mut sidebar_ui = ui.new_child(UiBuilder::new().id_salt("playlists").max_rect(sidebar));
        let data = PlaylistsData {
            status: &self.playlists,
            library: &self.library,
            selected: self.playlist_tracks.open_id(),
            accent,
        };
        let over_playlist =
            self.playlists_view
                .show(&mut sidebar_ui, sidebar, &data, &mut self.textures, actions);
        if let Some(playlist) = self.playlists_view.take_clicked() {
            // Its tracks replace the search results.
            self.close_middle_panels(Some(MiddlePanel::Playlist));
            self.playlist_tracks.toggle(playlist, time, actions);
        }

        // Middle column: the search box, and under it the search results (while there is a
        // query), the tracks of the selected playlist, or the album or artist that the user
        // clicked under a track. The search row has the same center as the «New playlist» button
        // of the playlists.
        let nav_width = 2.0 * NAV_BUTTON + NAV_GAP + 8.0;
        let search_rect = Rect::from_min_size(
            middle.min + vec2(nav_width, playlists::HEADER_CENTER_Y - SEARCH_HEIGHT / 2.0),
            vec2(middle.width() - nav_width, SEARCH_HEIGHT),
        );
        let panel_rect = Rect::from_min_max(
            pos2(middle.left(), middle.top() + SEARCH_HEIGHT + 12.0),
            middle.max,
        );
        let mut middle_ui = ui.new_child(UiBuilder::new().id_salt("browse").max_rect(middle));
        self.navigation_buttons(
            &mut middle_ui,
            middle.left(),
            search_rect.center().y,
            time,
            actions,
        );
        self.search
            .field(&mut middle_ui, search_rect, accent, actions, time);
        if self.search.is_open() {
            self.close_middle_panels(Some(MiddlePanel::Search));
        }
        let place = self.place();
        self.history.visit(place);
        let open = |id: &str, on: bool| {
            anim::tween_with(
                &ctx,
                Id::unique(id),
                if on { 1.0 } else { 0.0 },
                0.32,
                ease_in_out_cubic,
            )
        };
        let results_open = open("search-open", self.search.is_open());
        let playlist_open = open("playlist-open", self.playlist_tracks.is_open());
        let album_open = open("album-open", self.album.is_open());
        let artist_open = open("artist-open", self.artist.is_open());
        let search_data = SearchData {
            playback: &self.playback,
            accent,
        };
        let playlist_data = PlaylistTracksData {
            playback: &self.playback,
            accent,
        };
        let album_data = AlbumData {
            playback: &self.playback,
            accent,
        };
        // If no panel shows, the column shows a short guide. The panels add up (not `max`). Thus,
        // while one panel replaces a different panel, the guide does not show through the dip in
        // the middle of the cross-fade.
        let any_open = (results_open + playlist_open + album_open + artist_open).min(1.0);
        guide::show(&middle_ui, panel_rect, 1.0 - any_open);
        // The panel that opens goes above the panels that close.
        let mut panels = [
            (MiddlePanel::Search, self.search.is_open()),
            (MiddlePanel::Playlist, self.playlist_tracks.is_open()),
            (MiddlePanel::Album, self.album.is_open()),
            (MiddlePanel::Artist, self.artist.is_open()),
        ];
        panels.sort_by_key(|(_, is_open)| *is_open);
        let mut over_open_playlist = false;
        for (panel, _) in panels {
            match panel {
                MiddlePanel::Search => self.search.results(
                    &mut middle_ui,
                    panel_rect,
                    results_open,
                    &search_data,
                    &mut self.textures,
                    actions,
                ),
                MiddlePanel::Playlist => {
                    over_open_playlist = self.playlist_tracks.show(
                        &mut middle_ui,
                        panel_rect,
                        playlist_open,
                        &playlist_data,
                        &mut self.textures,
                        actions,
                    );
                }
                MiddlePanel::Album => self.album.show(
                    &mut middle_ui,
                    panel_rect,
                    album_open,
                    &album_data,
                    &mut self.textures,
                    actions,
                ),
                MiddlePanel::Artist => {
                    self.artist.show(
                        &mut middle_ui,
                        panel_rect,
                        artist_open,
                        accent,
                        &mut self.textures,
                        actions,
                    );
                }
            }
        }

        let mut queue_ui = ui.new_child(UiBuilder::new().id_salt("queue").max_rect(queue_column));
        let data = QueueData {
            queue: &self.queue,
            playback: &self.playback,
            elsewhere: self.elsewhere.as_ref(),
            play_here: self.play_here(),
            accent,
        };
        let over_queue = self.queue_view.show(
            &mut queue_ui,
            queue_column,
            &data,
            &mut self.textures,
            actions,
        );
        // The album that the user clicked under a track, in any column.
        if let Some((id, name)) = super::album::take_request(ui.ctx()) {
            self.open_album(id, name, time, actions);
        }
        // The artist that the user clicked under a track, in any column.
        if let Some((id, name)) = super::artist::take_request(ui.ctx()) {
            self.open_artist(id, name, time, actions);
        }
        over_queue || over_playlist || over_open_playlist
    }

    /// The command of «Play here» while the music plays on a different device. It brings the
    /// music to this app's player (or first asks for its permission). Nothing if this app cannot
    /// play.
    fn play_here(&self) -> Option<Command> {
        match self.web_player {
            WebPlayerStatus::Unavailable(_) | WebPlayerStatus::Failed(_) => None,
            WebPlayerStatus::NeedsReauth => Some(Command::ReauthorizeWebPlayer),
            WebPlayerStatus::Off | WebPlayerStatus::Starting | WebPlayerStatus::Ready { .. } => {
                Some(Command::StartWebPlayer)
            }
        }
    }

    /// Shows the current track in the window title (taskbar, Alt+Tab).
    fn update_window_title(&mut self, ctx: &Context) {
        let title = match (&self.auth, &self.playback.track) {
            (AuthStatus::LoggedIn { .. }, Some(track)) => {
                format!("{} · {} — {APP_NAME}", track.name, track.artist_line())
            }
            _ => APP_NAME.to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}

impl eframe::App for MusicApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}
