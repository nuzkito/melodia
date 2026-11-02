use std::time::Duration;

use super::models::ApiErrorEnvelope;

/// The errors that the Spotify client returns, already classified for the UI.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ApiError {
    #[error("there is no active Spotify device")]
    NoActiveDevice,
    #[error("Spotify Premium is required")]
    PremiumRequired,
    #[error("too many requests; wait {} s", .0.as_secs())]
    RateLimited(Duration),
    #[error("the Spotify session has expired")]
    SessionExpired,
    #[error("Spotify rejected the access token")]
    Unauthorized,
    /// Apps in development mode accept only the accounts in the "User Management" section of the
    /// Spotify dashboard.
    #[error("the account isn't authorized in the Spotify app")]
    UserNotRegistered,
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response from Spotify: {0}")]
    Decode(String),
    #[error("Spotify responded {status}: {message}")]
    Status {
        status: u16,
        message: String,
        reason: Option<String>,
    },
}

impl ApiError {
    /// Classifies a response that did not succeed, from its status and JSON body.
    pub fn from_response(status: u16, body: &str) -> Self {
        let detail = serde_json::from_str::<ApiErrorEnvelope>(body)
            .ok()
            .map(|envelope| envelope.error);
        let reason = detail.as_ref().and_then(|d| d.reason.clone());
        let message = detail
            .map(|d| d.message)
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| body.chars().take(200).collect());
        let lowercase = message.to_lowercase();

        match (status, reason.as_deref()) {
            (404, Some("NO_ACTIVE_DEVICE")) => Self::NoActiveDevice,
            (404, _) if lowercase.contains("no active device") => Self::NoActiveDevice,
            (403, Some("PREMIUM_REQUIRED")) => Self::PremiumRequired,
            (403, _) if lowercase.contains("premium") => Self::PremiumRequired,
            (403, _)
                if lowercase.contains("not be registered")
                    || lowercase.contains("not registered") =>
            {
                Self::UserNotRegistered
            }
            (401, _) => Self::Unauthorized,
            _ => Self::Status {
                status,
                message,
                reason,
            },
        }
    }

    /// The session cannot be used: the user must (repair the app and) log in again.
    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired | Self::Unauthorized | Self::UserNotRegistered
        )
    }

    /// A short message for notifications.
    pub fn user_message(&self) -> String {
        match self {
            Self::NoActiveDevice => {
                "No Spotify device is active. Choose “Play in this app” in the device selector."
                    .to_owned()
            }
            Self::PremiumRequired => {
                "Playing music requires a Spotify Premium account.".to_owned()
            }
            Self::RateLimited(wait) => format!(
                "Spotify is limiting requests. Try again in {} s.",
                wait.as_secs().max(1)
            ),
            Self::SessionExpired | Self::Unauthorized => {
                "The session has expired. Reconnect your Spotify account.".to_owned()
            }
            Self::UserNotRegistered => "Your account isn't authorized in the Spotify app. Add it at developer.spotify.com › your app › User Management and reconnect.".to_owned(),
            Self::Network(_) => "Can't connect to Spotify. Check your connection.".to_owned(),
            Self::Decode(_) => "Spotify sent an unexpected response.".to_owned(),
            Self::Status {
                status: 403,
                message,
                ..
            } => with_reason("Spotify doesn't allow this action right now", message),
            Self::Status {
                status: 404,
                message,
                ..
            } => with_reason("Spotify couldn't find the item", message),
            Self::Status { status, .. } if *status >= 500 => {
                "Spotify isn't responding right now. Try again in a moment.".to_owned()
            }
            Self::Status { message, .. } => format!("Spotify: {message}"),
        }
    }
}

/// `summary`, with Spotify's own explanation when it tells more than the status code (for example
/// «Restriction violated» on a 403).
fn with_reason(summary: &str, message: &str) -> String {
    match spotify_reason(message) {
        Some(reason) => format!("{summary}: {reason}."),
        None => format!("{summary}."),
    }
}

