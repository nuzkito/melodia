//! The live backend end to end, against a mock Spotify server.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use melodia::auth::{Session, SessionStore, unix_now};
use melodia::backend::{
    self, AuthStatus, BackendHandle, Command, Event, QueueAt, Timings, Toast, WebPlayerStatus,
};
use melodia::config::AppConfig;
use melodia::model::{
    LibraryEdit, LibraryKey, Playlist, PlaylistEdit, PlaylistsStatus, QueueEdit, QueuePosition,
};
use melodia::webplayer::{
    HostRequest, PageControl, PlayerEvent, PlayerEvents, WebPlayerHost, parse_page_message,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

fn track(id: &str, name: &str) -> Value {
    json!({
        "type": "track",
        "uri": format!("spotify:track:{id}"),
        "name": name,
        "duration_ms": 240_000,
        "artists": [{ "name": "Queen" }],
        "album": { "name": "Greatest Hits", "images": [] }
    })
}

fn config(server: &MockServer, dir: &Path) -> AppConfig {
    let dir = dir.to_str().unwrap().to_owned();
    let mut config = AppConfig::from_env(
        |key| match key {
            "MELODIA_CONFIG_DIR" => Some(dir.clone()),
            "SPOTIFY_CLIENT_ID" => Some("0123456789abcdef0123456789abcdef".into()),
            _ => None,
        },
        &[],
    );
    config.api_base = format!("{}/v1", server.uri());
    config.accounts_base = server.uri();
    config.timings = timings();
    config
}

/// The backend's timings in these tests: a fifth of the app's timings. Thus a test that waits for
/// a timer (or for no timer) does not take seconds.
fn timings() -> Timings {
    Timings::scaled(0.2)
}

/// Longer than the backend's short follow-ups (`state_after_command`, `state_after_page_report`,
/// `where_retry_after`). After this time, they have done their work.
fn after_follow_ups() -> Duration {
    timings().where_retry_after * 3 / 2
}

fn save_session(dir: &Path, expires_in: i64) {
    save_session_with_scope(dir, expires_in, "");
}

fn save_session_with_scope(dir: &Path, expires_in: i64, scope: &str) {
    let session = Session {
        access_token: "token-1".into(),
        refresh_token: "refresh-1".into(),
        expires_at: (unix_now() as i64 + expires_in) as u64,
        scope: scope.into(),
    };
    SessionStore::new(dir.join("session.json"))
        .save(&session)
        .unwrap();
}

/// The test's side of a running live backend. It keeps each event. Thus a wait for one event
/// never loses a different event that arrived first (the backend does some work at the same time,
/// so the order is not fixed).
struct Backend {
    handle: BackendHandle,
    task: JoinHandle<()>,
    log: Vec<Option<Event>>,
}

impl Backend {
    fn start(config: AppConfig) -> Self {
        let (handle, sink, commands) = backend::channel(|| {});
        let task = tokio::spawn(backend::run_live(config, commands, sink));
        Self {
            handle,
            task,
            log: Vec::new(),
        }
    }

    fn send(&self, command: Command) {
        self.handle.send(command);
    }

    /// Waits (up to 10 s) for an event that matches `predicate`, oldest first. It returns each
    /// event one time at most.
    async fn wait_for(&mut self, mut predicate: impl FnMut(&Event) -> bool) -> Event {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            while let Some(event) = self.handle.try_recv() {
                self.log.push(Some(event));
            }
            if let Some(slot) = self
                .log
                .iter_mut()
                .find(|slot| slot.as_ref().is_some_and(&mut predicate))
            {
                return slot.take().expect("the slot just matched");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for an event; received: {:#?}",
                self.log.iter().flatten().collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn web_status(
        &mut self,
        mut accept: impl FnMut(&WebPlayerStatus) -> bool,
    ) -> WebPlayerStatus {
        let Event::WebPlayer(status) = self
            .wait_for(|e| matches!(e, Event::WebPlayer(s) if accept(s)))
            .await
        else {
            unreachable!()
        };
        status
    }

    /// Closes the command channel and waits for the backend to end.
    async fn stop(self) {
        drop(self.handle);
        self.task.await.unwrap();
    }
}

async fn wait_for_request(
    server: &MockServer,
    http_method: &str,
    url_path: &str,
) -> wiremock::Request {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let requests = server.received_requests().await.unwrap();
        if let Some(request) = requests
            .into_iter()
            .find(|r| r.method.as_str() == http_method && r.url.path() == url_path)
        {
            return request;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no {http_method} {url_path} request"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn mount_get(server: &MockServer, url_path: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(url_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restores_the_session_and_controls_playback() {
    let server = MockServer::start().await;
    mount_get(
        &server,
        "/v1/me",
        json!({ "id": "alice", "display_name": "Alice" }),
    )
    .await;
    mount_get(
        &server,
        "/v1/me/player",
        json!({
            "device": { "id": "pc", "name": "PC", "type": "Computer", "is_active": true },
            "is_playing": false,
            "progress_ms": 30_000,
            "item": track("a", "Under Pressure")
        }),
    )
    .await;
    mount_get(
        &server,
        "/v1/me/player/queue",
        json!({ "queue": [track("d", "Radio Ga Ga")] }),
    )
    .await;
    mount_get(
        &server,
        "/v1/search",
        json!({ "tracks": { "items": [track("e", "Somebody to Love")], "total": 1 } }),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let mut backend = Backend::start(config(&server, dir.path()));

    // The backend uses the saved session again: no login is necessary.
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedIn { demo: false })))
        .await;
    backend
        .wait_for(|e| matches!(e, Event::Profile(p) if p.display_name == "Alice"))
        .await;
    // Paused on the PC: nothing plays on a different device, so the UI shows it as it is.
    backend
        .wait_for(|e| {
            matches!(e, Event::Playback(p) if !p.is_playing && p.track.as_ref().is_some_and(|t| t.name == "Under Pressure"))
        })
        .await;
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if !q.next.is_empty()))
        .await
    else {
        unreachable!()
    };
    let names = |tracks: &[melodia::model::Track]| {
        tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
    };
    assert!(
        queue.previous.is_empty(),
        "recently played tracks are not shown"
    );
    assert_eq!(queue.current.unwrap().name, "Under Pressure");
    assert_eq!(names(&queue.next), ["Radio Ga Ga"]);

    // «Play» shows immediately (optimistic update) and goes to Spotify. Without this app's
    // player, the music plays on the PC. After that, it is on a different device, and the UI
    // shows only its device.
    backend.send(Command::Resume);
    backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.is_playing))
        .await;
    wait_for_request(&server, "PUT", "/v1/me/player/play").await;
    let Event::PlayingElsewhere(Some(pc)) = backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await
    else {
        unreachable!()
    };
    assert_eq!(pc.name, "PC");

    backend.send(Command::Search {
        query: "queen".into(),
        offset: 0,
    });
    backend
        .wait_for(|e| {
            matches!(e, Event::SearchResults(page) if page.query == "queen" && page.tracks[0].name == "Somebody to Love")
        })
        .await;

    backend.send(Command::Logout);
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedOut { error: None, .. })))
        .await;
    assert!(
        SessionStore::new(dir.path().join("session.json"))
            .load()
            .is_none()
    );

    backend.stop().await;
}

