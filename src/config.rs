//! Application configuration: the location of files, the Spotify app (Client ID) to use, and how
//! the app receives the OAuth redirect.
//!
//! Sources, from highest to lowest priority:
//! 1. Environment variables (`SPOTIFY_CLIENT_ID`, `MELODIA_*`).
//! 2. `settings.json` in the configuration directory (written by the UI).
//! 3. A Client ID embedded at compile time (`SPOTIFY_CLIENT_ID=… cargo build`).

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::Timings;
use crate::model::{Library, LibraryItem};
use crate::webplayer::WebPlayerHost;

pub const APP_NAME: &str = "Melodia";
pub const DEFAULT_REDIRECT_PORT: u16 = 8898;
pub const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";
pub const SPOTIFY_ACCOUNTS_BASE: &str = "https://accounts.spotify.com";
pub const SPOTIFY_DASHBOARD_URL: &str = "https://developer.spotify.com/dashboard";

const BUILT_IN_CLIENT_ID: Option<&str> = option_env!("SPOTIFY_CLIENT_ID");

/// The URL where the app asks for its latest version during development: the `web` service of
/// `compose.yml`, which serves `web/version.json`.
pub const DEV_UPDATE_URL: &str = "http://127.0.0.1:8897/version.json";

/// The update manifest that the app asks for: `MELODIA_UPDATE_URL` at build time (the production
/// server, when there is one), or the development manifest.
pub const BUILT_IN_UPDATE_URL: &str = match option_env!("MELODIA_UPDATE_URL") {
    Some(url) if !url.is_empty() => url,
    _ => DEV_UPDATE_URL,
};
const SETTINGS_FILE: &str = "settings.json";
const SESSION_FILE: &str = "session.json";
const LOG_FILE: &str = "melodia.log";

#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub config_dir: PathBuf,
    pub client_id: Option<String>,
    /// The port of the loopback redirect URI. `0` asks the OS for a free port. Spotify accepts
    /// this when the registered URI has no port.
    pub redirect_port: u16,
    /// The address of the OAuth callback listener. It changes only in a container, where the host
    /// publishes the port.
    pub callback_bind: IpAddr,
    /// Use fake data in place of Spotify (`--demo` / `MELODIA_DEMO=1`).
    pub demo: bool,
    pub api_base: String,
    pub accounts_base: String,
    /// The update manifest (`version.json`) where the app looks for new versions.
    pub update_url: String,
    /// The host of «Play in this app» (`None` where it is not supported). The window, which hosts
    /// the player page, sets it.
    pub web_player: Option<WebPlayerHost>,
    /// The volume that the user last set in Melodia (0–100), for «Play in this app».
    pub volume: Option<u8>,
    /// The time that the real backend waits before each follow-up. The app always uses the
    /// default. The tests use shorter times.
    pub timings: Timings,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume: Option<u8>,
    /// The order of the playlists for each Spotify account (user ID → playlist IDs), as saved
    /// before the app had folders. The app reads it one time, then `playlist_library` replaces it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    playlist_order: BTreeMap<String, Vec<String>>,
    /// The folders and the order of the playlists in the sidebar, for each Spotify account (user
    /// ID → first level). Spotify cannot see folders or change the order of a library, so the app
    /// keeps them here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    playlist_library: BTreeMap<String, Vec<LibraryItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<ResumePoint>,
}

/// The position of the music the last time that it played in this app («Play in this app»). The
/// player page closes with the app and does not tell Spotify the position. Thus Spotify goes back
/// to an older position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePoint {
    pub uri: String,
    pub position_ms: u64,
}

/// More than one task writes `settings.json`: one at a time.
static SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Changes `settings.json` in `config_dir`, and keeps its other contents.
fn update_settings(config_dir: &Path, change: impl FnOnce(&mut Settings)) -> io::Result<()> {
    let _guard = crate::util::lock(&SETTINGS_LOCK);
    let path = config_dir.join(SETTINGS_FILE);
    let mut settings = read_settings(&path);
    change(&mut settings);
    let json = serde_json::to_vec_pretty(&settings).map_err(io::Error::other)?;
    write_private_file(&path, &json)
}

