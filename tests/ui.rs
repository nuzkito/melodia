//! UI tests: they drive the real interface through its accessibility tree with egui_kittest. The
//! raw channel endpoints replace the backend. Thus the tests send events and check the commands
//! that the UI sends.

use std::time::Instant;

use egui::{Key, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use melodia::backend::{
    self, AuthStatus, Command, CommandReceiver, Event, EventSink, QueueAt, Toast, ToastKind,
    WebPlayerStatus,
};
use melodia::model::{
    Album, AlbumHit, Artist, ArtistHit, Device, Folder, Library, LibraryEdit, LibraryItem,
    LibraryKey, PlaybackSnapshot, Playlist, PlaylistEdit, PlaylistsStatus, QueueEdit,
    QueuePosition, QueueSnapshot, Release, ReleaseKind, SearchPage, Track, UserProfile,
};
use melodia::ui::MusicApp;
use melodia::update::AvailableUpdate;
use melodia::webplayer::DEVICE_NAME;

const CLIENT_ID: &str = "0123456789abcdef0123456789abcdef";
const REDIRECT_URI: &str = "http://127.0.0.1:8898/callback";

struct Fixture {
    harness: Harness<'static, MusicApp>,
    events: EventSink,
    commands: CommandReceiver,
    /// The tracks in [`Command::CheckLiked`] requests that [`Self::liked_checks`] did not take
    /// yet.
    liked_checks: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_demo(false)
    }

    fn with_demo(demo: bool) -> Self {
        let (handle, events, commands) = backend::channel(|| {});
        let harness = Harness::builder()
            // Sufficiently tall for the full queue of the tests, with the card, to fit without a
            // scroll.
            .with_size([1120.0, 900.0])
            .build_ui_state(
                |ui, app: &mut MusicApp| app.show(ui),
                MusicApp::new(handle, demo),
            );
        Self {
            harness,
            events,
            commands,
            liked_checks: Vec::new(),
        }
    }

    fn logged_in() -> Self {
        let mut fixture = Self::new();
        fixture.send(Event::Auth(AuthStatus::LoggedIn { demo: false }));
        fixture.send(Event::Profile(UserProfile {
            id: "alice".into(),
            display_name: "Alice".into(),
        }));
        fixture
    }

    fn send(&mut self, event: Event) {
        self.events.send(event);
        self.harness.run();
    }

    /// `playback` plays, and it is the only track in the queue. Its card has the controls.
    fn play(&mut self, playback: PlaybackSnapshot) {
        let current = playback.track.clone();
        self.send(Event::Playback(playback));
        self.send(Event::Queue(QueueSnapshot {
            current,
            ..QueueSnapshot::default()
        }));
    }

    /// The commands sent after the last call. It ignores image requests and checks of «Liked
    /// Songs» (see [`Self::liked_checks`]).
    fn commands(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::LoadImage { .. } => {}
                Command::CheckLiked { uris } => self.liked_checks.extend(uris),
                command => commands.push(command),
            }
        }
        commands
    }

    /// Answers the only command sent after the last call, a [`Command::FindInPlaylist`], as the
    /// backend does. The playlist has the track if `found` is true.
    fn answer_find(&mut self, found: bool) {
        let commands = self.commands();
        let [
            Command::FindInPlaylist {
                playlist,
                index,
                track,
            },
        ] = commands.as_slice()
        else {
            panic!("expected a FindInPlaylist, got {commands:?}");
        };
        self.send(Event::FoundInPlaylist {
            playlist: playlist.clone(),
            index: *index,
            track: track.clone(),
            found,
        });
    }

    /// The tracks in [`Command::CheckLiked`] requests after the last call.
    fn liked_checks(&mut self) -> Vec<String> {
        let _ = self.commands();
        std::mem::take(&mut self.liked_checks)
    }

    /// Clicks the leftmost match of `label`.
    fn click(&mut self, label: &str) {
        self.leftmost(label).click();
        self.harness.run();
    }

    /// Clicks the rightmost match of `label`, in the queue.
    fn click_in_queue(&mut self, label: &str) {
        self.harness
            .get_all_by_label(label)
            .max_by(|a, b| a.rect().center().x.total_cmp(&b.rect().center().x))
            .unwrap_or_else(|| panic!("no {label}"))
            .click();
        self.harness.run();
    }

    fn leftmost<'a>(&'a self, label: &'a str) -> egui_kittest::Node<'a> {
        self.harness
            .get_all_by_label(label)
            .min_by(|a, b| a.rect().center().x.total_cmp(&b.rect().center().x))
            .unwrap_or_else(|| panic!("no {label}"))
    }

    fn count(&self, label: &str) -> usize {
        self.harness.query_all_by_label(label).count()
    }

    fn has(&self, label: &str) -> bool {
        self.count(label) > 0
    }

    fn center_of(&self, label: &str) -> Pos2 {
        self.leftmost(label).rect().center()
    }

    /// Pushes at `from`, moves past the drag threshold and then to `to`, but does not release.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.harness.hover_at(from);
        self.harness.drag_at(from);
        self.harness.run();
        self.harness.hover_at(from + vec2(0.0, 12.0));
        self.harness.run();
        self.harness.hover_at(to);
        self.harness.run();
    }

    /// The point where the test takes the card of the current track: its cover and title, above
    /// its controls.
    fn card_grip(&self, current: &Track) -> Pos2 {
        let card = self.leftmost(&row(current)).rect();
        Pos2::new(card.center().x, card.top() + 40.0)
    }

    fn drop_at(&mut self, at: Pos2) {
        self.harness.drop_at(at);
        self.harness.run();
    }

    fn search(&mut self, query: &str, results: Vec<Track>) {
        self.click("Search");
        self.harness.get_by_label("Search").type_text(query);
        self.harness.run_steps(3);
        self.send(Event::SearchResults(SearchPage {
            query: query.into(),
            offset: 0,
            total: results.len() as u32,
            tracks: results,
            ..Default::default()
        }));
    }
}

fn track(id: &str, name: &str, artist: &str, album: &str) -> Track {
    Track {
        uri: format!("spotify:track:{id}"),
        name: name.into(),
        artists: vec![artist.into()],
        album: album.into(),
        duration_ms: 200_000,
        cover_small: None,
        cover_large: None,
        explicit: false,
        album_id: None,
        artist_ids: vec![],
    }
}

fn bohemian() -> Track {
    track("1", "Bohemian Rhapsody", "Queen", "A Night at the Opera")
}

fn device(id: &str, name: &str, kind: &str, is_active: bool) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        is_active,
        volume_percent: Some(70),
        supports_volume: true,
    }
}

fn playing(track: Track) -> PlaybackSnapshot {
    PlaybackSnapshot {
        track: Some(track),
        is_playing: true,
        progress_ms: 60_000,
        measured_at: Instant::now(),
        device: Some(device("pc", "My PC", "Computer", true)),
        shuffle: false,
    }
}

#[test]
fn login_screen_sends_login() {
    let mut fixture = Fixture::new();
    fixture.send(Event::Auth(AuthStatus::LoggedOut {
        client_id: Some(CLIENT_ID.into()),
        redirect_uri: REDIRECT_URI.into(),
        error: Some("You cancelled access in Spotify".into()),
    }));

    assert!(fixture.has("Melodia"));
    assert!(fixture.has("You cancelled access in Spotify"));
    fixture.click("Connect to Spotify");
    assert_eq!(fixture.commands(), vec![Command::Login { client_id: None }]);
}

#[test]
fn the_pointer_is_a_hand_over_pill_buttons() {
    let mut fixture = Fixture::new();
    fixture.send(Event::Auth(AuthStatus::LoggedOut {
        client_id: Some(CLIENT_ID.into()),
        redirect_uri: REDIRECT_URI.into(),
        error: None,
    }));
    let at = fixture.center_of("Connect to Spotify");
    fixture.harness.hover_at(at);
    fixture.harness.run();
    assert_eq!(
        fixture.harness.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );

    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click("Hits — By Spotify");
    let at = fixture.center_of("Open in Spotify");
    fixture.harness.hover_at(at);
    fixture.harness.run();
    assert_eq!(
        fixture.harness.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
}

#[test]
fn client_id_is_validated_before_logging_in() {
    let mut fixture = Fixture::new();
    fixture.send(Event::Auth(AuthStatus::LoggedOut {
        client_id: None,
        redirect_uri: REDIRECT_URI.into(),
        error: None,
    }));
    assert!(
        fixture.has(REDIRECT_URI),
        "the redirect URI to register is shown"
    );

    // It is disabled until the user types a valid Client ID.
    fixture.click("Connect to Spotify");
    assert!(fixture.commands().is_empty());

    fixture.click("Client ID");
    fixture
        .harness
        .get_by_label("Client ID")
        .type_text("not-an-id");
    fixture.harness.run();
    assert!(fixture.has("The Client ID is 32 hexadecimal characters."));
    fixture.click("Connect to Spotify");
    assert!(fixture.commands().is_empty());

    fixture.click("Client ID");
    for _ in 0.."not-an-id".len() {
        fixture.harness.key_press(Key::Backspace);
    }
    fixture
        .harness
        .get_by_label("Client ID")
        .type_text(CLIENT_ID);
    fixture.harness.run();
    fixture.click("Connect to Spotify");
    assert_eq!(
        fixture.commands(),
        vec![Command::Login {
            client_id: Some(CLIENT_ID.into())
        }]
    );
}

#[test]
fn waiting_for_the_browser_can_be_cancelled() {
    let mut fixture = Fixture::new();
    fixture.send(Event::Auth(AuthStatus::WaitingForBrowser {
        auth_url: "https://accounts.spotify.com/authorize?client_id=x".into(),
        redirect_uri: REDIRECT_URI.into(),
    }));
    assert!(fixture.has("Waiting for authorization in the browser…"));
    fixture.click("Cancel");
    assert_eq!(fixture.commands(), vec![Command::CancelLogin]);
}

#[test]
fn spotify_s_logo_shows_on_the_login_screen() {
    let mut fixture = Fixture::new();
    fixture.send(Event::Auth(AuthStatus::LoggedOut {
        client_id: Some(CLIENT_ID.into()),
        redirect_uri: REDIRECT_URI.into(),
        error: None,
    }));

    let logo = fixture.leftmost("Spotify").rect();
    let connect = fixture.leftmost("Connect to Spotify").rect();
    assert!(logo.width() >= 70.0, "logo: {logo:?}");
    assert!(
        logo.top() > connect.bottom(),
        "logo: {logo:?}, button: {connect:?}"
    );
    // It replaces the word in «Playing music requires Spotify Premium.». It is on the footer's
    // line, right of the middle of the window, with «Playing music requires» before it.
    assert!(logo.bottom() > 900.0 - 34.0, "logo: {logo:?}");
    assert!(logo.left() > 1120.0 / 2.0, "logo: {logo:?}");
}

#[test]
fn the_demo_does_not_show_spotify_s_logo() {
    let mut fixture = Fixture::with_demo(true);
    fixture.send(Event::Auth(AuthStatus::LoggedOut {
        client_id: None,
        redirect_uri: REDIRECT_URI.into(),
        error: None,
    }));

    assert_eq!(fixture.count("Spotify"), 0);
}

#[test]
fn explicit_songs_show_the_explicit_badge() {
    let mut fixture = Fixture::logged_in();
    let explicit = Track {
        explicit: true,
        ..track("e", "Fat Bottomed Girls", "Queen", "Jazz")
    };
    fixture.send(Event::Playback(playing(explicit.clone())));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![],
        current: Some(explicit),
        next: vec![
            Track {
                explicit: true,
                ..somebody()
            },
            radio(),
        ],
        edits: 0,
        next_locked: false,
    }));

    // The heart of the card and of the explicit row, not of Radio Ga Ga.
    assert_eq!(fixture.count("Explicit"), 2);
}

#[test]
fn the_current_track_shows_in_the_queue_card() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));

    assert!(fixture.has("Bohemian Rhapsody — Queen · A Night at the Opera"));
}

#[test]
fn the_device_picker_sits_just_left_of_the_three_dot_menu() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));

    let device = fixture.harness.get_by_label("Device: My PC").rect();
    let menu = fixture.harness.get_by_label("More options").rect();
    assert!((device.right() - menu.left()).abs() < 0.5);
    assert!((device.top() - menu.top()).abs() < 0.5);
    // It is not in the card of the current track now.
    let volume = fixture.harness.get_by_label("Mute").rect();
    assert!(device.bottom() < volume.top());
}

#[test]
fn the_user_and_logout_are_in_the_three_dot_menu() {
    let mut fixture = Fixture::logged_in();
    assert!(!fixture.has("Alice"));
    assert!(!fixture.has("Log out"));
    fixture.click("More options");
    assert!(fixture.has("Alice"));
    assert!(fixture.has("Log out"));
}

#[test]
fn the_three_dot_menu_shows_the_app_version() {
    let mut fixture = Fixture::logged_in();
    let version = concat!("Melodia ", env!("CARGO_PKG_VERSION"));
    assert!(!fixture.has(version));
    fixture.click("More options");
    assert!(fixture.has(version));
}

fn opened_urls(fixture: &mut Fixture) -> Vec<String> {
    let mut opened = Vec::new();
    for _ in 0..4 {
        fixture.harness.step();
        for command in &fixture.harness.output().platform_output.commands {
            if let egui::OutputCommand::OpenUrl(open) = command {
                opened.push(open.url.clone());
            }
        }
    }
    opened
}

#[test]
fn a_new_version_is_announced_and_downloaded_from_the_three_dot_menu() {
    let mut fixture = Fixture::logged_in();
    fixture.click("More options");
    assert!(!fixture.has("Download Melodia 0.2.0"));
    fixture.click("More options");

    fixture.send(Event::UpdateAvailable(AvailableUpdate {
        version: "0.2.0".into(),
        url: "https://example.com/melodia".into(),
    }));
    let toasts: Vec<String> = fixture
        .harness
        .state()
        .toast_messages()
        .map(str::to_owned)
        .collect();
    assert_eq!(toasts, ["Melodia 0.2.0 is available"]);

    fixture.click("More options");
    // Under the version of the app.
    let version = fixture.center_of(concat!("Melodia ", env!("CARGO_PKG_VERSION")));
    assert!(fixture.center_of("Download Melodia 0.2.0").y > version.y);
    // As small as the version line, not a full menu row.
    assert!(fixture.leftmost("Download Melodia 0.2.0").rect().height() <= 24.0);
    fixture.leftmost("Download Melodia 0.2.0").click();
    assert_eq!(opened_urls(&mut fixture), ["https://example.com/melodia"]);
    // The menu closed.
    assert!(!fixture.has("Log out"));
    // It stays in the menu.
    fixture.click("More options");
    assert!(fixture.has("Download Melodia 0.2.0"));
}

#[test]
fn a_new_version_shows_an_update_button_left_of_the_menu_that_opens_the_download() {
    let mut fixture = Fixture::logged_in();
    assert!(!fixture.has("Update"));

    fixture.send(Event::UpdateAvailable(AvailableUpdate {
        version: "0.2.0".into(),
        url: "https://example.com/melodia".into(),
    }));
    fixture.harness.run_steps(2);
    let update = fixture.leftmost("Update").rect();
    let menu = fixture.leftmost("More options").rect();
    assert!(update.right() <= menu.left());
    assert!((update.center().y - menu.center().y).abs() < 1.0);
    // The device picker moves left to make space.
    let devices = fixture
        .harness
        .get_all_by_label_contains("Device: ")
        .next()
        .expect("the device picker")
        .rect();
    assert!(devices.right() <= update.left());

    fixture.leftmost("Update").click();
    assert_eq!(opened_urls(&mut fixture), ["https://example.com/melodia"]);
}

#[test]
fn the_changelog_opens_from_the_three_dot_menu_and_closes() {
    let mut fixture = Fixture::logged_in();
    assert!(!fixture.has("This is the first published version of Melodia!"));
    fixture.click("More options");
    fixture.click("Changelog");
    fixture.harness.run_steps(10);
    assert!(fixture.has("This is the first published version of Melodia!"));
    assert!(fixture.has("0.1.0 · 2026-11-02"));
    // The menu closed when the dialog opened.
    assert!(!fixture.has("Log out"));
    fixture.click("Close");
    fixture.harness.run_steps(10);
    assert!(!fixture.has("This is the first published version of Melodia!"));
}

#[test]
fn nothing_playing_shows_a_hint() {
    let fixture = Fixture::logged_in();
    assert!(fixture.has("The queue is empty"));
}

#[test]
fn the_empty_middle_column_shows_a_guide() {
    let mut fixture = Fixture::logged_in();
    fixture.harness.run_steps(30);
    assert!(fixture.has("Keyboard shortcuts"));
    assert!(fixture.has("Play or pause"));
    assert!(fixture.has("Volume, over the slider"));
    assert!(fixture.has("Drag and drop"));

    // Search results replace it.
    fixture.search("queen", vec![bohemian()]);
    fixture.harness.run_steps(30);
    assert!(!fixture.has("Keyboard shortcuts"));

    // The tracks of an open playlist also replace it.
    let mut fixture = open_favorites(vec![bohemian()]);
    fixture.harness.run_steps(30);
    assert!(!fixture.has("Keyboard shortcuts"));
}

#[test]
fn player_controls_send_commands() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));

    fixture.click("Pause");
    assert_eq!(fixture.commands(), vec![Command::Pause]);
    // The button changes immediately, before the backend confirms.
    assert!(fixture.has("Play"));
    fixture.click("Play");
    assert_eq!(fixture.commands(), vec![Command::Resume]);

    fixture.click("Next");
    fixture.click("Previous");
    assert_eq!(fixture.commands(), vec![Command::Next, Command::Previous]);
}

