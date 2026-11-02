//! Spotify login with the OAuth 2.0 Authorization Code flow + PKCE.
//!
//! PKCE lets a desktop app log in without a client secret. The browser goes to a loopback URI
//! (`http://127.0.0.1:PORT/callback`) that [`callback::wait_for_code`] serves. The app exchanges
//! the code for tokens, and [`SessionStore`] keeps them (on Windows, encrypted with DPAPI). Thus
//! the user logs in only one time.

mod callback;
#[cfg(windows)]
mod dpapi;
mod pkce;
mod session;

use std::net::IpAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use url::Url;

pub use callback::{CallbackOutcome, parse_callback, wait_for_code};
pub use pkce::{Pkce, random_token};
pub use session::{
    Session, SessionStore, TokenErrorResponse, TokenManager, TokenResponse, refresh_session,
    unix_now,
};

/// The permissions that the app asks Spotify for. Search needs no scope.
pub const SCOPES: &[&str] = &[
    "user-read-playback-state",
    "user-modify-playback-state",
    "user-read-currently-playing",
    // The playlists panel: read the lists and add tracks to them.
    "playlist-read-private",
    "playlist-read-collaborative",
    "playlist-modify-private",
    "playlist-modify-public",
    // «Play in this app» (Web Playback SDK).
    "streaming",
    "user-read-email",
    "user-read-private",
    // «Liked Songs»: see if a song is in it, and save or remove it.
    "user-library-read",
    "user-library-modify",
];

/// The maximum time to wait for the user to authorize the app in the browser.
pub const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error(
        "could not open port {port} to receive Spotify's response ({message}). Is another copy of the app open?"
    )]
    Bind { port: u16, message: String },
    #[error("you cancelled access in Spotify")]
    Denied,
    #[error("Spotify returned an error: {0}")]
    Provider(String),
    #[error("signing in timed out")]
    Timeout,
    #[error("Spotify rejected the authorization: {0}")]
    Exchange(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub accounts_base: String,
    /// `0` lets the OS choose a free port.
    pub redirect_port: u16,
    pub bind_addr: IpAddr,
}

pub fn authorize_url(
    accounts_base: &str,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> String {
    let mut url = Url::parse(&format!(
        "{}/authorize",
        accounts_base.trim_end_matches('/')
    ))
    .expect("the accounts base URL is valid");
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", code_challenge)
        .append_pair("state", state)
        .append_pair("scope", &SCOPES.join(" "));
    url.into()
}

/// Exchanges the authorization code for an access and a refresh token.
pub async fn exchange_code(
    http: &reqwest::Client,
    accounts_base: &str,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<Session, AuthError> {
    let response = http
        .post(session::token_url(accounts_base))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .map_err(|e| AuthError::Network(e.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| AuthError::Network(e.to_string()))?;

    if !status.is_success() {
        let reason = serde_json::from_str::<TokenErrorResponse>(&body)
            .map(|e| e.error_description.unwrap_or(e.error))
            .unwrap_or_else(|_| format!("HTTP {status}"));
        return Err(AuthError::Exchange(reason));
    }
    let token: TokenResponse =
        serde_json::from_str(&body).map_err(|e| AuthError::Exchange(e.to_string()))?;
    Session::from_token_response(token, None, unix_now())
        .ok_or_else(|| AuthError::Exchange("the response has no refresh token".into()))
}

/// Runs the full login. It listens on the redirect URI, gives the authorization URL to
/// `on_authorize_url` (which opens the browser), waits for the redirect and exchanges the code.
pub async fn login(
    http: &reqwest::Client,
    config: &OAuthConfig,
    on_authorize_url: impl FnOnce(&str),
) -> Result<Session, AuthError> {
    let listener = TcpListener::bind((config.bind_addr, config.redirect_port))
        .await
        .map_err(|e| AuthError::Bind {
            port: config.redirect_port,
            message: e.to_string(),
        })?;
    let port = listener
        .local_addr()
        .map_err(|e| AuthError::Io(e.to_string()))?
        .port();
    let redirect_uri = crate::config::redirect_uri_for_port(port);

    let pkce = Pkce::generate();
    let state = random_token(24);
    on_authorize_url(&authorize_url(
        &config.accounts_base,
        &config.client_id,
        &redirect_uri,
        &pkce.challenge,
        &state,
    ));

    let code = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(listener, state))
        .await
        .map_err(|_| AuthError::Timeout)??;
    exchange_code(
        http,
        &config.accounts_base,
        &config.client_id,
        &code,
        &redirect_uri,
        &pkce.verifier,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_contains_pkce_parameters() {
        let url = authorize_url(
            "https://accounts.spotify.com",
            "client123",
            "http://127.0.0.1:8898/callback",
            "challenge",
            "state42",
        );
        let parsed = Url::parse(&url).unwrap();
        assert_eq!(parsed.path(), "/authorize");
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], "client123");
        assert_eq!(params["response_type"], "code");
        assert_eq!(params["redirect_uri"], "http://127.0.0.1:8898/callback");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["code_challenge"], "challenge");
        assert_eq!(params["state"], "state42");
        assert!(params["scope"].contains("user-modify-playback-state"));
    }
}