/// Keeps the volume for the next time the app opens.
pub fn save_volume(config_dir: &Path, percent: u8) -> io::Result<()> {
    update_settings(config_dir, |settings| {
        settings.volume = Some(percent.min(100));
    })
}

/// The folders and the order of the playlists of the Spotify user `user_id` in the sidebar, if the
/// user arranged them. The function reads an order saved before there were folders as a library
/// without folders.
pub fn playlist_library(config_dir: &Path, user_id: &str) -> Option<Library> {
    let mut settings = read_settings(&config_dir.join(SETTINGS_FILE));
    if let Some(items) = settings.playlist_library.remove(user_id) {
        return Some(Library::new(items));
    }
    settings
        .playlist_order
        .remove(user_id)
        .map(|ids| Library::from_order(&ids))
}

/// Saves the folders and order of the playlists of `user_id` (an empty library forgets them).
pub fn save_playlist_library(
    config_dir: &Path,
    user_id: &str,
    library: &Library,
) -> io::Result<()> {
    update_settings(config_dir, |settings| {
        settings.playlist_order.remove(user_id);
        if library.is_empty() {
            settings.playlist_library.remove(user_id);
        } else {
            settings
                .playlist_library
                .insert(user_id.to_owned(), library.items().to_vec());
        }
    })
}

/// The position of the music the last time that it played in this app, if known.
pub fn resume_point(config_dir: &Path) -> Option<ResumePoint> {
    read_settings(&config_dir.join(SETTINGS_FILE)).resume
}

/// Keeps (or forgets, with `None`) the position of the music in this app.
pub fn save_resume_point(config_dir: &Path, point: Option<&ResumePoint>) -> io::Result<()> {
    update_settings(config_dir, |settings| settings.resume = point.cloned())
}

impl AppConfig {
    /// Reads the configuration from the process environment and arguments.
    pub fn load() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::from_env(|key| std::env::var(key).ok(), &args)
    }

    /// The folder where «Play in this app» keeps its WebView2 profile. It is a cache, so it is in
    /// the local AppData, not in the roaming AppData.
    pub fn web_data_dir(&self) -> PathBuf {
        directories::ProjectDirs::from("", "", APP_NAME)
            .map(|dirs| dirs.data_local_dir().join("webview"))
            .unwrap_or_else(|| self.config_dir.join("webview"))
    }

    /// Builds the configuration from an environment lookup function. Thus a test can use it and
    /// not change the real process environment.
    pub fn from_env(env: impl Fn(&str) -> Option<String>, args: &[String]) -> Self {
        let var = |key: &str| {
            env(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };

        let config_dir = var("MELODIA_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_config_dir);
        let settings = read_settings(&config_dir.join(SETTINGS_FILE));

        let client_id = var("SPOTIFY_CLIENT_ID")
            .or(settings.client_id)
            .or_else(|| BUILT_IN_CLIENT_ID.map(str::to_owned))
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());

        let redirect_port = var("MELODIA_PORT")
            .and_then(|port| port.parse().ok())
            .unwrap_or(DEFAULT_REDIRECT_PORT);

        let callback_bind = var("MELODIA_CALLBACK_BIND")
            .and_then(|addr| addr.parse().ok())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

        let demo = args.iter().any(|arg| arg == "--demo")
            || var("MELODIA_DEMO").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));

        Self {
            config_dir,
            client_id,
            redirect_port,
            callback_bind,
            demo,
            api_base: SPOTIFY_API_BASE.to_owned(),
            accounts_base: SPOTIFY_ACCOUNTS_BASE.to_owned(),
            update_url: var("MELODIA_UPDATE_URL").unwrap_or_else(|| BUILT_IN_UPDATE_URL.to_owned()),
            web_player: None,
            volume: settings.volume.map(|v| v.min(100)),
            timings: Timings::default(),
        }
    }

    pub fn session_path(&self) -> PathBuf {
        self.config_dir.join(SESSION_FILE)
    }

    pub fn log_path(&self) -> PathBuf {
        self.config_dir.join(LOG_FILE)
    }

    /// The redirect URI that the user must register in the Spotify dashboard.
    pub fn redirect_uri(&self) -> String {
        redirect_uri_for_port(self.redirect_port)
    }

    /// Stores a new Client ID in `settings.json` and starts to use it.
    pub fn save_client_id(&mut self, client_id: &str) -> io::Result<()> {
        let client_id = client_id.trim().to_owned();
        update_settings(&self.config_dir, |settings| {
            settings.client_id = Some(client_id.clone());
        })?;
        self.client_id = Some(client_id);
        Ok(())
    }
}

