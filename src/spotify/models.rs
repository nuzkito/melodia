//! Serde models for the part of the Spotify Web API that the app uses, and their conversion into
//! the domain types of [`crate::model`].
//!
//! The parse of items is tolerant. The models skip all items that they cannot understand (ads,
//! unknown item types, malformed entries). The full response does not fail.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::model::{
    Album, AlbumHit, ArtistHit, Device, Playlist, Release, ReleaseKind, SearchPage, Track,
    UserProfile,
};

/// Uses an explicit JSON `null` as a missing field.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiImage {
    pub url: String,
    #[serde(default)]
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiArtist {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiAlbum {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub images: Vec<ApiImage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiTrack {
    pub uri: String,
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub artists: Vec<ApiArtist>,
    #[serde(default)]
    pub album: Option<ApiAlbum>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub explicit: bool,
}

/// An item of a list, identified by the `type` field. The app supports only tracks. It skips
/// episodes, ads and all new types.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ApiItem {
    Track(ApiTrack),
    #[serde(other)]
    Unsupported,
}

impl ApiItem {
    pub fn into_track(self) -> Option<Track> {
        match self {
            Self::Track(track) => {
                let album = track.album.unwrap_or_default();
                let (cover_small, cover_large) = pick_covers(&album.images);
                let (artists, artist_ids) = track
                    .artists
                    .into_iter()
                    .filter(|artist| !artist.name.is_empty())
                    .map(|artist| (artist.name, artist.id.filter(|id| !id.is_empty())))
                    .unzip();
                Some(Track {
                    uri: track.uri,
                    name: track.name,
                    artists,
                    artist_ids,
                    album: album.name,
                    album_id: album.id.filter(|id| !id.is_empty()),
                    duration_ms: track.duration_ms,
                    cover_small,
                    cover_large,
                    explicit: track.explicit,
                })
            }
            Self::Unsupported => None,
        }
    }
}

/// Parses a JSON value that should hold a track object.
pub fn parse_item(value: Value) -> Option<Track> {
    if value.is_null() {
        return None;
    }
    serde_json::from_value::<ApiItem>(value).ok()?.into_track()
}

pub fn parse_items(values: impl IntoIterator<Item = Value>) -> Vec<Track> {
    values.into_iter().filter_map(parse_item).collect()
}

/// Selects a small image for thumbnails, and the largest image for the main cover. The small image
/// is the smallest image that is 100 px wide or more, so it stays sharp on HiDPI screens. An image
/// without a known width counts as large.
pub fn pick_covers(images: &[ApiImage]) -> (Option<String>, Option<String>) {
    let width = |image: &ApiImage| image.width.unwrap_or(640);
    let large = images.iter().max_by_key(|image| width(image));
    let small = images
        .iter()
        .filter(|image| width(image) >= 100)
        .min_by_key(|image| width(image))
        .or(large);
    (
        small.map(|image| image.url.clone()),
        large.map(|image| image.url.clone()),
    )
}

/// An album as `GET /albums/{id}` returns it, with the first page of its tracks (the other pages
/// come from `GET /albums/{id}/tracks`).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiFullAlbum {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `1975-10-31`, `1975-10` or `1975`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub release_date: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub images: Vec<ApiImage>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub tracks: ApiAlbumTracks,
}

/// A page of the tracks of an album. They come without their album, which
/// [`ApiFullAlbum::into_album`] adds.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiAlbumTracks {
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<Value>,
    #[serde(default)]
    pub total: u32,
}

impl ApiFullAlbum {
    /// The album with its tracks: the first page and then `more` (the items of the next pages). It
    /// skips the tracks that it cannot read.
    pub fn into_album(self, more: Vec<Value>) -> Album {
        let (cover_small, cover_large) = pick_covers(&self.images);
        let tracks = parse_items(self.tracks.items.into_iter().chain(more))
            .into_iter()
            .take(Album::MAX_TRACKS)
            .map(|track| Track {
                album: self.name.clone(),
                album_id: Some(self.id.clone()),
                cover_small: cover_small.clone(),
                cover_large: cover_large.clone(),
                ..track
            })
            .collect();
        Album {
            year: Album::year_of(&self.release_date),
            id: self.id,
            name: self.name,
            tracks,
        }
    }
}