/// A Spotify that has "Under Pressure" paused on the PC, with `queue` next.
async fn paused_under_pressure(queue: Value) -> MockServer {
    let server = MockServer::start().await;
    mount_get(&server, "/v1/me", json!({ "id": "alice" })).await;
    mount_get(
        &server,
        "/v1/me/player",
        json!({
            "device": { "id": "pc", "name": "PC", "type": "Computer", "is_active": true },
            "is_playing": false,
            "progress_ms": 30_000,
            "item": track("a", "Under Pressure")
        }),
    )
    .await;
    mount_get(&server, "/v1/me/player/queue", json!({ "queue": queue })).await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queue_is_back_when_the_app_opens_again() {
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let saved = dir.path().join("queue.json");

    let server = paused_under_pressure(json!([
        track("b", "Radio Ga Ga"),
        track("c", "Bohemian Rhapsody"),
        track("d", "Innuendo")
    ]))
    .await;
    let mut backend = Backend::start(config(&server, dir.path()));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 3))
        .await;
    // «Innuendo» becomes a previous track, and «Bohemian Rhapsody» goes first.
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 3,
        to: 0,
        uri: "spotify:track:d".into(),
    }));
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 3,
        to: 2,
        uri: "spotify:track:c".into(),
    }));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 2))
        .await;
    backend.stop().await;
    assert!(saved.exists(), "the queue was saved when the app closed");

    // The backend does not read Spotify's queue again: the app's own order is back.
    let server = paused_under_pressure(json!([track("e", "Somebody to Love")])).await;
    let mut backend = Backend::start(config(&server, dir.path()));
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.current.is_some()))
        .await
    else {
        unreachable!()
    };
    assert_eq!(names(&queue.previous), ["Innuendo"]);
    assert_eq!(queue.current.unwrap().name, "Under Pressure");
    assert_eq!(names(&queue.next), ["Bohemian Rhapsody", "Radio Ga Ga"]);

    // Spotify does not know that order: the resume sends it, from the old position.
    backend.send(Command::Resume);
    let play = wait_for_request(&server, "PUT", "/v1/me/player/play").await;
    let body: Value = serde_json::from_slice(&play.body).unwrap();
    assert_eq!(
        body["uris"],
        json!(["spotify:track:a", "spotify:track:c", "spotify:track:b"])
    );
    assert_eq!(body["position_ms"], 30_000);
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.url.path() == "/v1/me/player/queue"),
        "Spotify's queue was read"
    );

    // The logout forgets it.
    backend.send(Command::Logout);
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedOut { .. })))
        .await;
    backend.stop().await;
    assert!(!saved.exists(), "the saved queue is deleted on logout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playing_without_an_active_device_uses_an_available_one() {
    let server = MockServer::start().await;
    mount_get(&server, "/v1/me", json!({ "id": "alice" })).await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    mount_get(&server, "/v1/me/player/queue", json!({ "queue": [] })).await;
    mount_get(
        &server,
        "/v1/me/player/devices",
        json!({ "devices": [{ "id": "pc", "name": "Laptop", "type": "Computer", "is_active": false }] }),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .and(query_param("device_id", "pc"))
        .respond_with(ResponseTemplate::new(204))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": { "status": 404, "message": "Player command failed: No active device found", "reason": "NO_ACTIVE_DEVICE" }
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let mut backend = Backend::start(config(&server, dir.path()));
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedIn { .. })))
        .await;

    backend.send(Command::Resume);
    // It plays on the only device.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .any(|r| r.url.path() == "/v1/me/player/play" && r.url.query() == Some("device_id=pc"))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no play on the device"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spotify_is_asked_what_plays_once_and_never_polled() {
    let server = MockServer::start().await;
    mount_get(&server, "/v1/me", json!({ "id": "alice" })).await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    mount_get(&server, "/v1/me/player/queue", json!({ "queue": [] })).await;
    mount_get(&server, "/v1/me/player/devices", json!({ "devices": [] })).await;
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    // Here, the test only waits for the timers, so they are shorter than in the other tests.
    let mut config = config(&server, dir.path());
    config.timings = Timings::scaled(0.05);
    let tick = config.timings.tick;
    let mut backend = Backend::start(config);
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedIn { .. })))
        .await;

    wait_for_request(&server, "GET", "/v1/me/player").await;
    // Longer than a tick and than all follow-ups: nothing else asks Spotify again.
    tokio::time::sleep(tick * 6 / 5).await;
    assert_eq!(state_requests(&server).await, 1);
    assert_eq!(gets(&server, "/v1/me/player/queue").await, 0);
    assert_eq!(gets(&server, "/v1/me/player/devices").await, 0);

    // The picker asks for them.
    backend.send(Command::RefreshDevices);
    wait_for_request(&server, "GET", "/v1/me/player/devices").await;

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_session_asks_to_log_in_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant", "error_description": "Refresh token revoked"
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), -60);
    let mut backend = Backend::start(config(&server, dir.path()));

    let event = backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedOut { .. })))
        .await;
    let Event::Auth(AuthStatus::LoggedOut {
        error, client_id, ..
    }) = event
    else {
        unreachable!()
    };
    assert!(error.unwrap().contains("expired"));
    assert_eq!(
        client_id.as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
    assert!(
        SessionStore::new(dir.path().join("session.json"))
            .load()
            .is_none()
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_session_the_login_screen_is_requested() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut backend = Backend::start(config(&server, dir.path()));

    let event = backend.wait_for(|e| matches!(e, Event::Auth(_))).await;
    assert_eq!(
        event,
        Event::Auth(AuthStatus::LoggedOut {
            client_id: Some("0123456789abcdef0123456789abcdef".into()),
            redirect_uri: "http://127.0.0.1:8898/callback".into(),
            error: None,
        })
    );

    backend.stop().await;
}

// --- «Play in this app» -------------------------------------------------

/// A message posted by the player page.
fn page(json: &str) -> PlayerEvent {
    PlayerEvent::Message(parse_page_message(json).unwrap())
}

/// A replacement for the window of the player page. When it opens (at the volume that it gets),
/// the fake page posts the return value of `on_start`. For each token, it posts the return value
/// of `on_token`.
fn fake_host(
    on_start: impl Fn(Option<u8>) -> Vec<PlayerEvent> + Send + 'static,
    on_token: impl Fn(&str) -> Vec<PlayerEvent> + Send + 'static,
) -> WebPlayerHost {
    recording_host(on_start, on_token, Controls::default())
}

/// The player commands that a fake page received.
type Controls = Arc<std::sync::Mutex<Vec<PageControl>>>;

/// As [`fake_host`], but it keeps the player commands that it gets in `controls`.
fn recording_host(
    on_start: impl Fn(Option<u8>) -> Vec<PlayerEvent> + Send + 'static,
    on_token: impl Fn(&str) -> Vec<PlayerEvent> + Send + 'static,
    controls: Controls,
) -> WebPlayerHost {
    let (host, mut requests) = WebPlayerHost::new(|| {});
    tokio::spawn(async move {
        let mut open: Option<(u64, PlayerEvents)> = None;
        while let Some(request) = requests.recv().await {
            let posted = match request {
                HostRequest::Start {
                    generation,
                    volume,
                    events,
                } => {
                    open = Some((generation, events));
                    on_start(volume)
                }
                HostRequest::Token(token) => on_token(&token),
                HostRequest::Volume(_) => Vec::new(),
                HostRequest::Control(control) => {
                    controls.lock().unwrap().push(control);
                    Vec::new()
                }
                HostRequest::Stop => {
                    open = None;
                    Vec::new()
                }
            };
            if let Some((generation, events)) = &open {
                for event in posted {
                    let _ = events.send((*generation, event));
                }
            }
        }
    });
    host
}

/// A page that does nothing.
fn silent_host() -> WebPlayerHost {
    fake_host(|_| Vec::new(), |_| Vec::new())
}

/// Spotify mocks for a logged-in user listening to "Under Pressure".
async fn mount_playing_account(server: &MockServer) {
    mount_get(server, "/v1/me", json!({ "id": "alice" })).await;
    mount_get(
        server,
        "/v1/me/player",
        json!({
            "device": { "id": "phone", "name": "Phone", "type": "Smartphone", "is_active": true },
            "is_playing": true,
            "progress_ms": 1000,
            "item": track("a", "Under Pressure")
        }),
    )
    .await;
    mount_get(server, "/v1/me/player/queue", json!({ "queue": [] })).await;
    mount_get(server, "/v1/me/player/devices", json!({ "devices": [] })).await;
}

/// Logs in with a session that has `scope` and selects «Play in this app».
async fn start_web_player(
    server: &MockServer,
    scope: &str,
    host: Option<WebPlayerHost>,
) -> (Backend, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, scope);
    let mut config = config(server, dir.path());
    config.web_player = host;
    let mut backend = Backend::start(config);
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedIn { .. })))
        .await;
    // Let Spotify first tell what plays.
    wait_for_request(server, "GET", "/v1/me/player").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    backend.send(Command::StartWebPlayer);
    (backend, dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_player_gets_a_token_and_receives_the_music() {
    let server = MockServer::start().await;
    mount_playing_account(&server).await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let host = fake_host(
        |_| {
            vec![
                page(r#"{"type":"log","message":"page up"}"#),
                page(r#"{"type":"need_token"}"#),
            ]
        },
        |token| match token {
            "token-1" => vec![page(r#"{"type":"ready","device_id":"web-device"}"#)],
            _ => vec![page(
                r#"{"type":"error","kind":"authentication_error","message":"unexpected token"}"#,
            )],
        },
    );
    let (mut backend, _dir) =
        start_web_player(&server, "user-read-playback-state streaming", Some(host)).await;

    backend
        .web_status(|s| *s == WebPlayerStatus::Starting)
        .await;
    let ready = backend
        .web_status(|s| !matches!(s, WebPlayerStatus::Starting))
        .await;
    assert_eq!(
        ready,
        WebPlayerStatus::Ready {
            device_id: "web-device".into()
        }
    );
    // The music moved to the new device and continues to play.
    let transfer = wait_for_request(&server, "PUT", "/v1/me/player").await;
    let body: Value = serde_json::from_slice(&transfer.body).unwrap();
    assert_eq!(body, json!({ "device_ids": ["web-device"], "play": true }));

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_player_needs_the_streaming_permission() {
    let server = MockServer::start().await;
    mount_playing_account(&server).await;
    let (mut backend, _dir) =
        start_web_player(&server, "user-read-playback-state", Some(silent_host())).await;

    assert_eq!(
        backend.web_status(|_| true).await,
        WebPlayerStatus::NeedsReauth
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_player_is_unavailable_without_a_host() {
    let server = MockServer::start().await;
    mount_playing_account(&server).await;

    let (mut backend, _dir) = start_web_player(&server, "streaming", None).await;
    assert!(matches!(
        backend.web_status(|_| true).await,
        WebPlayerStatus::Unavailable(_)
    ));
    backend.stop().await;

    // The window of the page is gone.
    let (gone, requests) = WebPlayerHost::new(|| {});
    drop(requests);
    let (mut backend, _dir) = start_web_player(&server, "streaming", Some(gone)).await;
    let WebPlayerStatus::Failed(message) = backend.web_status(|_| true).await else {
        panic!("expected a failure");
    };
    assert!(message.contains("isn't responding"), "{message}");
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_player_reports_drm_errors_and_crashes() {
    let server = MockServer::start().await;
    mount_playing_account(&server).await;

    let drm = fake_host(
        |_| {
            vec![page(
                r#"{"type":"error","kind":"initialization_error","message":"Failed to initialize player"}"#,
            )]
        },
        |_| Vec::new(),
    );
    let (mut backend, _dir) = start_web_player(&server, "streaming", Some(drm)).await;
    let WebPlayerStatus::Failed(message) = backend
        .web_status(|s| matches!(s, WebPlayerStatus::Failed(_)))
        .await
    else {
        unreachable!()
    };
    assert!(
        message.contains("DRM") && message.contains("Failed to initialize player"),
        "{message}"
    );
    backend.stop().await;

    let crash = fake_host(|_| vec![PlayerEvent::Closed], |_| Vec::new());
    let (mut backend, _dir) = start_web_player(&server, "streaming", Some(crash)).await;
    let WebPlayerStatus::Failed(message) = backend
        .web_status(|s| matches!(s, WebPlayerStatus::Failed(_)))
        .await
    else {
        unreachable!()
    };
    assert!(message.contains("closed unexpectedly"), "{message}");
    backend.stop().await;
}

// --- Melodia's own queue -----------------------------------------------------

/// Spotify paused on the PC, "Under Pressure" at 0:30: what it shows when the app opens in the
/// queue tests.
fn paused_on_pc() -> Value {
    json!({
        "device": {
            "id": "pc", "name": "PC", "type": "Computer", "is_active": true,
            "volume_percent": 80, "supports_volume": true
        },
        "is_playing": false,
        "shuffle_state": false,
        "progress_ms": 30_000,
        "item": track("a", "Under Pressure")
    })
}

/// The music that plays here when the queue tests start (see [`start_here`]).
struct Account {
    playing: bool,
    shuffle: bool,
    progress_ms: u64,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            playing: true,
            shuffle: false,
            progress_ms: 30_000,
        }
    }
}

/// The report of the player page when `id` (with the name `name`, 4 minutes) plays here (or is
/// paused here) at `progress_ms`.
fn here(id: &str, name: &str, playing: bool, progress_ms: u64) -> String {
    json!({
        "type": "state",
        "paused": !playing,
        "position": progress_ms,
        "track_uri": format!("spotify:track:{id}"),
        "track": track(id, name),
        "here": true
    })
    .to_string()
}

/// The Spotify account of the queue tests: "Under Pressure" (4 minutes) with Bicycle Race, Killer
/// Queen and Radio Ga Ga in Spotify's queue. When the app opens, Spotify shows it paused on the PC.
async fn mount_queue_account(server: &MockServer) {
    mount_get(server, "/v1/me", json!({ "id": "alice" })).await;
    mount_get(server, "/v1/me/player/devices", json!({ "devices": [] })).await;
    mount_get(server, "/v1/me/player", paused_on_pc()).await;
    mount_get(
        server,
        "/v1/me/player/queue",
        json!({ "queue": [track("b", "Bicycle Race"), track("c", "Killer Queen"), track("d", "Radio Ga Ga")] }),
    )
    .await;
    for (http_method, url_path) in [
        ("PUT", "/v1/me/player"),
        ("PUT", "/v1/me/player/play"),
        ("PUT", "/v1/me/player/pause"),
        ("PUT", "/v1/me/player/shuffle"),
        ("PUT", "/v1/me/player/volume"),
        ("POST", "/v1/me/player/queue"),
    ] {
        Mock::given(method(http_method))
            .and(path(url_path))
            .respond_with(ResponseTemplate::new(204))
            .mount(server)
            .await;
    }
}

/// Starts the backend with a player page, which takes the music when the app opens. Then the page
/// tells that "Under Pressure" plays here, as `account` tells.
async fn start_here(
    server: &MockServer,
    account: &Account,
) -> (Backend, ScriptedPage, tempfile::TempDir) {
    let page = ScriptedPage::default();
    let (mut backend, dir) = start_with_host(server, page.host(), None);
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;
    wait_for_request(server, "PUT", "/v1/me/player").await;
    let mut report: Value = serde_json::from_str(&here(
        "a",
        "Under Pressure",
        account.playing,
        account.progress_ms,
    ))
    .unwrap();
    report["shuffle"] = json!(account.shuffle);
    page.post(&report.to_string());
    backend
        .wait_for(|e| {
            matches!(e, Event::Playback(p) if p.device.as_ref().is_some_and(|d| d.id == "web-device"))
        })
        .await;
    (backend, page, dir)
}

/// As [`start_here`], and waits until the UI gets Spotify's queue.
async fn start_with_queue(
    server: &MockServer,
    account: &Account,
) -> (Backend, ScriptedPage, tempfile::TempDir) {
    let (mut backend, page, dir) = start_here(server, account).await;
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 3))
        .await;
    (backend, page, dir)
}

fn names(tracks: &[melodia::model::Track]) -> Vec<&str> {
    tracks.iter().map(|t| t.name.as_str()).collect()
}

/// The `(method, path, query)` of the player commands that Spotify received, but not the moves of
/// the music to a different device.
async fn player_commands(server: &MockServer) -> Vec<(String, String, String)> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() != "GET" && r.url.path() != "/v1/me/player")
        .map(|r| {
            (
                r.method.to_string(),
                r.url.path().to_owned(),
                r.url.query().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn innuendo() -> melodia::model::Track {
    melodia::model::Track {
        uri: "spotify:track:e".into(),
        name: "Innuendo".into(),
        artists: vec!["Queen".into()],
        album: "Innuendo".into(),
        duration_ms: 390_000,
        cover_small: None,
        cover_large: None,
        explicit: false,
        album_id: None,
        artist_ids: vec![],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reordering_never_interrupts_the_music() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, page, _dir) = start_with_queue(&server, &Account::default()).await;

    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 3,
        to: 1,
        uri: "spotify:track:d".into(),
    }));
    backend.send(Command::AddToQueue { track: innuendo() });
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 1 && q.next.len() == 4))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        names(&queue.next),
        ["Radio Ga Ga", "Bicycle Race", "Killer Queen", "Innuendo"]
    );
    assert!(!queue.next_locked);

    // Most of the track must still play, so the backend tells Spotify nothing: nothing restarts.
    tokio::time::sleep(timings().expectation_ttl).await;
    assert!(player_commands(&server).await.is_empty());
    assert!(page.controls.lock().unwrap().is_empty());

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn moving_the_current_track_changes_what_counts_as_played() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let account = Account {
        progress_ms: 230_000, // 10 s left
        ..Account::default()
    };
    let (mut backend, _page, _dir) = start_with_queue(&server, &account).await;

    // "Under Pressure" goes after Bicycle Race, which counts as played. Positions count the full
    // queue: the current track is at 0.
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 0,
        to: 1,
        uri: "spotify:track:a".into(),
    }));
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 1))
        .await
    else {
        unreachable!()
    };
    assert_eq!(names(&queue.previous), ["Bicycle Race"]);
    assert_eq!(queue.current.unwrap().name, "Under Pressure");
    assert_eq!(names(&queue.next), ["Killer Queen", "Radio Ga Ga"]);

    // Spotify is about to play Bicycle Race next. Thus Killer Queen goes into its queue, and the
    // music does not stop.
    let queued = wait_for_request(&server, "POST", "/v1/me/player/queue").await;
    assert_eq!(queued.url.query(), Some("uri=spotify%3Atrack%3Ac"));
    assert!(
        !player_commands(&server)
            .await
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/play"),
        "the current track is never restarted"
    );

    // A played track can come next again (immediately after the track sent to Spotify).
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 0,
        to: 1,
        uri: "spotify:track:b".into(),
    }));
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 2))
        .await
    else {
        unreachable!()
    };
    assert!(queue.previous.is_empty());
    assert_eq!(
        names(&queue.next),
        ["Killer Queen", "Bicycle Race", "Radio Ga Ga"]
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn near_the_end_the_next_track_is_queued_in_spotify() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let account = Account {
        progress_ms: 230_000, // 10 s left
        ..Account::default()
    };
    let (mut backend, _page, _dir) = start_with_queue(&server, &account).await;

    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 3,
        to: 1,
        uri: "spotify:track:d".into(),
    }));
    // Spotify is about to play Bicycle Race next: Radio Ga Ga goes into its queue.
    let queued = wait_for_request(&server, "POST", "/v1/me/player/queue").await;
    assert_eq!(queued.url.query(), Some("uri=spotify%3Atrack%3Ad"));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next_locked))
        .await;
    assert!(
        !player_commands(&server)
            .await
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/play"),
        "the current track is never restarted"
    );

    // The app sent it to Spotify: no track can go before it now.
    backend.send(Command::AddToQueue { track: innuendo() });
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 4,
        to: 1,
        uri: "spotify:track:e".into(),
    }));
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 2))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        names(&queue.next),
        ["Radio Ga Ga", "Innuendo", "Bicycle Race", "Killer Queen"]
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playing_a_queued_track_jumps_to_it() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, _page, _dir) = start_with_queue(&server, &Account::default()).await;

    backend.send(Command::PlayFromQueue {
        position: QueuePosition::Next(1),
        uri: "spotify:track:c".into(),
    });
    let Event::Queue(queue) = backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.name == "Killer Queen"))
        })
        .await
    else {
        unreachable!()
    };
    // The skipped track stays in the history. The other tracks come next.
    assert_eq!(names(&queue.previous), ["Under Pressure", "Bicycle Race"]);
    assert_eq!(names(&queue.next), ["Radio Ga Ga"]);

    let request = wait_for_request(&server, "PUT", "/v1/me/player/play").await;
    assert_eq!(request.url.query(), None, "it plays where it already plays");
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body,
        json!({ "uris": ["spotify:track:c", "spotify:track:d"], "offset": { "position": 0 } })
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edits_made_while_paused_wait_for_the_music() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let account = Account {
        playing: false,
        progress_ms: 230_000,
        ..Account::default()
    };
    let (mut backend, page, _dir) = start_with_queue(&server, &account).await;

    backend.send(Command::EditQueue(QueueEdit::Insert {
        index: 0,
        track: innuendo(),
    }));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 1))
        .await;
    tokio::time::sleep(after_follow_ups()).await;
    assert!(player_commands(&server).await.is_empty());

    // «Play» only resumes on the page (Spotify's list stays full). Immediately, the new next
    // track goes into Spotify's queue.
    backend.send(Command::Resume);
    page.wait_for_control(PageControl::Resume).await;
    page.post(&here("a", "Under Pressure", true, 230_000));
    let queued = wait_for_request(&server, "POST", "/v1/me/player/queue").await;
    assert_eq!(queued.url.query(), Some("uri=spotify%3Atrack%3Ae"));
    assert!(
        !player_commands(&server)
            .await
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/play"),
        "a plain resume"
    );

    backend.stop().await;
}