#[test]
fn keyboard_shortcuts_control_playback() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));

    fixture.harness.key_press(Key::Space);
    fixture.harness.run();
    assert_eq!(fixture.commands(), vec![Command::Pause]);
}

#[test]
fn slash_focuses_the_search_box_without_typing_itself() {
    let mut fixture = Fixture::logged_in();
    // A real key push arrives as a key and as text in the same frame.
    for pressed in [true, false] {
        fixture.harness.event(egui::Event::Key {
            key: Key::Slash,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        if pressed {
            fixture.harness.event(egui::Event::Text("/".into()));
        }
    }
    fixture.harness.run();

    let field = fixture.leftmost("Search");
    assert!(field.is_focused());
    assert_eq!(field.value().as_deref().unwrap_or(""), "");
}

#[test]
fn queue_shows_previous_current_and_next_tracks() {
    let mut fixture = Fixture::logged_in();
    let current = bohemian();
    fixture.send(Event::Playback(playing(current.clone())));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![track("0", "Killer Queen", "Queen", "Sheer Heart Attack")],
        current: Some(current),
        next: vec![track(
            "2",
            "You're My Best Friend",
            "Queen",
            "A Night at the Opera",
        )],
        edits: 0,
        next_locked: false,
    }));

    for label in [
        "Killer Queen — Queen · Sheer Heart Attack",
        "Bohemian Rhapsody — Queen · A Night at the Opera",
        "You're My Best Friend — Queen · A Night at the Opera",
    ] {
        assert!(fixture.has(label), "missing {label}");
    }
    for label in ["PREVIOUS", "NOW PLAYING", "UP NEXT"] {
        assert!(!fixture.has(label), "unexpected {label}");
    }
}

#[test]
fn search_is_debounced_and_results_can_be_played_or_queued() {
    let mut fixture = Fixture::logged_in();
    fixture.click("Search");
    fixture.harness.get_by_label("Search").type_text("queen");
    fixture.harness.step();
    assert!(fixture.commands().is_empty(), "waits until typing stops");

    // In the harness, the time moves forward 0.25 s at each step.
    fixture.harness.run_steps(3);
    assert_eq!(
        fixture.commands(),
        vec![Command::Search {
            query: "queen".into(),
            offset: 0
        }]
    );

    let results = vec![
        bohemian(),
        track("3", "Somebody to Love", "Queen", "A Day at the Races"),
    ];
    fixture.send(Event::SearchResults(SearchPage {
        query: "queen".into(),
        offset: 0,
        tracks: results.clone(),
        total: 25,
        ..Default::default()
    }));
    assert!(fixture.has("Somebody to Love — Queen · A Day at the Races"));

    fixture.click("Add Somebody to Love to the queue");
    fixture.click("Play Somebody to Love");
    fixture.click("Show more results");
    assert_eq!(
        fixture.commands(),
        vec![
            Command::AddToQueue {
                track: results[1].clone()
            },
            Command::PlayTrack {
                track: results[1].clone(),
            },
            Command::Search {
                query: "queen".into(),
                offset: 2
            },
        ]
    );
}

#[test]
fn stale_search_results_are_ignored() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::SearchResults(SearchPage {
        query: "nobody asked".into(),
        offset: 0,
        tracks: vec![bohemian()],
        total: 1,
        ..Default::default()
    }));
    assert!(!fixture.has("Bohemian Rhapsody — Queen · A Night at the Opera"));
}

impl Fixture {
    fn press(&mut self, key: Key) {
        self.harness.key_press(key);
        self.harness.run();
    }

    /// egui shows a selected widget as toggled on.
    fn is_selected(&self, label: &str) -> bool {
        self.leftmost(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
    }
}

#[test]
fn search_results_can_be_chosen_with_the_arrow_keys() {
    let mut fixture = Fixture::logged_in();
    let results = vec![bohemian(), somebody(), radio()];
    fixture.search("queen", results.clone());
    fixture.commands();

    fixture.press(Key::ArrowDown);
    fixture.press(Key::ArrowDown);
    assert!(fixture.is_selected(&row(&somebody())));
    assert!(!fixture.is_selected(&row(&bohemian())));
    fixture.press(Key::Enter);
    // The search box keeps the focus, so the highlight can move.
    assert!(fixture.leftmost("Search").is_focused());
    fixture.press(Key::ArrowUp);
    fixture.press(Key::Enter);
    // Up from the first result goes back to the text: Enter plays nothing.
    fixture.press(Key::ArrowUp);
    assert!(!fixture.is_selected(&row(&bohemian())));
    fixture.press(Key::Enter);
    assert_eq!(
        fixture.commands(),
        vec![
            Command::PlayTrack {
                track: results[1].clone(),
            },
            Command::PlayTrack {
                track: results[0].clone(),
            },
        ]
    );
}

#[test]
fn typing_again_drops_the_highlighted_result() {
    let mut fixture = Fixture::logged_in();
    fixture.search("queen", vec![bohemian(), somebody()]);
    fixture.press(Key::ArrowDown);
    assert!(fixture.is_selected(&row(&bohemian())));
    fixture.harness.get_by_label("Search").type_text("s");
    fixture.harness.run();
    assert!(!fixture.is_selected(&row(&bohemian())));
}

#[test]
fn the_arrow_keys_load_more_results_at_the_end() {
    let mut fixture = Fixture::logged_in();
    fixture.click("Search");
    fixture.harness.get_by_label("Search").type_text("queen");
    fixture.harness.run_steps(3);
    fixture.send(Event::SearchResults(SearchPage {
        query: "queen".into(),
        offset: 0,
        tracks: vec![bohemian(), somebody()],
        total: 25,
        ..Default::default()
    }));
    fixture.commands();
    fixture.press(Key::ArrowDown);
    assert!(fixture.commands().is_empty());
    // On the last result, one time: the request for the next page is already sent.
    fixture.press(Key::ArrowDown);
    fixture.press(Key::ArrowDown);
    assert_eq!(
        fixture.commands(),
        vec![Command::Search {
            query: "queen".into(),
            offset: 2
        }]
    );
}

#[test]
fn a_track_with_the_keyboard_focus_plays_with_enter() {
    let mut fixture = queue_fixture();
    fixture.leftmost(&row(&somebody())).focus();
    fixture.harness.run();
    fixture.press(Key::Enter);
    assert_eq!(
        fixture.commands(),
        vec![Command::PlayFromQueue {
            position: QueuePosition::Next(1),
            uri: somebody().uri,
        }]
    );
}

#[test]
fn a_playlist_with_the_keyboard_focus_opens_with_enter() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.commands();
    fixture.leftmost(&playlist_row(&favorites())).focus();
    fixture.harness.run();
    fixture.press(Key::Enter);
    assert!(fixture.commands().iter().any(
        |c| matches!(c, Command::LoadPlaylistTracks { playlist } if playlist.id == favorites().id)
    ),);
}

#[test]
fn buttons_can_be_pressed_with_the_keyboard() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.commands();
    fixture.leftmost("Pause").focus();
    fixture.harness.run();
    fixture.press(Key::Enter);
    assert_eq!(fixture.commands(), vec![Command::Pause]);
}

#[test]
fn devices_can_be_selected() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.send(Event::Devices(vec![
        device("pc", "My PC", "Computer", true),
        device("phone", "Phone", "Smartphone", false),
    ]));

    fixture.click("Device: My PC");
    assert_eq!(fixture.commands(), vec![Command::RefreshDevices]);
    fixture.click("Phone");
    assert_eq!(
        fixture.commands(),
        vec![Command::TransferPlayback {
            device_id: "phone".into()
        }]
    );
}

#[test]
fn the_device_list_is_read_again_only_while_the_picker_is_open() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.send(Event::Devices(vec![device(
        "pc", "My PC", "Computer", true,
    )]));
    fixture.click("Device: My PC");
    assert_eq!(fixture.commands(), vec![Command::RefreshDevices]);

    // Each frame of the harness is a quarter of a second: 10 s later.
    for _ in 0..41 {
        fixture.harness.step();
    }
    assert_eq!(fixture.commands(), vec![Command::RefreshDevices]);

    fixture.press(Key::Escape);
    for _ in 0..41 {
        fixture.harness.step();
    }
    assert!(fixture.commands().is_empty());
}

#[test]
fn reopening_the_device_picker_soon_does_not_read_the_list_again() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.send(Event::Devices(vec![device(
        "pc", "My PC", "Computer", true,
    )]));
    fixture.click("Device: My PC");
    assert_eq!(fixture.commands(), vec![Command::RefreshDevices]);

    // Closed and opened again immediately: the list that the UI read still shows.
    fixture.press(Key::Escape);
    fixture.click("Device: My PC");
    assert!(fixture.commands().is_empty());

    // 10 s after the read, the UI reads it again when it opens.
    fixture.press(Key::Escape);
    for _ in 0..41 {
        fixture.harness.step();
    }
    assert!(fixture.commands().is_empty());
    fixture.click("Device: My PC");
    assert_eq!(fixture.commands(), vec![Command::RefreshDevices]);
}

#[test]
fn logout_and_toasts() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Toast(Toast {
        kind: ToastKind::Success,
        message: "“Bohemian Rhapsody” added to the queue".into(),
    }));
    let toasts: Vec<String> = fixture
        .harness
        .state()
        .toast_messages()
        .map(str::to_owned)
        .collect();
    assert_eq!(toasts, ["“Bohemian Rhapsody” added to the queue"]);

    fixture.click("More options");
    fixture.click("Log out");
    assert_eq!(fixture.commands(), vec![Command::Logout]);

    fixture.send(Event::Auth(AuthStatus::LoggedOut {
        client_id: Some(CLIENT_ID.into()),
        redirect_uri: REDIRECT_URI.into(),
        error: None,
    }));
    assert!(fixture.has("Connect to Spotify"));
    assert!(!fixture.has("Alice"));
}

#[test]
fn in_app_player_is_offered_in_the_device_picker() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));

    fixture.click("Device: My PC");
    fixture.click("Play in this app");
    assert_eq!(
        fixture.commands(),
        vec![Command::RefreshDevices, Command::StartWebPlayer]
    );

    // Old sessions do not have the `streaming` permission.
    fixture.send(Event::WebPlayer(WebPlayerStatus::NeedsReauth));
    fixture.click("Reconnect");
    assert_eq!(fixture.commands(), vec![Command::ReauthorizeWebPlayer]);
}

#[test]
fn in_app_player_errors_are_explained_and_can_be_retried() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.send(Event::WebPlayer(WebPlayerStatus::Failed(
        "Widevine not available.".into(),
    )));

    fixture.click("Device: My PC");
    assert!(fixture.has("Widevine not available. Click to try again."));
    fixture.commands();
    fixture.click("Play in this app");
    assert_eq!(fixture.commands(), vec![Command::StartWebPlayer]);
}

#[test]
fn the_device_picker_shows_the_name_of_another_device_playing() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));

    let device = fixture.harness.get_by_label("Device: My PC").rect();
    let menu = fixture.harness.get_by_label("More options").rect();
    assert!(
        device.width() > menu.width() + 20.0,
        "the button grows to the left to show «My PC»"
    );
    assert!((device.right() - menu.left()).abs() < 0.5);
}

#[test]
fn the_device_picker_shows_no_name_while_playing_in_this_app() {
    let mut fixture = Fixture::logged_in();
    let mut playback = playing(bohemian());
    playback.device = Some(device("web", DEVICE_NAME, "Computer", true));
    fixture.play(playback);

    let device = fixture.harness.get_by_label("Device: This app").rect();
    let menu = fixture.harness.get_by_label("More options").rect();
    assert!((device.width() - menu.width()).abs() < 0.5);
}

#[test]
fn active_in_app_player_is_not_listed_twice() {
    let mut fixture = Fixture::logged_in();
    let web = device("web", DEVICE_NAME, "Computer", true);
    let mut playback = playing(bohemian());
    playback.device = Some(web.clone());
    fixture.play(playback);
    fixture.send(Event::WebPlayer(WebPlayerStatus::Ready {
        device_id: "web".into(),
    }));
    fixture.send(Event::Devices(vec![
        web,
        device("pc", "My PC", "Computer", false),
    ]));

    fixture.click("Device: This app");
    assert!(fixture.has("Play in this app"));
    assert!(fixture.has("My PC"));
    assert!(
        !fixture.has(DEVICE_NAME),
        "the SDK's device is the «Play in this app» row"
    );
}

// --- Volume, queue and drag and drop ---------------------------------------------

fn killer_queen() -> Track {
    track("0", "Killer Queen", "Queen", "Sheer Heart Attack")
}

fn best_friend() -> Track {
    track(
        "2",
        "You're My Best Friend",
        "Queen",
        "A Night at the Opera",
    )
}

fn somebody() -> Track {
    track("3", "Somebody to Love", "Queen", "A Day at the Races")
}

fn radio() -> Track {
    track("4", "Radio Ga Ga", "Queen", "The Works")
}

fn row(track: &Track) -> String {
    format!("{} — {}", track.name, track.subtitle())
}

/// "Bohemian Rhapsody" plays; three tracks come next.
fn queue_fixture() -> Fixture {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![killer_queen()],
        current: Some(bohemian()),
        next: vec![best_friend(), somebody(), radio()],
        edits: 0,
        next_locked: false,
    }));
    fixture
}

#[test]
fn a_maximized_window_gives_the_extra_width_to_the_middle_column() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.harness.set_size(vec2(1920.0, 1000.0));
    fixture.harness.run();

    let search = fixture.leftmost("Search").rect();
    let volume = fixture.leftmost("Volume").rect();
    // «Back» and «Forward» use a small part of it, on the left.
    assert!(search.width() > 700.0, "search: {search:?}");
    assert!(volume.width() < 600.0, "volume: {volume:?}");
    // The queue goes to the right edge of the window.
    assert!(volume.right() > 1800.0, "volume: {volume:?}");
}

#[test]
fn volume_can_be_changed_and_muted() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));

    // The middle of the slider.
    fixture.click("Volume");
    assert_eq!(fixture.commands(), vec![Command::SetVolume { percent: 50 }]);
    fixture.click("Mute");
    assert_eq!(fixture.commands(), vec![Command::SetVolume { percent: 0 }]);
    fixture.click("Unmute");
    assert_eq!(fixture.commands(), vec![Command::SetVolume { percent: 50 }]);

    fixture
        .harness
        .key_press_modifiers(Modifiers::COMMAND, Key::ArrowUp);
    fixture.harness.run();
    assert_eq!(fixture.commands(), vec![Command::SetVolume { percent: 60 }]);
}

/// Turns the mouse wheel by `delta` points over `at` (positive is up) while the test holds
/// `modifiers`.
fn wheel_at(fixture: &mut Fixture, at: Pos2, delta: f32, modifiers: Modifiers) {
    fixture.harness.hover_at(at);
    fixture.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, delta),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers,
    });
    fixture.harness.run_steps(40);
}

#[test]
fn ctrl_and_the_mouse_wheel_change_the_volume() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));

    // From 70 %: two notches up, then one down.
    let slider = fixture.center_of("Volume");
    wheel_at(&mut fixture, slider, 80.0, Modifiers::COMMAND);
    let commands = fixture.commands();
    assert_eq!(commands.last(), Some(&Command::SetVolume { percent: 80 }));
    assert!(
        commands
            .iter()
            .all(|c| matches!(c, Command::SetVolume { .. })),
        "{commands:?}"
    );
    wheel_at(&mut fixture, slider, -40.0, Modifiers::COMMAND);
    assert_eq!(
        fixture.commands().last(),
        Some(&Command::SetVolume { percent: 75 })
    );
}

#[test]
fn the_mouse_wheel_without_ctrl_leaves_the_volume_alone() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.commands();

    let slider = fixture.center_of("Volume");
    wheel_at(&mut fixture, slider, 80.0, Modifiers::NONE);
    let commands = fixture.commands();
    assert!(
        !commands
            .iter()
            .any(|c| matches!(c, Command::SetVolume { .. })),
        "{commands:?}"
    );
}

#[test]
fn a_narrow_queue_shows_only_the_speaker_of_the_volume() {
    let mut fixture = Fixture::logged_in();
    fixture.harness.set_size(vec2(880.0, 600.0));
    fixture.send(Event::Playback(playing(bohemian())));
    fixture.send(Event::Queue(QueueSnapshot {
        current: Some(bohemian()),
        next: vec![killer_queen(), somebody()],
        ..QueueSnapshot::default()
    }));

    // There is no slider: the speaker fits after «Next».
    assert_eq!(fixture.count("Volume"), 0);
    let speaker = fixture.harness.get_by_label("Mute").rect();
    let next = fixture.leftmost("Next").rect();
    let pause = fixture.leftmost("Pause").rect();
    for other in [next, pause] {
        assert!(!speaker.intersects(other), "{speaker:?} {other:?}");
    }
    assert!(next.right() < speaker.left());

    // The upcoming tracks show under the card, in the window.
    let upcoming = fixture
        .harness
        .get_by_label("Killer Queen — Queen · Sheer Heart Attack")
        .rect();
    assert!(upcoming.top() > pause.bottom() && upcoming.bottom() < 600.0);

    // Ctrl + wheel over the speaker still changes the volume.
    wheel_at(&mut fixture, speaker.center(), 40.0, Modifiers::COMMAND);
    assert_eq!(
        fixture.commands().last(),
        Some(&Command::SetVolume { percent: 75 })
    );
}

