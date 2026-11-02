//! The Spotify Web API client against a mock server.

use std::sync::Arc;
use std::time::Duration;

use melodia::auth::{Session, SessionStore, TokenManager, unix_now};
use melodia::spotify::{ApiError, PlayOffset, PlayRequest, SpotifyClient};
use serde_json::{Value, json};
use wiremock::matchers::{
    body_json, body_string_contains, header, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn session(access_token: &str, expires_in: i64) -> Session {
    Session {
        access_token: access_token.into(),
        refresh_token: "refresh-1".into(),
        expires_at: (unix_now() as i64 + expires_in) as u64,
        scope: String::new(),
    }
}

fn client(server: &MockServer, session: Session, store: Option<SessionStore>) -> SpotifyClient {
    let http = reqwest::Client::new();
    let tokens = TokenManager::new(http.clone(), &server.uri(), "client-id", session, store);
    SpotifyClient::new(http, format!("{}/v1", server.uri()), Arc::new(tokens))
}

fn track_json(id: &str, name: &str) -> Value {
    json!({
        "type": "track",
        "id": id,
        "uri": format!("spotify:track:{id}"),
        "name": name,
        "duration_ms": 215_000,
        "artists": [{ "name": "Queen" }, { "name": "David Bowie" }],
        "album": {
            "name": "Hot Space",
            "images": [
                { "url": "https://i.scdn.co/640", "width": 640, "height": 640 },
                { "url": "https://i.scdn.co/300", "width": 300, "height": 300 },
                { "url": "https://i.scdn.co/64", "width": 64, "height": 64 }
            ]
        }
    })
}

#[tokio::test]
async fn search_sends_the_expected_query_and_parses_tracks() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/search"))
        .and(query_param("q", "under pressure"))
        .and(query_param("type", "track"))
        .and(query_param("limit", "10"))
        .and(query_param("offset", "10"))
        .and(query_param("market", "from_token"))
        .and(header("authorization", "Bearer token-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tracks": {
                "items": [track_json("1", "Under Pressure"), null],
                "total": 42,
                "offset": 10,
                "limit": 10
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let page = client(&server, session("token-1", 3600), None)
        .search("under pressure", 10)
        .await
        .unwrap();
    assert_eq!(page.query, "under pressure");
    assert_eq!(page.offset, 10);
    assert_eq!(page.total, 42);
    assert!(page.has_more());
    let track = &page.tracks[0];
    assert_eq!(page.tracks.len(), 1, "null items are skipped");
    assert_eq!(track.name, "Under Pressure");
    assert_eq!(track.artist_line(), "Queen, David Bowie");
    assert_eq!(track.album, "Hot Space");
    assert_eq!(track.cover_large.as_deref(), Some("https://i.scdn.co/640"));
    assert_eq!(track.cover_small.as_deref(), Some("https://i.scdn.co/300"));
}

#[tokio::test]
async fn the_first_search_page_also_has_albums_and_artists() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/search"))
        .and(query_param("q", "queen"))
        .and(query_param("type", "track,album,artist"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tracks": { "items": [track_json("1", "Under Pressure")], "total": 1 },
            "albums": { "items": [
                {
                    "id": "opera",
                    "name": "A Night at the Opera",
                    "album_type": "album",
                    "release_date": "1975-11-21",
                    "total_tracks": 12,
                    "artists": [{ "id": "queen", "name": "Queen" }],
                    "images": [{ "url": "https://i.scdn.co/opera", "width": 300 }]
                },
                null
            ], "total": 1 },
            "artists": { "items": [
                {
                    "id": "queen",
                    "name": "Queen",
                    "images": [{ "url": "https://i.scdn.co/queen", "width": 300 }]
                },
                { "name": "Without an ID" }
            ], "total": 1 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let page = client(&server, session("token-1", 3600), None)
        .search("queen", 0)
        .await
        .unwrap();
    assert_eq!(page.tracks.len(), 1);
    assert_eq!(page.artists.len(), 1, "unreadable artists are skipped");
    assert_eq!(page.artists[0].name, "Queen");
    assert_eq!(
        page.artists[0].cover.as_deref(),
        Some("https://i.scdn.co/queen")
    );
    assert_eq!(page.albums.len(), 1, "unreadable albums are skipped");
    assert_eq!(page.albums[0].release.id, "opera");
    assert_eq!(page.albums[0].subtitle(), "Album · Queen");
}

#[tokio::test]
async fn no_content_playback_state_means_nothing_is_playing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let state = client(&server, session("token-1", 3600), None)
        .playback_state()
        .await
        .unwrap();
    assert!(state.is_none());
}

#[tokio::test]
async fn playback_state_includes_track_progress_and_device() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player"))
        .and(query_param("market", "from_token"))
        .and(query_param_is_missing("additional_types"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device": {
                "id": "dev1", "name": "Living Room", "type": "Speaker", "is_active": true,
                "volume_percent": 35, "supports_volume": true
            },
            "is_playing": true,
            "shuffle_state": true,
            "progress_ms": 61_000,
            "item": track_json("1", "Under Pressure"),
            "currently_playing_type": "track"
        })))
        .mount(&server)
        .await;

    let state = client(&server, session("token-1", 3600), None)
        .playback_state()
        .await
        .unwrap()
        .unwrap();
    assert!(state.is_playing);
    assert!(state.shuffle);
    assert_eq!(state.progress_ms, 61_000);
    assert_eq!(state.track.unwrap().name, "Under Pressure");
    let device = state.device.unwrap();
    assert_eq!(
        (device.id.as_str(), device.name.as_str()),
        ("dev1", "Living Room")
    );
    assert_eq!(device.volume_percent, Some(35));
}

