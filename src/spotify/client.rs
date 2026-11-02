use std::sync::Arc;
use std::time::Instant;

use reqwest::{Method, StatusCode, header};
use serde::de::DeserializeOwned;
use serde_json::json;

use super::error::ApiError;
use super::models::{
    ApiAlbumTracks, ApiArtist, ApiArtistAlbums, ApiDevice, ApiDevices, ApiFullAlbum,
    ApiPlaybackState, ApiPlaylist, ApiPlaylistItems, ApiPlaylistPage, ApiQueue, ApiSnapshot,
    ApiUser, PlayRequest, parse_item, parse_items, parse_search,
};
use super::rate_limit::{RateLimit, parse_retry_after};
use crate::auth::TokenManager;
use crate::model::{
    Album, Artist, Device, PlaybackSnapshot, Playlist, SearchPage, Track, UserProfile,
};
use crate::util;

/// The results for each search request. Since February 2026, Spotify limits `limit` to 10 for
/// apps in development mode. The client gets more results with `offset`.
pub const SEARCH_PAGE_SIZE: u32 = 10;
/// The releases of an artist for each request. In development mode, Spotify refuses a larger
/// `limit` («Invalid limit»), as with search.
const ARTIST_ALBUMS_PAGE_SIZE: u32 = 10;
/// The playlists, or the tracks of a playlist or an album, for each request.
const PLAYLIST_PAGE_SIZE: u32 = 50;
/// The client cuts longer libraries here.
const MAX_PLAYLISTS: u32 = 500;
/// The maximum number of URIs that Spotify accepts in one request to `/me/library`.
pub const LIBRARY_BATCH: usize = 40;

/// The entries of a playlist, from [`SpotifyClient::playlist_entries`].
#[derive(Debug, Clone, Default)]
pub struct PlaylistEntries {
    pub entries: Vec<Option<Track>>,
    /// They go to the end of the playlist.
    pub complete: bool,
    /// The number of entries of the playlist (also the entries that the client did not read).
    pub total: u32,
}

/// Typed client for the Spotify Web API.
///
/// A clone is cheap. Each request gets a valid access token from the [`TokenManager`]. After a
/// `401`, the client refreshes the token one time and tries again. After a `429`, the client
/// sends nothing until the backoff ends ([`RateLimit`], which the clones share).
#[derive(Clone)]
pub struct SpotifyClient {
    http: reqwest::Client,
    api_base: String,
    tokens: Arc<TokenManager>,
    rate_limit: Arc<std::sync::Mutex<RateLimit>>,
}

impl SpotifyClient {
    pub fn new(
        http: reqwest::Client,
        api_base: impl Into<String>,
        tokens: Arc<TokenManager>,
    ) -> Self {
        Self {
            http,
            api_base: api_base.into().trim_end_matches('/').to_owned(),
            tokens,
            rate_limit: Arc::default(),
        }
    }

    pub fn tokens(&self) -> &Arc<TokenManager> {
        &self.tokens
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, ApiError> {
        let url = format!("{}{}", self.api_base, path);
        let mut refreshed = false;
        loop {
            if let Some(wait) = util::lock(&self.rate_limit).remaining(Instant::now()) {
                return Err(ApiError::RateLimited(wait));
            }
            let token = self.tokens.access_token().await?;
            let mut request = self.http.request(method.clone(), &url).bearer_auth(&token);
            if !query.is_empty() {
                request = request.query(query);
            }
            request = match &body {
                Some(json) => request.json(json),
                // Spotify answers 411 to a POST/PUT with no body and no length.
                None if method != Method::GET => request.header(header::CONTENT_LENGTH, "0"),
                None => request,
            };

            let request = request.build()?;
            // The client logs each Web API request with its full URL (with the query). The logger
            // adds the time.
            log::info!("spotify api: {} {}", request.method(), request.url());
            let response = self.http.execute(request).await?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED && !refreshed {
                self.tokens.invalidate(&token).await;
                refreshed = true;
                continue;
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after);
                let wait = util::lock(&self.rate_limit).on_limited(retry_after, Instant::now());
                log::warn!(
                    "spotify api: 429 (Retry-After: {retry_after:?}); no requests for {} s",
                    wait.as_secs()
                );
                return Err(ApiError::RateLimited(wait));
            }
            if status != StatusCode::UNAUTHORIZED {
                util::lock(&self.rate_limit).on_success();
            }
            if status.is_success() {
                return Ok(response);
            }
            let text = response.text().await.unwrap_or_default();
            return Err(ApiError::from_response(status.as_u16(), &text));
        }
    }