#[test]
fn an_empty_queue_shows_an_empty_card_without_skipping() {
    let mut fixture = Fixture::logged_in();
    let mut idle = PlaybackSnapshot::idle();
    idle.device = Some(device("pc", "My PC", "Computer", true));
    fixture.send(Event::Playback(idle));

    assert!(fixture.has("The queue is empty"));
    assert!(fixture.has("Nothing playing — Pick a song to get started"));
    for label in ["Previous", "Next", "Play"] {
        fixture.click(label);
    }
    assert!(fixture.commands().is_empty());

    // The volume and the device picker still work.
    fixture.click("Volume");
    assert!(matches!(
        fixture.commands().as_slice(),
        [Command::SetVolume { .. }]
    ));
    fixture.click("Device: My PC");
    assert_eq!(fixture.commands(), vec![Command::RefreshDevices]);
}

#[test]
fn music_playing_on_another_device_hides_the_player_until_it_is_brought_back() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::WebPlayer(WebPlayerStatus::Ready {
        device_id: "web".into(),
    }));
    fixture.play(playing(bohemian()));
    assert!(fixture.has("Pause"));

    // The backend clears the queue and tells where the music went.
    fixture.send(Event::Playback(PlaybackSnapshot::idle()));
    fixture.send(Event::Queue(QueueSnapshot::default()));
    fixture.send(Event::PlayingElsewhere(Some(device(
        "phone",
        "Nuria's phone",
        "Smartphone",
        true,
    ))));
    assert!(fixture.has("Playing on Nuria's phone"));
    assert!(fixture.has("Device: Nuria's phone"));
    // There is no player: the app cannot control the music on that device.
    for label in [
        "Play",
        "Pause",
        "Next",
        "Previous",
        "Volume",
        "The queue is empty",
    ] {
        assert!(!fixture.has(label), "{label} is shown");
    }
    let _ = fixture.commands();
    fixture.harness.key_press(Key::Space);
    fixture
        .harness
        .key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::ArrowRight);
    fixture.harness.run();
    assert!(fixture.commands().is_empty());

    // «Play here» brings the music to this app's player.
    fixture.click("Play here");
    assert_eq!(fixture.commands(), vec![Command::StartWebPlayer]);
    fixture.send(Event::PlayingElsewhere(None));
    fixture.play(playing(bohemian()));
    assert!(!fixture.has("Playing on Nuria's phone"));
    assert!(fixture.has("Pause"));
}

#[test]
fn play_here_asks_for_the_permission_or_is_not_offered_if_this_app_cannot_play() {
    let mut fixture = Fixture::logged_in();
    let phone = device("phone", "Nuria's phone", "Smartphone", true);
    fixture.send(Event::PlayingElsewhere(Some(phone)));
    fixture.send(Event::WebPlayer(WebPlayerStatus::NeedsReauth));
    fixture.click("Play here");
    assert_eq!(fixture.commands(), vec![Command::ReauthorizeWebPlayer]);

    fixture.send(Event::WebPlayer(WebPlayerStatus::Unavailable(
        "Only available on Windows.".into(),
    )));
    assert!(fixture.has("Playing on Nuria's phone"));
    assert!(!fixture.has("Play here"));
}

#[test]
fn the_demo_badge_sits_next_to_the_device_picker() {
    let mut fixture = Fixture::with_demo(true);
    fixture.send(Event::Auth(AuthStatus::LoggedIn { demo: true }));
    let badge = fixture.harness.get_by_label("DEMO").rect();
    let menu = fixture
        .harness
        .get_by_label("Device: Choose a device")
        .rect();
    assert!(badge.right() < menu.left());
    assert!(menu.left() - badge.right() < 12.0);
    assert!((badge.center().y - menu.center().y).abs() < 1.0);
}

#[test]
fn volume_is_disabled_when_the_device_does_not_allow_it() {
    let mut fixture = Fixture::logged_in();
    let mut playback = playing(bohemian());
    playback.device.as_mut().unwrap().supports_volume = false;
    fixture.play(playback);

    fixture.click("Volume");
    fixture.click("Mute");
    assert!(fixture.commands().is_empty());
}

#[test]
fn queue_tracks_play_from_the_button_beside_their_others_not_from_the_cover() {
    let mut fixture = queue_fixture();
    // Nothing covers the artwork: the ▶ is at the right, left of the ♥ and the ✕.
    let row = fixture
        .leftmost("Somebody to Love — Queen · A Day at the Races")
        .rect();
    let play = fixture.center_of("Play Somebody to Love");
    let remove = fixture.center_of("Remove Somebody to Love from the queue");
    assert!(play.x > row.center().x, "play: {play:?}, row: {row:?}");
    assert!(play.x < remove.x, "play: {play:?}, remove: {remove:?}");
    fixture.click("Play Somebody to Love");
    fixture.click("Play Killer Queen");
    // The current track is a card with its own controls.
    fixture.click_in_queue("Pause");
    assert_eq!(
        fixture.commands(),
        vec![
            Command::PlayFromQueue {
                position: QueuePosition::Next(1),
                uri: somebody().uri,
            },
            Command::PlayFromQueue {
                position: QueuePosition::Previous(0),
                uri: killer_queen().uri,
            },
            Command::Pause,
        ]
    );
    // The card's button follows play/pause.
    assert_eq!(fixture.count("Play"), 1);
}

#[test]
fn the_current_track_card_has_the_controls() {
    let mut fixture = queue_fixture();
    for label in ["Previous", "Next", "Pause", "Volume", "Mute"] {
        assert_eq!(fixture.count(label), 1, "{label} in the card");
    }
    fixture.click_in_queue("Next");
    fixture.click_in_queue("Previous");
    fixture.click_in_queue("Mute");
    let seek = fixture
        .harness
        .get_all_by_label_contains("Position")
        .max_by(|a, b| a.rect().center().x.total_cmp(&b.rect().center().x))
        .unwrap();
    assert!(seek.rect().center().x > fixture.center_of("Pause").x);
    seek.click();
    fixture.harness.run();

    let commands = fixture.commands();
    assert_eq!(
        commands[..3],
        [
            Command::Next,
            Command::Previous,
            Command::SetVolume { percent: 0 }
        ]
    );
    assert!(
        matches!(commands[3..], [Command::Seek { .. }]),
        "{commands:?}"
    );
}

/// Scrolls the queue by `delta` points (positive goes up the list).
fn scroll_queue(fixture: &mut Fixture, delta: f32) {
    let over = fixture.center_of("Queue") + vec2(100.0, 300.0);
    fixture.harness.hover_at(over);
    fixture.harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, delta),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    });
    fixture.harness.run_steps(40);
}

#[test]
fn the_current_track_sticks_to_the_edges_while_scrolling() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));
    let many = |from: usize| -> Vec<Track> {
        (from..from + 20)
            .map(|i| track(&i.to_string(), &format!("Track {i}"), "Queen", "Hits"))
            .collect()
    };
    let (previous, next) = (many(100), many(200));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: previous.clone(),
        current: Some(bohemian()),
        next: next.clone(),
        edits: 0,
        next_locked: false,
    }));
    fixture.harness.run_steps(40);
    let card = row(&bohemian());
    let title_bottom = fixture.center_of("Queue").y + 20.0;

    // Far down the list: the card stays at the top.
    scroll_queue(&mut fixture, -3000.0);
    assert!(fixture.has(&row(next.last().unwrap())));
    assert!(!fixture.has(&row(&next[0])), "scrolled past");
    let pinned = fixture.leftmost(&card).rect();
    assert!(
        pinned.top() > title_bottom && pinned.top() < title_bottom + 20.0,
        "{pinned:?}"
    );
    fixture.click_in_queue("Pause");
    assert_eq!(fixture.commands(), vec![Command::Pause]);

    // At the top again: the card stays at the bottom.
    scroll_queue(&mut fixture, 6000.0);
    assert!(fixture.has(&row(&previous[0])));
    let pinned = fixture.leftmost(&card).rect();
    assert!(
        pinned.bottom() > 860.0 && pinned.bottom() <= 900.0,
        "{pinned:?}"
    );
}

#[test]
fn a_new_current_track_goes_to_the_top_of_the_queue() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));
    let previous: Vec<Track> = (100..120)
        .map(|i| track(&i.to_string(), &format!("Track {i}"), "Queen", "Hits"))
        .collect();
    let next = vec![best_friend(), somebody()];
    fixture.send(Event::Queue(QueueSnapshot {
        previous: previous.clone(),
        current: Some(bohemian()),
        next: next.clone(),
        edits: 0,
        next_locked: false,
    }));
    fixture.harness.run_steps(40);

    // With a long history and few upcoming tracks, the card is still at the top, under a small
    // part of the last played track. The next tracks show below it.
    let title_bottom = fixture.center_of("Queue").y + 20.0;
    let card = fixture.leftmost(&row(&bohemian())).rect();
    assert!(
        card.top() > title_bottom + 20.0 && card.top() < title_bottom + 60.0,
        "{card:?}"
    );
    assert!(fixture.has(&row(previous.last().unwrap())));
    assert!(!fixture.has(&row(&previous[previous.len() - 2])));
    for track in &next {
        assert!(fixture.center_of(&row(track)).y > card.bottom());
    }
}

#[test]
fn queue_tracks_can_be_reordered_by_dragging() {
    let mut fixture = queue_fixture();
    let from = fixture.center_of(&row(&best_friend()));
    // Below the middle of the last track: it goes to the end.
    let to = fixture.center_of(&row(&radio())) + vec2(0.0, 20.0);
    fixture.drag(from, to);
    assert!(
        fixture.has("It will go here"),
        "a slot shows where it will go"
    );

    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        // Positions count the full queue: Killer Queen, then the current track.
        vec![Command::EditQueue(QueueEdit::Move {
            from: 2,
            to: 4,
            uri: best_friend().uri,
        })]
    );
    assert!(!fixture.has("It will go here"));

    // The new order shows immediately, and an older snapshot does not undo it.
    let top = |fixture: &Fixture, track: &Track| fixture.center_of(&row(track)).y;
    let check_order = |fixture: &Fixture| {
        assert!(top(fixture, &somebody()) < top(fixture, &radio()));
        assert!(top(fixture, &radio()) < top(fixture, &best_friend()));
    };
    check_order(&fixture);
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![killer_queen()],
        current: Some(bohemian()),
        next: vec![best_friend(), somebody(), radio()],
        edits: 0,
        next_locked: false,
    }));
    check_order(&fixture);
}

#[test]
fn a_played_track_can_be_dragged_to_come_next_again() {
    let mut fixture = queue_fixture();
    // The played track is fully visible, so the list does not scroll by itself.
    scroll_queue(&mut fixture, 1000.0);
    let from = fixture.center_of(&row(&killer_queen()));
    // Below the middle of the card of the current track: immediately after it.
    let to = fixture.center_of(&row(&bohemian())) + vec2(0.0, 20.0);
    fixture.drag(from, to);
    assert!(fixture.has("It will go here"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Move {
            from: 0,
            to: 1,
            uri: killer_queen().uri,
        })]
    );
    let top = |track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&bohemian()) < top(&killer_queen()));
    assert!(top(&killer_queen()) < top(&best_friend()));
}

#[test]
fn an_upcoming_track_can_be_dragged_before_the_current_one() {
    let mut fixture = queue_fixture();
    // The played track is fully visible, so the list does not scroll by itself.
    scroll_queue(&mut fixture, 1000.0);
    let from = fixture.center_of(&row(&somebody()));
    let to = fixture.center_of(&row(&killer_queen())) + vec2(0.0, 20.0);
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Move {
            from: 3,
            to: 1,
            uri: somebody().uri,
        })]
    );
    // It counts as played.
    let top = |track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&killer_queen()) < top(&somebody()));
    assert!(top(&somebody()) < top(&bohemian()));
    fixture
        .harness
        .hover_at(fixture.center_of(&row(&somebody())));
    fixture.harness.run();
    fixture.click("Remove Somebody to Love from the queue");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::RemovePrevious {
            index: 1,
            uri: somebody().uri,
        })],
        "removed as a previous track"
    );
}

#[test]
fn the_current_track_can_be_dragged_past_the_upcoming_ones() {
    let mut fixture = queue_fixture();
    let from = fixture.card_grip(&bohemian());
    let card = fixture.leftmost(&row(&bohemian())).rect();
    fixture.drag(from, from);
    assert!(fixture.has("It will go here"), "it leaves a slot");
    // The same size as the card, not as a row.
    let slot = fixture.harness.get_by_label("It will go here").rect();
    assert!(slot.height() > card.height() - 16.0, "{slot:?} {card:?}");
    // The floating copy has the same size as the card. Two rows down, it passes the first two
    // upcoming tracks.
    let step = fixture.center_of(&row(&somebody())).y - fixture.center_of(&row(&best_friend())).y;
    let to = Pos2::new(from.x, from.y + 2.0 * step);
    fixture.harness.hover_at(to);
    fixture.harness.run();
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Move {
            from: 1,
            to: 3,
            uri: bohemian().uri,
        })]
    );
    let top = |track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&best_friend()) < top(&somebody()));
    assert!(top(&somebody()) < top(&bohemian()));
    assert!(top(&bohemian()) < top(&radio()));
}

/// The tops of the rows of `track`, from the top of the queue down (its copies, when it is there
/// more than one time).
fn copies_of(fixture: &Fixture, track: &Track) -> Vec<f32> {
    let mut tops: Vec<f32> = fixture
        .harness
        .query_all_by_label(&row(track))
        .map(|node| node.rect().top())
        .collect();
    tops.sort_by(f32::total_cmp);
    tops
}

#[test]
fn repeated_tracks_can_be_reordered_and_move_on() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![best_friend()],
        current: Some(bohemian()),
        next: vec![best_friend(), somebody(), best_friend()],
        edits: 0,
        next_locked: false,
    }));
    assert_eq!(copies_of(&fixture, &best_friend()).len(), 3);

    // The last copy goes before "Somebody to Love": that copy moves.
    let from = fixture
        .harness
        .get_all_by_label(&row(&best_friend()))
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .rect()
        .center();
    let to = fixture.center_of(&row(&somebody())) - vec2(0.0, 20.0);
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Move {
            from: 4,
            to: 3,
            uri: best_friend().uri,
        })]
    );
    let copies = copies_of(&fixture, &best_friend());
    let somebody_top = fixture.center_of(&row(&somebody())).y;
    assert_eq!(copies.len(), 3);
    assert!(copies[2] < somebody_top, "both upcoming copies come first");

    // The music goes to the next copy: all copies are still there, each in its position.
    fixture.send(Event::Playback(playing(best_friend())));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![best_friend(), bohemian()],
        current: Some(best_friend()),
        next: vec![best_friend(), somebody()],
        edits: 1,
        next_locked: false,
    }));
    // The new card goes to the top: scroll up to also see the oldest copy.
    scroll_queue(&mut fixture, 3000.0);
    let copies = copies_of(&fixture, &best_friend());
    let bohemian_top = fixture.center_of(&row(&bohemian())).y;
    let somebody_top = fixture.center_of(&row(&somebody())).y;
    // The player column also shows the current copy.
    assert!(copies.len() >= 3);
    assert!(copies[0] < bohemian_top);
    assert!(copies.iter().filter(|top| **top > bohemian_top).count() >= 2);
    assert!(copies.iter().all(|top| *top < somebody_top));
}

#[test]
fn upcoming_tracks_can_be_removed_from_the_queue() {
    let mut fixture = queue_fixture();
    assert!(
        !fixture.has("Remove Bohemian Rhapsody from the queue"),
        "not the current track"
    );

    fixture
        .harness
        .hover_at(fixture.center_of(&row(&somebody())));
    fixture.harness.run();
    fixture.click("Remove Somebody to Love from the queue");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Remove {
            index: 1,
            uri: somebody().uri,
        })]
    );
    // It goes immediately.
    assert!(!fixture.has(&row(&somebody())));
    assert!(fixture.has(&row(&radio())));
}

#[test]
fn previous_tracks_can_be_removed_from_the_queue() {
    let mut fixture = queue_fixture();
    fixture
        .harness
        .hover_at(fixture.center_of(&row(&killer_queen())));
    fixture.harness.run();
    fixture.click("Remove Killer Queen from the queue");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::RemovePrevious {
            index: 0,
            uri: killer_queen().uri,
        })]
    );
    // It goes immediately, and nothing stays before the current track.
    assert!(!fixture.has(&row(&killer_queen())));
    assert!(fixture.has(&row(&best_friend())));
}

#[test]
fn the_whole_queue_can_be_cleared() {
    let mut fixture = queue_fixture();
    fixture.click("Clear the queue");
    assert_eq!(fixture.commands(), vec![Command::ClearQueue]);
    // All tracks go immediately, and the music stops.
    for track in [killer_queen(), bohemian(), best_friend(), radio()] {
        assert!(!fixture.has(&row(&track)), "{} is still there", track.name);
    }
    assert!(fixture.has("The queue is empty"));
    assert!(!fixture.has("Clear the queue"), "nothing left to clear");

    // An older snapshot does not bring it back.
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![killer_queen()],
        current: Some(bohemian()),
        next: vec![best_friend()],
        edits: 0,
        next_locked: false,
    }));
    assert!(fixture.has("The queue is empty"));
}

#[test]
fn search_results_can_be_dropped_into_the_queue() {
    let mut fixture = queue_fixture();
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new.clone()]);
    fixture.commands();

    let from = fixture.center_of(&row(&new));
    // Just above the middle of the second upcoming track: between the first two.
    let to = fixture.center_of(&row(&somebody())) - vec2(0.0, 10.0);
    fixture.drag(from, to);
    assert!(fixture.has("It will go here"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Insert {
            index: 1,
            track: new.clone(),
        })]
    );
    // When the search closes, only the queued copy stays on the screen.
    fixture.click("Clear search");
    let top = |track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&best_friend()) < top(&new) && top(&new) < top(&somebody()));
}

/// A queue with upcoming tracks far below the visible part of the list.
fn long_queue() -> QueueSnapshot {
    QueueSnapshot {
        previous: vec![killer_queen()],
        current: Some(bohemian()),
        next: (0..20)
            .map(|i| track(&format!("n{i}"), &format!("Song {i}"), "Queen", "Hits"))
            .collect(),
        edits: 0,
        next_locked: false,
    }
}