/// Moves the backend's events into its log.
fn drain(backend: &mut Backend) {
    while let Some(event) = backend.handle.try_recv() {
        backend.log.push(Some(event));
    }
}

/// True if the backend showed nothing playing and nothing in the queue after the log cleared.
fn shows_nothing(backend: &Backend) {
    for event in backend.log.iter().flatten() {
        match event {
            Event::Queue(q) => assert!(q.current.is_none() && q.next.is_empty(), "{q:#?}"),
            Event::Playback(p) => assert!(p.track.is_none() && !p.is_playing, "{p:#?}"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clearing_the_queue_stops_the_music_and_hides_everything() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, page, _dir) = start_with_queue(&server, &Account::default()).await;

    backend.send(Command::ClearQueue);
    page.wait_for_control(PageControl::Pause).await;
    // After the pause, Spotify still has the track loaded.
    page.post(&here("a", "Under Pressure", false, 30_000));
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 1))
        .await
    else {
        unreachable!()
    };
    assert!(queue.previous.is_empty() && queue.current.is_none() && queue.next.is_empty());

    // Later reports still show the paused track. It stays hidden, and «play» brings nothing back.
    // The reports come before the pause expectation expires (`expectation_ttl`), when the
    // backend asks Spotify (here, the mock).
    tokio::time::sleep(timings().expectation_ttl / 8).await;
    drain(&mut backend);
    backend.log.clear();
    page.post(&here("a", "Under Pressure", false, 30_000));
    backend.send(Command::Resume);
    tokio::time::sleep(timings().expectation_ttl / 5).await;
    drain(&mut backend);
    shows_nothing(&backend);
    assert_eq!(*page.controls.lock().unwrap(), [PageControl::Pause]);
    let commands = player_commands(&server).await;
    assert!(
        commands.is_empty(),
        "only the pause, on the page: {commands:?}"
    );

    // A different track starts from the beginning.
    backend.send(Command::PlayTrack { track: innuendo() });
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.current.as_ref() == Some(&innuendo())))
        .await;

    backend.stop().await;
}

/// Removes Bicycle Race, Killer Queen and Radio Ga Ga: the queue ends with "Under Pressure". It
/// starts from the end, so Spotify's next track is always the app's next track.
async fn empty_the_upcoming_tracks(backend: &mut Backend) {
    for (index, id) in [(2, "d"), (1, "c"), (0, "b")] {
        backend.send(Command::EditQueue(QueueEdit::Remove {
            index,
            uri: format!("spotify:track:{id}"),
        }));
    }
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 3 && q.next.is_empty()))
        .await;
}

/// Waits until the queue shows "Under Pressure" as played and nothing more.
async fn wait_for_the_end(backend: &mut Backend) {
    backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.is_none()
                && q.next.is_empty()
                && names(&q.previous).last() == Some(&"Under Pressure"))
        })
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_plays_after_the_end_of_the_queue() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let account = Account {
        progress_ms: 236_000, // 4 s left
        ..Account::default()
    };
    let (mut backend, page, _dir) = start_with_queue(&server, &account).await;
    empty_the_upcoming_tracks(&mut backend).await;

    // Spotify continues by itself with a track that is left in its list.
    page.post(&here("b", "Bicycle Race", true, 0));
    page.wait_for_control(PageControl::Pause).await;
    wait_for_the_end(&mut backend).await;
    let commands = player_commands(&server).await;
    assert!(
        !commands
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/queue"),
        "nothing is queued: {commands:?}"
    );

    // Later reports show Bicycle Race paused: it stays hidden. The reports come before the pause
    // expectation expires (`expectation_ttl`), when the backend asks Spotify (here, the mock).
    tokio::time::sleep(timings().expectation_ttl / 8).await;
    drain(&mut backend);
    backend.log.clear();
    page.post(&here("b", "Bicycle Race", false, 0));
    tokio::time::sleep(timings().expectation_ttl / 5).await;
    drain(&mut backend);
    shows_nothing(&backend);

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn next_on_the_last_track_ends_the_queue() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, page, _dir) = start_with_queue(&server, &Account::default()).await;
    empty_the_upcoming_tracks(&mut backend).await;

    backend.send(Command::Next);
    page.wait_for_control(PageControl::Pause).await;
    page.post(&here("a", "Under Pressure", false, 30_000));
    wait_for_the_end(&mut backend).await;
    assert!(
        !page.controls.lock().unwrap().contains(&PageControl::Next),
        "Spotify is not asked for another track"
    );
    assert!(player_commands(&server).await.is_empty());

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spotify_queue_padding_is_not_shown() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    // Only Bicycle Race remains: Spotify repeats it (and the current track).
    Mock::given(method("GET"))
        .and(path("/v1/me/player/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "queue": [track("b", "Bicycle Race"), track("a", "Under Pressure"),
                      track("b", "Bicycle Race"), track("a", "Under Pressure")]
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    let (mut backend, _page, _dir) = start_here(&server, &Account::default()).await;
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if !q.next.is_empty()))
        .await
    else {
        unreachable!()
    };
    assert_eq!(names(&queue.next), ["Bicycle Race"]);

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shuffle_is_turned_off_to_keep_the_order() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let account = Account {
        shuffle: true,
        ..Account::default()
    };
    let (backend, _page, _dir) = start_with_queue(&server, &account).await;

    backend.send(Command::PlayFromQueue {
        position: QueuePosition::Next(1),
        uri: "spotify:track:c".into(),
    });
    wait_for_request(&server, "PUT", "/v1/me/player/play").await;
    let shuffle = wait_for_request(&server, "PUT", "/v1/me/player/shuffle").await;
    assert_eq!(shuffle.url.query(), Some("state=false"));
    let requests = server.received_requests().await.unwrap();
    let position = |p: &str| requests.iter().position(|r| r.url.path() == p).unwrap();
    assert!(position("/v1/me/player/shuffle") < position("/v1/me/player/play"));

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_this_apps_player_the_volume_goes_to_spotify_and_is_saved() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let mut backend = Backend::start(config(&server, dir.path()));
    // Paused on the PC: nothing plays on a different device.
    backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.track.is_some()))
        .await;

    for percent in [10, 20, 30] {
        backend.send(Command::SetVolume { percent });
    }
    backend
        .wait_for(|e| {
            matches!(e, Event::Playback(p) if p.device.as_ref().and_then(|d| d.volume_percent) == Some(30))
        })
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let volumes: Vec<String> = player_commands(&server)
            .await
            .into_iter()
            .filter(|(_, path, _)| path == "/v1/me/player/volume")
            .map(|(_, _, query)| query)
            .collect();
        let saved = AppConfig::from_env(
            |key| (key == "MELODIA_CONFIG_DIR").then(|| dir.path().to_str().unwrap().into()),
            &[],
        )
        .volume;
        if volumes.last().map(String::as_str) == Some("volume_percent=30") && saved == Some(30) {
            assert!(volumes.len() <= 3, "{volumes:?}");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{volumes:?} {saved:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    backend.stop().await;
}

/// Nuria's phone plays "Something Else", as Spotify shows it.
fn playing_on_phone() -> Value {
    json!({
        "device": {
            "id": "phone", "name": "Nuria's phone", "type": "Smartphone",
            "is_active": true, "volume_percent": 60, "supports_volume": true
        },
        "is_playing": true,
        "shuffle_state": false,
        "progress_ms": 1000,
        "item": track("z", "Something Else")
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn music_started_on_another_device_hides_the_queue_until_it_plays_here_again() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, page, _dir) = start_with_queue(&server, &Account::default()).await;
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 1,
        to: 3,
        uri: "spotify:track:b".into(),
    }));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 1))
        .await;

    // A user starts a different track on the phone. The page is not the device now, and only
    // Spotify knows where the music went.
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(200).set_body_json(playing_on_phone()))
        .with_priority(1)
        .mount(&server)
        .await;
    page.post(r#"{"type":"inactive"}"#);
    let Event::PlayingElsewhere(Some(phone)) = backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await
    else {
        unreachable!()
    };
    assert_eq!(phone.name, "Nuria's phone");
    // This app's queue is gone, and the player also.
    backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.previous.is_empty() && q.current.is_none() && q.next.is_empty())
        })
        .await;
    backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.track.is_none() && p.device.is_none()))
        .await;

    // The app does not control the phone, and it does not ask Spotify again.
    let asked = state_requests(&server).await;
    let queue_reads = gets(&server, "/v1/me/player/queue").await;
    backend.send(Command::Resume);
    backend.send(Command::Next);
    page.post(
        r#"{"type":"state","paused":false,"position":2000,"track_uri":"spotify:track:z","here":false}"#,
    );
    tokio::time::sleep(after_follow_ups()).await;
    assert!(player_commands(&server).await.is_empty());
    assert!(page.controls.lock().unwrap().is_empty());
    assert_eq!(state_requests(&server).await, asked);
    assert_eq!(gets(&server, "/v1/me/player/queue").await, queue_reads);

    // Sent here from the phone: the player and Spotify's queue come back.
    Mock::given(method("GET"))
        .and(path("/v1/me/player/queue"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "queue": [track("y", "Spotify's Own")] })),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    page.post(&here("z", "Something Else", true, 3000));
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(None)))
        .await;
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if names(&q.next) == ["Spotify's Own"]))
        .await
    else {
        unreachable!()
    };
    assert_eq!(queue.current.unwrap().name, "Something Else");

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playing_a_song_while_the_music_is_elsewhere_brings_it_here() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(200).set_body_json(playing_on_phone()))
        .with_priority(1)
        .mount(&server)
        .await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_with_host(&server, page.host(), None);
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await;
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;

    backend.send(Command::PlayTrack { track: innuendo() });
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(None)))
        .await;
    let play = wait_for_request(&server, "PUT", "/v1/me/player/play").await;
    assert_eq!(play.url.query(), Some("device_id=web-device"));
    // The page tells that the music plays here: the backend does not ask Spotify.
    let asked = state_requests(&server).await;
    page.post(&here("e", "Innuendo", true, 0));
    backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.name == "Innuendo"))
        })
        .await;
    tokio::time::sleep(timings().here_check_after * 5 / 4).await;
    assert_eq!(state_requests(&server).await, asked);
    drain(&mut backend);
    assert!(
        !backend
            .log
            .iter()
            .flatten()
            .any(|e| matches!(e, Event::PlayingElsewhere(Some(_)))),
        "the music stays here"
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queue_moves_on_in_melodias_order() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, page, _dir) = start_with_queue(&server, &Account::default()).await;

    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 3,
        to: 2,
        uri: "spotify:track:d".into(),
    }));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 1))
        .await;

    // "Under Pressure" ends and Spotify continues with its next track, which is also Melodia's
    // next track. Melodia follows with its own order.
    let reads = gets(&server, "/v1/me/player/queue").await;
    page.post(&here("b", "Bicycle Race", true, 0));
    let Event::Queue(queue) = backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.name == "Bicycle Race"))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(names(&queue.next), ["Radio Ga Ga", "Killer Queen"]);
    assert_eq!(names(&queue.previous).last(), Some(&"Under Pressure"));
    assert_eq!(
        gets(&server, "/v1/me/player/queue").await,
        reads,
        "Spotify's queue is not asked"
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_track_played_out_of_order_leaves_the_rest_in_place() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, page, _dir) = start_with_queue(&server, &Account::default()).await;

    // The app's order: Radio Ga Ga, Bicycle Race, Killer Queen. "Next" two times on the phone, as
    // a remote control, makes Spotify play its third track, Killer Queen.
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 3,
        to: 1,
        uri: "spotify:track:d".into(),
    }));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.edits == 1))
        .await;
    page.post(&here("c", "Killer Queen", true, 0));
    let Event::Queue(queue) = backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.name == "Killer Queen"))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(names(&queue.next), ["Radio Ga Ga", "Bicycle Race"]);

    backend.stop().await;
}