pub fn redirect_uri_for_port(port: u16) -> String {
    if port == 0 {
        "http://127.0.0.1/callback".to_owned()
    } else {
        format!("http://127.0.0.1:{port}/callback")
    }
}

/// Spotify Client IDs are 32 hexadecimal characters.
pub fn is_valid_client_id(client_id: &str) -> bool {
    let client_id = client_id.trim();
    client_id.len() == 32 && client_id.chars().all(|c| c.is_ascii_hexdigit())
}

fn default_config_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", APP_NAME)
        .map(|dirs| dirs.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".melodia"))
}

fn read_settings(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes a file atomically (temporary file + rename). On Unix, only the current user can read
/// it. On Windows, the AppData ACLs of each user apply.
pub(crate) fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn config_with(vars: &[(&str, &str)], args: &[&str]) -> AppConfig {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        AppConfig::from_env(|key| vars.get(key).cloned(), &args)
    }

    #[test]
    fn environment_overrides_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with(
            &[
                ("MELODIA_CONFIG_DIR", dir.path().to_str().unwrap()),
                ("SPOTIFY_CLIENT_ID", " 0123456789abcdef0123456789abcdef "),
                ("MELODIA_PORT", "9000"),
                ("MELODIA_CALLBACK_BIND", "0.0.0.0"),
            ],
            &[],
        );
        assert_eq!(config.config_dir, dir.path());
        assert_eq!(
            config.client_id.as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(config.redirect_port, 9000);
        assert_eq!(config.callback_bind, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(config.redirect_uri(), "http://127.0.0.1:9000/callback");
        assert!(!config.demo);
    }

    #[test]
    fn the_update_url_can_be_changed_from_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().to_str().unwrap();
        let config = config_with(&[("MELODIA_CONFIG_DIR", dir)], &[]);
        assert_eq!(config.update_url, BUILT_IN_UPDATE_URL);

        let config = config_with(
            &[
                ("MELODIA_CONFIG_DIR", dir),
                ("MELODIA_UPDATE_URL", " http://web:80/version.json "),
            ],
            &[],
        );
        assert_eq!(config.update_url, "http://web:80/version.json");
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_with(
            &[
                ("MELODIA_CONFIG_DIR", dir.path().to_str().unwrap()),
                ("MELODIA_PORT", "not-a-port"),
                ("MELODIA_CALLBACK_BIND", "nowhere"),
                ("SPOTIFY_CLIENT_ID", "   "),
            ],
            &[],
        );
        assert_eq!(config.redirect_port, DEFAULT_REDIRECT_PORT);
        assert_eq!(config.callback_bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(config.redirect_uri(), "http://127.0.0.1:8898/callback");
    }

    #[test]
    fn saved_client_id_is_loaded_back() {
        let dir = tempfile::tempdir().unwrap();
        let vars = [("MELODIA_CONFIG_DIR", dir.path().to_str().unwrap())];
        let mut config = config_with(&vars, &[]);
        config
            .save_client_id("fedcba9876543210fedcba9876543210")
            .unwrap();

        let reloaded = config_with(&vars, &[]);
        assert_eq!(
            reloaded.client_id.as_deref(),
            Some("fedcba9876543210fedcba9876543210")
        );
    }

    #[test]
    fn volume_is_remembered_with_the_client_id() {
        let dir = tempfile::tempdir().unwrap();
        let vars = [("MELODIA_CONFIG_DIR", dir.path().to_str().unwrap())];
        assert_eq!(config_with(&vars, &[]).volume, None);

        let mut config = config_with(&vars, &[]);
        config
            .save_client_id("fedcba9876543210fedcba9876543210")
            .unwrap();
        save_volume(dir.path(), 35).unwrap();
        save_volume(dir.path(), 140).unwrap();

        let reloaded = config_with(&vars, &[]);
        assert_eq!(reloaded.volume, Some(100));
        assert_eq!(
            reloaded.client_id.as_deref(),
            Some("fedcba9876543210fedcba9876543210")
        );
    }

    #[test]
    fn the_playlist_library_is_remembered_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let vars = [("MELODIA_CONFIG_DIR", dir.path().to_str().unwrap())];
        assert_eq!(playlist_library(dir.path(), "alice"), None);

        let mut config = config_with(&vars, &[]);
        config
            .save_client_id("fedcba9876543210fedcba9876543210")
            .unwrap();
        let order = |ids: &[&str]| ids.iter().map(ToString::to_string).collect::<Vec<_>>();
        let alice = Library::new(vec![
            LibraryItem::Folder(crate::model::Folder {
                id: "folder-1".into(),
                name: "Festivals".into(),
                items: vec![LibraryItem::Playlist("b".into())],
            }),
            LibraryItem::Playlist("a".into()),
        ]);
        save_playlist_library(dir.path(), "alice", &alice).unwrap();
        let bob = Library::from_order(&order(&["x", "y", "z"]));
        save_playlist_library(dir.path(), "bob", &bob).unwrap();
        save_volume(dir.path(), 40).unwrap();

        assert_eq!(playlist_library(dir.path(), "alice"), Some(alice.clone()));
        assert_eq!(playlist_library(dir.path(), "bob"), Some(bob));
        // The other settings stay.
        let reloaded = config_with(&vars, &[]);
        assert_eq!(reloaded.volume, Some(40));
        assert!(reloaded.client_id.is_some());

        save_playlist_library(dir.path(), "bob", &Library::default()).unwrap();
        assert_eq!(playlist_library(dir.path(), "bob"), None);
        assert_eq!(playlist_library(dir.path(), "alice"), Some(alice));
    }

    #[test]
    fn an_order_saved_before_folders_existed_is_still_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"playlist_order": {"alice": ["b", "a"]}}"#,
        )
        .unwrap();
        let library = playlist_library(dir.path(), "alice").unwrap();
        assert_eq!(library.playlist_ids(), ["b", "a"]);

        // The save replaces it.
        save_playlist_library(dir.path(), "alice", &library).unwrap();
        let saved = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(!saved.contains("playlist_order"), "{saved}");
        assert_eq!(playlist_library(dir.path(), "alice"), Some(library));
    }

    #[test]
    fn the_resume_point_is_remembered_with_the_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(resume_point(dir.path()), None);

        save_volume(dir.path(), 40).unwrap();
        let point = ResumePoint {
            uri: "spotify:track:a".into(),
            position_ms: 95_000,
        };
        save_resume_point(dir.path(), Some(&point)).unwrap();
        assert_eq!(resume_point(dir.path()), Some(point));

        save_resume_point(dir.path(), None).unwrap();
        assert_eq!(resume_point(dir.path()), None);
        let vars = [("MELODIA_CONFIG_DIR", dir.path().to_str().unwrap())];
        assert_eq!(config_with(&vars, &[]).volume, Some(40));
    }

    #[test]
    fn environment_client_id_wins_over_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let mut config = config_with(&[("MELODIA_CONFIG_DIR", path)], &[]);
        config
            .save_client_id("fedcba9876543210fedcba9876543210")
            .unwrap();

        let config = config_with(
            &[
                ("MELODIA_CONFIG_DIR", path),
                ("SPOTIFY_CLIENT_ID", "0123456789abcdef0123456789abcdef"),
            ],
            &[],
        );
        assert_eq!(
            config.client_id.as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn demo_mode_from_flag_or_environment() {
        assert!(config_with(&[], &["melodia", "--demo"]).demo);
        assert!(config_with(&[("MELODIA_DEMO", "1")], &[]).demo);
        assert!(!config_with(&[("MELODIA_DEMO", "0")], &[]).demo);
    }

    #[test]
    fn dynamic_port_redirect_uri_has_no_port() {
        assert_eq!(redirect_uri_for_port(0), "http://127.0.0.1/callback");
    }

    #[test]
    fn validates_client_ids() {
        assert!(is_valid_client_id("0123456789abcdef0123456789ABCDEF"));
        assert!(is_valid_client_id(" 0123456789abcdef0123456789abcdef\n"));
        assert!(!is_valid_client_id("0123456789abcdef"));
        assert!(!is_valid_client_id("0123456789abcdef0123456789abcdeg"));
    }
}
