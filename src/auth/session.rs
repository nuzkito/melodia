//! Persistent session (OAuth tokens) and automatic refresh of the access token.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::write_private_file;
use crate::spotify::ApiError;

/// Refresh the access token this number of seconds before it expires.
const REFRESH_MARGIN_SECS: u64 = 60;

/// The tokens of a logged-in user, stored on the disk to keep the session between runs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    /// The Unix time (seconds) when the access token expires.
    pub expires_at: u64,
    #[serde(default)]
    pub scope: String,
}

impl fmt::Debug for Session {
    // Never print tokens, also not in debug logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Builds a session from a token endpoint response. On a refresh, Spotify can omit the refresh
    /// token. Then the previous refresh token stays valid.
    pub fn from_token_response(
        response: TokenResponse,
        previous_refresh_token: Option<&str>,
        now: u64,
    ) -> Option<Self> {
        let refresh_token = response
            .refresh_token
            .filter(|token| !token.is_empty())
            .or_else(|| previous_refresh_token.map(str::to_owned))?;
        Some(Self {
            access_token: response.access_token,
            refresh_token,
            expires_at: now.saturating_add(response.expires_in),
            scope: response.scope,
        })
    }

    pub fn needs_refresh(&self, now: u64) -> bool {
        now + REFRESH_MARGIN_SECS >= self.expires_at
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The successful response of `POST /api/token`.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

fn default_expires_in() -> u64 {
    3600
}

/// The error response of `POST /api/token`.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenErrorResponse {
    pub error: String,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// The session file on Windows: the session, encrypted with DPAPI, in base64.
#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct ProtectedSession {
    protected: String,
}

#[cfg(windows)]
fn encode(session: &Session) -> io::Result<Vec<u8>> {
    use base64::Engine as _;

    let json = serde_json::to_vec(session).map_err(io::Error::other)?;
    let protected = super::dpapi::protect(&json)?;
    let file = ProtectedSession {
        protected: base64::engine::general_purpose::STANDARD.encode(protected),
    };
    serde_json::to_vec_pretty(&file).map_err(io::Error::other)
}

#[cfg(windows)]
fn decode(bytes: &[u8]) -> Result<Session, String> {
    use base64::Engine as _;

    let file: ProtectedSession = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let protected = base64::engine::general_purpose::STANDARD
        .decode(file.protected)
        .map_err(|e| e.to_string())?;
    let json = super::dpapi::unprotect(&protected).map_err(|e| e.to_string())?;
    serde_json::from_slice(&json).map_err(|e| e.to_string())
}

/// On other systems, the session file is plain JSON that only the user can read (see
/// `write_private_file`).
#[cfg(not(windows))]
fn encode(session: &Session) -> io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(session).map_err(io::Error::other)
}