#[test]
fn a_track_added_out_of_sight_says_where_it_went() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));
    let mut queue = long_queue();
    fixture.send(Event::Queue(queue.clone()));
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new.clone()]);
    fixture.click("Add Innuendo to the queue");
    assert!(!fixture.has("Added below"));

    // It goes at the end, far below the visible tracks.
    queue.next.push(new.clone());
    fixture.send(Event::Queue(queue));
    assert!(fixture.has("Added below"));

    // The notice scrolls the list there, and goes away.
    fixture.click("Added below");
    fixture.harness.run();
    assert!(!fixture.has("Added below"));
    let view = fixture.center_of("Queue").y;
    assert!(fixture.center_of(&row(&new)).y > view);
    assert!(fixture.center_of(&row(&new)).y < 900.0);
}

#[test]
fn tracks_added_in_sight_need_no_notice() {
    let mut fixture = queue_fixture();
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new.clone()]);
    fixture.click("Add Innuendo to the queue");
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![killer_queen()],
        current: Some(bohemian()),
        next: vec![best_friend(), somebody(), radio(), new],
        edits: 0,
        next_locked: false,
    }));
    assert!(!fixture.has("Added below"));
}

#[test]
fn several_tracks_added_out_of_sight_are_counted() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));
    let mut queue = long_queue();
    fixture.send(Event::Queue(queue.clone()));
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new]);
    fixture.click("Add Innuendo to the queue");
    // As for a playlist in the queue: all its tracks arrive at the same time.
    queue.next.extend([best_friend(), somebody(), radio()]);
    fixture.send(Event::Queue(queue));
    assert!(fixture.has("3 tracks added below"));
}

#[test]
fn a_track_dropped_on_the_empty_queue_plays() {
    let mut fixture = Fixture::logged_in();
    fixture.search("queen", vec![somebody()]);
    fixture.commands();

    let from = fixture.center_of(&row(&somebody()));
    let to = fixture.center_of("The queue is empty");
    fixture.drag(from, to);
    assert!(fixture.has("Drop it here to play it"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Insert {
            index: 0,
            track: somebody(),
        })]
    );
}

#[test]
fn dropping_outside_the_queue_changes_nothing() {
    let mut fixture = queue_fixture();
    let from = fixture.center_of(&row(&best_friend()));
    // The middle column, with nothing open in it.
    let outside = fixture.center_of("Search") + vec2(0.0, 200.0);
    fixture.drag(from, outside);
    fixture.drop_at(outside);
    assert!(fixture.commands().is_empty());
    let top = |track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&best_friend()) < top(&somebody()));
}

#[test]
fn search_results_fill_the_middle_column() {
    let mut fixture = queue_fixture();
    fixture.search("queen", vec![somebody()]);
    fixture.harness.run_steps(40);
    // In the results and in the queue.
    assert_eq!(fixture.count(&row(&somebody())), 2);
    let results = fixture.leftmost(&row(&somebody())).rect();
    let search = fixture.center_of("Search");
    assert!(results.left() < search.x && results.right() > search.x);
    // The controls stay in the queue card.
    fixture.commands();
    fixture.click("Pause");
    fixture.click("Next");
    assert_eq!(fixture.commands(), vec![Command::Pause, Command::Next]);

    fixture.click("Clear search");
    fixture.harness.run_steps(40);
    assert_eq!(fixture.count(&row(&somebody())), 1);
}

#[test]
fn a_track_on_its_way_cannot_be_moved_or_overtaken() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playback(playing(bohemian())));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![killer_queen()],
        current: Some(bohemian()),
        next: vec![best_friend(), somebody(), radio()],
        edits: 0,
        next_locked: true,
    }));

    assert!(
        !fixture.has("Remove You're My Best Friend from the queue"),
        "nor can it be removed"
    );
    // The app already sent it to Spotify's queue: it does not move…
    let locked = fixture.center_of(&row(&best_friend()));
    let below = fixture.center_of(&row(&radio())) + vec2(0.0, 20.0);
    fixture.drag(locked, below);
    fixture.drop_at(below);
    assert!(fixture.commands().is_empty());

    // …and nothing goes before it: a track dropped at the top goes second.
    let from = fixture.center_of(&row(&radio()));
    let top = fixture.center_of(&row(&best_friend())) - vec2(0.0, 25.0);
    fixture.drag(from, top);
    fixture.drop_at(top);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Move {
            from: 4,
            to: 3,
            uri: radio().uri,
        })]
    );

    // The current track also does not move: the track sent to Spotify must follow it.
    let card = fixture.card_grip(&bohemian());
    let below = fixture.center_of(&row(&radio())) + vec2(0.0, 20.0);
    fixture.drag(card, below);
    fixture.drop_at(below);
    assert!(fixture.commands().is_empty());
}

// --- Playlists ----------------------------------------------------------------------

fn playlist(id: &str, name: &str, total: u32, editable: bool) -> Playlist {
    Playlist {
        id: id.into(),
        name: name.into(),
        owner: if editable { "Alice" } else { "Spotify" }.into(),
        total,
        cover: None,
        editable,
        snapshot: None,
    }
}

fn favorites() -> Playlist {
    playlist("fav", "Favorites", 12, true)
}

fn hits() -> Playlist {
    playlist("hits", "Hits", 50, false)
}

fn playlist_row(playlist: &Playlist) -> String {
    let subtitle = if playlist.editable {
        playlist.count_line()
    } else {
        format!("By {}", playlist.owner)
    };
    format!("{} — {}", playlist.name, subtitle)
}

fn with_playlists(mut fixture: Fixture) -> Fixture {
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites(),
        hits(),
    ])));
    fixture
}

#[test]
fn playlists_are_listed() {
    let fixture = with_playlists(Fixture::logged_in());
    assert!(fixture.has("Playlists"));
    assert!(fixture.has(&playlist_row(&favorites())));
    assert!(fixture.has("Hits — By Spotify"));
}

#[test]
fn search_results_can_be_dropped_on_a_playlist() {
    let mut fixture = with_playlists(queue_fixture());
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new.clone()]);
    fixture.commands();

    let from = fixture.center_of(&row(&new));
    let to = fixture.center_of(&playlist_row(&favorites()));
    fixture.drag(from, to);
    fixture.drop_at(to);
    fixture.answer_find(false);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites(),
            edit: PlaylistEdit::Insert {
                index: None,
                track: new,
            },
        }]
    );
}

/// Drops the queue's copy of `track` on the sidebar row of `playlist`.
fn drop_queued_on(fixture: &mut Fixture, track: &Track, playlist: &Playlist) {
    let from = fixture
        .harness
        .get_all_by_label(&row(track))
        .max_by(|a, b| a.rect().center().x.total_cmp(&b.rect().center().x))
        .expect("the track is queued")
        .rect()
        .center();
    let to = fixture.center_of(&playlist_row(playlist));
    fixture.drag(from, to);
    fixture.drop_at(to);
}

const ALREADY_THERE: &str = "Already in this playlist";

#[test]
fn a_track_the_playlist_already_has_is_added_again_only_if_confirmed() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.commands();
    drop_queued_on(&mut fixture, &somebody(), &favorites());
    fixture.answer_find(true);
    assert!(fixture.has(ALREADY_THERE));
    assert!(fixture.has("“Somebody to Love” is already in “Favorites”. Add it again?"));
    assert_eq!(fixture.commands(), vec![], "nothing is added yet");

    fixture.click("Add again");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites(),
            edit: PlaylistEdit::Insert {
                index: None,
                track: somebody(),
            },
        }]
    );
    fixture.harness.run_steps(10);
    assert!(!fixture.has(ALREADY_THERE));
    assert!(fixture.has(&playlist_row(&favorites_with(13))));
}

#[test]
fn adding_a_track_the_playlist_already_has_can_be_cancelled() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.commands();
    drop_queued_on(&mut fixture, &somebody(), &favorites());
    fixture.answer_find(true);
    fixture.click("Don't add");
    fixture.harness.run_steps(10);
    assert!(!fixture.has(ALREADY_THERE));
    assert_eq!(fixture.commands(), vec![]);
    assert!(fixture.has(&playlist_row(&favorites())));

    // Esc also cancels, and the UI does not use it as a shortcut (for example, to close the
    // search).
    drop_queued_on(&mut fixture, &somebody(), &favorites());
    fixture.answer_find(true);
    fixture.press_with(Modifiers::NONE, Key::Escape);
    fixture.harness.run_steps(10);
    assert!(!fixture.has(ALREADY_THERE));
    assert_eq!(fixture.commands(), vec![]);
}

#[test]
fn enter_confirms_adding_a_track_again() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.commands();
    drop_queued_on(&mut fixture, &somebody(), &favorites());
    fixture.answer_find(true);
    fixture.press_with(Modifiers::NONE, Key::Enter);
    assert!(matches!(
        fixture.commands().as_slice(),
        [Command::EditPlaylist {
            edit: PlaylistEdit::Insert { .. },
            ..
        }]
    ));
}

#[test]
fn the_tracks_of_an_opened_playlist_say_at_once_whether_it_has_a_track() {
    let mut fixture = open_favorites_in(queue_fixture(), vec![somebody(), innuendo()]);
    fixture.commands();
    // It has it: the dialog opens without asking the backend.
    drop_queued_on(&mut fixture, &somebody(), &favorites_with(2));
    assert!(fixture.has(ALREADY_THERE));
    assert_eq!(fixture.commands(), vec![]);
    fixture.click("Don't add");
    fixture.harness.run_steps(10);

    // It does not have it: the UI adds it immediately.
    drop_queued_on(&mut fixture, &radio(), &favorites_with(2));
    assert!(!fixture.has(ALREADY_THERE));
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(2),
            edit: PlaylistEdit::Insert {
                index: None,
                track: radio(),
            },
        }]
    );
}

#[test]
fn a_track_just_added_to_a_playlist_asks_before_being_added_again() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.commands();
    drop_queued_on(&mut fixture, &somebody(), &favorites());
    fixture.answer_find(false);
    assert_eq!(fixture.commands().len(), 1);
    fixture.harness.run_steps(10);

    // The UI never read its tracks, but the user added the track from here.
    drop_queued_on(&mut fixture, &somebody(), &favorites_with(13));
    assert!(fixture.has(ALREADY_THERE));
    assert_eq!(fixture.commands(), vec![]);
}

#[test]
fn a_second_drop_answered_after_the_first_add_still_asks() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.commands();
    // The two drops ask the backend before the UI adds a track.
    drop_queued_on(&mut fixture, &somebody(), &favorites());
    let first = fixture.commands();
    drop_queued_on(&mut fixture, &somebody(), &favorites());
    let second = fixture.commands();
    for command in [first, second].concat() {
        let Command::FindInPlaylist {
            playlist,
            index,
            track,
        } = command
        else {
            panic!("expected a FindInPlaylist, got {command:?}");
        };
        fixture.send(Event::FoundInPlaylist {
            playlist,
            index,
            track,
            found: false,
        });
    }
    assert_eq!(fixture.commands().len(), 1, "added once");
    assert!(fixture.has(ALREADY_THERE));
}

#[test]
fn queued_tracks_can_be_dropped_on_a_playlist() {
    let mut fixture = with_playlists(queue_fixture());
    let from = fixture.center_of(&row(&somebody()));
    let to = fixture.center_of(&playlist_row(&favorites()));
    fixture.drag(from, to);
    fixture.drop_at(to);
    fixture.answer_find(false);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites(),
            edit: PlaylistEdit::Insert {
                index: None,
                track: somebody(),
            },
        }]
    );
    // The queue stays as it was.
    let top = |track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&best_friend()) < top(&somebody()) && top(&somebody()) < top(&radio()));
}

#[test]
fn playlists_can_be_dropped_into_the_queue() {
    let mut fixture = with_playlists(queue_fixture());
    let from = fixture.center_of(&playlist_row(&favorites()));
    // Just above the middle of the second upcoming track: between the first two.
    let to = fixture.center_of(&row(&somebody())) - vec2(0.0, 10.0);
    fixture.drag(from, to);
    assert!(fixture.has("It will go here"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::QueuePlaylist {
            playlist: favorites(),
            index: 1,
        }]
    );
    assert!(!fixture.has("It will go here"));
}

#[test]
fn a_playlist_dropped_on_the_empty_queue_plays() {
    let mut fixture = with_playlists(Fixture::logged_in());
    let from = fixture.center_of(&playlist_row(&favorites()));
    let to = fixture.center_of("The queue is empty");
    fixture.drag(from, to);
    assert!(fixture.has("Drop it here to play it"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::QueuePlaylist {
            playlist: favorites(),
            index: 0,
        }]
    );
}

fn calm() -> Playlist {
    playlist("calm", "Calm", 5, true)
}

fn playlist_ids(playlists: &[Playlist]) -> Vec<String> {
    playlists.iter().map(|p| p.id.clone()).collect()
}

/// The playlists from top to bottom, as the sidebar shows them.
fn sidebar_order(fixture: &Fixture, playlists: &[Playlist]) -> Vec<String> {
    let mut shown: Vec<_> = playlists
        .iter()
        .map(|p| (fixture.center_of(&playlist_row(p)).y, p.id.clone()))
        .collect();
    shown.sort_by(|a, b| a.0.total_cmp(&b.0));
    shown.into_iter().map(|(_, id)| id).collect()
}

#[test]
fn playlists_can_be_rearranged_by_dragging() {
    let mut fixture = queue_fixture();
    let spotify = vec![favorites(), hits(), calm()];
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(spotify.clone())));
    fixture.commands();

    // Dropped at its old position: nothing changes.
    let from = fixture.center_of(&playlist_row(&favorites()));
    fixture.drag(from, from - vec2(0.0, 6.0));
    assert!(fixture.has("It will go here"));
    fixture.drop_at(from - vec2(0.0, 6.0));
    assert!(fixture.commands().is_empty());

    // When the test lifts it, it leaves a gap and the other items move up. Thus the point just
    // above the old middle of the second item is now between the other two.
    let to = fixture.center_of(&playlist_row(&hits())) - vec2(0.0, 10.0);
    fixture.drag(from, to);
    assert!(
        fixture.has("It will go here"),
        "a slot shows where it will go"
    );
    fixture.drop_at(to);
    let rearranged = vec![hits(), favorites(), calm()];
    assert_eq!(fixture.commands(), vec![to_place("fav", 1)]);
    fixture.harness.run_steps(30);
    assert!(!fixture.has("It will go here"));
    assert_eq!(sidebar_order(&fixture, &spotify), playlist_ids(&rearranged));

    // A list that loaded before the backend had the new order does not undo it. A new playlist
    // goes where Spotify puts it, at the top.
    let new = playlist("new", "Nueva", 0, true);
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        new.clone(),
        favorites(),
        hits(),
        calm(),
    ])));
    fixture.harness.run_steps(30);
    assert_eq!(
        sidebar_order(&fixture, &[new, favorites(), hits(), calm()]),
        ["new", "hits", "fav", "calm"]
    );
}

/// The playlist `id` moved to `index` of the first level.
fn to_place(id: &str, index: usize) -> Command {
    Command::EditLibrary(LibraryEdit::Move {
        item: LibraryKey::Playlist(id.into()),
        to: None,
        index: Some(index),
    })
}

#[test]
fn tracks_are_still_added_to_rearranged_playlists() {
    let mut fixture = queue_fixture();
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites(),
        hits(),
        calm(),
    ])));
    // "Calm" to the top.
    let from = fixture.center_of(&playlist_row(&calm()));
    let to = fixture.center_of(&playlist_row(&favorites())) - vec2(0.0, 20.0);
    fixture.drag(from, to);
    fixture.drop_at(to);
    fixture.harness.run_steps(30);
    assert_eq!(fixture.commands(), vec![to_place("calm", 0)]);

    // A track dragged over a playlist lights it up, but does not open a slot. The track goes at
    // the end of the playlist.
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new.clone()]);
    fixture.commands();
    let from = fixture.center_of(&row(&new));
    let to = fixture.center_of(&playlist_row(&calm()));
    fixture.drag(from, to);
    assert!(!fixture.has("It will go here"));
    fixture.drop_at(to);
    fixture.answer_find(false);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: calm(),
            edit: PlaylistEdit::Insert {
                index: None,
                track: new,
            },
        }]
    );
    // The playlists stay at their positions (one has one more track).
    fixture.harness.run_steps(30);
    let filled = playlist("calm", "Calm", 6, true);
    assert_eq!(
        sidebar_order(&fixture, &[favorites(), hits(), filled]),
        ["calm", "fav", "hits"]
    );
}

#[test]
fn playlists_ask_for_permission_or_a_retry() {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playlists(PlaylistsStatus::NeedsReauth));
    assert!(fixture.has("No access to your playlists"));
    fixture.click("Reconnect");
    assert_eq!(fixture.commands(), vec![Command::Login { client_id: None }]);

    fixture.send(Event::Playlists(PlaylistsStatus::Failed(
        "Can't connect to Spotify.".into(),
    )));
    fixture.click("Try again");
    assert_eq!(fixture.commands(), vec![Command::RefreshPlaylists]);

    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![])));
    assert!(fixture.has("You have no playlists"));
    fixture.click("Create a playlist");
    assert!(fixture.has("Name of the new playlist"));
}