// --- «Play in this app» by default -------------------------------------------

/// A fake page that becomes the device "web-device" when it gets a token, if it opened at
/// `volume`.
fn fake_ready_host(volume: u8) -> WebPlayerHost {
    ready_host_recording(volume, Controls::default())
}

/// As [`fake_ready_host`], but it keeps the player commands that it gets in `controls`.
fn ready_host_recording(volume: u8, controls: Controls) -> WebPlayerHost {
    recording_host(
        move |opened_at| {
            if opened_at == Some(volume) {
                vec![page(r#"{"type":"need_token"}"#)]
            } else {
                let error = format!(
                    r#"{{"type":"error","kind":"webview","message":"wrong volume: {opened_at:?}"}}"#
                );
                vec![page(&error), PlayerEvent::Closed]
            }
        },
        |_| vec![page(r#"{"type":"ready","device_id":"web-device"}"#)],
        controls,
    )
}

fn start_with_host(
    server: &MockServer,
    host: WebPlayerHost,
    volume: Option<u8>,
) -> (Backend, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, "streaming");
    if let Some(volume) = volume {
        melodia::config::save_volume(dir.path(), volume).unwrap();
    }
    let mut config = config(server, dir.path());
    config.web_player = Some(host);
    (Backend::start(config), dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn this_app_is_the_device_when_nothing_plays_elsewhere() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;

    let (mut backend, _dir) = start_with_host(&server, fake_ready_host(35), Some(35));
    // It started at the volume from last time, without a request.
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;
    // The music moves here, paused: nothing starts to play by itself.
    let transfer = wait_for_request(&server, "PUT", "/v1/me/player").await;
    let body: Value = serde_json::from_slice(&transfer.body).unwrap();
    assert_eq!(body, json!({ "device_ids": ["web-device"], "play": false }));

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_device_active_the_music_starts_straight_on_this_app() {
    let server = MockServer::start().await;
    mount_get(&server, "/v1/me", json!({ "id": "alice" })).await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let (mut backend, _dir) = start_with_host(&server, fake_ready_host(35), Some(35));
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;
    wait_for_request(&server, "PUT", "/v1/me/player").await;

    backend.send(Command::Resume);
    let play = wait_for_request(&server, "PUT", "/v1/me/player/play").await;
    // The backend does not first try without a device, only to learn that no device is active.
    assert_eq!(play.url.query(), Some("device_id=web-device"));
    let plays = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/v1/me/player/play")
        .count();
    assert_eq!(plays, 1);

    backend.stop().await;
}

/// Mocks a paused "Under Pressure" at 0:30, last played on `device`. Starts the backend with the
/// saved position of this app at 1:35.
async fn reopen_paused_on(
    server: &MockServer,
    device: Value,
    controls: Controls,
) -> (Backend, tempfile::TempDir) {
    reopen_paused_remembering(server, device, Some(95_000), controls).await
}

/// As [`reopen_paused_on`], with the saved position of this app at `saved_ms` (or none). The page
/// keeps the player commands that it gets in `controls`.
async fn reopen_paused_remembering(
    server: &MockServer,
    device: Value,
    saved_ms: Option<u64>,
    controls: Controls,
) -> (Backend, tempfile::TempDir) {
    mount_get(server, "/v1/me", json!({ "id": "alice" })).await;
    mount_get(server, "/v1/me/player/devices", json!({ "devices": [] })).await;
    mount_get(server, "/v1/me/player/queue", json!({ "queue": [] })).await;
    mount_get(
        server,
        "/v1/me/player",
        json!({
            "device": device,
            "is_playing": false,
            "shuffle_state": false,
            "progress_ms": 30_000,
            "item": track("a", "Under Pressure")
        }),
    )
    .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, "streaming");
    melodia::config::save_volume(dir.path(), 35).unwrap();
    if let Some(position_ms) = saved_ms {
        melodia::config::save_resume_point(
            dir.path(),
            Some(&melodia::config::ResumePoint {
                uri: "spotify:track:a".into(),
                position_ms,
            }),
        )
        .unwrap();
    }
    let mut config = config(server, dir.path());
    config.web_player = Some(ready_host_recording(35, controls));
    (Backend::start(config), dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopening_the_app_goes_back_to_where_the_music_was() {
    let server = MockServer::start().await;
    let this_app = json!({
        "id": "web-device", "name": "Melodia", "type": "Computer",
        "is_active": true, "volume_percent": 100, "supports_volume": true
    });
    let controls = Controls::default();
    let (mut backend, _dir) = reopen_paused_on(&server, this_app, controls.clone()).await;

    // Spotify tells 0:30, but the app was at 1:35 when it closed: the page goes back there.
    backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.progress_ms == 95_000 && !p.is_playing))
        .await;
    assert_eq!(*controls.lock().unwrap(), [PageControl::Seek(95_000)]);

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_position_in_this_app_is_saved_for_next_time() {
    let server = MockServer::start().await;
    let this_app = json!({
        "id": "web-device", "name": "Melodia", "type": "Computer",
        "is_active": true, "volume_percent": 100, "supports_volume": true
    });
    let (backend, dir) =
        reopen_paused_remembering(&server, this_app, None, Controls::default()).await;

    let saved = || {
        let settings = std::fs::read(dir.path().join("settings.json")).unwrap_or_default();
        serde_json::from_slice::<Value>(&settings).unwrap_or_default()["resume"].clone()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while saved() != json!({ "uri": "spotify:track:a", "position_ms": 30_000 }) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the position was not saved: {}",
            saved()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn music_last_played_elsewhere_keeps_its_position() {
    let server = MockServer::start().await;
    let phone = json!({
        "id": "phone", "name": "Nuria's phone", "type": "Smartphone",
        "is_active": true, "volume_percent": 100, "supports_volume": true
    });
    let (backend, _dir) = reopen_paused_on(&server, phone, Controls::default()).await;

    wait_for_request(&server, "PUT", "/v1/me/player").await;
    tokio::time::sleep(after_follow_ups()).await;
    assert!(
        !player_commands(&server)
            .await
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/seek"),
        "the position was left as Spotify has it"
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn music_playing_elsewhere_is_left_alone() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(200).set_body_json(playing_on_phone()))
        .with_priority(1)
        .mount(&server)
        .await;

    let (mut backend, _dir) = start_with_host(&server, fake_ready_host(100), Some(100));
    let Event::PlayingElsewhere(Some(phone)) = backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await
    else {
        unreachable!()
    };
    assert_eq!(phone.id, "phone");
    // This app's player becomes ready, so the user can send the music here. But it does not take
    // the music.
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;
    tokio::time::sleep(after_follow_ups()).await;
    drain(&mut backend);
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.method.as_str() != "GET" || r.url.path() == "/v1/me/player/queue"),
        "nothing was moved, controlled or read: {:?}",
        requests.iter().map(|r| r.url.path()).collect::<Vec<_>>()
    );
    assert!(
        !backend
            .log
            .iter()
            .flatten()
            .any(|e| matches!(e, Event::Queue(q) if q.current.is_some())),
        "nothing is shown as playing here"
    );

    backend.stop().await;
}

const PLAYLIST_SCOPES: &str = "user-modify-playback-state playlist-read-private \
    playlist-read-collaborative playlist-modify-private playlist-modify-public";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playlists_are_queued_whole_and_receive_tracks() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    mount_get(
        &server,
        "/v1/me/playlists",
        json!({
            "total": 2,
            "items": [
                {
                    "id": "mine", "name": "Favorites", "collaborative": false,
                    "owner": { "id": "alice", "display_name": "Alice" },
                    "images": [], "items": { "total": 2 }
                },
                {
                    "id": "theirs", "name": "Hits", "collaborative": false,
                    "owner": { "id": "spotify", "display_name": "Spotify" },
                    "images": [], "items": { "total": 50 }
                }
            ]
        }),
    )
    .await;
    mount_get(
        &server,
        "/v1/playlists/mine/items",
        json!({
            "total": 2,
            "items": [{ "item": track("e", "Innuendo") }, { "item": track("f", "Mustapha") }]
        }),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/v1/playlists/mine/items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "snapshot_id": "s" })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, PLAYLIST_SCOPES);
    let mut backend = Backend::start(config(&server, dir.path()));
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = backend
        .wait_for(|e| matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_))))
        .await
    else {
        unreachable!()
    };
    assert_eq!(playlists.len(), 2);
    assert!(playlists[0].editable && playlists[0].total == 2);
    assert!(!playlists[1].editable);
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 3))
        .await;

    // Its tracks go in the queue, after the first upcoming track, and the music does not stop.
    backend.send(Command::QueuePlaylist {
        playlist: playlists[0].clone(),
        index: 1,
    });
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 5))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        names(&queue.next),
        [
            "Bicycle Race",
            "Innuendo",
            "Mustapha",
            "Killer Queen",
            "Radio Ga Ga"
        ]
    );
    assert!(player_commands(&server).await.is_empty());

    // A track added to it.
    backend.send(Command::EditPlaylist {
        playlist: playlists[0].clone(),
        edit: PlaylistEdit::Insert {
            index: None,
            track: innuendo(),
        },
    });
    let request = wait_for_request(&server, "POST", "/v1/playlists/mine/items").await;
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body, json!({ "uris": ["spotify:track:e"] }));

    backend.stop().await;
}

/// Three playlists of Alice, in Spotify's order.
async fn mount_three_playlists(server: &MockServer) {
    let playlist = |id: &str, name: &str| {
        json!({
            "id": id, "name": name, "collaborative": false,
            "owner": { "id": "alice", "display_name": "Alice" },
            "images": [], "items": { "total": 4 }
        })
    };
    mount_get(
        server,
        "/v1/me/playlists",
        json!({
            "total": 3,
            "items": [
                playlist("mine", "Favorites"),
                playlist("run", "Running"),
                playlist("calm", "Calm")
            ]
        }),
    )
    .await;
}