/// An album as `GET /artists/{id}/albums` lists it, without its tracks.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiSimpleAlbum {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `album`, `single` or `compilation`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub album_type: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub release_date: String,
    #[serde(default)]
    pub total_tracks: u32,
    #[serde(default, deserialize_with = "null_as_default")]
    pub images: Vec<ApiImage>,
    /// Only search results use it, to tell the artist of the album.
    #[serde(default, deserialize_with = "null_as_default")]
    pub artists: Vec<ApiArtist>,
}

impl ApiSimpleAlbum {
    pub fn into_hit(self) -> Option<AlbumHit> {
        let artists = self
            .artists
            .iter()
            .map(|artist| artist.name.clone())
            .filter(|name| !name.is_empty())
            .collect();
        Some(AlbumHit {
            release: self.into_release()?,
            artists,
        })
    }

    pub fn into_release(self) -> Option<Release> {
        if self.id.is_empty() {
            return None;
        }
        let kind = match self.album_type.to_ascii_lowercase().as_str() {
            "single" => ReleaseKind::Single,
            _ => ReleaseKind::Album,
        };
        Some(Release {
            cover: pick_covers(&self.images).0,
            id: self.id,
            name: self.name,
            kind,
            release_date: self.release_date,
            total_tracks: self.total_tracks,
        })
    }
}

/// A page of `GET /artists/{id}/albums`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiArtistAlbums {
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<Value>,
    #[serde(default)]
    pub total: u32,
}