#[cfg(not(windows))]
fn decode(bytes: &[u8]) -> Result<Session, String> {
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

/// Reads and writes the session file.
#[derive(Debug, Clone)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Option<Session> {
        let bytes = std::fs::read(&self.path).ok()?;
        match decode(&bytes) {
            Ok(session) => Some(session),
            Err(error) => {
                log::warn!("Ignoring unreadable session file: {error}");
                None
            }
        }
    }

    pub fn save(&self, session: &Session) -> io::Result<()> {
        write_private_file(&self.path, &encode(session)?)
    }

    pub fn clear(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// Owns the current session and gives valid access tokens. When they are about to expire, it
/// refreshes them (one time, also with concurrent callers).
pub struct TokenManager {
    http: reqwest::Client,
    token_url: String,
    client_id: String,
    store: Option<SessionStore>,
    session: tokio::sync::Mutex<Session>,
}

impl TokenManager {
    pub fn new(
        http: reqwest::Client,
        accounts_base: &str,
        client_id: impl Into<String>,
        session: Session,
        store: Option<SessionStore>,
    ) -> Self {
        Self {
            http,
            token_url: token_url(accounts_base),
            client_id: client_id.into(),
            store,
            session: tokio::sync::Mutex::new(session),
        }
    }

    pub async fn access_token(&self) -> Result<String, ApiError> {
        let mut session = self.session.lock().await;
        if session.needs_refresh(unix_now()) {
            let mut refreshed = refresh_session(
                &self.http,
                &self.token_url,
                &self.client_id,
                &session.refresh_token,
            )
            .await?;
            // A refresh keeps the permissions of the session, also if it omits them.
            if refreshed.scope.is_empty() {
                refreshed.scope.clone_from(&session.scope);
            }
            if let Some(store) = &self.store
                && let Err(error) = store.save(&refreshed)
            {
                log::warn!("Could not save the refreshed session: {error}");
            }
            *session = refreshed;
        }
        Ok(session.access_token.clone())
    }

    /// Marks `rejected_token` as expired, so that the next call refreshes it.
    pub async fn invalidate(&self, rejected_token: &str) {
        let mut session = self.session.lock().await;
        if session.access_token == rejected_token {
            session.expires_at = 0;
        }
    }

    pub async fn session(&self) -> Session {
        self.session.lock().await.clone()
    }
}

pub(crate) fn token_url(accounts_base: &str) -> String {
    format!("{}/api/token", accounts_base.trim_end_matches('/'))
}

/// Exchanges a refresh token for a new access token.
pub async fn refresh_session(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<Session, ApiError> {
    let response = http
        .post(token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;

    if status.is_success() {
        let token: TokenResponse = serde_json::from_str(&body)?;
        return Session::from_token_response(token, Some(refresh_token), unix_now())
            .ok_or_else(|| ApiError::Decode("the response has no refresh token".into()));
    }
    if matches!(status, StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED) {
        // invalid_grant (revoked or expired refresh token), invalid_client… There is nothing to
        // try again: the user must log in again.
        if let Ok(error) = serde_json::from_str::<TokenErrorResponse>(&body) {
            log::warn!(
                "Refreshing the session failed: {} ({})",
                error.error,
                error.error_description.unwrap_or_default()
            );
        }
        return Err(ApiError::SessionExpired);
    }
    Err(ApiError::from_response(status.as_u16(), &body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_response(refresh: Option<&str>) -> TokenResponse {
        TokenResponse {
            access_token: "access".into(),
            scope: "user-read-playback-state".into(),
            expires_in: 3600,
            refresh_token: refresh.map(str::to_owned),
        }
    }

    #[test]
    fn session_keeps_previous_refresh_token_when_missing() {
        let session =
            Session::from_token_response(token_response(None), Some("old"), 1_000).unwrap();
        assert_eq!(session.refresh_token, "old");
        assert_eq!(session.expires_at, 4_600);

        let session =
            Session::from_token_response(token_response(Some("new")), Some("old"), 1_000).unwrap();
        assert_eq!(session.refresh_token, "new");

        assert!(Session::from_token_response(token_response(None), None, 1_000).is_none());
    }

    #[test]
    fn an_absurd_lifetime_does_not_overflow() {
        let response = TokenResponse {
            expires_in: u64::MAX,
            ..token_response(Some("r"))
        };
        let session = Session::from_token_response(response, None, 1_000).unwrap();
        assert_eq!(session.expires_at, u64::MAX);
        assert!(!session.needs_refresh(1_000));
    }

    #[test]
    fn refresh_is_needed_shortly_before_expiry() {
        let session = Session::from_token_response(token_response(Some("r")), None, 0).unwrap();
        assert!(!session.needs_refresh(3_000));
        assert!(session.needs_refresh(3_541));
        assert!(session.needs_refresh(10_000));
    }

    #[test]
    fn debug_output_hides_tokens() {
        let session =
            Session::from_token_response(token_response(Some("secret")), None, 0).unwrap();
        let debug = format!("{session:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("access"));
    }

    #[test]
    fn store_roundtrip_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("nested").join("session.json"));
        assert!(store.load().is_none());

        let session = Session::from_token_response(token_response(Some("r")), None, 42).unwrap();
        store.save(&session).unwrap();
        assert_eq!(store.load(), Some(session));

        store.clear().unwrap();
        assert!(store.load().is_none());
        store.clear().unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn the_session_file_does_not_show_the_tokens_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let session =
            Session::from_token_response(token_response(Some("secret")), None, 42).unwrap();
        let store = SessionStore::new(&path);
        store.save(&session).unwrap();

        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("secret"), "{saved}");
        assert!(!saved.contains("access"), "{saved}");
        assert_eq!(store.load(), Some(session));
    }

    #[test]
    fn a_session_file_that_cannot_be_decrypted_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, br#"{"protected": "bm90IGEgRFBBUEkgYmxvYg=="}"#).unwrap();
        assert!(SessionStore::new(path).load().is_none());
    }

    #[test]
    fn corrupted_session_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(SessionStore::new(path).load().is_none());
    }
}