async fn loaded_playlists(backend: &mut Backend) -> Vec<String> {
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = backend
        .wait_for(|e| matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_))))
        .await
    else {
        unreachable!()
    };
    playlists.into_iter().map(|p| p.id).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_user_is_read_once_per_session_however_often_the_playlists_are() {
    let server = MockServer::start().await;
    mount_three_playlists(&server).await;
    mount_my_playlist(&server, 200).await;
    let (mut backend, _dir) = start_with_playlist(&server).await;
    loaded_playlists(&mut backend).await;
    let Event::Profile(profile) = backend.wait_for(|e| matches!(e, Event::Profile(_))).await else {
        unreachable!()
    };
    assert_eq!(profile.id, "alice");

    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Insert {
            index: None,
            track: innuendo(),
        },
    });
    loaded_playlists(&mut backend).await;
    backend.send(Command::RefreshPlaylists);
    loaded_playlists(&mut backend).await;

    let gets = |requests: &[wiremock::Request], url_path: &str| {
        requests
            .iter()
            .filter(|r| r.method.as_str() == "GET" && r.url.path() == url_path)
            .count()
    };
    let requests = server.received_requests().await.unwrap();
    assert_eq!(gets(&requests, "/v1/me/playlists"), 3);
    assert_eq!(gets(&requests, "/v1/me"), 1);
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_playlist_is_listed_even_before_spotify_lists_it() {
    let server = MockServer::start().await;
    mount_three_playlists(&server).await;
    mount_my_playlist(&server, 200).await;
    Mock::given(method("POST"))
        .and(path("/v1/me/playlists"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "trip", "name": "Road Trip", "collaborative": false,
            "owner": { "id": "alice", "display_name": "Alice" },
            "images": [], "items": { "total": 0 }
        })))
        .mount(&server)
        .await;
    let (mut backend, _dir) = start_with_playlist(&server).await;
    loaded_playlists(&mut backend).await;

    backend.send(Command::CreatePlaylist {
        name: "Road Trip".into(),
        folder: None,
    });
    let Event::PlaylistCreated { name, id } = backend
        .wait_for(|e| matches!(e, Event::PlaylistCreated { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!((name.as_str(), id.as_deref()), ("Road Trip", Some("trip")));
    // Spotify does not list it yet, but the backend puts it first, one time.
    assert_eq!(
        loaded_playlists(&mut backend).await,
        ["trip", "mine", "run", "calm"]
    );
    backend.send(Command::RefreshPlaylists);
    assert_eq!(
        loaded_playlists(&mut backend).await,
        ["mine", "run", "calm"]
    );
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ui_is_told_when_a_playlist_could_not_be_created() {
    let server = MockServer::start().await;
    mount_three_playlists(&server).await;
    mount_my_playlist(&server, 200).await;
    Mock::given(method("POST"))
        .and(path("/v1/me/playlists"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "status": 400, "message": "Bad request" }
        })))
        .mount(&server)
        .await;
    let (mut backend, _dir) = start_with_playlist(&server).await;
    loaded_playlists(&mut backend).await;

    backend.send(Command::CreatePlaylist {
        name: "Road Trip".into(),
        folder: None,
    });
    let Event::PlaylistCreated { name, id } = backend
        .wait_for(|e| matches!(e, Event::PlaylistCreated { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!((name.as_str(), id), ("Road Trip", None));
    assert!(
        backend.log.iter().flatten().any(
            |e| matches!(e, Event::Toast(t) if t.message.contains("Could not create “Road Trip”"))
        ),
        "a toast says why"
    );
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_folders_and_order_of_the_playlists_are_kept_by_the_app() {
    let server = MockServer::start().await;
    mount_three_playlists(&server).await;
    mount_my_playlist(&server, 200).await;
    let (mut backend, dir) = start_with_playlist(&server).await;
    assert_eq!(
        loaded_playlists(&mut backend).await,
        ["mine", "run", "calm"]
    );

    // "Running" goes into a new folder, and "Calm" to the top.
    backend.send(Command::EditLibrary(LibraryEdit::CreateFolder {
        id: "folder-1".into(),
        name: "Entreno".into(),
        parent: None,
    }));
    backend.send(Command::EditLibrary(LibraryEdit::Move {
        item: LibraryKey::Playlist("run".into()),
        to: Some("folder-1".into()),
        index: None,
    }));
    backend.send(Command::EditLibrary(LibraryEdit::Move {
        item: LibraryKey::Playlist("calm".into()),
        to: None,
        index: Some(0),
    }));
    let order = ["calm", "run", "mine"].map(String::from).to_vec();
    let library = json!([
        {"playlist": "calm"},
        {"folder": {"id": "folder-1", "name": "Entreno", "items": [{"playlist": "run"}]}},
        {"playlist": "mine"}
    ]);
    // The backend reads the playlists again after an edit: they are in the new order.
    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Insert {
            index: None,
            track: innuendo(),
        },
    });
    // Each list comes after the library, which tells that it follows the edits.
    let Event::Library {
        library: sent,
        edits,
    } = backend
        .wait_for(|e| matches!(e, Event::Library { edits: 3, .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(edits, 3);
    assert_eq!(serde_json::to_value(sent.items()).unwrap(), library);
    assert_eq!(loaded_playlists(&mut backend).await, order);
    backend.send(Command::RefreshPlaylists);
    assert_eq!(loaded_playlists(&mut backend).await, order);
    backend.stop().await;

    // The backend saves it for the account, so it is there the next time the app opens.
    let saved = || {
        let settings = std::fs::read(dir.path().join("settings.json")).unwrap_or_default();
        serde_json::from_slice::<Value>(&settings).unwrap_or_default()["playlist_library"]["alice"]
            .clone()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while saved() != library {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the order was not saved"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut backend = Backend::start(config(&server, dir.path()));
    assert_eq!(loaded_playlists(&mut backend).await, order);
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playlists_need_the_new_permissions() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, _page, _dir) = start_with_queue(&server, &Account::default()).await;
    backend
        .wait_for(|e| matches!(e, Event::Playlists(PlaylistsStatus::NeedsReauth)))
        .await;
    backend.stop().await;
}

fn my_playlist() -> Playlist {
    Playlist {
        id: "mine".into(),
        name: "Favorites".into(),
        owner: "Alice".into(),
        total: 4,
        cover: None,
        editable: true,
        snapshot: None,
    }
}

/// "Favorites": Bicycle Race, an entry that the app cannot show, Killer Queen and Bicycle Race
/// again. Changes answer with `snapshot_id` "v2".
async fn mount_my_playlist(server: &MockServer, edits_status: u16) {
    Mock::given(method("GET"))
        .and(path("/v1/playlists/mine"))
        .and(query_param("fields", "snapshot_id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "snapshot_id": "v1" })))
        .mount(server)
        .await;
    mount_get(
        server,
        "/v1/playlists/mine/items",
        json!({
            "total": 4,
            "items": [
                { "item": track("b", "Bicycle Race") },
                { "item": null },
                { "item": track("c", "Killer Queen") },
                { "item": track("b", "Bicycle Race") }
            ]
        }),
    )
    .await;
    for http_method in ["PUT", "POST", "DELETE"] {
        Mock::given(method(http_method))
            .and(path("/v1/playlists/mine/items"))
            .respond_with(
                ResponseTemplate::new(edits_status).set_body_json(json!({ "snapshot_id": "v2" })),
            )
            .mount(server)
            .await;
    }
}

/// Starts the backend with the playlist permissions and reads "Favorites".
async fn start_with_playlist(server: &MockServer) -> (Backend, tempfile::TempDir) {
    mount_queue_account(server).await;
    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, PLAYLIST_SCOPES);
    let mut backend = Backend::start(config(server, dir.path()));
    backend.send(Command::LoadPlaylistTracks {
        playlist: my_playlist(),
    });
    let Event::PlaylistTracks { tracks, edits, .. } = backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        names(&tracks),
        ["Bicycle Race", "Killer Queen", "Bicycle Race"]
    );
    assert_eq!(edits, 0);
    (backend, dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reading_a_playlist_tells_its_current_track_count() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 200).await;
    mount_queue_account(&server).await;
    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, PLAYLIST_SCOPES);
    let mut backend = Backend::start(config(&server, dir.path()));
    // The library listed it with fewer tracks: a different app added some.
    let mut outdated = my_playlist();
    outdated.total = 2;
    backend.send(Command::LoadPlaylistTracks { playlist: outdated });
    let Event::PlaylistTracks { total, .. } = backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(total, 4, "every entry, also the one that can't be shown");
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_playlist_is_read_to_tell_whether_it_has_a_track() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 200).await;
    mount_queue_account(&server).await;
    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, PLAYLIST_SCOPES);
    let mut backend = Backend::start(config(&server, dir.path()));
    let killer_queen = melodia::model::Track {
        uri: "spotify:track:c".into(),
        name: "Killer Queen".into(),
        ..innuendo()
    };
    for (track, expected) in [(killer_queen, true), (innuendo(), false)] {
        backend.send(Command::FindInPlaylist {
            playlist: my_playlist(),
            index: Some(1),
            track: track.clone(),
        });
        let Event::FoundInPlaylist {
            playlist,
            index,
            found,
            ..
        } = backend
            .wait_for(|e| matches!(e, Event::FoundInPlaylist { track: t, .. } if *t == track))
            .await
        else {
            unreachable!()
        };
        assert_eq!((playlist, index, found), (my_playlist(), Some(1), expected));
    }
    // One read: the second answer comes from the data of the first read.
    let reads = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path() == "/v1/playlists/mine/items")
        .count();
    assert_eq!(reads, 1);
    // The backend added nothing.
    assert!(playlist_changes(&server, "POST").await.is_empty());
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_album_dropped_on_a_playlist_tells_which_songs_it_has_and_is_added_at_once() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 200).await;
    mount_queue_account(&server).await;
    mount_get(
        &server,
        "/v1/albums/sheer",
        json!({
            "id": "sheer", "name": "Sheer Heart Attack", "release_date": "1974-11-08",
            "tracks": { "items": [track("c", "Killer Queen"), track("h", "Now I'm Here")], "total": 2 }
        }),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, PLAYLIST_SCOPES);
    let mut backend = Backend::start(config(&server, dir.path()));

    backend.send(Command::FindAlbumInPlaylist {
        playlist: my_playlist(),
        id: "sheer".into(),
        name: "Sheer Heart Attack".into(),
    });
    let Event::FoundAlbumInPlaylist {
        playlist,
        name,
        tracks,
        found,
    } = backend
        .wait_for(|e| matches!(e, Event::FoundAlbumInPlaylist { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(playlist, my_playlist());
    assert_eq!(name, "Sheer Heart Attack");
    assert_eq!(names(&tracks), ["Killer Queen", "Now I'm Here"]);
    assert_eq!(found, ["spotify:track:c"]);
    assert!(
        playlist_changes(&server, "POST").await.is_empty(),
        "nothing added yet"
    );

    // All of them, in one request.
    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Append { tracks },
    });
    let request = wait_for_request(&server, "POST", "/v1/playlists/mine/items").await;
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body,
        json!({ "uris": ["spotify:track:c", "spotify:track:h"] })
    );
    backend.stop().await;
}

/// The bodies of the requests with `http_method` to the tracks of "Favorites".
async fn playlist_changes(server: &MockServer, http_method: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == http_method && r.url.path() == "/v1/playlists/mine/items")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playlist_tracks_are_reordered_and_removed_by_position() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 200).await;
    let (mut backend, _dir) = start_with_playlist(&server).await;

    // The second Bicycle Race goes first: positions count the hidden entry.
    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Move {
            from: 2,
            to: 0,
            uri: "spotify:track:b".into(),
        },
    });
    let request = wait_for_request(&server, "PUT", "/v1/playlists/mine/items").await;
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body,
        json!({ "range_start": 3, "insert_before": 0, "range_length": 1, "snapshot_id": "v1" })
    );

    // To remove the first copy, the backend removes all copies (the only method that Spotify
    // allows). Then it puts the other copy back at its position.
    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Remove {
            index: 0,
            uri: "spotify:track:b".into(),
        },
    });
    let request = wait_for_request(&server, "POST", "/v1/playlists/mine/items").await;
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body, json!({ "uris": ["spotify:track:b"], "position": 0 }));
    assert_eq!(
        playlist_changes(&server, "DELETE").await,
        [json!({ "items": [{ "uri": "spotify:track:b" }] })]
    );

    // The lists read now include the two edits.
    backend.send(Command::LoadPlaylistTracks {
        playlist: my_playlist(),
    });
    backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { edits: 2, .. }))
        .await;
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_playlist_opened_again_is_read_again_only_after_an_edit() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 200).await;
    let (mut backend, _dir) = start_with_playlist(&server).await;

    backend.send(Command::LoadPlaylistTracks {
        playlist: my_playlist(),
    });
    let Event::PlaylistTracks { tracks, edits, .. } = backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(names(&tracks).len(), 3);
    assert_eq!(edits, 0);
    assert_eq!(gets(&server, "/v1/playlists/mine/items").await, 1);

    // Listed with a different count: a different app changed it, so the backend reads it again.
    let mut changed = my_playlist();
    changed.total = 5;
    backend.send(Command::LoadPlaylistTracks { playlist: changed });
    backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { .. }))
        .await;
    assert_eq!(gets(&server, "/v1/playlists/mine/items").await, 2);

    // Listed with the version of the read ("v1"): the backend uses the read again.
    let version = |snapshot: &str| Playlist {
        snapshot: Some(snapshot.into()),
        ..my_playlist()
    };
    backend.send(Command::LoadPlaylistTracks {
        playlist: version("v1"),
    });
    backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { .. }))
        .await;
    assert_eq!(gets(&server, "/v1/playlists/mine/items").await, 2);
    // Listed with a different version (new order from a different app, same count): the backend
    // reads it again.
    backend.send(Command::LoadPlaylistTracks {
        playlist: version("v9"),
    });
    backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { .. }))
        .await;
    assert_eq!(gets(&server, "/v1/playlists/mine/items").await, 3);

    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Move {
            from: 1,
            to: 0,
            uri: "spotify:track:c".into(),
        },
    });
    backend.send(Command::LoadPlaylistTracks {
        playlist: my_playlist(),
    });
    backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { edits: 1, .. }))
        .await;
    assert_eq!(gets(&server, "/v1/playlists/mine/items").await, 4);
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_playlist_edit_is_undone() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 500).await;
    let (mut backend, _dir) = start_with_playlist(&server).await;

    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Move {
            from: 0,
            to: 1,
            uri: "spotify:track:b".into(),
        },
    });
    backend
        .wait_for(|e| matches!(e, Event::Toast(Toast { .. })))
        .await;
    // The backend reads Spotify's tracks again, so the UI shows them as they are.
    let Event::PlaylistTracks { tracks, edits, .. } = backend
        .wait_for(|e| matches!(e, Event::PlaylistTracks { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(edits, 1);
    assert_eq!(
        names(&tracks),
        ["Bicycle Race", "Killer Queen", "Bicycle Race"]
    );
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_track_dropped_between_two_goes_at_its_position() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 200).await;
    let (backend, _dir) = start_with_playlist(&server).await;

    // Before Killer Queen, the second track shown: the position counts the hidden entry before
    // it.
    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Insert {
            index: Some(1),
            track: innuendo(),
        },
    });
    let request = wait_for_request(&server, "POST", "/v1/playlists/mine/items").await;
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body, json!({ "uris": ["spotify:track:e"], "position": 2 }));
    backend.stop().await;
}