impl ApiArtistAlbums {
    /// The releases of the page that the model can read.
    pub fn into_releases(self) -> Vec<Release> {
        self.items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<ApiSimpleAlbum>(item).ok())
            .filter_map(ApiSimpleAlbum::into_release)
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiPlaybackState {
    #[serde(default)]
    pub device: Option<ApiDevice>,
    #[serde(default)]
    pub progress_ms: Option<u64>,
    #[serde(default)]
    pub is_playing: bool,
    #[serde(default)]
    pub shuffle_state: bool,
    #[serde(default)]
    pub item: Option<Value>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiDevice {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_restricted: bool,
    #[serde(default)]
    pub volume_percent: Option<u32>,
    /// Missing in old responses. Use true: Spotify answers if it is false.
    #[serde(default = "default_true")]
    pub supports_volume: bool,
}

impl ApiDevice {
    pub fn into_device(self) -> Option<Device> {
        Some(Device {
            id: self.id.filter(|id| !id.is_empty())?,
            name: self.name,
            kind: self.kind,
            is_active: self.is_active,
            volume_percent: self.volume_percent.map(|v| v.min(100) as u8),
            supports_volume: self.supports_volume,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiDevices {
    #[serde(default, deserialize_with = "null_as_default")]
    pub devices: Vec<ApiDevice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiQueue {
    #[serde(default, deserialize_with = "null_as_default")]
    pub queue: Vec<Value>,
}

/// Reads the answer of `GET /search` (`raw`, as Spotify sent it or as the cache kept it). It skips
/// all data that it does not understand.
pub fn parse_search(query: &str, offset: u32, raw: Value) -> SearchPage {
    let response: ApiSearchResponse = serde_json::from_value(raw).unwrap_or_default();
    let paging = response.tracks.unwrap_or_default();
    SearchPage {
        query: query.to_owned(),
        offset,
        tracks: parse_items(paging.items),
        albums: response.albums.unwrap_or_default().into_album_hits(),
        artists: response.artists.unwrap_or_default().into_artist_hits(),
        total: paging.total,
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiSearchResponse {
    #[serde(default)]
    pub tracks: Option<ApiPaging>,
    #[serde(default)]
    pub albums: Option<ApiPaging>,
    #[serde(default)]
    pub artists: Option<ApiPaging>,
}

impl ApiPaging {
    /// The albums of the page that the model can read.
    pub fn into_album_hits(self) -> Vec<AlbumHit> {
        self.items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<ApiSimpleAlbum>(item).ok())
            .filter_map(ApiSimpleAlbum::into_hit)
            .collect()
    }

    /// The artists of the page that the model can read.
    pub fn into_artist_hits(self) -> Vec<ArtistHit> {
        self.items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<ApiFullArtist>(item).ok())
            .filter_map(ApiFullArtist::into_hit)
            .collect()
    }
}

/// An artist as the search returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiFullArtist {
    #[serde(default, deserialize_with = "null_as_default")]
    pub id: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub images: Vec<ApiImage>,
}

impl ApiFullArtist {
    pub fn into_hit(self) -> Option<ArtistHit> {
        if self.id.is_empty() {
            return None;
        }
        Some(ArtistHit {
            cover: pick_covers(&self.images).0,
            id: self.id,
            name: self.name,
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiPaging {
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<Value>,
    #[serde(default)]
    pub total: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiUser {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

impl ApiUser {
    pub fn into_profile(self) -> UserProfile {
        let display_name = self
            .display_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.id.clone());
        UserProfile {
            id: self.id,
            display_name,
        }
    }
}

/// The number of tracks of a playlist: `{"href": "…", "total": 12}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiCount {
    #[serde(default)]
    pub total: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiOwner {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A playlist as `GET /me/playlists` lists it. Since February 2026, the track count is under
/// `items` (it was `tracks`).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPlaylist {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub owner: Option<ApiOwner>,
    #[serde(default)]
    pub collaborative: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub images: Vec<ApiImage>,
    #[serde(default)]
    pub items: Option<ApiCount>,
    #[serde(default)]
    pub tracks: Option<ApiCount>,
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

impl ApiPlaylist {
    /// `user_id` is the current user. The user can edit their playlists (and the playlists that
    /// they collaborate on).
    pub fn into_playlist(self, user_id: &str) -> Playlist {
        let owner = self.owner.unwrap_or_default();
        let (cover, _) = pick_covers(&self.images);
        Playlist {
            editable: self.collaborative || (!owner.id.is_empty() && owner.id == user_id),
            owner: owner
                .display_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(owner.id),
            total: self.items.or(self.tracks).unwrap_or_default().total,
            cover,
            id: self.id,
            name: self.name,
            snapshot: self.snapshot_id.filter(|snapshot| !snapshot.is_empty()),
        }
    }
}

/// A page of playlists. The model skips malformed entries (or `null`s).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiPlaylistPage {
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<Value>,
    #[serde(default)]
    pub total: u32,
}

/// An entry of `GET /playlists/{id}/items`. The track is under `item` (under `track` before
/// February 2026).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiPlaylistEntry {
    #[serde(default)]
    pub item: Option<Value>,
    #[serde(default)]
    pub track: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiPlaylistItems {
    #[serde(default, deserialize_with = "null_as_default")]
    pub items: Vec<ApiPlaylistEntry>,
    #[serde(default)]
    pub total: u32,
}

impl ApiPlaylistItems {
    pub fn into_tracks(self) -> Vec<Track> {
        self.into_entries().into_iter().flatten().collect()
    }

    /// All entries, in order. `None` for the entries that the app cannot show (gone from Spotify,
    /// unknown types…). They still have a position in the playlist, which edits must count.
    pub fn into_entries(self) -> Vec<Option<Track>> {
        self.items
            .into_iter()
            .map(|entry| entry.item.or(entry.track).and_then(parse_item))
            .collect()
    }
}

/// The answer to the requests that change a playlist, and to the request with
/// `fields=snapshot_id`: its version after the change.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiSnapshot {
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

/// The error body: `{"error": {"status": 404, "message": "…", "reason": "…"}}`.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorEnvelope {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorDetail {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// The body of `PUT /me/player/play`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayRequest {
    pub uris: Vec<String>,
    pub offset: PlayOffset,
    /// The start position of the first track (to continue from its last position).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayOffset {
    pub position: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn image(url: &str, width: Option<u32>) -> Value {
        json!({ "url": url, "width": width, "height": width })
    }

    #[test]
    fn parses_tracks_with_artists_album_and_covers() {
        let value = json!({
            "type": "track",
            "uri": "spotify:track:1",
            "name": "Bohemian Rhapsody",
            "duration_ms": 354_000,
            "artists": [{ "id": "queen", "name": "Queen" }, { "name": "" }, { "name": "Nadie" }],
            "album": {
                "id": "opera",
                "name": "A Night at the Opera",
                "images": [image("big", Some(640)), image("mid", Some(300)), image("tiny", Some(64))]
            }
        });
        let track = parse_item(value).unwrap();
        assert_eq!(track.album_id.as_deref(), Some("opera"));
        assert_eq!(track.name, "Bohemian Rhapsody");
        assert_eq!(track.artists, vec!["Queen", "Nadie"]);
        assert_eq!(track.artist_id(0), Some("queen"));
        assert_eq!(track.artist_id(1), None);
        assert_eq!(track.album, "A Night at the Opera");
        assert_eq!(track.duration_ms, 354_000);
        assert_eq!(track.cover_small.as_deref(), Some("mid"));
        assert_eq!(track.cover_large.as_deref(), Some("big"));
        assert_eq!(track.subtitle(), "Queen, Nadie · A Night at the Opera");
    }

    #[test]
    fn a_track_is_explicit_only_when_spotify_says_so() {
        let item = |explicit: Value| {
            json!({
                "type": "track",
                "uri": "spotify:track:1",
                "name": "Fat Bottomed Girls",
                "explicit": explicit,
            })
        };
        assert!(parse_item(item(json!(true))).unwrap().explicit);
        assert!(!parse_item(item(json!(false))).unwrap().explicit);
        assert!(!parse_item(item(Value::Null)).unwrap().explicit);
        let missing = json!({ "type": "track", "uri": "spotify:track:1", "name": "Jazz" });
        assert!(!parse_item(missing).unwrap().explicit);
    }

    #[test]
    fn the_releases_of_an_artist_are_read_leniently() {
        let page: ApiArtistAlbums = serde_json::from_value(json!({
            "items": [
                {
                    "id": "opera",
                    "name": "A Night at the Opera",
                    "album_type": "album",
                    "release_date": "1975-11-21",
                    "total_tracks": 12,
                    "images": [image("big", Some(640)), image("mid", Some(300))]
                },
                { "id": "bohemian", "name": "Bohemian Rhapsody", "album_type": "SINGLE", "release_date": null },
                { "name": "Without an ID" },
                null
            ],
            "total": 4
        }))
        .unwrap();
        let releases = page.into_releases();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].kind, ReleaseKind::Album);
        assert_eq!(releases[0].year(), Some(1975));
        assert_eq!(releases[0].total_tracks, 12);
        assert_eq!(releases[0].cover.as_deref(), Some("mid"));
        assert_eq!(releases[1].kind, ReleaseKind::Single);
        assert_eq!(releases[1].year(), None);
    }

    #[test]
    fn album_tracks_get_the_name_id_and_covers_of_their_album() {
        let track = |n: u32, name: &str| {
            json!({
                "type": "track",
                "uri": format!("spotify:track:{n}"),
                "name": name,
                "duration_ms": 180_000,
                "artists": [{ "name": "Queen" }]
            })
        };
        let album: ApiFullAlbum = serde_json::from_value(json!({
            "id": "opera",
            "name": "A Night at the Opera",
            "release_date": "1975-11-21",
            "images": [image("big", Some(640)), image("mid", Some(300))],
            "tracks": {
                "items": [track(1, "Death on Two Legs"), { "type": "ad" }, track(2, "Lazing on a Sunday Afternoon")],
                "total": 3
            }
        }))
        .unwrap();
        let album = album.into_album(vec![track(3, "Bohemian Rhapsody")]);
        assert_eq!(album.name, "A Night at the Opera");
        assert_eq!(album.year, Some(1975));
        let names: Vec<_> = album.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Death on Two Legs",
                "Lazing on a Sunday Afternoon",
                "Bohemian Rhapsody"
            ]
        );
        let last = &album.tracks[2];
        assert_eq!(last.album, "A Night at the Opera");
        assert_eq!(last.album_id.as_deref(), Some("opera"));
        assert_eq!(last.cover_small.as_deref(), Some("mid"));
        assert_eq!(last.cover_large.as_deref(), Some("big"));
    }

    #[test]
    fn an_album_without_a_release_date_or_tracks_still_loads() {
        let album: ApiFullAlbum = serde_json::from_value(json!({
            "id": "x",
            "name": "Unreleased",
            "release_date": null,
            "tracks": null
        }))
        .unwrap();
        let album = album.into_album(Vec::new());
        assert_eq!(album.year, None);
        assert!(album.tracks.is_empty());
    }

    #[test]
    fn skips_unknown_and_malformed_items() {
        let tracks = parse_items(vec![
            json!(null),
            json!({ "type": "ad", "uri": "x", "name": "Ad" }),
            json!({ "type": "episode", "uri": "spotify:episode:9", "name": "Chapter 1", "duration_ms": 1000 }),
            json!({ "type": "track", "name": "no uri" }),
            json!({ "type": "track", "uri": "spotify:track:2", "name": "Ok", "album": null, "artists": null }),
        ]);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].name, "Ok");
        assert_eq!(tracks[0].album, "");
        assert!(tracks[0].cover_large.is_none());
    }

    #[test]
    fn picks_covers_with_unknown_sizes() {
        let images: Vec<ApiImage> =
            serde_json::from_value(json!([image("a", None), image("b", Some(64))])).unwrap();
        assert_eq!(pick_covers(&images), (Some("a".into()), Some("a".into())));
        assert_eq!(pick_covers(&[]), (None, None));
    }

    #[test]
    fn devices_without_id_are_ignored() {
        let devices: ApiDevices = serde_json::from_value(json!({
            "devices": [
                { "id": "abc", "name": "PC", "type": "Computer", "is_active": true },
                { "id": null, "name": "Restricted", "type": "Speaker", "is_active": false }
            ]
        }))
        .unwrap();
        let devices: Vec<Device> = devices
            .devices
            .into_iter()
            .filter_map(ApiDevice::into_device)
            .collect();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "PC");
        assert!(devices[0].is_active);
    }

    #[test]
    fn user_without_display_name_falls_back_to_id() {
        let user: ApiUser =
            serde_json::from_value(json!({ "id": "nuzkito", "display_name": null })).unwrap();
        assert_eq!(user.into_profile().display_name, "nuzkito");
    }

    #[test]
    fn play_request_serializes_uris_and_offset() {
        let mut request = PlayRequest {
            uris: vec!["spotify:track:1".into(), "spotify:track:2".into()],
            offset: PlayOffset { position: 1 },
            position_ms: None,
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({ "uris": ["spotify:track:1", "spotify:track:2"], "offset": { "position": 1 } })
        );
        request.position_ms = Some(61_000);
        assert_eq!(
            serde_json::to_value(&request).unwrap()["position_ms"],
            json!(61_000)
        );
    }

    #[test]
    fn parses_playlists_old_and_new() {
        let page: ApiPlaylistPage = serde_json::from_value(json!({
            "total": 3,
            "items": [
                {
                    "id": "p1", "name": "Mine", "collaborative": false,
                    "owner": { "id": "alice", "display_name": "Alice" },
                    "images": [image("big", Some(640)), image("small", Some(300))],
                    "items": { "href": "…", "total": 12 },
                    "snapshot_id": "v7"
                },
                {
                    "id": "p2", "name": "Theirs", "collaborative": false,
                    "owner": { "id": "spotify", "display_name": null },
                    "images": null,
                    "tracks": { "total": 1 }
                },
                null
            ]
        }))
        .unwrap();
        let playlists: Vec<Playlist> = page
            .items
            .into_iter()
            .filter_map(|value| serde_json::from_value::<ApiPlaylist>(value).ok())
            .map(|playlist| playlist.into_playlist("alice"))
            .collect();
        assert_eq!(playlists.len(), 2);
        assert_eq!(playlists[0].total, 12);
        assert_eq!(playlists[0].cover.as_deref(), Some("small"));
        assert!(playlists[0].editable);
        assert_eq!(playlists[0].snapshot.as_deref(), Some("v7"));
        assert_eq!(playlists[1].owner, "spotify");
        assert_eq!(playlists[1].snapshot, None);
        assert_eq!(playlists[1].count_line(), "1 song");
        assert!(!playlists[1].editable);
    }

    #[test]
    fn playlist_entries_use_item_or_track() {
        let page: ApiPlaylistItems = serde_json::from_value(json!({
            "total": 3,
            "items": [
                { "item": { "type": "track", "uri": "spotify:track:1", "name": "One" } },
                { "track": { "type": "track", "uri": "spotify:track:2", "name": "Two" } },
                { "item": null }
            ]
        }))
        .unwrap();
        let entries: Vec<Option<String>> = page
            .clone()
            .into_entries()
            .into_iter()
            .map(|t| t.map(|t| t.name))
            .collect();
        assert_eq!(entries, [Some("One".into()), Some("Two".into()), None]);
        let names: Vec<String> = page.into_tracks().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["One", "Two"]);
    }

    #[test]
    fn devices_report_their_volume() {
        let device: ApiDevice = serde_json::from_value(json!({
            "id": "phone", "name": "Phone", "type": "Smartphone",
            "volume_percent": 64, "supports_volume": false
        }))
        .unwrap();
        let device = device.into_device().unwrap();
        assert_eq!(device.volume_percent, Some(64));
        assert!(!device.supports_volume);

        let old: ApiDevice =
            serde_json::from_value(json!({ "id": "pc", "name": "PC", "type": "Computer" }))
                .unwrap();
        let old = old.into_device().unwrap();
        assert_eq!(old.volume_percent, None);
        assert!(old.supports_volume);
    }
}