#[tokio::test]
async fn queue_and_devices_are_parsed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currently_playing": track_json("1", "Under Pressure"),
            "queue": [
                track_json("2", "Radio Ga Ga"),
                { "type": "episode", "uri": "spotify:episode:9", "name": "Chapter 1", "duration_ms": 1000,
                  "images": [], "show": { "name": "Podcast", "images": [] } },
                { "type": "something-new", "uri": "x", "name": "?" }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player/devices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "devices": [
                { "id": "pc", "name": "PC", "type": "Computer", "is_active": false, "is_restricted": false },
                { "id": "tv", "name": "Tele", "type": "TV", "is_active": false, "is_restricted": true }
            ]
        })))
        .mount(&server)
        .await;

    let client = client(&server, session("token-1", 3600), None);
    let queue = client.queue().await.unwrap();
    assert_eq!(
        queue.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["Radio Ga Ga"],
        "episodes and unknown items are skipped"
    );
    let devices = client.devices().await.unwrap();
    assert_eq!(devices.len(), 1, "restricted devices cannot be controlled");
}

#[tokio::test]
async fn expired_token_is_refreshed_and_saved_before_the_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .and(body_string_contains("client_id=client-id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "token-2",
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "refresh-2",
            "scope": "user-read-playback-state"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/me"))
        .and(header("authorization", "Bearer token-2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": "alice", "display_name": "Alice" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("session.json"));
    let client = client(&server, session("token-1", -10), Some(store.clone()));
    let profile = client.current_user().await.unwrap();
    assert_eq!(profile.display_name, "Alice");

    let saved = store.load().expect("the refreshed session is persisted");
    assert_eq!(saved.access_token, "token-2");
    assert_eq!(saved.refresh_token, "refresh-2");
}

#[tokio::test]
async fn rejected_token_is_refreshed_once_and_the_request_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player/queue"))
        .and(header("authorization", "Bearer token-1"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "status": 401, "message": "The access token expired" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "token-2", "token_type": "Bearer", "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/me/player/queue"))
        .and(header("authorization", "Bearer token-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "queue": [] })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server, session("token-1", 3600), None);
    assert!(client.queue().await.unwrap().is_empty());
    // Without a new refresh token, the previous one stays valid.
    assert_eq!(client.tokens().session().await.refresh_token, "refresh-1");
}

#[tokio::test]
async fn revoked_refresh_token_means_the_session_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant", "error_description": "Refresh token revoked"
        })))
        .mount(&server)
        .await;
    let result = client(&server, session("token-1", -10), None).queue().await;
    assert_eq!(result, Err(ApiError::SessionExpired));
}