/// `GET /me/playlists` with "Favorites" that has 4 tracks for the first `stale` reads, and 3
/// after them.
struct LaggingCount {
    reads: std::sync::atomic::AtomicUsize,
    stale: usize,
}

impl Respond for LaggingCount {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        let total = if read < self.stale { 4 } else { 3 };
        ResponseTemplate::new(200).set_body_json(json!({
            "total": 1,
            "items": [{
                "id": "mine", "name": "Favorites", "collaborative": false,
                "owner": { "id": "alice", "display_name": "Alice" },
                "images": [], "items": { "total": total }
            }]
        }))
    }
}

async fn favoritas_total(backend: &mut Backend) -> u32 {
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = backend
        .wait_for(|e| matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_))))
        .await
    else {
        unreachable!()
    };
    playlists.iter().find(|p| p.id == "mine").unwrap().total
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removal_is_not_undone_by_an_old_count() {
    let server = MockServer::start().await;
    mount_my_playlist(&server, 200).await;
    // Spotify still shows the old count immediately after the removal.
    Mock::given(method("GET"))
        .and(path("/v1/me/playlists"))
        .respond_with(LaggingCount {
            reads: Default::default(),
            stale: 2,
        })
        .mount(&server)
        .await;
    let (mut backend, _dir) = start_with_playlist(&server).await;
    assert_eq!(favoritas_total(&mut backend).await, 4);

    backend.send(Command::EditPlaylist {
        playlist: my_playlist(),
        edit: PlaylistEdit::Remove {
            index: 1,
            uri: "spotify:track:c".into(),
        },
    });
    // The list sent after the removal already includes it in the count.
    assert_eq!(favoritas_total(&mut backend).await, 3);
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_track_added_after_the_last_one_is_queued_while_spotify_has_more() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let account = Account {
        progress_ms: 236_000, // 4 s left
        ..Account::default()
    };
    let (mut backend, page, _dir) = start_with_queue(&server, &account).await;
    empty_the_upcoming_tracks(&mut backend).await;

    backend.send(Command::AddToQueue { track: innuendo() });
    let queued = wait_for_request(&server, "POST", "/v1/me/player/queue").await;
    assert_eq!(queued.url.query(), Some("uri=spotify%3Atrack%3Ae"));

    page.post(&here("e", "Innuendo", true, 1000));
    backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.name == "Innuendo")
                && q.next.is_empty())
        })
        .await;
    tokio::time::sleep(after_follow_ups()).await;
    assert!(
        !page.controls.lock().unwrap().contains(&PageControl::Pause),
        "Innuendo is not paused"
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_track_added_at_the_end_of_spotifys_list_starts_when_spotify_stops() {
    let server = MockServer::start().await;
    // Spotify's list ends with "Under Pressure": nothing comes after it.
    Mock::given(method("GET"))
        .and(path("/v1/me/player/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "queue": [] })))
        .with_priority(1)
        .mount(&server)
        .await;
    mount_queue_account(&server).await;
    let account = Account {
        progress_ms: 236_000, // 4 s left
        ..Account::default()
    };
    let (mut backend, page, _dir) = start_here(&server, &account).await;
    wait_for_request(&server, "GET", "/v1/me/player/queue").await;

    backend.send(Command::AddToQueue { track: innuendo() });
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if names(&q.next) == ["Innuendo"]))
        .await;

    // At the end of its list, Spotify stops on the same track.
    page.post(&here("a", "Under Pressure", false, 0));
    let play = wait_for_request(&server, "PUT", "/v1/me/player/play").await;
    let body: Value = serde_json::from_slice(&play.body).unwrap();
    assert_eq!(body["uris"], json!(["spotify:track:e"]));
    backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.name == "Innuendo")
                && names(&q.previous).last() == Some(&"Under Pressure"))
        })
        .await;
    let commands = player_commands(&server).await;
    assert!(
        !commands
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/queue" || path == "/v1/me/player/pause"),
        "Spotify would not play a queued track after its list: {commands:?}"
    );
    assert!(page.controls.lock().unwrap().is_empty());

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_album_is_loaded_to_show_it() {
    let server = MockServer::start().await;
    mount_playing_account(&server).await;
    mount_get(
        &server,
        "/v1/albums/opera",
        json!({
            "id": "opera",
            "name": "A Night at the Opera",
            "release_date": "1975-11-21",
            "images": [],
            "tracks": { "items": [track("b", "Bohemian Rhapsody")], "total": 1 }
        }),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/v1/albums/gone"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(
                json!({ "error": { "status": 404, "message": "Resource not found" } }),
            ),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let mut backend = Backend::start(config(&server, dir.path()));
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedIn { .. })))
        .await;

    backend.send(Command::LoadAlbum { id: "opera".into() });
    let Event::Album(album) = backend.wait_for(|e| matches!(e, Event::Album(_))).await else {
        unreachable!()
    };
    assert_eq!(album.name, "A Night at the Opera");
    assert_eq!(album.year, Some(1975));
    assert_eq!(names(&album.tracks), ["Bohemian Rhapsody"]);
    assert_eq!(album.tracks[0].album, "A Night at the Opera");

    // The album's panel shows a failure, and the user stays logged in.
    backend.send(Command::LoadAlbum { id: "gone".into() });
    backend
        .wait_for(|e| matches!(e, Event::AlbumFailed { id, .. } if id == "gone"))
        .await;
    backend.stop().await;
}

/// The number of GET requests for `url_path` that Spotify received.
async fn gets(server: &MockServer, url_path: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path() == url_path)
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn searches_albums_and_artists_read_lately_are_not_asked_for_again() {
    let server = MockServer::start().await;
    mount_playing_account(&server).await;
    mount_get(
        &server,
        "/v1/search",
        json!({ "tracks": { "items": [track("s", "Somebody to Love")], "total": 1 } }),
    )
    .await;
    mount_get(
        &server,
        "/v1/albums/opera",
        json!({
            "id": "opera", "name": "A Night at the Opera", "release_date": "1975-11-21",
            "tracks": { "items": [track("b", "Bohemian Rhapsody")], "total": 1 }
        }),
    )
    .await;
    mount_get(
        &server,
        "/v1/artists/queen",
        json!({ "id": "queen", "name": "Queen" }),
    )
    .await;
    mount_get(
        &server,
        "/v1/artists/queen/albums",
        json!({ "total": 0, "items": [] }),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let mut backend = Backend::start(config(&server, dir.path()));
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedIn { .. })))
        .await;

    for query in ["queen", "Queen "] {
        backend.send(Command::Search {
            query: query.into(),
            offset: 0,
        });
        // The answer has the text as the user typed it.
        backend
            .wait_for(|e| matches!(e, Event::SearchResults(page) if page.query == query && page.tracks.len() == 1))
            .await;
    }
    for _ in 0..2 {
        backend.send(Command::LoadAlbum { id: "opera".into() });
        backend
            .wait_for(|e| matches!(e, Event::Album(album) if album.tracks.len() == 1))
            .await;
        backend.send(Command::LoadArtist { id: "queen".into() });
        backend
            .wait_for(|e| matches!(e, Event::Artist(artist) if artist.name == "Queen"))
            .await;
    }
    assert_eq!(gets(&server, "/v1/search").await, 1);
    assert_eq!(gets(&server, "/v1/albums/opera").await, 1);
    assert_eq!(gets(&server, "/v1/artists/queen").await, 1);

    // The logout forgets them.
    backend.send(Command::Logout);
    backend
        .wait_for(|e| matches!(e, Event::Auth(AuthStatus::LoggedOut { .. })))
        .await;
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_read_before_closing_are_reused_after_opening_again() {
    let server = MockServer::start().await;
    mount_playing_account(&server).await;
    mount_get(
        &server,
        "/v1/search",
        json!({ "tracks": { "items": [track("s", "Somebody to Love")], "total": 1 } }),
    )
    .await;
    mount_get(
        &server,
        "/v1/albums/opera",
        json!({
            "id": "opera", "name": "A Night at the Opera", "release_date": "1975-11-21",
            "tracks": { "items": [track("b", "Bohemian Rhapsody")], "total": 1 }
        }),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);

    let open = |server: &MockServer| Backend::start(config(server, dir.path()));
    let read_both = async |backend: &mut Backend| {
        backend.send(Command::Search {
            query: "queen".into(),
            offset: 0,
        });
        backend
            .wait_for(|e| matches!(e, Event::SearchResults(page) if page.tracks.len() == 1))
            .await;
        backend.send(Command::LoadAlbum { id: "opera".into() });
        backend
            .wait_for(|e| matches!(e, Event::Album(album) if album.tracks.len() == 1))
            .await;
    };
    let mut backend = open(&server);
    backend.wait_for(|e| matches!(e, Event::Profile(_))).await;
    read_both(&mut backend).await;
    backend.stop().await;
    assert!(dir.path().join("cache.json").exists());

    let mut backend = open(&server);
    // The backend uses the saved answers when it knows the account.
    backend.wait_for(|e| matches!(e, Event::Profile(_))).await;
    read_both(&mut backend).await;
    assert_eq!(gets(&server, "/v1/search").await, 1);
    assert_eq!(gets(&server, "/v1/albums/opera").await, 1);
    backend.stop().await;

    // A different account does not use them.
    server.reset().await;
    mount_playing_account(&server).await;
    Mock::given(method("GET"))
        .and(path("/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "pablo" })))
        .with_priority(1)
        .mount(&server)
        .await;
    mount_get(
        &server,
        "/v1/search",
        json!({ "tracks": { "items": [track("s", "Somebody to Love")], "total": 1 } }),
    )
    .await;
    let mut backend = open(&server);
    backend.wait_for(|e| matches!(e, Event::Profile(_))).await;
    backend.send(Command::Search {
        query: "queen".into(),
        offset: 0,
    });
    backend
        .wait_for(|e| matches!(e, Event::SearchResults(page) if page.tracks.len() == 1))
        .await;
    assert_eq!(gets(&server, "/v1/search").await, 1);
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_artist_arrives_with_their_releases_which_can_be_queued_whole() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, _page, _dir) = start_with_queue(&server, &Account::default()).await;
    mount_get(
        &server,
        "/v1/artists/queen",
        json!({ "id": "queen", "name": "Queen" }),
    )
    .await;
    let release = |id: &str, kind: &str, date: &str| json!({ "id": id, "name": id, "album_type": kind, "release_date": date, "total_tracks": 2 });
    mount_get(
        &server,
        "/v1/artists/queen/albums",
        json!({
            "total": 3,
            "items": [
                release("innuendo", "album", "1991-02-04"),
                release("works", "album", "1984"),
                release("headlong", "single", "1991-05-13")
            ]
        }),
    )
    .await;
    mount_get(
        &server,
        "/v1/albums/innuendo",
        json!({
            "id": "innuendo", "name": "Innuendo", "release_date": "1991-02-04",
            "tracks": { "items": [track("e", "Innuendo"), track("f", "Mustapha")], "total": 2 }
        }),
    )
    .await;

    backend.send(Command::LoadArtist { id: "queen".into() });
    let Event::Artist(artist) = backend.wait_for(|e| matches!(e, Event::Artist(_))).await else {
        unreachable!()
    };
    assert_eq!(artist.name, "Queen");
    let ids: Vec<_> = artist.releases.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["headlong", "innuendo", "works"]);
    // The backend reads their songs only when necessary.
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| r.url.path().starts_with("/v1/albums/"))
    );

    // Dropped on the queue, after the first upcoming track: all its songs go there, and the music
    // does not stop.
    backend.send(Command::QueueAlbum {
        id: "innuendo".into(),
        name: "Innuendo".into(),
        at: QueueAt::Next(1),
    });
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 5))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        names(&queue.next),
        [
            "Bicycle Race",
            "Innuendo",
            "Mustapha",
            "Killer Queen",
            "Radio Ga Ga"
        ]
    );
    assert!(player_commands(&server).await.is_empty());
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_album_can_be_played_now_or_queued_at_the_end_from_its_menu() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, _page, _dir) = start_with_queue(&server, &Account::default()).await;
    mount_get(
        &server,
        "/v1/albums/innuendo",
        json!({
            "id": "innuendo", "name": "Innuendo", "release_date": "1991-02-04",
            "tracks": { "items": [track("e", "Innuendo"), track("f", "Mustapha")], "total": 2 }
        }),
    )
    .await;
    let album = |at| Command::QueueAlbum {
        id: "innuendo".into(),
        name: "Innuendo".into(),
        at,
    };

    // «Play last»: after the upcoming tracks.
    backend.send(album(QueueAt::End));
    let Event::Queue(queue) = backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 5))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        names(&queue.next),
        [
            "Bicycle Race",
            "Killer Queen",
            "Radio Ga Ga",
            "Innuendo",
            "Mustapha"
        ]
    );

    // «Play»: its first song plays now, and the other songs go first, before the tracks that were
    // there.
    backend.send(album(QueueAt::Now));
    let Event::Queue(queue) = backend
        .wait_for(|e| {
            matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.name == "Innuendo"))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        names(&queue.next),
        [
            "Mustapha",
            "Bicycle Race",
            "Killer Queen",
            "Radio Ga Ga",
            "Innuendo",
            "Mustapha"
        ]
    );
    assert_eq!(names(&queue.previous).last(), Some(&"Under Pressure"));
    backend.stop().await;
}