#[test]
fn a_new_playlist_can_be_created() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.commands();
    // Its card shows at the top, with the field on the line of its name.
    fixture.click("New playlist");
    fixture.harness.run_steps(3);
    assert!(fixture.has("New playlist, no name yet"));

    // Without a name, there is nothing to create.
    fixture.harness.key_press(Key::Enter);
    fixture.harness.run();
    assert!(fixture.commands().is_empty());

    fixture
        .harness
        .get_by_label("Name of the new playlist")
        .type_text("  Road Trip ");
    fixture.harness.run();
    let card = "  Road Trip  — New playlist";
    assert!(fixture.center_of(card).y < fixture.center_of(&playlist_row(&favorites())).y);
    fixture.harness.key_press(Key::Enter);
    fixture.harness.run();
    assert_eq!(
        fixture.commands(),
        vec![Command::CreatePlaylist {
            name: "Road Trip".into(),
            folder: None,
        }]
    );
    assert!(!fixture.has("Name of the new playlist"), "the field closes");

    // Its card stays until Spotify creates it…
    let card = "Road Trip — New playlist";
    let top = fixture.center_of(card).y;
    fixture.send(Event::PlaylistCreated {
        name: "Road Trip".into(),
        id: Some("trip".into()),
    });
    assert!(fixture.has(card));
    // …and lists it: its row replaces the card.
    let road_trip = playlist("trip", "Road Trip", 0, true);
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        road_trip.clone(),
        favorites(),
        hits(),
    ])));
    assert!(!fixture.has(card));
    assert_eq!(fixture.center_of(&playlist_row(&road_trip)).y, top);

    // ✓ also creates it.
    fixture.click("New playlist");
    fixture.harness.run_steps(3);
    fixture
        .harness
        .get_by_label("Name of the new playlist")
        .type_text("Viajes");
    fixture.harness.run();
    fixture.click("Create playlist");
    assert_eq!(
        fixture.commands(),
        vec![Command::CreatePlaylist {
            name: "Viajes".into(),
            folder: None,
        }]
    );
    assert!(fixture.has("Viajes — New playlist"));
}

#[test]
fn the_card_of_a_playlist_that_could_not_be_created_leaves() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click("New playlist");
    fixture.harness.run_steps(3);
    fixture
        .harness
        .get_by_label("Name of the new playlist")
        .type_text("Road Trip");
    fixture.harness.run();
    fixture.harness.key_press(Key::Enter);
    fixture.harness.run();
    assert!(fixture.has("Road Trip — New playlist"));

    fixture.send(Event::PlaylistCreated {
        name: "Road Trip".into(),
        id: None,
    });
    assert!(!fixture.has("Road Trip — New playlist"));
    assert!(fixture.has(&playlist_row(&favorites())));
}

#[test]
fn creating_a_playlist_can_be_cancelled() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click("New playlist");
    fixture.harness.run_steps(3);
    fixture
        .harness
        .get_by_label("Name of the new playlist")
        .type_text("Borrador");
    fixture.harness.run();
    fixture.harness.key_press(Key::Escape);
    fixture.harness.run_steps(3);
    assert!(!fixture.has("Borrador — New playlist"));

    // Without a name, a click on a different area also removes it.
    fixture.click("New playlist");
    fixture.harness.run_steps(3);
    fixture.click(&playlist_row(&favorites()));
    fixture.harness.run_steps(3);
    assert!(!fixture.has("New playlist, no name yet"));
    assert!(
        fixture
            .commands()
            .iter()
            .all(|c| !matches!(c, Command::CreatePlaylist { .. }))
    );
}

#[test]
fn the_card_of_a_new_folder_becomes_one_of_a_new_playlist() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click("New folder");
    fixture.harness.run_steps(3);
    assert!(fixture.has("New folder, no name yet"));
    fixture.click("New playlist");
    fixture.harness.run_steps(3);
    assert!(!fixture.has("New folder, no name yet"));
    assert!(fixture.has("New playlist, no name yet"));
    assert!(fixture.has("Name of the new playlist"));
}

#[test]
fn the_playlist_title_shows_the_total_duration_of_its_songs() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.click(&playlist_row(&favorites()));
    let mut tracks: Vec<Track> = (0..12)
        .map(|i| track(&i.to_string(), "Innuendo", "Queen", "Innuendo"))
        .collect();
    tracks[0].duration_ms = 3_600_000;
    fixture.send(Event::PlaylistTracks {
        id: favorites().id,
        tracks,
        total: favorites().total,
        edits: 0,
    });
    // One hour and 11 songs of 3:20.
    assert!(fixture.has("1 h 37 min"));

    // Of a playlist of a different user, the owner shows before the duration. The panel shows
    // only part of the songs, so the sum is a minimum.
    fixture.click(&playlist_row(&hits()));
    fixture.send(Event::PlaylistTracks {
        id: hits().id,
        tracks: vec![bohemian()],
        total: 600,
        edits: 0,
    });
    assert!(fixture.has("By Spotify · Over 3 min"));
}

#[test]
fn clicking_a_playlist_shows_its_tracks() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.commands();
    fixture.click(&playlist_row(&favorites()));
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadPlaylistTracks {
            playlist: favorites()
        }]
    );
    assert!(fixture.has("Songs in “Favorites”"));
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    let other = track("8", "Mustapha", "Queen", "Jazz");
    fixture.send(Event::PlaylistTracks {
        id: favorites().id,
        tracks: vec![new.clone(), other.clone()],
        total: favorites().total,
        edits: 0,
    });
    assert!(fixture.has(&row(&new)));

    fixture.click("Add Innuendo to the queue");
    fixture.click("Play Mustapha");
    assert_eq!(
        fixture.commands(),
        vec![
            Command::AddToQueue { track: new.clone() },
            // Only that song plays: the songs after it do not go in the queue.
            Command::PlayTrack { track: other },
        ]
    );

    // A second click hides them. There is no close button.
    fixture.click(&playlist_row(&favorites()));
    fixture.harness.run_steps(4);
    assert!(!fixture.has("Songs in “Favorites”"));
    fixture.click(&playlist_row(&favorites()));
    assert!(fixture.has(&row(&new)), "shown at once from the cache");
    assert!(!fixture.has("Close playlist"));
}

/// Double-clicks the name of the open playlist, `name`.
fn double_click_title(fixture: &mut Fixture, name: &str) {
    let label = format!("Songs in “{name}”");
    // egui accepts one click in each frame, and the test clock moves a quarter of a second in each
    // frame. This is too slow for a double click.
    fixture
        .harness
        .ctx
        .options_mut(|o| o.input_options.max_double_click_delay = 2.0);
    let title = fixture.harness.get_by_label(&label);
    title.click();
    title.click();
    fixture.harness.run();
}

#[test]
fn the_open_playlist_is_renamed_with_a_double_click_on_its_name() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click(&playlist_row(&favorites()));
    fixture.commands();
    double_click_title(&mut fixture, "Favorites");
    let field = fixture.harness.get_by_label("New playlist name");
    assert_eq!(field.value().as_deref(), Some("Favorites"));
    assert!(field.is_focused());

    // The name is selected: the typed text replaces it.
    field.type_text("  Road Trip ");
    fixture.harness.run();
    fixture.harness.key_press(Key::Enter);
    fixture.harness.run();
    assert_eq!(
        fixture.commands(),
        vec![Command::RenamePlaylist {
            playlist: favorites(),
            name: "Road Trip".into(),
        }]
    );
    assert!(!fixture.has("New playlist name"), "the field closes");
    assert!(fixture.has("Songs in “Road Trip”"));
    let renamed = Playlist {
        name: "Road Trip".into(),
        ..favorites()
    };
    assert!(fixture.has(&playlist_row(&renamed)), "the sidebar too");
}

#[test]
fn renaming_a_playlist_can_be_cancelled() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click(&playlist_row(&favorites()));
    fixture.commands();

    // Esc keeps the name as it was, and the playlist open.
    double_click_title(&mut fixture, "Favorites");
    fixture
        .harness
        .get_by_label("New playlist name")
        .type_text("Otra");
    fixture.harness.run();
    fixture.harness.key_press(Key::Escape);
    fixture.harness.run();
    assert!(!fixture.has("New playlist name"));
    assert!(fixture.has("Songs in “Favorites”"));

    // The same name, or no name, changes nothing.
    double_click_title(&mut fixture, "Favorites");
    fixture.harness.key_press(Key::Enter);
    fixture.harness.run();
    assert!(!fixture.has("New playlist name"));
    double_click_title(&mut fixture, "Favorites");
    fixture.harness.key_press(Key::Backspace);
    fixture.harness.run();
    fixture.harness.key_press(Key::Enter);
    fixture.harness.run();
    assert!(
        fixture.has("New playlist name"),
        "an empty name keeps the field open"
    );
    assert!(fixture.commands().is_empty());
}

#[test]
fn a_pencil_shows_next_to_the_playlist_name_on_hover_and_renames_it() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click(&playlist_row(&favorites()));
    fixture.commands();
    assert!(!fixture.has("Rename playlist"), "hidden until hovered");

    fixture.harness.get_by_label("Songs in “Favorites”").hover();
    fixture.harness.run();
    assert!(fixture.has("Rename playlist"));

    fixture.click("Rename playlist");
    let field = fixture.harness.get_by_label("New playlist name");
    assert_eq!(field.value().as_deref(), Some("Favorites"));
}

#[test]
fn someone_elses_playlist_cannot_be_renamed() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click("Hits — By Spotify");
    double_click_title(&mut fixture, "Hits");
    assert!(!fixture.has("New playlist name"));
    fixture.harness.get_by_label("Songs in “Hits”").hover();
    fixture.harness.run();
    assert!(!fixture.has("Rename playlist"), "no pencil");
}

#[test]
fn playlist_tracks_can_be_dropped_into_the_queue() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.click(&playlist_row(&favorites()));
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.send(Event::PlaylistTracks {
        id: favorites().id,
        tracks: vec![new.clone()],
        total: favorites().total,
        edits: 0,
    });
    fixture.harness.run_steps(4);
    fixture.commands();

    let from = fixture.center_of(&row(&new));
    // Just above the middle of the second upcoming track: between the first two.
    let to = fixture.center_of(&row(&somebody())) - vec2(0.0, 10.0);
    fixture.drag(from, to);
    assert!(fixture.has("It will go here"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditQueue(QueueEdit::Insert {
            index: 1,
            track: new,
        })]
    );
}

#[test]
fn searching_replaces_the_playlist_tracks() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click(&playlist_row(&favorites()));
    fixture.send(Event::PlaylistTracks {
        id: favorites().id,
        tracks: vec![killer_queen()],
        total: favorites().total,
        edits: 0,
    });
    fixture.search("queen", vec![somebody()]);
    fixture.harness.run_steps(4);
    assert!(fixture.has(&row(&somebody())));
    assert!(!fixture.has("Songs in “Favorites”"));

    // When a playlist opens, the search also clears.
    fixture.click(&playlist_row(&favorites()));
    fixture.harness.run_steps(4);
    assert!(fixture.has(&row(&killer_queen())));
    assert!(!fixture.has(&row(&somebody())));
}

#[test]
fn tracks_of_someone_elses_playlist_are_not_requested() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.commands();
    fixture.click("Hits — By Spotify");
    assert!(fixture.commands().is_empty());
    assert!(fixture.has("Its songs can't be shown"));
}

#[test]
fn someone_elses_playlist_can_be_opened_in_spotify() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click("Hits — By Spotify");
    fixture.leftmost("Open in Spotify").click();
    let mut opened = Vec::new();
    for _ in 0..4 {
        fixture.harness.step();
        for command in &fixture.harness.output().platform_output.commands {
            if let egui::OutputCommand::OpenUrl(open) = command {
                opened.push(open.url.clone());
            }
        }
    }
    assert_eq!(opened, ["https://open.spotify.com/playlist/hits"]);
}

#[test]
fn a_playlist_that_fails_to_load_has_no_button_to_open_it_in_spotify() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click(&playlist_row(&favorites()));
    fixture.send(Event::PlaylistTracksFailed {
        id: favorites().id,
        message: "no connection".to_owned(),
    });
    fixture.harness.run_steps(4);
    assert!(fixture.has("Its songs can't be shown"));
    assert!(!fixture.has("Open in Spotify"));
}

#[test]
fn the_open_playlist_is_reloaded_when_it_changes() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.click(&playlist_row(&favorites()));
    fixture.commands();
    let mut grown = favorites();
    grown.total += 1;
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        grown.clone(),
        hits(),
    ])));
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadPlaylistTracks { playlist: grown }]
    );
}

#[test]
fn opening_a_playlist_updates_its_song_count() {
    // A different app added two songs to it.
    let mut fixture = with_playlists(queue_fixture());
    fixture.click(&playlist_row(&favorites()));
    fixture.send(Event::PlaylistTracks {
        id: favorites().id,
        tracks: vec![killer_queen()],
        total: 14,
        edits: 0,
    });
    fixture.harness.run_steps(4);
    assert!(fixture.has(&playlist_row(&favorites_with(14))));
    assert!(!fixture.has(&playlist_row(&favorites())));
    // One more add counts from there.
    fixture.commands();
    let from = fixture.center_of(&row(&somebody()));
    let to = fixture.center_of(&playlist_row(&favorites_with(14)));
    fixture.drag(from, to);
    fixture.drop_at(to);
    fixture.answer_find(false);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(14),
            edit: PlaylistEdit::Insert {
                index: None,
                track: somebody(),
            },
        }]
    );
}

/// "Favorites" open, with `tracks` (and nothing playing).
fn open_favorites(tracks: Vec<Track>) -> Fixture {
    open_favorites_in(Fixture::logged_in(), tracks)
}

/// "Favorites" open in `fixture`, with `tracks`.
fn open_favorites_in(fixture: Fixture, tracks: Vec<Track>) -> Fixture {
    let mut fixture = with_playlists(fixture);
    let mut favorites = favorites();
    favorites.total = tracks.len() as u32;
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites.clone(),
        hits(),
    ])));
    fixture.click(&playlist_row(&favorites));
    fixture.send(Event::PlaylistTracks {
        id: favorites.id,
        tracks,
        total: favorites.total,
        edits: 0,
    });
    fixture.harness.run_steps(30);
    fixture.commands();
    fixture
}

fn favorites_with(total: u32) -> Playlist {
    let mut favorites = favorites();
    favorites.total = total;
    favorites
}

#[test]
fn playlist_tracks_can_be_reordered_by_dragging() {
    let tracks = vec![killer_queen(), best_friend(), somebody(), radio()];
    let mut fixture = open_favorites(tracks.clone());
    let from = fixture.center_of(&row(&killer_queen()));
    // When the test lifts it, it leaves a gap and the other tracks move up. Thus the point just
    // above the old third track is now below its middle, before the last track.
    let to = fixture.center_of(&row(&somebody())) - vec2(0.0, 20.0);
    fixture.drag(from, to);
    assert!(
        fixture.has("It will go here"),
        "a slot shows where it will go"
    );
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(4),
            edit: PlaylistEdit::Move {
                from: 0,
                to: 2,
                uri: killer_queen().uri,
            },
        }]
    );
    fixture.harness.run_steps(30);
    assert!(!fixture.has("It will go here"));

    // The new order shows immediately, and the list read before the edit does not undo it.
    let top = |fixture: &Fixture, track: &Track| fixture.center_of(&row(track)).y;
    let check_order = |fixture: &Fixture| {
        assert!(top(fixture, &best_friend()) < top(fixture, &somebody()));
        assert!(top(fixture, &somebody()) < top(fixture, &killer_queen()));
        assert!(top(fixture, &killer_queen()) < top(fixture, &radio()));
    };
    check_order(&fixture);
    fixture.send(Event::PlaylistTracks {
        id: favorites().id,
        tracks: tracks.clone(),
        total: 4,
        edits: 0,
    });
    fixture.harness.run_steps(30);
    check_order(&fixture);
    // When Spotify has it, the UI shows the list that the backend read again.
    fixture.send(Event::PlaylistTracks {
        id: favorites().id,
        tracks,
        total: 4,
        edits: 1,
    });
    fixture.harness.run_steps(30);
    assert!(top(&fixture, &killer_queen()) < top(&fixture, &best_friend()));
}

#[test]
fn dropping_a_playlist_track_elsewhere_leaves_the_playlist_alone() {
    let mut fixture = open_favorites(vec![killer_queen(), best_friend(), somebody()]);
    let from = fixture.center_of(&row(&killer_queen()));
    let to = fixture.center_of("Playlists");
    fixture.drag(from, to);
    // Its position stays open while it is away.
    assert!(fixture.has("It will go here"));
    fixture.drop_at(to);
    assert!(fixture.commands().is_empty());
    fixture.harness.run_steps(30);
    let top = |fixture: &Fixture, track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&fixture, &killer_queen()) < top(&fixture, &best_friend()));
}

#[test]
fn repeated_playlist_tracks_move_the_copy_picked() {
    let tracks = vec![best_friend(), somebody(), best_friend(), radio()];
    let mut fixture = open_favorites(tracks);
    // The second copy goes first.
    let from = fixture
        .harness
        .get_all_by_label(&row(&best_friend()))
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .rect()
        .center();
    let to = fixture.center_of(&row(&best_friend())) - vec2(0.0, 20.0);
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(4),
            edit: PlaylistEdit::Move {
                from: 2,
                to: 0,
                uri: best_friend().uri,
            },
        }]
    );
    fixture.harness.run_steps(30);
    let copies = copies_of(&fixture, &best_friend());
    let somebody_top = fixture.center_of(&row(&somebody())).y;
    assert_eq!(copies.len(), 2);
    assert!(copies[1] < somebody_top, "both copies come first");
}