    /// A GET that returns `None` for `204 No Content` or an empty body.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<T>, ApiError> {
        let response = self.send(Method::GET, path, query, None).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let bytes = response.bytes().await?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    async fn command(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<(), ApiError> {
        self.send(method, path, query, body).await.map(drop)
    }

    /// A request that changes a playlist. Returns its new snapshot ID, if Spotify sends it.
    async fn playlist_change(
        &self,
        method: Method,
        id: &str,
        body: serde_json::Value,
    ) -> Result<Option<String>, ApiError> {
        let path = format!("/playlists/{id}/items");
        let response = self.send(method, &path, &[], Some(body)).await?;
        let bytes = response.bytes().await?;
        Ok(serde_json::from_slice::<ApiSnapshot>(&bytes)
            .ok()
            .and_then(|answer| answer.snapshot_id))
    }

    /// `GET /me`: the user that is logged in.
    async fn me(&self) -> Result<ApiUser, ApiError> {
        self.get_json("/me", &[])
            .await?
            .ok_or_else(|| ApiError::Decode("empty profile".into()))
    }

    pub async fn current_user(&self) -> Result<UserProfile, ApiError> {
        Ok(self.me().await?.into_profile())
    }

    /// A page of results. The first page (offset 0) also has albums and artists. The next pages
    /// only continue with the tracks, because the offset counts the tracks.
    pub async fn search(&self, query: &str, offset: u32) -> Result<SearchPage, ApiError> {
        let raw = self.search_raw(query, offset).await?;
        Ok(parse_search(query, offset, raw))
    }

    /// The answer of `GET /search` as Spotify sends it. The cache keeps the full answer, and
    /// [`parse_search`] reads it.
    pub async fn search_raw(
        &self,
        query: &str,
        offset: u32,
    ) -> Result<serde_json::Value, ApiError> {
        let types = if offset == 0 {
            "track,album,artist"
        } else {
            "track"
        };
        let params = [
            ("q", query.to_owned()),
            ("type", types.to_owned()),
            ("limit", SEARCH_PAGE_SIZE.to_string()),
            ("offset", offset.to_string()),
            ("market", "from_token".to_owned()),
        ];
        Ok(self
            .get_json("/search", &params)
            .await?
            .unwrap_or(serde_json::Value::Null))
    }

    /// The current playback, or `None` when no device is active.
    pub async fn playback_state(&self) -> Result<Option<PlaybackSnapshot>, ApiError> {
        let params = [("market", "from_token".to_owned())];
        let state: Option<ApiPlaybackState> = self.get_json("/me/player", &params).await?;
        let measured_at = Instant::now();
        Ok(state.map(|state| PlaybackSnapshot {
            track: state.item.and_then(parse_item),
            is_playing: state.is_playing,
            progress_ms: state.progress_ms.unwrap_or(0),
            measured_at,
            device: state.device.and_then(ApiDevice::into_device),
            shuffle: state.shuffle_state,
        }))
    }

    /// The upcoming tracks (user queue first, then the remaining tracks of the context).
    pub async fn queue(&self) -> Result<Vec<Track>, ApiError> {
        let queue: Option<ApiQueue> = self.get_json("/me/player/queue", &[]).await?;
        Ok(queue.map(|q| parse_items(q.queue)).unwrap_or_default())
    }

    /// The devices that the app can control through the Web API.
    pub async fn devices(&self) -> Result<Vec<Device>, ApiError> {
        let devices: Option<ApiDevices> = self.get_json("/me/player/devices", &[]).await?;
        Ok(devices
            .map(|d| {
                d.devices
                    .into_iter()
                    .filter(|device| !device.is_restricted)
                    .filter_map(ApiDevice::into_device)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Resumes playback, or plays `request` if there is one.
    pub async fn play(
        &self,
        device_id: Option<&str>,
        request: Option<&PlayRequest>,
    ) -> Result<(), ApiError> {
        let query: Vec<(&str, String)> = device_id
            .map(|id| ("device_id", id.to_owned()))
            .into_iter()
            .collect();
        let body = request.map(serde_json::to_value).transpose()?;
        self.command(Method::PUT, "/me/player/play", &query, body)
            .await
    }

    pub async fn pause(&self) -> Result<(), ApiError> {
        self.command(Method::PUT, "/me/player/pause", &[], None)
            .await
    }

    pub async fn next(&self) -> Result<(), ApiError> {
        self.command(Method::POST, "/me/player/next", &[], None)
            .await
    }

    pub async fn previous(&self) -> Result<(), ApiError> {
        self.command(Method::POST, "/me/player/previous", &[], None)
            .await
    }

    pub async fn seek(&self, position_ms: u64) -> Result<(), ApiError> {
        let query = [("position_ms", position_ms.to_string())];
        self.command(Method::PUT, "/me/player/seek", &query, None)
            .await
    }

    /// The volume of the active device, from 0 to 100.
    pub async fn set_volume(&self, percent: u8) -> Result<(), ApiError> {
        let query = [("volume_percent", percent.min(100).to_string())];
        self.command(Method::PUT, "/me/player/volume", &query, None)
            .await
    }

    /// Adds `uri` to Spotify's own queue. It plays after the current track, before the other
    /// tracks of the list, and stops nothing. (The order of that queue cannot change, and the app
    /// cannot remove tracks from it.)
    pub async fn add_to_queue(&self, uri: &str) -> Result<(), ApiError> {
        let query = [("uri", uri.to_owned())];
        self.command(Method::POST, "/me/player/queue", &query, None)
            .await
    }

    /// True for each item of `uris` (at most [`LIBRARY_BATCH`]) that is in the user's library.
    /// For tracks, this is «Liked Songs». The order is the same as in `uris`.
    pub async fn library_contains(&self, uris: &[String]) -> Result<Vec<bool>, ApiError> {
        let query = [("uris", uris.join(","))];
        Ok(self
            .get_json("/me/library/contains", &query)
            .await?
            .unwrap_or_default())
    }

    /// Saves `uris` (at most [`LIBRARY_BATCH`]) to the user's library: a track goes to «Liked
    /// Songs». Since February 2026, this replaces `PUT /me/tracks`.
    pub async fn save_to_library(&self, uris: &[String]) -> Result<(), ApiError> {
        let query = [("uris", uris.join(","))];
        self.command(Method::PUT, "/me/library", &query, None).await
    }

    /// Removes `uris` (at most [`LIBRARY_BATCH`]) from the user's library.
    pub async fn remove_from_library(&self, uris: &[String]) -> Result<(), ApiError> {
        let query = [("uris", uris.join(","))];
        self.command(Method::DELETE, "/me/library", &query, None)
            .await
    }

    pub async fn set_shuffle(&self, on: bool) -> Result<(), ApiError> {
        let query = [("state", on.to_string())];
        self.command(Method::PUT, "/me/player/shuffle", &query, None)
            .await
    }

    /// The playlists of the user `user_id` (their own, followed and collaborative playlists), in
    /// the order of their library.
    pub async fn playlists(&self, user_id: &str) -> Result<Vec<Playlist>, ApiError> {
        let mut playlists = Vec::new();
        let mut offset = 0;
        while offset < MAX_PLAYLISTS {
            let params = [
                ("limit", PLAYLIST_PAGE_SIZE.to_string()),
                ("offset", offset.to_string()),
            ];
            let page: ApiPlaylistPage = self
                .get_json("/me/playlists", &params)
                .await?
                .unwrap_or_default();
            let received = page.items.len() as u32;
            playlists.extend(
                page.items
                    .into_iter()
                    .filter_map(|value| serde_json::from_value::<ApiPlaylist>(value).ok())
                    .map(|playlist| playlist.into_playlist(user_id)),
            );
            offset += received;
            if received == 0 || offset >= page.total {
                break;
            }
        }
        Ok(playlists)
    }

    /// The first `max` tracks of a playlist. Spotify lets the app read only the playlists that the
    /// user owns or collaborates on.
    pub async fn playlist_tracks(&self, id: &str, max: usize) -> Result<Vec<Track>, ApiError> {
        let read = self.playlist_entries(id, 0, max).await?;
        Ok(read.entries.into_iter().flatten().collect())
    }

    /// The entries of a playlist from `start`, in order, until the client finds `max` tracks.
    /// Entries that the app cannot show are `None` (they keep their position).
    pub async fn playlist_entries(
        &self,
        id: &str,
        start: usize,
        max: usize,
    ) -> Result<PlaylistEntries, ApiError> {
        let path = format!("/playlists/{id}/items");
        let mut entries = Vec::new();
        let mut found = 0;
        let mut offset = start as u32;
        let mut total = 0;
        while found < max {
            let params = [
                ("limit", PLAYLIST_PAGE_SIZE.to_string()),
                ("offset", offset.to_string()),
                ("market", "from_token".to_owned()),
            ];
            let page: ApiPlaylistItems = self.get_json(&path, &params).await?.unwrap_or_default();
            let received = page.items.len() as u32;
            total = page.total;
            for entry in page.into_entries() {
                if found == max {
                    break;
                }
                found += usize::from(entry.is_some());
                entries.push(entry);
            }
            offset += received;
            if received == 0 || offset >= total {
                break;
            }
        }
        Ok(PlaylistEntries {
            complete: start + entries.len() >= total as usize,
            entries,
            total,
        })
    }

    /// The album `id` with its tracks (up to [`Album::MAX_TRACKS`]). The first page of tracks
    /// comes with the album. The client reads the other pages with `GET /albums/{id}/tracks`.
    pub async fn album(&self, id: &str) -> Result<Album, ApiError> {
        let market = ("market", "from_token".to_owned());
        let album: ApiFullAlbum = self
            .get_json(&format!("/albums/{id}"), std::slice::from_ref(&market))
            .await?
            .ok_or_else(|| ApiError::Decode("empty album".into()))?;
        let total = (album.tracks.total as usize).min(Album::MAX_TRACKS);
        let mut offset = album.tracks.items.len();
        let mut more = Vec::new();
        let path = format!("/albums/{id}/tracks");
        while offset < total {
            let params = [
                ("limit", PLAYLIST_PAGE_SIZE.to_string()),
                ("offset", offset.to_string()),
                market.clone(),
            ];
            let page: ApiAlbumTracks = self.get_json(&path, &params).await?.unwrap_or_default();
            if page.items.is_empty() {
                break;
            }
            offset += page.items.len();
            more.extend(page.items);
        }
        Ok(album.into_album(more))
    }

    /// The artist `id` with their albums and singles (all the pages), without their
    /// tracks: [`Self::album`] reads them. The list does not include compilations, or albums where
    /// the artist only appears.
    pub async fn artist(&self, id: &str) -> Result<Artist, ApiError> {
        let artist: ApiArtist = self
            .get_json(&format!("/artists/{id}"), &[])
            .await?
            .ok_or_else(|| ApiError::Decode("empty artist".into()))?;
        let path = format!("/artists/{id}/albums");
        let mut releases = Vec::new();
        let mut offset = 0;
        loop {
            let params = [
                ("include_groups", "album,single".to_owned()),
                ("market", "from_token".to_owned()),
                ("limit", ARTIST_ALBUMS_PAGE_SIZE.to_string()),
                ("offset", offset.to_string()),
            ];
            let page: ApiArtistAlbums = self.get_json(&path, &params).await?.unwrap_or_default();
            let received = page.items.len() as u32;
            let total = page.total;
            releases.extend(page.into_releases());
            offset += received;
            if received == 0 || offset >= total {
                break;
            }
        }
        Ok(Artist::new(id.to_owned(), artist.name, releases))
    }

    /// Creates an empty, private playlist with the name `name` in the user's library, and returns
    /// it.
    pub async fn create_playlist(&self, name: &str) -> Result<Playlist, ApiError> {
        let body = json!({ "name": name, "public": false });
        let response = self
            .send(Method::POST, "/me/playlists", &[], Some(body))
            .await?;
        let bytes = response.bytes().await?;
        let playlist: ApiPlaylist = serde_json::from_slice(&bytes)?;
        // It is of the user: the user is its owner.
        let owner = playlist
            .owner
            .as_ref()
            .map(|owner| owner.id.clone())
            .unwrap_or_default();
        let mut playlist = playlist.into_playlist(&owner);
        playlist.editable = true;
        Ok(playlist)
    }

    /// Gives the playlist `id` the name `name` (only its owner can do this).
    pub async fn rename_playlist(&self, id: &str, name: &str) -> Result<(), ApiError> {
        let body = json!({ "name": name });
        self.command(Method::PUT, &format!("/playlists/{id}"), &[], Some(body))
            .await
    }

    /// The current version of a playlist, which the changes refer to.
    pub async fn playlist_snapshot(&self, id: &str) -> Result<Option<String>, ApiError> {
        let query = [("fields", "snapshot_id".to_owned())];
        let answer: Option<ApiSnapshot> =
            self.get_json(&format!("/playlists/{id}"), &query).await?;
        Ok(answer.and_then(|answer| answer.snapshot_id))
    }

    /// Adds `uris` to the playlist `id`, from `position` (at the end with `None`). Returns the new
    /// snapshot ID.
    pub async fn add_to_playlist(
        &self,
        id: &str,
        uris: &[String],
        position: Option<usize>,
    ) -> Result<Option<String>, ApiError> {
        let mut body = json!({ "uris": uris });
        if let Some(position) = position {
            body["position"] = json!(position);
        }
        self.playlist_change(Method::POST, id, body).await
    }

    /// Moves the entry at `range_start` of the playlist `id` before the entry at `insert_before`
    /// (positions from before the move; the length of the playlist moves it to the end). Returns
    /// the new snapshot ID.
    pub async fn reorder_playlist(
        &self,
        id: &str,
        range_start: usize,
        insert_before: usize,
        snapshot_id: Option<&str>,
    ) -> Result<Option<String>, ApiError> {
        let mut body = json!({
            "range_start": range_start,
            "insert_before": insert_before,
            "range_length": 1,
        });
        if let Some(snapshot_id) = snapshot_id {
            body["snapshot_id"] = json!(snapshot_id);
        }
        self.playlist_change(Method::PUT, id, body).await
    }

    /// Removes all copies of `uri` from the playlist `id` (the Web API removes only by URI).
    /// Returns the new snapshot ID.
    ///
    /// The request does not send a `snapshot_id`. With one, Spotify applies the removal to that
    /// version and merges it with the current version. The result can be a duplicate track, not a
    /// removed track.
    pub async fn remove_from_playlist(
        &self,
        id: &str,
        uri: &str,
    ) -> Result<Option<String>, ApiError> {
        let body = json!({ "items": [{ "uri": uri }] });
        self.playlist_change(Method::DELETE, id, body).await
    }

    pub async fn transfer_playback(&self, device_id: &str, play: bool) -> Result<(), ApiError> {
        let body = json!({ "device_ids": [device_id], "play": play });
        self.command(Method::PUT, "/me/player", &[], Some(body))
            .await
    }
}