const LIBRARY_SCOPES: &str = "user-library-read user-library-modify";

/// Starts the backend with the «Liked Songs» permissions. Spotify tells that Under Pressure is
/// liked and Innuendo is not, and answers changes with `changes_status`.
async fn start_with_library(
    server: &MockServer,
    changes_status: u16,
) -> (Backend, tempfile::TempDir) {
    mount_queue_account(server).await;
    Mock::given(method("GET"))
        .and(path("/v1/me/library/contains"))
        .and(query_param("uris", "spotify:track:a,spotify:track:e"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([true, false])))
        .mount(server)
        .await;
    for http_method in ["PUT", "DELETE"] {
        Mock::given(method(http_method))
            .and(path("/v1/me/library"))
            .respond_with(ResponseTemplate::new(changes_status))
            .mount(server)
            .await;
    }
    let dir = tempfile::tempdir().unwrap();
    save_session_with_scope(dir.path(), 3600, LIBRARY_SCOPES);
    let backend = Backend::start(config(server, dir.path()));
    (backend, dir)
}

/// The `uris` of the requests with `http_method` to `/me/library`.
async fn library_changes(server: &MockServer, http_method: &str) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == http_method && r.url.path() == "/v1/me/library")
        .flat_map(|r| {
            r.url
                .query_pairs()
                .filter(|(key, _)| key == "uris")
                .map(|(_, value)| value.into_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn liked_songs_are_checked_and_changed() {
    let server = MockServer::start().await;
    let (mut backend, _dir) = start_with_library(&server, 200).await;

    backend.send(Command::CheckLiked {
        uris: vec!["spotify:track:a".into(), "spotify:track:e".into()],
    });
    let Event::LikedStates(states) = backend
        .wait_for(|e| matches!(e, Event::LikedStates(_)))
        .await
    else {
        unreachable!()
    };
    assert_eq!(
        states,
        vec![
            ("spotify:track:a".to_owned(), true),
            ("spotify:track:e".to_owned(), false)
        ]
    );

    backend.send(Command::SetLiked {
        track: innuendo(),
        liked: true,
    });
    backend.send(Command::SetLiked {
        track: innuendo(),
        liked: false,
    });
    wait_for_request(&server, "DELETE", "/v1/me/library").await;
    assert_eq!(library_changes(&server, "PUT").await, ["spotify:track:e"]);
    assert_eq!(
        library_changes(&server, "DELETE").await,
        ["spotify:track:e"]
    );
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_like_that_spotify_rejects_is_undone() {
    let server = MockServer::start().await;
    let (mut backend, _dir) = start_with_library(&server, 500).await;

    backend.send(Command::SetLiked {
        track: innuendo(),
        liked: true,
    });
    let Event::LikedChanged { uri, liked } = backend
        .wait_for(|e| matches!(e, Event::LikedChanged { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!((uri.as_str(), liked), ("spotify:track:e", false));
    let Event::Toast(Toast { message, .. }) =
        backend.wait_for(|e| matches!(e, Event::Toast(_))).await
    else {
        unreachable!()
    };
    assert!(
        message.starts_with("Could not save “Innuendo” to Liked Songs."),
        "{message}"
    );
    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn liked_songs_need_the_new_permissions() {
    let server = MockServer::start().await;
    mount_queue_account(&server).await;
    let (mut backend, _page, _dir) = start_with_queue(&server, &Account::default()).await;
    backend.send(Command::CheckLiked {
        uris: vec!["spotify:track:a".into()],
    });
    backend
        .wait_for(|e| matches!(e, Event::LikedNeedsReauth))
        .await;
    backend.stop().await;
}

// --- The player page reports the music ---------------------------------------

/// A player page that speaks when the test tells it to (with [`ScriptedPage::post`]). It becomes
/// the device "web-device" immediately when it gets a token.
#[derive(Clone, Default)]
struct ScriptedPage {
    open: Arc<std::sync::Mutex<Option<(u64, PlayerEvents)>>>,
    /// Each volume that the backend set on the page.
    volumes: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Each player command that the backend gave the page.
    controls: Arc<std::sync::Mutex<Vec<PageControl>>>,
}

impl ScriptedPage {
    fn host(&self) -> WebPlayerHost {
        let (host, mut requests) = WebPlayerHost::new(|| {});
        let page = self.clone();
        tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                match request {
                    HostRequest::Start {
                        generation, events, ..
                    } => {
                        *page.open.lock().unwrap() = Some((generation, events));
                        page.post(r#"{"type":"need_token"}"#);
                    }
                    HostRequest::Token(_) => {
                        page.post(r#"{"type":"ready","device_id":"web-device"}"#);
                    }
                    HostRequest::Volume(percent) => page.volumes.lock().unwrap().push(percent),
                    HostRequest::Control(control) => page.controls.lock().unwrap().push(control),
                    HostRequest::Stop => *page.open.lock().unwrap() = None,
                }
            }
        });
        host
    }

    fn post(&self, json: &str) {
        if let Some((generation, events)) = &*self.open.lock().unwrap() {
            let _ = events.send((*generation, page(json)));
        }
    }

    /// Waits until the backend gives the page `control`.
    async fn wait_for_control(&self, control: PageControl) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !self.controls.lock().unwrap().contains(&control) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the page never got {control:?}: {:?}",
                self.controls.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// Spotify mocks for "Under Pressure", which plays in this app from `progress_ms` when the test
/// starts, with `queue` in Spotify's queue. When the app opens, Spotify shows it paused at its
/// last position.
async fn mount_playing_here(server: &MockServer, progress_ms: u64, queue: Value) {
    mount_get(server, "/v1/me", json!({ "id": "alice" })).await;
    mount_get(server, "/v1/me/player/devices", json!({ "devices": [] })).await;
    mount_get(server, "/v1/me/player/queue", json!({ "queue": queue })).await;
    mount_get(
        server,
        "/v1/me/player",
        json!({
            "device": { "id": "web-device", "name": "Melodia", "type": "Computer", "is_active": true },
            "is_playing": false,
            "progress_ms": progress_ms,
            "item": track("a", "Under Pressure")
        }),
    )
    .await;
    for http_method in ["PUT", "POST"] {
        Mock::given(method(http_method))
            .respond_with(ResponseTemplate::new(204))
            .mount(server)
            .await;
    }
}

/// Starts the backend with `page`, which takes the music when the app opens. Then the page tells
/// that "Under Pressure" plays here from `progress_ms`.
async fn start_playing_here(
    server: &MockServer,
    page: &ScriptedPage,
    progress_ms: u64,
) -> (Backend, tempfile::TempDir) {
    let (mut backend, dir) = start_with_host(server, page.host(), None);
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;
    wait_for_request(server, "PUT", "/v1/me/player").await;
    page.post(&here("a", "Under Pressure", true, progress_ms));
    backend
        .wait_for(|e| {
            matches!(e, Event::Playback(p) if p.is_playing && p.device.as_ref().is_some_and(|d| d.id == "web-device"))
        })
        .await;
    (backend, dir)
}

/// The number of times that the backend asked Spotify what plays.
async fn state_requests(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path() == "/v1/me/player")
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn music_playing_here_is_followed_from_the_page_without_asking_spotify() {
    let server = MockServer::start().await;
    mount_playing_here(&server, 1000, json!([track("b", "Bohemian Rhapsody")])).await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_playing_here(&server, &page, 1000).await;
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if names(&q.next) == ["Bohemian Rhapsody"]))
        .await;
    let asked = state_requests(&server).await;

    // Paused from the phone, as a remote control: the page tells it.
    page.post(r#"{"type":"state","paused":true,"position":5000,"track_uri":"spotify:track:a"}"#);
    backend
        .wait_for(|e| matches!(e, Event::Playback(p) if !p.is_playing && p.progress_ms == 5000))
        .await;
    // Resumed here, by the page itself, which confirms it.
    backend.send(Command::Resume);
    page.wait_for_control(PageControl::Resume).await;
    page.post(r#"{"type":"state","paused":false,"position":5000,"track_uri":"spotify:track:a"}"#);
    // The next track, which Melodia knows from Spotify's queue.
    page.post(r#"{"type":"state","paused":false,"position":0,"track_uri":"spotify:track:b"}"#);
    let Event::Playback(playing) = backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.track_uri() == Some("spotify:track:b")))
        .await
    else {
        unreachable!()
    };
    assert!(playing.is_playing);
    assert_eq!(
        playing.device.map(|d| d.name),
        Some("Melodia".to_owned()),
        "the device is kept"
    );
    // Longer than the time for a command to complete: the backend never asked Spotify.
    tokio::time::sleep(timings().expectation_ttl * 6 / 5).await;
    assert_eq!(state_requests(&server).await, asked);
    assert!(
        !player_commands(&server)
            .await
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/play"),
        "Spotify was asked to resume"
    );

    // The music goes to a different device: only Spotify knows which.
    page.post(r#"{"type":"inactive"}"#);
    wait_for_state_request(&server, asked).await;

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_volume_of_music_playing_here_is_set_on_the_page_without_asking_spotify() {
    let server = MockServer::start().await;
    mount_playing_here(&server, 1000, json!([])).await;
    let page = ScriptedPage::default();
    let (mut backend, dir) = start_playing_here(&server, &page, 1000).await;

    for percent in [10, 20, 30] {
        backend.send(Command::SetVolume { percent });
    }
    backend
        .wait_for(|e| {
            matches!(e, Event::Playback(p) if p.device.as_ref().and_then(|d| d.volume_percent) == Some(30))
        })
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let saved = AppConfig::from_env(
            |key| (key == "MELODIA_CONFIG_DIR").then(|| dir.path().to_str().unwrap().into()),
            &[],
        )
        .volume;
        if saved == Some(30) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "saved {saved:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*page.volumes.lock().unwrap(), [10, 20, 30]);

    // A later report of the page keeps the volume set here.
    page.post(r#"{"type":"state","paused":true,"position":5000,"track_uri":"spotify:track:a"}"#);
    let Event::Playback(paused) = backend
        .wait_for(|e| matches!(e, Event::Playback(p) if !p.is_playing && p.progress_ms == 5000))
        .await
    else {
        unreachable!()
    };
    assert_eq!(paused.device.and_then(|d| d.volume_percent), Some(30));
    assert!(
        !player_commands(&server)
            .await
            .iter()
            .any(|(_, path, _)| path == "/v1/me/player/volume"),
        "Spotify was asked to change the volume"
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_track_the_page_reports_that_melodia_does_not_know_is_asked_to_spotify() {
    let server = MockServer::start().await;
    mount_playing_here(&server, 1000, json!([])).await;
    let page = ScriptedPage::default();
    let (backend, _dir) = start_playing_here(&server, &page, 1000).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let asked = state_requests(&server).await;

    // The page did not describe it: only Spotify can tell what it is.
    page.post(r#"{"type":"state","paused":false,"position":0,"track_uri":"spotify:track:z"}"#);
    wait_for_state_request(&server, asked).await;

    backend.stop().await;
}

/// Spotify mocks for "Under Pressure", which plays on Nuria's phone, with "Bohemian Rhapsody"
/// next.
async fn mount_playing_on_phone(server: &MockServer) {
    mount_get(server, "/v1/me", json!({ "id": "alice" })).await;
    mount_get(server, "/v1/me/player/devices", json!({ "devices": [] })).await;
    mount_get(
        server,
        "/v1/me/player/queue",
        json!({ "queue": [track("b", "Bohemian Rhapsody")] }),
    )
    .await;
    mount_get(
        server,
        "/v1/me/player",
        json!({
            "device": {
                "id": "phone", "name": "Nuria's phone", "type": "Smartphone",
                "is_active": true, "volume_percent": 60, "supports_volume": true
            },
            "is_playing": true,
            "shuffle_state": false,
            "progress_ms": 1000,
            "item": track("a", "Under Pressure")
        }),
    )
    .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
}

/// The number of times that the backend asked Spotify for the device list.
async fn device_list_requests(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path() == "/v1/me/player/devices")
        .count()
}

/// Waits until the backend asks Spotify what plays more than `before` times.
async fn wait_for_state_request(server: &MockServer, before: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while state_requests(server).await <= before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "Spotify was not asked"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn music_sent_here_is_followed_from_the_page_once_it_says_it_plays_here() {
    let server = MockServer::start().await;
    mount_playing_on_phone(&server).await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_web_player(&server, "streaming", Some(page.host())).await;
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await;
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;
    // The user chose this: the music comes here, and continues to play.
    let transfer = wait_for_request(&server, "PUT", "/v1/me/player").await;
    let body: Value = serde_json::from_slice(&transfer.body).unwrap();
    assert_eq!(body, json!({ "device_ids": ["web-device"], "play": true }));
    let asked = state_requests(&server).await;

    // Spotify still shows the phone: the page tells that the music is here.
    page.post(&here("a", "Under Pressure", true, 2000));
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(None)))
        .await;
    let Event::Playback(playing) = backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.progress_ms == 2000))
        .await
    else {
        unreachable!()
    };
    let device = playing.device.unwrap();
    assert_eq!(
        (device.id.as_str(), device.name.as_str()),
        ("web-device", "Melodia")
    );
    // The backend reads its queue again.
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if names(&q.next) == ["Bohemian Rhapsody"]))
        .await;
    // Longer than the wait before the backend asks Spotify, if the page tells nothing.
    tokio::time::sleep(timings().here_check_after * 3 / 2).await;
    assert_eq!(state_requests(&server).await, asked, "Spotify was asked");
    assert_eq!(
        device_list_requests(&server).await,
        0,
        "the device list was read with the picker closed"
    );

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn music_sent_here_from_another_device_is_seen_at_once_and_spotify_says_where_it_went() {
    let server = MockServer::start().await;
    mount_playing_on_phone(&server).await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_with_host(&server, page.host(), None);
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await;
    backend
        .web_status(|s| matches!(s, WebPlayerStatus::Ready { .. }))
        .await;
    tokio::time::sleep(after_follow_ups()).await;
    let asked = state_requests(&server).await;

    // Sent here from the phone: the page tells this, so Spotify does not have to.
    page.post(&here("a", "Under Pressure", true, 7000));
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(None)))
        .await;
    let Event::Playback(playing) = backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.progress_ms == 7000))
        .await
    else {
        unreachable!()
    };
    assert_eq!(playing.device.map(|d| d.id), Some("web-device".to_owned()));
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if names(&q.next) == ["Bohemian Rhapsody"]))
        .await;
    tokio::time::sleep(after_follow_ups()).await;
    assert_eq!(state_requests(&server).await, asked, "Spotify was asked");

    // Sent back to the phone: only Spotify knows where the music went.
    page.post(
        r#"{"type":"state","paused":false,"position":9000,"track_uri":"spotify:track:a","here":false}"#,
    );
    wait_for_state_request(&server, asked).await;
    let Event::PlayingElsewhere(Some(phone)) = backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await
    else {
        unreachable!()
    };
    assert_eq!(phone.name, "Nuria's phone");

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transfer_to_another_device_shows_it_at_once() {
    let server = MockServer::start().await;
    // Spotify still shows the phone as active for a moment after the transfer.
    mount_get(
        &server,
        "/v1/me/player/devices",
        json!({ "devices": [
            { "id": "phone", "name": "Nuria's phone", "type": "Smartphone", "is_active": true },
            { "id": "speaker", "name": "Kitchen", "type": "Speaker", "is_active": false }
        ] }),
    )
    .await;
    mount_playing_on_phone(&server).await;
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let mut backend = Backend::start(config(&server, dir.path()));
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await;
    backend.send(Command::RefreshDevices);
    backend
        .wait_for(|e| matches!(e, Event::Devices(d) if d.len() == 2))
        .await;

    let asked = state_requests(&server).await;
    backend.send(Command::TransferPlayback {
        device_id: "speaker".into(),
    });
    let Event::PlayingElsewhere(Some(speaker)) = backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(d)) if d.id == "speaker"))
        .await
    else {
        unreachable!()
    };
    assert_eq!(speaker.name, "Kitchen");
    let Event::Devices(devices) = backend
        .wait_for(|e| matches!(e, Event::Devices(d) if d.len() == 2))
        .await
    else {
        unreachable!()
    };
    let active: Vec<&str> = devices
        .iter()
        .filter(|d| d.is_active)
        .map(|d| d.id.as_str())
        .collect();
    assert_eq!(active, ["speaker"]);
    let transfer = wait_for_request(&server, "PUT", "/v1/me/player").await;
    let body: Value = serde_json::from_slice(&transfer.body).unwrap();
    assert_eq!(body, json!({ "device_ids": ["speaker"], "play": true }));

    // The list that the backend read again still shows the phone: it is old.
    backend.send(Command::RefreshDevices);
    let Event::Devices(devices) = backend
        .wait_for(|e| matches!(e, Event::Devices(d) if d.len() == 2))
        .await
    else {
        unreachable!()
    };
    assert!(
        devices.iter().all(|d| d.is_active == (d.id == "speaker")),
        "{devices:?}"
    );
    assert_eq!(state_requests(&server).await, asked, "Spotify was asked");

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_device_list_is_read_after_a_transfer_only_while_the_picker_shows_it() {
    let server = MockServer::start().await;
    mount_playing_on_phone(&server).await;
    let dir = tempfile::tempdir().unwrap();
    save_session(dir.path(), 3600);
    let mut backend = Backend::start(config(&server, dir.path()));
    backend
        .wait_for(|e| matches!(e, Event::PlayingElsewhere(Some(_))))
        .await;

    backend.send(Command::TransferPlayback {
        device_id: "speaker".into(),
    });
    wait_for_request(&server, "PUT", "/v1/me/player").await;
    tokio::time::sleep(after_follow_ups()).await;
    assert_eq!(device_list_requests(&server).await, 0);

    // The picker opens and asks for the list, then the music moves.
    backend.send(Command::RefreshDevices);
    backend.send(Command::TransferPlayback {
        device_id: "phone".into(),
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while device_list_requests(&server).await < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the list was not read again after the transfer"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn player_commands_for_music_playing_here_are_done_by_the_page() {
    let server = MockServer::start().await;
    mount_playing_here(&server, 1000, json!([])).await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_playing_here(&server, &page, 1000).await;

    backend.send(Command::Seek {
        position_ms: 60_000,
    });
    page.wait_for_control(PageControl::Seek(60_000)).await;
    page.post(r#"{"type":"state","paused":false,"position":60000,"track_uri":"spotify:track:a"}"#);
    backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.progress_ms == 60_000))
        .await;
    backend.send(Command::Pause);
    page.wait_for_control(PageControl::Pause).await;
    tokio::time::sleep(after_follow_ups()).await;
    assert!(
        player_commands(&server)
            .await
            .iter()
            .all(|(_, path, _)| path == "/v1/me/player"),
        "only the transfer reaches Spotify: {:?}",
        player_commands(&server).await
    );

    // The SDK could not pause: the Web API does it.
    page.post(r#"{"type":"control_failed","action":"pause","position":null,"message":"no"}"#);
    wait_for_request(&server, "PUT", "/v1/me/player/pause").await;

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_track_playing_here_is_shown_as_the_page_describes_it() {
    let server = MockServer::start().await;
    mount_playing_here(&server, 1000, json!([])).await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_playing_here(&server, &page, 1000).await;
    let asked = state_requests(&server).await;

    page.post(
        r#"{"type":"state","paused":false,"position":0,"track_uri":"spotify:track:z",
            "track":{"type":"track","uri":"spotify:track:z","name":"Innuendo","duration_ms":391000,
                "artists":[{"name":"Queen","id":"queen"}],
                "album":{"name":"Innuendo","id":"innuendo","images":[{"url":"cover","width":300}]}}}"#,
    );
    let Event::Playback(playing) = backend
        .wait_for(|e| matches!(e, Event::Playback(p) if p.track_uri() == Some("spotify:track:z")))
        .await
    else {
        unreachable!()
    };
    let track = playing.track.unwrap();
    assert_eq!(track.name, "Innuendo");
    assert_eq!(track.artist_ids, [Some("queen".to_owned())]);
    assert_eq!(track.album_id.as_deref(), Some("innuendo"));
    assert_eq!(track.cover_small.as_deref(), Some("cover"));
    tokio::time::sleep(after_follow_ups()).await;
    assert_eq!(state_requests(&server).await, asked, "Spotify was asked");

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spotifys_queue_is_followed_from_the_tracks_the_page_says_come_next() {
    let server = MockServer::start().await;
    let queue = json!([
        track("b", "Bicycle Race"),
        track("c", "Killer Queen"),
        track("d", "Radio Ga Ga")
    ]);
    mount_playing_here(&server, 1000, queue).await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_playing_here(&server, &page, 1000).await;
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 3))
        .await;
    let read = gets(&server, "/v1/me/player/queue").await;

    // The next track starts, and the page tells what follows: the same as on the screen.
    page.post(
        r#"{"type":"state","paused":false,"position":0,"track_uri":"spotify:track:b",
            "next_uris":["spotify:track:c","spotify:track:d"]}"#,
    );
    backend
        .wait_for(
            |e| matches!(e, Event::Queue(q) if names(&q.next) == ["Killer Queen", "Radio Ga Ga"]),
        )
        .await;
    tokio::time::sleep(after_follow_ups()).await;
    assert_eq!(gets(&server, "/v1/me/player/queue").await, read);

    // A different track comes next, from a change on a different device: the backend reads the
    // queue.
    page.post(
        r#"{"type":"state","paused":true,"position":1000,"track_uri":"spotify:track:b",
            "next_uris":["spotify:track:y"]}"#,
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while gets(&server, "/v1/me/player/queue").await == read {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the queue was not read"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    backend.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_the_page_says_spotify_plays_next_decides_whether_to_queue_the_next_track() {
    let server = MockServer::start().await;
    let queue = json!([
        track("b", "Bicycle Race"),
        track("c", "Killer Queen"),
        track("d", "Radio Ga Ga")
    ]);
    // 10 s remain: the backend decides Spotify's next track now.
    mount_playing_here(&server, 230_000, queue).await;
    let page = ScriptedPage::default();
    let (mut backend, _dir) = start_playing_here(&server, &page, 230_000).await;
    backend
        .wait_for(|e| matches!(e, Event::Queue(q) if q.next.len() == 3))
        .await;
    // Spotify's queue tells Bicycle Race, but the page has newer data.
    page.post(
        r#"{"type":"state","paused":false,"position":231000,"track_uri":"spotify:track:a",
            "next_uris":["spotify:track:y"]}"#,
    );
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Bicycle Race stays next: it goes into Spotify's queue.
    backend.send(Command::EditQueue(QueueEdit::Move {
        from: 3,
        to: 2,
        uri: "spotify:track:d".into(),
    }));
    let queued = wait_for_request(&server, "POST", "/v1/me/player/queue").await;
    assert_eq!(queued.url.query(), Some("uri=spotify%3Atrack%3Ab"));

    backend.stop().await;
}