#[test]
fn playlist_tracks_are_removed_from_their_menu_not_with_a_button() {
    let tracks = vec![best_friend(), somebody(), best_friend()];
    let mut fixture = open_favorites(tracks);
    // The second copy of a repeated track: only that copy goes.
    let second = fixture
        .harness
        .get_all_by_label(&row(&best_friend()))
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .rect()
        .center();
    fixture.harness.hover_at(second);
    fixture.harness.run();
    assert!(
        fixture.has("Add You're My Best Friend to the queue"),
        "the ＋ is still there"
    );
    assert!(
        !fixture.has("Remove You're My Best Friend from the playlist"),
        "no ✕ on hover"
    );
    fixture
        .harness
        .get_all_by_label(&row(&best_friend()))
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click_secondary();
    fixture.harness.run();
    fixture.click("Remove from playlist");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(3),
            edit: PlaylistEdit::Remove {
                index: 2,
                uri: best_friend().uri,
            },
        }]
    );
    fixture.harness.run_steps(30);
    // It goes immediately, and the count changes.
    assert_eq!(copies_of(&fixture, &best_friend()).len(), 1);
    assert!(fixture.has(&row(&somebody())));
    assert!(fixture.has(&playlist_row(&favorites_with(2))));
}

#[test]
fn tracks_added_to_the_open_playlist_show_at_once() {
    let mut fixture = open_favorites(vec![killer_queen()]);
    fixture.search("queen", vec![radio()]);
    fixture.commands();
    let from = fixture.center_of(&row(&radio()));
    let to = fixture.center_of(&playlist_row(&favorites_with(1)));
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(fixture.commands().len(), 1);
    assert!(fixture.has(&playlist_row(&favorites_with(2))));
    // Back to the playlist: the new track is at its end.
    fixture.click(&playlist_row(&favorites_with(2)));
    fixture.harness.run_steps(30);
    assert!(fixture.has(&row(&radio())));
}

fn innuendo() -> Track {
    track("9", "Innuendo", "Queen", "Innuendo")
}

fn mustapha() -> Track {
    track("8", "Mustapha", "Queen", "Jazz")
}

fn bicycle() -> Track {
    track("7", "Bicycle Race", "Queen", "Jazz")
}

#[test]
fn the_current_track_can_be_dropped_on_a_playlist() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.commands();
    let from = fixture.card_grip(&bohemian());
    let to = fixture.center_of(&playlist_row(&favorites()));
    fixture.drag(from, to);
    fixture.drop_at(to);
    fixture.answer_find(false);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites(),
            edit: PlaylistEdit::Insert {
                index: None,
                track: bohemian(),
            },
        }]
    );
    // It is a copy: the queue stays as it was.
    fixture.harness.run_steps(30);
    let top = |track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&killer_queen()) < top(&bohemian()) && top(&bohemian()) < top(&best_friend()));
}

#[test]
fn tracks_that_cannot_move_can_still_be_added_to_a_playlist() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.send(Event::Playback(playing(bohemian())));
    fixture.send(Event::Queue(QueueSnapshot {
        previous: vec![killer_queen()],
        current: Some(bohemian()),
        next: vec![best_friend(), somebody(), radio()],
        edits: 0,
        next_locked: true,
    }));
    fixture.commands();

    // The queue does not accept them: no slot opens there, and their rows stay at their
    // positions.
    let card = fixture.card_grip(&bohemian());
    let below = fixture.center_of(&row(&radio())) + vec2(0.0, 20.0);
    fixture.drag(card, below);
    assert!(!fixture.has("It will go here"));
    let to = fixture.center_of(&playlist_row(&favorites()));
    fixture.harness.hover_at(to);
    fixture.harness.run();
    fixture.drop_at(to);
    fixture.answer_find(false);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites(),
            edit: PlaylistEdit::Insert {
                index: None,
                track: bohemian(),
            },
        }]
    );

    let locked = fixture.center_of(&row(&best_friend()));
    let to = fixture.center_of(&playlist_row(&favorites_with(13)));
    fixture.drag(locked, to);
    assert!(!fixture.has("It will go here"));
    fixture.drop_at(to);
    fixture.answer_find(false);
    // The count increased with the first add.
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(13),
            edit: PlaylistEdit::Insert {
                index: None,
                track: best_friend(),
            },
        }]
    );
}

#[test]
fn a_played_track_dropped_in_the_open_playlist_goes_between_its_songs() {
    let mut fixture = open_favorites_in(queue_fixture(), vec![innuendo(), mustapha(), bicycle()]);
    // The played track is fully visible, so the list does not scroll by itself.
    scroll_queue(&mut fixture, 1000.0);
    let from = fixture.center_of(&row(&killer_queen()));
    // Just above the middle of the second track: between the first two.
    let to = fixture.center_of(&row(&mustapha())) - vec2(0.0, 10.0);
    fixture.drag(from, to);
    // A slot opens in the playlist (and its position in the queue stays open).
    assert_eq!(fixture.count("It will go here"), 2);
    let slot = fixture.leftmost("It will go here").rect().center().y;
    assert!(slot > fixture.center_of(&row(&innuendo())).y);
    assert!(slot < fixture.center_of(&row(&mustapha())).y);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(3),
            edit: PlaylistEdit::Insert {
                index: Some(1),
                track: killer_queen(),
            },
        }]
    );
    fixture.harness.run_steps(30);
    assert!(!fixture.has("It will go here"));
    // It shows there immediately, and the queue keeps it.
    assert_eq!(fixture.count(&row(&killer_queen())), 2);
    let top = |fixture: &Fixture, track: &Track| fixture.center_of(&row(track)).y;
    assert!(top(&fixture, &innuendo()) < top(&fixture, &killer_queen()));
    assert!(top(&fixture, &killer_queen()) < top(&fixture, &mustapha()));
    assert!(fixture.has(&playlist_row(&favorites_with(4))));
}

#[test]
fn the_current_track_dropped_in_the_open_playlist_can_go_last() {
    let mut fixture = open_favorites_in(queue_fixture(), vec![innuendo(), mustapha()]);
    let from = fixture.card_grip(&bohemian());
    let to = fixture.center_of(&row(&mustapha())) + vec2(0.0, 60.0);
    fixture.drag(from, to);
    assert!(fixture.has("It will go here"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(2),
            edit: PlaylistEdit::Insert {
                index: Some(2),
                track: bohemian(),
            },
        }]
    );
}

#[test]
fn an_empty_open_playlist_takes_a_dropped_track() {
    let mut fixture = open_favorites_in(queue_fixture(), vec![]);
    assert!(fixture.has("Empty playlist"));
    let from = fixture.center_of(&row(&somebody()));
    let to = fixture.center_of("Empty playlist");
    fixture.drag(from, to);
    assert!(!fixture.has("Empty playlist"), "the slot shows instead");
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditPlaylist {
            playlist: favorites_with(0),
            edit: PlaylistEdit::Insert {
                index: Some(0),
                track: somebody(),
            },
        }]
    );
    fixture.harness.run_steps(30);
    assert_eq!(fixture.count(&row(&somebody())), 2);
}

#[test]
fn someone_elses_open_playlist_takes_no_tracks() {
    let mut fixture = with_playlists(queue_fixture());
    fixture.click("Hits — By Spotify");
    fixture.harness.run_steps(30);
    fixture.commands();
    // The played track is fully visible, so the list does not scroll by itself.
    scroll_queue(&mut fixture, 1000.0);
    let from = fixture.center_of(&row(&killer_queen()));
    let to = fixture.center_of("Its songs can't be shown");
    fixture.drag(from, to);
    // Only the position that it left in the queue.
    assert_eq!(fixture.count("It will go here"), 1);
    fixture.drop_at(to);
    assert!(fixture.commands().is_empty());
}

#[test]
fn window_buttons_are_drawn_by_the_app() {
    let mut fixture = Fixture::new();
    fixture.send(Event::Auth(AuthStatus::LoggedOut {
        client_id: Some(CLIENT_ID.into()),
        redirect_uri: REDIRECT_URI.into(),
        error: None,
    }));
    for label in ["Minimize", "Maximize", "Close window"] {
        assert!(fixture.has(label), "no {label} on the login screen");
    }

    let fixture = Fixture::logged_in();
    for label in ["Minimize", "Maximize", "Close window"] {
        assert!(fixture.has(label), "no {label} on the main screen");
    }
    // The ☰ menu goes immediately before the window buttons.
    let menu = fixture.harness.get_by_label("More options").rect();
    let minimize = fixture.harness.get_by_label("Minimize").rect();
    assert!(menu.right() <= minimize.left() + 0.5);
    assert!((menu.top() - minimize.top()).abs() < 0.5);
}

// --- Back and forward -------------------------------------------------------------

fn with_three_playlists() -> Fixture {
    let mut fixture = Fixture::logged_in();
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites(),
        hits(),
        calm(),
    ])));
    fixture
}

impl Fixture {
    fn press_with(&mut self, modifiers: Modifiers, key: Key) {
        self.harness.key_press_modifiers(modifiers, key);
        self.harness.run_steps(4);
    }

    fn mouse_button(&mut self, button: egui::PointerButton) {
        for pressed in [true, false] {
            self.harness.event(egui::Event::PointerButton {
                pos: Pos2::new(500.0, 400.0),
                button,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        self.harness.run_steps(4);
    }

    /// The playlist with its tracks on the screen, if there is one.
    fn open_playlist(&self) -> Option<&'static str> {
        ["Favorites", "Hits", "Calm"]
            .into_iter()
            .find(|name| self.has(&format!("Songs in “{name}”")))
    }
}

#[test]
fn back_and_forward_go_through_the_playlists_opened() {
    let mut fixture = with_three_playlists();
    fixture.harness.run_steps(4);
    assert!(fixture.leftmost("Back").accesskit_node().is_disabled());
    for playlist in [favorites(), calm(), hits()] {
        fixture.click(&playlist_row(&playlist));
    }
    fixture.harness.run_steps(4);
    assert_eq!(fixture.open_playlist(), Some("Hits"));

    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    assert_eq!(fixture.open_playlist(), Some("Calm"));
    fixture.click("Back");
    fixture.harness.run_steps(4);
    assert_eq!(fixture.open_playlist(), Some("Favorites"));
    // Before the first view, the guide.
    fixture.mouse_button(egui::PointerButton::Extra1);
    assert_eq!(fixture.open_playlist(), None);
    assert!(fixture.has("Keyboard shortcuts"));

    fixture.press_with(Modifiers::COMMAND, Key::ArrowRight);
    assert_eq!(fixture.open_playlist(), Some("Favorites"));
    fixture.click("Forward");
    fixture.harness.run_steps(4);
    assert_eq!(fixture.open_playlist(), Some("Calm"));
    fixture.mouse_button(egui::PointerButton::Extra2);
    assert_eq!(fixture.open_playlist(), Some("Hits"));
    assert!(fixture.leftmost("Forward").accesskit_node().is_disabled());
}

#[test]
fn going_back_to_a_search_restores_its_last_text() {
    let mut fixture = with_three_playlists();
    fixture.search("queen", vec![bohemian()]);
    fixture.click(&playlist_row(&favorites()));
    fixture.harness.run_steps(4);
    assert!(!fixture.has(&row(&bohemian())));

    // Its results are still there: there is no new search.
    fixture.commands();
    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    assert_eq!(fixture.open_playlist(), None);
    assert!(fixture.has(&row(&bohemian())));
    assert_eq!(fixture.commands(), vec![]);

    // The text typed last comes back, and the UI searches for it.
    fixture.click("Search");
    fixture.harness.get_by_label("Search").type_text(" live");
    fixture.harness.run_steps(3);
    fixture.click(&playlist_row(&calm()));
    fixture.commands();
    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    assert_eq!(fixture.open_playlist(), None);
    assert_eq!(
        fixture.commands(),
        vec![Command::Search {
            query: "queen live".into(),
            offset: 0
        }]
    );
    // Forward goes to the playlist again.
    fixture.press_with(Modifiers::COMMAND, Key::ArrowRight);
    assert_eq!(fixture.open_playlist(), Some("Calm"));
}

#[test]
fn playlists_that_are_gone_are_skipped_going_back() {
    let mut fixture = with_three_playlists();
    for playlist in [favorites(), calm(), hits()] {
        fixture.click(&playlist_row(&playlist));
    }
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites(),
        hits(),
    ])));
    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    assert_eq!(fixture.open_playlist(), Some("Favorites"));
}

#[test]
fn ctrl_shift_and_the_arrows_skip_tracks() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    fixture.commands();
    let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    fixture.press_with(ctrl_shift, Key::ArrowRight);
    fixture.press_with(ctrl_shift, Key::ArrowLeft);
    assert_eq!(fixture.commands(), vec![Command::Next, Command::Previous]);
    // Ctrl alone navigates.
    fixture.press_with(Modifiers::COMMAND, Key::ArrowRight);
    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    assert_eq!(fixture.commands(), vec![]);
}

// --- Albums ------------------------------------------------------------------------

/// A track of "A Night at the Opera", which the user can open.
fn in_opera(track: Track) -> Track {
    Track {
        album_id: Some("opera".into()),
        ..track
    }
}

fn opera() -> Album {
    Album {
        id: "opera".into(),
        name: "A Night at the Opera".into(),
        year: Some(1975),
        tracks: vec![in_opera(best_friend()), in_opera(bohemian())],
    }
}

const OPEN_OPERA: &str = "Open album “A Night at the Opera”";
const OPERA_SONGS: &str = "Songs in the album “A Night at the Opera”";

#[test]
fn the_album_in_the_card_opens_it_in_the_middle_column() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(in_opera(bohemian())));
    fixture.commands();
    fixture.click(OPEN_OPERA);
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadAlbum { id: "opera".into() }]
    );
    // Its name shows immediately, and its songs show when they load.
    fixture.harness.run_steps(4);
    assert!(fixture.has(OPERA_SONGS));
    assert!(!fixture.has("Keyboard shortcuts"));

    fixture.send(Event::Album(opera()));
    fixture.harness.run_steps(4);
    // The year it came out and the total duration of its two songs of 3:20.
    assert!(fixture.has("1975 · 7 min"));
    assert!(fixture.has(&row(&in_opera(best_friend()))));

    // A song of the album plays only that song.
    fixture.click("Play You're My Best Friend");
    assert_eq!(
        fixture.commands(),
        vec![Command::PlayTrack {
            track: in_opera(best_friend()),
        }]
    );

    // Esc closes it.
    fixture.press_with(Modifiers::NONE, Key::Escape);
    fixture.harness.run_steps(4);
    assert!(!fixture.has(OPERA_SONGS));
    assert!(fixture.has("Keyboard shortcuts"));
}

#[test]
fn the_album_of_any_track_in_a_list_opens_it() {
    let mut fixture = Fixture::logged_in();
    fixture.search("queen", vec![in_opera(best_friend())]);
    fixture.harness.run_steps(4);
    fixture.commands();
    fixture.click(OPEN_OPERA);
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadAlbum { id: "opera".into() }]
    );
    fixture.harness.run_steps(4);
    assert!(fixture.has(OPERA_SONGS));
}

#[test]
fn a_track_without_an_album_has_no_link_in_the_card() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    assert!(!fixture.has(OPEN_OPERA));
}

#[test]
fn an_album_that_cannot_be_loaded_says_why() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(in_opera(bohemian())));
    fixture.click(OPEN_OPERA);
    fixture.send(Event::AlbumFailed {
        id: "opera".into(),
        message: "Spotify is not responding.".into(),
    });
    fixture.harness.run_steps(4);
    assert!(fixture.has("The album can't be shown"));
}

#[test]
fn an_album_takes_the_place_of_the_playlist_and_back_returns_to_it() {
    let mut fixture = with_three_playlists();
    fixture.play(playing(in_opera(bohemian())));
    fixture.click(&playlist_row(&favorites()));
    fixture.harness.run_steps(4);
    assert_eq!(fixture.open_playlist(), Some("Favorites"));

    fixture.click(OPEN_OPERA);
    fixture.send(Event::Album(opera()));
    fixture.harness.run_steps(4);
    assert_eq!(fixture.open_playlist(), None);
    assert!(fixture.has(OPERA_SONGS));

    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    assert_eq!(fixture.open_playlist(), Some("Favorites"));
    assert!(!fixture.has(OPERA_SONGS));

    // Forward opens the album again, and loads it again.
    fixture.commands();
    fixture.press_with(Modifiers::COMMAND, Key::ArrowRight);
    assert!(fixture.has(OPERA_SONGS));
    assert!(fixture.has("1975 · 7 min"));
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadAlbum { id: "opera".into() }]
    );

    // A search replaces it.
    fixture.search("queen", vec![radio()]);
    fixture.harness.run_steps(4);
    assert!(!fixture.has(OPERA_SONGS));
}

// --- Artists -----------------------------------------------------------------------

/// `track` by Queen, which the user can open.
fn by_queen(track: Track) -> Track {
    Track {
        artist_ids: vec![Some("queen".into())],
        ..track
    }
}

fn release(id: &str, name: &str, kind: ReleaseKind, date: &str, total_tracks: u32) -> Release {
    Release {
        id: id.into(),
        name: name.into(),
        kind,
        release_date: date.into(),
        total_tracks,
        cover: None,
    }
}

/// An album ("A Night at the Opera") and two singles, one older and one newer.
fn queen() -> Artist {
    Artist::new(
        "queen".into(),
        "Queen".into(),
        vec![
            release(
                "opera",
                "A Night at the Opera",
                ReleaseKind::Album,
                "1975-11-21",
                12,
            ),
            release(
                "bohemian-single",
                "Bohemian Rhapsody",
                ReleaseKind::Single,
                "1975-10-31",
                2,
            ),
            release(
                "innuendo-single",
                "Innuendo",
                ReleaseKind::Single,
                "1991-01-14",
                1,
            ),
        ],
    )
}

const OPEN_QUEEN: &str = "Open artist “Queen”";
const QUEEN_RELEASES: &str = "Albums and singles of “Queen”";
const OPERA_RELEASE: &str = "A Night at the Opera — Album · 1975 · 12 songs";