/// The useful part of an error message from Spotify, if there is one. The function ignores bodies
/// that are not Spotify's JSON (an HTML error page…) and messages that only repeat the status.
fn spotify_reason(message: &str) -> Option<&str> {
    const ONLY_THE_STATUS: [&str; 4] = ["not found", "resource not found", "forbidden", "unknown"];
    let reason = message.trim();
    // Player errors repeat this prefix before the real reason.
    let reason = reason
        .strip_prefix("Player command failed:")
        .unwrap_or(reason)
        .trim()
        .trim_end_matches('.')
        .trim();
    let looks_like_markup = reason.starts_with('<') || reason.starts_with('{');
    let says_nothing = ONLY_THE_STATUS.contains(&reason.to_lowercase().as_str());
    (!reason.is_empty() && !looks_like_markup && !says_nothing).then_some(reason)
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_decode() {
            Self::Decode(error.to_string())
        } else {
            Self::Network(error.to_string())
        }
    }
}

/// JSON that does not have the expected shape.
impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::Decode(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_player_errors_by_reason() {
        let body = r#"{"error":{"status":404,"message":"Player command failed: No active device found","reason":"NO_ACTIVE_DEVICE"}}"#;
        assert_eq!(ApiError::from_response(404, body), ApiError::NoActiveDevice);

        let body = r#"{"error":{"status":403,"message":"Player command failed: Premium required","reason":"PREMIUM_REQUIRED"}}"#;
        assert_eq!(
            ApiError::from_response(403, body),
            ApiError::PremiumRequired
        );
    }

    #[test]
    fn detects_accounts_missing_from_the_dashboard() {
        let body = r#"{"error":{"status":403,"message":"Check settings on developer.spotify.com/dashboard, the user may not be registered."}}"#;
        let error = ApiError::from_response(403, body);
        assert_eq!(error, ApiError::UserNotRegistered);
        assert!(error.is_auth_failure());
        assert!(error.user_message().contains("User Management"));
    }

    #[test]
    fn keeps_unknown_errors_with_their_message() {
        let body = r#"{"error":{"status":403,"message":"Player command failed: Restriction violated","reason":"UNKNOWN"}}"#;
        let error = ApiError::from_response(403, body);
        assert_eq!(
            error,
            ApiError::Status {
                status: 403,
                message: "Player command failed: Restriction violated".into(),
                reason: Some("UNKNOWN".into()),
            }
        );
        assert_eq!(
            error.user_message(),
            "Spotify doesn't allow this action right now: Restriction violated."
        );
    }

    #[test]
    fn not_found_errors_show_spotify_s_reason() {
        let body = r#"{"error":{"status":404,"message":"Invalid playlist Id"}}"#;
        assert_eq!(
            ApiError::from_response(404, body).user_message(),
            "Spotify couldn't find the item: Invalid playlist Id."
        );
    }

    #[test]
    fn reasons_that_add_nothing_are_left_out() {
        let generic = r#"{"error":{"status":404,"message":"Resource not found"}}"#;
        assert_eq!(
            ApiError::from_response(404, generic).user_message(),
            "Spotify couldn't find the item."
        );
        let html = ApiError::from_response(403, "<html>Forbidden</html>");
        assert_eq!(
            html.user_message(),
            "Spotify doesn't allow this action right now."
        );
        assert_eq!(
            ApiError::from_response(404, "").user_message(),
            "Spotify couldn't find the item."
        );
    }

    #[test]
    fn handles_non_json_bodies() {
        let error = ApiError::from_response(502, "<html>Bad gateway</html>");
        assert!(matches!(error, ApiError::Status { status: 502, .. }));
        assert!(error.user_message().contains("isn't responding"));
        assert_eq!(ApiError::from_response(401, ""), ApiError::Unauthorized);
        assert!(ApiError::Unauthorized.is_auth_failure());
    }
}