#[tokio::test]
async fn player_errors_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": { "status": 404, "message": "Player command failed: No active device found", "reason": "NO_ACTIVE_DEVICE" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/me/player/next"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": { "status": 403, "message": "Player command failed: Premium required", "reason": "PREMIUM_REQUIRED" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/pause"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .mount(&server)
        .await;

    let client = client(&server, session("token-1", 3600), None);
    assert_eq!(client.play(None, None).await, Err(ApiError::NoActiveDevice));
    assert_eq!(client.next().await, Err(ApiError::PremiumRequired));
    assert_eq!(
        client.pause().await,
        Err(ApiError::RateLimited(Duration::from_secs(7)))
    );
}

#[tokio::test]
async fn player_commands_send_the_expected_requests() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .and(query_param("device_id", "dev1"))
        .and(body_json(json!({
            "uris": ["spotify:track:1", "spotify:track:2"],
            "offset": { "position": 1 }
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/me/player/previous"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/seek"))
        .and(query_param("position_ms", "90000"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/play"))
        .and(body_json(json!({
            "uris": ["spotify:track:3", "spotify:track:4"],
            "offset": { "position": 0 },
            "position_ms": 61_000
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/volume"))
        .and(query_param("volume_percent", "100"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/shuffle"))
        .and(query_param("state", "false"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/me/player/queue"))
        .and(query_param("uri", "spotify:track:5"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player"))
        .and(body_json(json!({ "device_ids": ["dev2"], "play": true })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server, session("token-1", 3600), None);
    let request = PlayRequest {
        uris: vec!["spotify:track:1".into(), "spotify:track:2".into()],
        offset: PlayOffset { position: 1 },
        position_ms: None,
    };
    client.play(Some("dev1"), Some(&request)).await.unwrap();
    // The queue in a new order, and the current track resumes from its position.
    let request = PlayRequest {
        uris: vec!["spotify:track:3".into(), "spotify:track:4".into()],
        offset: PlayOffset { position: 0 },
        position_ms: Some(61_000),
    };
    client.play(None, Some(&request)).await.unwrap();
    client.previous().await.unwrap();
    client.seek(90_000).await.unwrap();
    client.set_volume(140).await.unwrap();
    client.set_shuffle(false).await.unwrap();
    client.add_to_queue("spotify:track:5").await.unwrap();
    client.transfer_playback("dev2", true).await.unwrap();
}

#[tokio::test]
async fn playlists_are_created_empty_and_private() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/me/playlists"))
        .and(body_json(json!({ "name": "Road Trip", "public": false })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "new-1",
            "name": "Road Trip",
            "owner": { "id": "alice", "display_name": "Alice" },
            "collaborative": false,
            "images": null,
            "items": { "total": 0 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let playlist = client(&server, session("token-1", 3600), None)
        .create_playlist("Road Trip")
        .await
        .unwrap();
    assert_eq!(playlist.id, "new-1");
    assert_eq!(playlist.name, "Road Trip");
    assert_eq!(playlist.owner, "Alice");
    assert_eq!(playlist.total, 0);
    assert!(playlist.editable);
}

#[tokio::test]
async fn playlists_are_renamed() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/playlists/pl-1"))
        .and(body_json(json!({ "name": "Road Trip" })))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    client(&server, session("token-1", 3600), None)
        .rename_playlist("pl-1", "Road Trip")
        .await
        .unwrap();
}

#[tokio::test]
async fn an_album_is_read_with_all_its_tracks_and_its_year() {
    let server = MockServer::start().await;
    let simple = |n: u32, name: &str| {
        json!({
            "type": "track",
            "uri": format!("spotify:track:{n}"),
            "name": name,
            "duration_ms": 200_000,
            "artists": [{ "name": "Queen" }]
        })
    };
    Mock::given(method("GET"))
        .and(path("/v1/albums/opera"))
        .and(query_param("market", "from_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "opera",
            "name": "A Night at the Opera",
            "release_date": "1975-11-21",
            "release_date_precision": "day",
            "images": [{ "url": "https://i.scdn.co/640", "width": 640, "height": 640 }],
            "tracks": {
                "items": [simple(1, "Death on Two Legs"), simple(2, "You're My Best Friend")],
                "total": 3,
                "next": "…"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/albums/opera/tracks"))
        .and(query_param("offset", "2"))
        .and(query_param("limit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [simple(3, "Bohemian Rhapsody")],
            "total": 3
        })))
        .expect(1)
        .mount(&server)
        .await;

    let album = client(&server, session("token-1", 3600), None)
        .album("opera")
        .await
        .unwrap();
    assert_eq!(album.name, "A Night at the Opera");
    assert_eq!(album.year, Some(1975));
    let names: Vec<_> = album.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Death on Two Legs",
            "You're My Best Friend",
            "Bohemian Rhapsody"
        ]
    );
    assert!(
        album
            .tracks
            .iter()
            .all(|t| t.album == "A Night at the Opera"
                && t.album_id.as_deref() == Some("opera")
                && t.cover_large.as_deref() == Some("https://i.scdn.co/640"))
    );
}

#[tokio::test]
async fn an_artist_is_read_with_their_albums_and_singles() {
    let server = MockServer::start().await;
    let release = |id: &str, kind: &str, date: &str| {
        json!({
            "id": id,
            "name": id.to_uppercase(),
            "album_type": kind,
            "release_date": date,
            "total_tracks": 10,
            "images": []
        })
    };
    Mock::given(method("GET"))
        .and(path("/v1/artists/queen"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "queen",
            "name": "Queen"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/artists/queen/albums"))
        .and(query_param("include_groups", "album,single"))
        .and(query_param("market", "from_token"))
        .and(query_param("limit", "10"))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                release("opera", "album", "1975-11-21"),
                release("innuendo", "album", "1991-02-04"),
            ],
            "total": 3
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/artists/queen/albums"))
        .and(query_param("offset", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [release("bohemian", "single", "1975-10-31")],
            "total": 3
        })))
        .expect(1)
        .mount(&server)
        .await;

    let artist = client(&server, session("token-1", 3600), None)
        .artist("queen")
        .await
        .unwrap();
    assert_eq!(artist.name, "Queen");
    let ids: Vec<_> = artist.releases.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["innuendo", "opera", "bohemian"]);
}

#[tokio::test]
async fn liked_songs_use_the_library_endpoints_with_uris() {
    let server = MockServer::start().await;
    let uris = "spotify:track:1,spotify:track:2";
    Mock::given(method("GET"))
        .and(path("/v1/me/library/contains"))
        .and(query_param("uris", uris))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([false, true])))
        .expect(1)
        .mount(&server)
        .await;
    for http_method in ["PUT", "DELETE"] {
        Mock::given(method(http_method))
            .and(path("/v1/me/library"))
            .and(query_param("uris", "spotify:track:1"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    let api = client(&server, session("token-1", 3600), None);
    let both = ["spotify:track:1".to_owned(), "spotify:track:2".to_owned()];
    assert_eq!(api.library_contains(&both).await.unwrap(), [false, true]);
    api.save_to_library(&both[..1]).await.unwrap();
    api.remove_from_library(&both[..1]).await.unwrap();
}

#[tokio::test]
async fn after_a_429_nothing_is_sent_until_the_wait_is_over() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/v1/me/player/pause"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/me/player/next"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;

    let client = client(&server, session("token-1", 3600), None);
    assert_eq!(
        client.pause().await,
        Err(ApiError::RateLimited(Duration::from_secs(30)))
    );
    // Other requests, also from clones, also wait: they fail, and the client does not send them.
    for api in [client.clone(), client] {
        match api.next().await {
            Err(ApiError::RateLimited(wait)) => {
                assert!(wait > Duration::from_secs(25) && wait <= Duration::from_secs(30));
            }
            other => panic!("expected to be rate limited, got {other:?}"),
        }
        assert!(api.pause().await.is_err());
    }
}