#[test]
fn the_artist_in_the_card_opens_with_their_albums_and_singles_by_date() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(by_queen(bohemian())));
    fixture.commands();
    fixture.click(OPEN_QUEEN);
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadArtist { id: "queen".into() }]
    );
    // Their name shows immediately, and their releases show when they load.
    fixture.harness.run_steps(4);
    assert!(fixture.has(QUEEN_RELEASES));
    assert!(!fixture.has("Keyboard shortcuts"));

    fixture.send(Event::Artist(queen()));
    fixture.harness.run_steps(4);
    assert!(fixture.has("1 album · 2 singles"));
    // Albums and singles together, the newest first, without their songs.
    let y = |label: &str| fixture.center_of(label).y;
    let innuendo = y("Innuendo — Single · 1991 · 1 song");
    let opera = y(OPERA_RELEASE);
    let bohemian = y("Bohemian Rhapsody — Single · 1975 · 2 songs");
    assert!(innuendo < opera && opera < bohemian);
    assert!(!fixture.has("Play Bohemian Rhapsody"));

    // A release opens as an album. «Back» goes back to the artist, which shows immediately while
    // it loads again.
    fixture.click(OPERA_RELEASE);
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadAlbum { id: "opera".into() }]
    );
    fixture.harness.run_steps(4);
    assert!(fixture.has(OPERA_SONGS));
    assert!(!fixture.has(QUEEN_RELEASES));
    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    fixture.harness.run_steps(4);
    assert!(fixture.has(OPERA_RELEASE));
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadArtist { id: "queen".into() }]
    );

    // Esc closes it.
    fixture.press_with(Modifiers::NONE, Key::Escape);
    fixture.harness.run_steps(4);
    assert!(!fixture.has(QUEEN_RELEASES));
    assert!(fixture.has("Keyboard shortcuts"));
}

#[test]
fn an_album_of_an_artist_dropped_into_the_queue_goes_there_whole() {
    let mut fixture = queue_fixture();
    fixture.search("queen", vec![by_queen(innuendo_track())]);
    fixture.harness.run_steps(4);
    fixture.click(OPEN_QUEEN);
    fixture.send(Event::Artist(queen()));
    fixture.harness.run_steps(4);
    fixture.commands();

    let from = fixture.center_of(OPERA_RELEASE);
    // Just above the middle of the second upcoming track: between the first two.
    let to = fixture.center_of(&row(&somebody())) - vec2(0.0, 10.0);
    fixture.drag(from, to);
    assert!(fixture.has("It will go here"));
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::QueueAlbum {
            id: "opera".into(),
            name: "A Night at the Opera".into(),
            at: QueueAt::Next(1),
        }]
    );
}

/// The songs of "A Night at the Opera", as read when the user drops it on a playlist (only a few).
fn opera_songs() -> Vec<Track> {
    vec![bohemian(), radio(), somebody()]
}

/// Opens Queen with the playlists listed and drops "A Night at the Opera" on «Favorites». The
/// answer tells that the playlist has the songs in `found`.
fn drop_opera_on_favorites(found: &[Track]) -> Fixture {
    let mut fixture = with_playlists(queue_fixture());
    fixture.search("queen", vec![by_queen(innuendo_track())]);
    fixture.harness.run_steps(4);
    fixture.click(OPEN_QUEEN);
    fixture.send(Event::Artist(queen()));
    fixture.harness.run_steps(4);
    fixture.commands();

    let from = fixture.center_of(OPERA_RELEASE);
    let to = fixture.center_of(&playlist_row(&favorites()));
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::FindAlbumInPlaylist {
            playlist: favorites(),
            id: "opera".into(),
            name: "A Night at the Opera".into(),
        }]
    );
    fixture.send(Event::FoundAlbumInPlaylist {
        playlist: favorites(),
        name: "A Night at the Opera".into(),
        tracks: opera_songs(),
        found: found.iter().map(|track| track.uri.clone()).collect(),
    });
    fixture
}

fn appended(tracks: Vec<Track>) -> Vec<Command> {
    vec![Command::EditPlaylist {
        playlist: favorites(),
        edit: PlaylistEdit::Append { tracks },
    }]
}

#[test]
fn an_album_dropped_on_a_playlist_adds_all_its_songs() {
    let mut fixture = drop_opera_on_favorites(&[]);
    assert!(!fixture.has(ALREADY_THERE));
    assert_eq!(fixture.commands(), appended(opera_songs()));
    fixture.harness.run_steps(10);
    assert!(fixture.has(&playlist_row(&favorites_with(15))));
}

#[test]
fn an_album_with_songs_the_playlist_has_asks_whether_to_skip_them() {
    let mut fixture = drop_opera_on_favorites(&[radio()]);
    assert!(fixture.has(ALREADY_THERE));
    assert!(fixture.has("1 of the 3 songs of “A Night at the Opera” is already in “Favorites”."));
    assert_eq!(fixture.commands(), vec![], "nothing is added yet");
    fixture.click("Skip repeated");
    assert_eq!(fixture.commands(), appended(vec![bohemian(), somebody()]));
    fixture.harness.run_steps(10);
    assert!(!fixture.has(ALREADY_THERE));
    assert!(fixture.has(&playlist_row(&favorites_with(14))));
}

#[test]
fn an_album_with_songs_the_playlist_has_can_be_added_whole() {
    let mut fixture = drop_opera_on_favorites(&[radio(), somebody()]);
    assert!(fixture.has("2 of the 3 songs of “A Night at the Opera” are already in “Favorites”."));
    fixture.click("Add all");
    assert_eq!(fixture.commands(), appended(opera_songs()));

    // Enter skips them. Esc adds nothing.
    let mut fixture = drop_opera_on_favorites(&[radio()]);
    fixture.press_with(Modifiers::NONE, Key::Enter);
    assert_eq!(fixture.commands(), appended(vec![bohemian(), somebody()]));
    let mut fixture = drop_opera_on_favorites(&[radio()]);
    fixture.press_with(Modifiers::NONE, Key::Escape);
    fixture.harness.run_steps(10);
    assert!(!fixture.has(ALREADY_THERE));
    assert_eq!(fixture.commands(), vec![]);
}

#[test]
fn an_album_the_playlist_already_has_whole_is_added_again_only_if_confirmed() {
    let mut fixture = drop_opera_on_favorites(&opera_songs());
    assert!(fixture.has(
        "All the songs of “A Night at the Opera” are already in “Favorites”. Add them again?"
    ));
    assert!(!fixture.has("Skip repeated"));
    fixture.click("Add again");
    assert_eq!(fixture.commands(), appended(opera_songs()));
}

/// A track that is not in the queue of [`queue_fixture`].
fn innuendo_track() -> Track {
    track("9", "Innuendo", "Queen", "Innuendo")
}

#[test]
fn each_artist_of_a_track_in_a_list_opens_them() {
    let mut fixture = Fixture::logged_in();
    let under_pressure = Track {
        artists: vec!["Queen".into(), "David Bowie".into()],
        artist_ids: vec![Some("queen".into()), Some("bowie".into())],
        ..track("5", "Under Pressure", "Queen", "Hot Space")
    };
    fixture.search("queen", vec![under_pressure]);
    fixture.harness.run_steps(4);
    fixture.commands();
    assert!(fixture.has(OPEN_QUEEN));
    fixture.click("Open artist “David Bowie”");
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadArtist { id: "bowie".into() }]
    );
    fixture.harness.run_steps(4);
    assert!(fixture.has("Albums and singles of “David Bowie”"));
}

#[test]
fn a_track_without_the_ids_of_its_artists_has_no_links_to_them() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(bohemian()));
    assert!(!fixture.has(OPEN_QUEEN));
}

#[test]
fn an_artist_that_cannot_be_loaded_says_why() {
    let mut fixture = Fixture::logged_in();
    fixture.play(playing(by_queen(bohemian())));
    fixture.click(OPEN_QUEEN);
    fixture.send(Event::ArtistFailed {
        id: "queen".into(),
        message: "Spotify is not responding.".into(),
    });
    fixture.harness.run_steps(4);
    assert!(fixture.has("The artist can't be shown"));
}

#[test]
fn songs_can_be_saved_to_liked_songs() {
    let mut fixture = open_favorites(vec![bohemian()]);
    // The UI asks one time about each song that shows.
    assert_eq!(fixture.liked_checks(), vec![bohemian().uri]);
    fixture.send(Event::LikedStates(vec![(bohemian().uri, false)]));
    fixture.harness.run_steps(3);
    assert!(fixture.liked_checks().is_empty());

    fixture.click("Save Bohemian Rhapsody to Liked Songs");
    assert_eq!(
        fixture.commands(),
        vec![Command::SetLiked {
            track: bohemian(),
            liked: true
        }]
    );
    // The heart fills immediately, and an older answer does not empty it.
    assert!(fixture.has("Remove Bohemian Rhapsody from Liked Songs"));
    fixture.send(Event::LikedStates(vec![(bohemian().uri, false)]));
    assert!(fixture.has("Remove Bohemian Rhapsody from Liked Songs"));

    fixture.click("Remove Bohemian Rhapsody from Liked Songs");
    assert_eq!(
        fixture.commands(),
        vec![Command::SetLiked {
            track: bohemian(),
            liked: false
        }]
    );
    assert!(fixture.has("Save Bohemian Rhapsody to Liked Songs"));
}

#[test]
fn liked_songs_show_their_heart_in_every_list() {
    let mut fixture = queue_fixture();
    assert!(fixture.has(&format!("Save {} to Liked Songs", best_friend().name)));
    fixture.search("queen", vec![bohemian()]);
    fixture.send(Event::LikedStates(vec![(bohemian().uri, true)]));
    assert!(fixture.has("Remove Bohemian Rhapsody from Liked Songs"));
    // Local files have no heart.
    let mut local = bohemian();
    local.uri = "spotify:local:Queen::Bohemian+Rhapsody:354".into();
    let mut fixture = Fixture::logged_in();
    fixture.search("queen", vec![local]);
    assert!(!fixture.has("Save Bohemian Rhapsody to Liked Songs"));
    assert!(!fixture.has("Remove Bohemian Rhapsody from Liked Songs"));
}

#[test]
fn a_like_that_spotify_rejects_goes_back() {
    let mut fixture = open_favorites(vec![bohemian()]);
    fixture.send(Event::LikedStates(vec![(bohemian().uri, false)]));
    fixture.click("Save Bohemian Rhapsody to Liked Songs");
    fixture.send(Event::LikedChanged {
        uri: bohemian().uri,
        liked: false,
    });
    assert!(fixture.has("Save Bohemian Rhapsody to Liked Songs"));
}

#[test]
fn without_the_permission_the_heart_waits_for_the_new_login() {
    let mut fixture = open_favorites(vec![bohemian()]);
    fixture.send(Event::LikedNeedsReauth);
    fixture.click("Save Bohemian Rhapsody to Liked Songs");
    assert_eq!(
        fixture.commands(),
        vec![Command::SetLiked {
            track: bohemian(),
            liked: true
        }]
    );
    assert!(fixture.has("Save Bohemian Rhapsody to Liked Songs"));
    // The backend saves it when the account is connected again.
    fixture.send(Event::LikedChanged {
        uri: bohemian().uri,
        liked: true,
    });
    assert!(fixture.has("Remove Bohemian Rhapsody from Liked Songs"));
}

#[test]
fn the_song_playing_can_be_saved_to_liked_songs_from_its_card() {
    // Bohemian Rhapsody plays: it is only in the card, not in a row.
    let mut fixture = queue_fixture();
    fixture.send(Event::LikedStates(vec![(bohemian().uri, false)]));
    assert_eq!(fixture.count("Save Bohemian Rhapsody to Liked Songs"), 1);
    fixture.click("Save Bohemian Rhapsody to Liked Songs");
    assert_eq!(
        fixture.commands(),
        vec![Command::SetLiked {
            track: bohemian(),
            liked: true
        }]
    );
    assert!(fixture.has("Remove Bohemian Rhapsody from Liked Songs"));
}

#[test]
fn searching_also_finds_artists_and_albums_that_open_on_a_click() {
    let mut fixture = Fixture::logged_in();
    fixture.click("Search");
    fixture.harness.get_by_label("Search").type_text("queen");
    fixture.harness.run_steps(3);
    fixture.send(Event::SearchResults(SearchPage {
        query: "queen".into(),
        artists: vec![ArtistHit {
            id: "queen".into(),
            name: "Queen".into(),
            cover: None,
        }],
        albums: vec![AlbumHit {
            release: Release {
                id: "opera".into(),
                name: "Queen II".into(),
                kind: ReleaseKind::Album,
                release_date: "1975-11-21".into(),
                total_tracks: 12,
                cover: None,
            },
            artists: vec!["Queen".into()],
        }],
        tracks: vec![bohemian()],
        total: 1,
        ..Default::default()
    }));
    fixture.harness.run_steps(4);
    fixture.commands();
    // Each item tells what it is. The album also tells its artist.
    assert!(fixture.has("Queen — Artist"));
    assert!(fixture.has("Queen II — Album · Queen"));

    fixture.click("Queen II — Album · Queen");
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadAlbum { id: "opera".into() }]
    );
    fixture.harness.run_steps(4);
    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    fixture.harness.run_steps(4);
    fixture.commands();

    fixture.click("Queen — Artist");
    assert_eq!(
        fixture.commands(),
        vec![Command::LoadArtist { id: "queen".into() }]
    );

    // The songs still play from their own position in the list.
    fixture.press_with(Modifiers::COMMAND, Key::ArrowLeft);
    fixture.harness.run_steps(4);
    fixture.commands();
    fixture.click("Play Bohemian Rhapsody");
    assert_eq!(
        fixture.commands(),
        vec![Command::PlayTrack { track: bohemian() }]
    );
}

#[test]
fn the_search_result_that_best_matches_the_query_goes_first() {
    let mut fixture = Fixture::logged_in();
    fixture.click("Search");
    fixture
        .harness
        .get_by_label("Search")
        .type_text("bohemian rhapsody");
    fixture.harness.run_steps(3);
    fixture.send(Event::SearchResults(SearchPage {
        query: "bohemian rhapsody".into(),
        artists: vec![ArtistHit {
            id: "queen".into(),
            name: "Queen".into(),
            cover: None,
        }],
        tracks: vec![bohemian()],
        total: 1,
        ..Default::default()
    }));
    fixture.harness.run_steps(4);
    fixture.commands();
    // The song is the first result, so ↓ + Enter plays it.
    fixture.press(Key::ArrowDown);
    fixture.press(Key::Enter);
    assert_eq!(
        fixture.commands(),
        vec![Command::PlayTrack { track: bohemian() }]
    );
}

// --- Folders of playlists ---------------------------------------------------

/// «Festivals» (with «Calm» in it), then «Favorites» and «Hits».
fn festivals_library() -> Library {
    Library::new(vec![
        LibraryItem::Folder(Folder {
            id: "folder-1".into(),
            name: "Festivals".into(),
            items: vec![LibraryItem::Playlist("calm".into())],
        }),
        LibraryItem::Playlist("fav".into()),
        LibraryItem::Playlist("hits".into()),
    ])
}

fn with_festivals(mut fixture: Fixture) -> Fixture {
    fixture.send(Event::Library {
        library: festivals_library(),
        edits: 0,
    });
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites(),
        hits(),
        calm(),
    ])));
    fixture.commands();
    fixture
}

const FESTIVALS_ROW: &str = "Festivals — Folder · 1 playlist";

#[test]
fn the_card_of_a_new_folder_goes_away_with_esc_or_without_a_name() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.commands();
    fixture.click("New folder");
    fixture.harness.run_steps(3);
    assert!(fixture.has("New folder, no name yet"));
    fixture
        .harness
        .get_by_label("Name of the new folder")
        .type_text("Festivals");
    fixture.harness.run();
    fixture.harness.key_press(Key::Escape);
    fixture.harness.run_steps(3);
    assert!(!fixture.has("Festivals — New folder"));

    // Without a name, a click on a different area also removes it.
    fixture.click("New folder");
    fixture.harness.run_steps(3);
    fixture.click(&playlist_row(&favorites()));
    fixture.harness.run_steps(3);
    assert!(!fixture.has("New folder, no name yet"));
    assert!(
        fixture
            .commands()
            .iter()
            .all(|c| !matches!(c, Command::EditLibrary(_)))
    );
}

#[test]
fn folders_are_created_opened_and_left() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.commands();
    assert!(
        fixture.has("Playlists"),
        "the first level has the plain title"
    );
    // Its card shows at the top, with the field on the line of its name.
    fixture.click("New folder");
    fixture.harness.run_steps(3);
    fixture
        .harness
        .get_by_label("Name of the new folder")
        .type_text("Festivals");
    fixture.harness.run();
    let card = "Festivals — New folder";
    assert!(fixture.center_of(card).y < fixture.center_of(&playlist_row(&favorites())).y);
    let field = fixture.center_of("Name of the new folder");
    assert!(field.y < fixture.center_of(card).y);
    fixture.harness.key_press(Key::Enter);
    fixture.harness.run();
    assert!(!fixture.has(card), "it is the folder now");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditLibrary(LibraryEdit::CreateFolder {
            id: "folder-1".into(),
            name: "Festivals".into(),
            parent: None,
        })]
    );
    // It shows immediately, at the top.
    fixture.harness.run_steps(10);
    let row = "Festivals — Folder · Empty";
    assert!(fixture.center_of(row).y < fixture.center_of(&playlist_row(&favorites())).y);

    // In the folder, the title is a breadcrumb.
    fixture.click(row);
    assert!(fixture.has("Back to Playlists"));
    assert!(fixture.has("Festivals"));
    assert!(fixture.has("This folder is empty"));
    assert!(!fixture.has(&playlist_row(&favorites())));

    // New items go in it. The empty folder offers to create one.
    fixture.click("Create a playlist");
    fixture
        .harness
        .get_by_label("Name of the new playlist")
        .type_text("Resurrection Fest");
    fixture.harness.run();
    fixture.click("Create playlist");
    assert_eq!(
        fixture.commands(),
        vec![Command::CreatePlaylist {
            name: "Resurrection Fest".into(),
            folder: Some("folder-1".into()),
        }]
    );
    fixture.click("New folder");
    fixture
        .harness
        .get_by_label("Name of the new folder")
        .type_text("2026");
    fixture.harness.run();
    fixture.click("Create folder");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditLibrary(LibraryEdit::CreateFolder {
            id: "folder-2".into(),
            name: "2026".into(),
            parent: Some("folder-1".into()),
        })]
    );

    // Two levels down, the breadcrumb shows the previous level.
    fixture.harness.run_steps(10);
    fixture.click("2026 — Folder · Empty");
    assert!(fixture.has("Back to Festivals"));
    assert!(fixture.has("2026"));
    fixture.click("Back to Festivals");
    assert!(fixture.has("2026 — Folder · Empty"));
    fixture.click("Back to Playlists");
    assert!(fixture.has(&playlist_row(&favorites())));
    assert!(!fixture.has("Back to Playlists"));
}

#[test]
fn playlists_are_dragged_into_folders_and_out_through_the_breadcrumb() {
    let mut fixture = with_festivals(queue_fixture());
    assert!(!fixture.has(&playlist_row(&calm())), "inside its folder");

    // Over a folder, no slot opens between rows: the item goes in.
    let from = fixture.center_of(&playlist_row(&favorites()));
    let to = fixture.center_of(FESTIVALS_ROW);
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditLibrary(LibraryEdit::Move {
            item: LibraryKey::Playlist("fav".into()),
            to: Some("folder-1".into()),
            index: None,
        })]
    );
    fixture.harness.run_steps(30);
    assert!(fixture.has("Festivals — Folder · 2 playlists"));
    assert!(!fixture.has(&playlist_row(&favorites())));

    // A library from before the move does not undo it.
    fixture.send(Event::Library {
        library: festivals_library(),
        edits: 0,
    });
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites(),
        hits(),
        calm(),
    ])));
    assert!(fixture.has("Festivals — Folder · 2 playlists"));

    // In the folder, when the test drops the item on the previous level of the breadcrumb, it
    // goes back there, immediately after the folder.
    fixture.click("Festivals — Folder · 2 playlists");
    let from = fixture.center_of(&playlist_row(&favorites()));
    let to = fixture.center_of("Back to Playlists");
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditLibrary(LibraryEdit::Move {
            item: LibraryKey::Playlist("fav".into()),
            to: None,
            index: Some(1),
        })]
    );
    fixture.harness.run_steps(30);
    assert!(!fixture.has(&playlist_row(&favorites())));
    fixture.click("Back to Playlists");
    assert!(fixture.has(&playlist_row(&favorites())));
    assert!(fixture.has(FESTIVALS_ROW));
}

#[test]
fn folders_are_rearranged_but_never_queued() {
    let mut fixture = with_festivals(queue_fixture());
    // Dropped below the other items: it goes last.
    let from = fixture.center_of(FESTIVALS_ROW);
    let to = fixture.center_of(&playlist_row(&hits())) + vec2(0.0, 30.0);
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert_eq!(
        fixture.commands(),
        vec![Command::EditLibrary(LibraryEdit::Move {
            item: LibraryKey::Folder("folder-1".into()),
            to: None,
            index: Some(2),
        })]
    );
    fixture.harness.run_steps(30);
    assert!(
        fixture.center_of(FESTIVALS_ROW).y > fixture.center_of(&playlist_row(&hits())).y,
        "last"
    );

    // The queue does not accept it.
    let from = fixture.center_of(FESTIVALS_ROW);
    let to = fixture.center_of(&row(&somebody()));
    fixture.drag(from, to);
    fixture.drop_at(to);
    assert!(fixture.commands().is_empty());
}

#[test]
fn a_folder_is_renamed_and_deleted_from_its_menu() {
    let mut fixture = with_festivals(Fixture::logged_in());
    fixture.leftmost(FESTIVALS_ROW).click_secondary();
    fixture.harness.run();
    fixture.click("Rename folder");
    assert!(!fixture.has("Delete folder"), "the menu closes");
    fixture.harness.run_steps(3);
    let field = fixture.center_of("New name of the folder");
    let row = fixture.center_of(FESTIVALS_ROW);
    assert!(
        field.y < row.y && row.y - field.y < 20.0,
        "the field takes the place of the name, on its line of the row"
    );
    fixture
        .harness
        .key_press_modifiers(Modifiers::COMMAND, Key::A);
    fixture
        .harness
        .get_by_label("New name of the folder")
        .type_text("Fiestas");
    fixture.harness.run();
    fixture.click("Rename folder");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditLibrary(LibraryEdit::RenameFolder {
            id: "folder-1".into(),
            name: "Fiestas".into(),
        })]
    );
    fixture.harness.run_steps(10);
    let renamed = "Fiestas — Folder · 1 playlist";
    assert!(fixture.has(renamed));

    // After the delete, its contents go to its position.
    fixture.leftmost(renamed).click_secondary();
    fixture.harness.run();
    fixture.click("Delete folder");
    assert_eq!(
        fixture.commands(),
        vec![Command::EditLibrary(LibraryEdit::DeleteFolder {
            id: "folder-1".into(),
        })]
    );
    fixture.harness.run_steps(10);
    assert!(!fixture.has(renamed));
    assert!(
        fixture.center_of(&playlist_row(&calm())).y
            < fixture.center_of(&playlist_row(&favorites())).y
    );
}

#[test]
fn a_folder_that_is_gone_is_left() {
    let mut fixture = with_festivals(Fixture::logged_in());
    fixture.click(FESTIVALS_ROW);
    assert!(fixture.has(&playlist_row(&calm())));
    fixture.send(Event::Library {
        library: Library::default(),
        edits: 0,
    });
    fixture.send(Event::Playlists(PlaylistsStatus::Loaded(vec![
        favorites(),
        hits(),
        calm(),
    ])));
    assert!(!fixture.has("Back to Playlists"));
    assert!(fixture.has(&playlist_row(&favorites())));
}

// --- Track menu (right click) -------------------------------------------------

impl Fixture {
    /// Opens the menu of the leftmost row labeled `label` with a right click.
    fn open_menu(&mut self, label: &str) {
        self.leftmost(label).click_secondary();
        self.harness.run();
    }
}

#[test]
fn a_search_result_has_a_menu_to_play_queue_and_like_it() {
    let mut fixture = queue_fixture();
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new.clone()]);
    fixture.send(Event::LikedStates(vec![(new.uri.clone(), false)]));
    fixture.commands();

    fixture.open_menu(&row(&new));
    assert!(fixture.has("Play next"));
    assert!(fixture.has("Play last"));
    assert!(
        !fixture.has("Remove from playlist"),
        "search results are in no playlist"
    );
    fixture.click("Play next");
    assert!(!fixture.has("Play next"), "the menu closes");
    fixture.open_menu(&row(&new));
    fixture.click("Play last");
    fixture.open_menu(&row(&new));
    fixture.click("Play");
    fixture.open_menu(&row(&new));
    fixture.click("Like");
    assert_eq!(
        fixture.commands(),
        vec![
            Command::EditQueue(QueueEdit::Insert {
                index: 0,
                track: new.clone(),
            }),
            Command::AddToQueue { track: new.clone() },
            Command::PlayTrack { track: new.clone() },
            Command::SetLiked {
                track: new.clone(),
                liked: true,
            },
        ]
    );
    // Now it offers to remove the like.
    fixture.open_menu(&row(&new));
    assert!(fixture.has("Unlike"));
    assert!(!fixture.has("Like"));
    fixture.click("Unlike");
    assert_eq!(
        fixture.commands(),
        vec![Command::SetLiked {
            track: new,
            liked: false,
        }]
    );
}

#[test]
fn play_next_from_the_menu_puts_the_track_first_in_the_queue() {
    let mut fixture = queue_fixture();
    let new = track("9", "Innuendo", "Queen", "Innuendo");
    fixture.search("innuendo", vec![new.clone()]);
    fixture.open_menu(&row(&new));
    fixture.click("Play next");
    fixture.harness.run_steps(30);
    // In the results and, immediately under the current track, in the queue.
    let copies = copies_of(&fixture, &new);
    assert_eq!(copies.len(), 2);
    let in_queue = fixture
        .harness
        .get_all_by_label(&row(&new))
        .max_by(|a, b| a.rect().center().x.total_cmp(&b.rect().center().x))
        .unwrap()
        .rect()
        .center()
        .y;
    assert!(in_queue < fixture.center_of(&row(&best_friend())).y);
}

#[test]
fn a_playlist_track_can_be_played_or_removed_from_its_menu() {
    let tracks = vec![killer_queen(), best_friend(), somebody()];
    let mut fixture = open_favorites(tracks.clone());
    fixture.open_menu(&row(&best_friend()));
    // «Play» plays only that song, as its ▶ does.
    fixture.click("Play");
    fixture.open_menu(&row(&best_friend()));
    fixture.click("Remove from playlist");
    assert_eq!(
        fixture.commands(),
        vec![
            Command::PlayTrack {
                track: best_friend(),
            },
            Command::EditPlaylist {
                playlist: favorites_with(3),
                edit: PlaylistEdit::Remove {
                    index: 1,
                    uri: best_friend().uri,
                },
            },
        ]
    );
    fixture.harness.run_steps(30);
    assert!(!fixture.has(&row(&best_friend())));
    assert!(fixture.has(&playlist_row(&favorites_with(2))));
}

#[test]
fn escape_closes_the_track_menu_but_not_the_playlist() {
    let mut fixture = open_favorites(vec![killer_queen(), somebody()]);
    fixture.open_menu(&row(&somebody()));
    assert!(fixture.has("Remove from playlist"));
    fixture.press(Key::Escape);
    fixture.harness.run_steps(5);
    assert!(!fixture.has("Remove from playlist"));
    assert!(fixture.has(&row(&somebody())), "the playlist stays open");
    // A click on a different area also closes it, and does nothing.
    fixture.open_menu(&row(&somebody()));
    fixture.click(&row(&killer_queen()));
    fixture.harness.run_steps(5);
    assert!(!fixture.has("Remove from playlist"));
    assert!(fixture.commands().is_empty());
}

impl Fixture {
    /// Opens the menu of the row labeled `label`, clicks «Share» and returns the copied texts.
    fn share(&mut self, label: &str) -> Vec<String> {
        self.open_menu(label);
        self.leftmost("Share").click();
        let mut copied = Vec::new();
        for _ in 0..4 {
            self.harness.step();
            for command in &self.harness.output().platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    copied.push(text.clone());
                }
            }
        }
        copied
    }

    fn toasts(&self) -> Vec<String> {
        self.harness
            .state()
            .toast_messages()
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn share_copies_the_spotify_link_of_a_song_an_album_or_an_artist() {
    let mut fixture = Fixture::logged_in();
    fixture.click("Search");
    fixture.harness.get_by_label("Search").type_text("queen");
    fixture.harness.run_steps(3);
    let song = track(
        "4u7EnebtmKWzUH433cf5Qv",
        "Bohemian Rhapsody",
        "Queen",
        "Queen II",
    );
    fixture.send(Event::SearchResults(SearchPage {
        query: "queen".into(),
        artists: vec![ArtistHit {
            id: "1dfeR4HaWDbWqFHLkxsg1d".into(),
            name: "Queen".into(),
            cover: None,
        }],
        albums: vec![AlbumHit {
            release: release(
                "4ZdQAB4AbLB8SwVpBe6ywP",
                "Queen II",
                ReleaseKind::Album,
                "1975-11-21",
                12,
            ),
            artists: vec!["Queen".into()],
        }],
        tracks: vec![song.clone()],
        total: 1,
        ..Default::default()
    }));
    fixture.harness.run_steps(4);
    fixture.commands();

    fixture.open_menu("Queen — Artist");
    assert!(fixture.has("Share"));
    assert!(!fixture.has("Play next"), "an artist has only «Share»");
    fixture.press(Key::Escape);
    fixture.harness.run_steps(5);

    let shares = [
        ("Queen — Artist", "artist/1dfeR4HaWDbWqFHLkxsg1d", "Queen"),
        (
            "Queen II — Album · Queen",
            "album/4ZdQAB4AbLB8SwVpBe6ywP",
            "Queen II",
        ),
        (
            &row(&song),
            "track/4u7EnebtmKWzUH433cf5Qv",
            "Bohemian Rhapsody",
        ),
    ];
    for (label, path, name) in shares {
        assert_eq!(
            fixture.share(label),
            [format!("https://open.spotify.com/{path}")]
        );
        assert_eq!(
            fixture.toasts().last(),
            Some(&format!("Copied the link to “{name}”"))
        );
        fixture.harness.run_steps(5);
        assert!(!fixture.has("Share"), "the menu closes");
    }
    assert!(fixture.commands().is_empty());
}

#[test]
fn share_copies_the_spotify_link_of_a_playlist_in_the_sidebar() {
    let mut fixture = with_playlists(Fixture::logged_in());
    fixture.commands();
    assert_eq!(
        fixture.share("Hits — By Spotify"),
        ["https://open.spotify.com/playlist/hits"]
    );
    assert_eq!(fixture.toasts(), ["Copied the link to “Hits”"]);
    assert!(fixture.commands().is_empty(), "the playlist does not open");
}

#[test]
fn a_local_file_has_no_link_to_share() {
    let mut fixture = queue_fixture();
    let mut local = track("x", "Innuendo (demo)", "Queen", "Innuendo");
    local.uri = "spotify:local:Queen:Innuendo:Innuendo+(demo):390".into();
    fixture.search("innuendo", vec![local.clone()]);
    fixture.open_menu(&row(&local));
    assert!(fixture.has("Play next"));
    assert!(!fixture.has("Share"));
}

/// The three commands of an album's menu, «Play», «Play next» and «Play
/// last», for "A Night at the Opera".
fn opera_menu_commands() -> Vec<Command> {
    [QueueAt::Now, QueueAt::Next(0), QueueAt::End]
        .into_iter()
        .map(|at| Command::QueueAlbum {
            id: "opera".into(),
            name: "A Night at the Opera".into(),
            at,
        })
        .collect()
}

impl Fixture {
    /// Selects each of «Play», «Play next» and «Play last» in the menu of the row with the label
    /// `label`.
    fn choose_each_play_option(&mut self, label: &str) {
        for option in ["Play", "Play next", "Play last"] {
            self.open_menu(label);
            self.click(option);
        }
    }
}

#[test]
fn an_album_in_the_search_results_has_a_menu_to_play_or_queue_it() {
    let mut fixture = queue_fixture();
    fixture.click("Search");
    fixture
        .harness
        .get_by_label("Search")
        .type_text("a night at the opera");
    fixture.harness.run_steps(3);
    fixture.send(Event::SearchResults(SearchPage {
        query: "a night at the opera".into(),
        albums: vec![AlbumHit {
            release: release(
                "opera",
                "A Night at the Opera",
                ReleaseKind::Album,
                "1975-11-21",
                12,
            ),
            artists: vec!["Queen".into()],
        }],
        ..Default::default()
    }));
    fixture.harness.run_steps(4);
    fixture.commands();

    let album = "A Night at the Opera — Album · Queen";
    fixture.open_menu(album);
    assert!(fixture.has("Play next"));
    assert!(!fixture.has("Like"), "albums have no like");
    assert!(!fixture.has("Remove from playlist"));
    fixture.press(Key::Escape);
    fixture.harness.run_steps(5);
    assert!(fixture.has(album), "Esc closes only the menu");

    fixture.choose_each_play_option(album);
    assert_eq!(fixture.commands(), opera_menu_commands());
}

#[test]
fn an_album_of_an_artist_has_a_menu_to_play_or_queue_it() {
    let mut fixture = queue_fixture();
    fixture.search("queen", vec![by_queen(innuendo_track())]);
    fixture.harness.run_steps(4);
    fixture.click(OPEN_QUEEN);
    fixture.send(Event::Artist(queen()));
    fixture.harness.run_steps(4);
    fixture.commands();

    fixture.choose_each_play_option(OPERA_RELEASE);
    assert_eq!(fixture.commands(), opera_menu_commands());
    assert!(fixture.has(QUEEN_RELEASES), "the artist stays open");
}

#[test]
fn a_song_of_an_album_has_the_same_menu_as_a_search_result() {
    let mut fixture = queue_fixture();
    fixture.search("queen", vec![in_opera(best_friend())]);
    fixture.click(OPEN_OPERA);
    fixture.send(Event::Album(opera()));
    fixture.harness.run_steps(4);
    fixture.send(Event::LikedStates(vec![(best_friend().uri, false)]));
    fixture.commands();

    let song = in_opera(best_friend());
    fixture.open_menu(&row(&song));
    assert!(fixture.has("Like"));
    assert!(!fixture.has("Remove from playlist"));
    fixture.press(Key::Escape);
    fixture.choose_each_play_option(&row(&song));
    fixture.open_menu(&row(&song));
    fixture.click("Like");
    assert_eq!(
        fixture.commands(),
        vec![
            // «Play» plays only that song, as its ▶ does.
            Command::PlayTrack {
                track: song.clone(),
            },
            Command::EditQueue(QueueEdit::Insert {
                index: 0,
                track: song.clone(),
            }),
            Command::AddToQueue {
                track: song.clone(),
            },
            Command::SetLiked {
                track: song,
                liked: true,
            },
        ]
    );
}
