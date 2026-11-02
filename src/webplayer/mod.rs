//! «Play in this app» (Windows): the Web Playback SDK in a hidden WebView2, and the protocol
//! between it and the backend. A WebView2 runs on the thread of the window, and the backend runs
//! on its own thread. Thus they communicate through a [`WebPlayerHost`]. For more information,
//! see `docs/webplayer.md`.

#[cfg(windows)]
mod embedded;

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[cfg(windows)]
pub use embedded::EmbeddedPlayer;

/// The name of the Spotify Connect device that the SDK creates (also in `player.html`).
pub const DEVICE_NAME: &str = "Melodia";

pub const PLAYER_PAGE: &str = include_str!("player.html");

/// Messages from the player page to Melodia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromPage {
    Log {
        message: String,
    },
    /// The SDK needs a (new) access token.
    NeedToken,
    /// The SDK is connected. `device_id` is its Spotify Connect device.
    Ready {
        device_id: String,
    },
    NotReady {
        device_id: String,
    },
    /// `kind` is an SDK error event (`initialization_error`,
    /// `authentication_error`, `account_error`, `playback_error`,
    /// `autoplay_failed`) or a problem of the page or its host (`drm`,
    /// `webview`, `network`, `connect`, `script`).
    Error {
        kind: String,
        message: String,
    },
    /// The playback state of the SDK changed, on this device or (sometimes) on a different device.
    /// [`PageState::here`] tells which.
    State(PageState),
    /// The SDK is not the device that plays now: the music went to a different device
    /// (`player_state_changed` with no state).
    Inactive,
    /// The page did its tasks before the app closes (see `prepareToClose` in `player.html`). The
    /// window can close it now.
    ReadyToClose,
    /// The SDK could not do a [`PageControl`] (`action` and `position` as in
    /// [`PageControl::parse`]): the Web API must do it.
    ControlFailed {
        action: String,
        #[serde(default)]
        position: Option<u64>,
        message: String,
    },
}

/// What the SDK reports in `player_state_changed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageState {
    pub paused: bool,
    pub position: u64,
    pub track_uri: Option<String>,
    /// Missing if the SDK did not tell.
    #[serde(default)]
    pub shuffle: Option<bool>,
    /// The current track in the Web API format (`player.html` converts the SDK format). Thus the
    /// app can show a track that Melodia does not know, without a request to Spotify. Read it with
    /// [`crate::spotify::models::parse_item`].
    #[serde(default)]
    pub track: Option<serde_json::Value>,
    /// The requested track, when Spotify plays a relinked copy of it (a different URI for the same
    /// song).
    #[serde(default)]
    pub linked_from: Option<String>,
    /// What Spotify plays after the current track. The SDK knows only the first two or three
    /// tracks. Missing if the SDK did not tell.
    #[serde(default)]
    pub next_uris: Option<Vec<String>>,
    /// True if the music plays on this SDK (`getCurrentState` has a state only for local
    /// playback), false if it plays on a different device. Missing if the page could not tell.
    #[serde(default)]
    pub here: Option<bool>,
}

/// A player command that the SDK does on the page itself, without a Web API request. This is only
/// possible while the music plays here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageControl {
    Resume,
    Pause,
    /// Makes the volume silent until the music starts to play (a start through the Web API), then
    /// fades it in. It is not a player command: it has no Web API equivalent.
    FadeIn,
    Next,
    Previous,
    /// To this position, in ms.
    Seek(u64),
}

impl PageControl {
    /// The call to `window.melodia.control` that does it.
    pub fn script(self) -> String {
        match self {
            Self::Seek(position) => format!("window.melodia.control(\"seek\", {position})"),
            other => format!("window.melodia.control(\"{}\")", other.action()),
        }
    }

    /// Its name in `player.html`.
    pub fn action(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Pause => "pause",
            Self::FadeIn => "fade_in",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Seek(_) => "seek",
        }
    }

    /// The control named `action` (with `position` for a seek).
    pub fn parse(action: &str, position: Option<u64>) -> Option<Self> {
        Some(match action {
            "resume" => Self::Resume,
            "pause" => Self::Pause,
            "fade_in" => Self::FadeIn,
            "next" => Self::Next,
            "previous" => Self::Previous,
            "seek" => Self::Seek(position?),
            _ => return None,
        })
    }
}

/// An action of the player page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    Message(FromPage),
    /// The page is gone (the host could not create it, or it closed).
    Closed,
}

/// The destination of the page's events, tagged with the generation of the [`HostRequest::Start`]
/// that created it. Thus the backend can ignore a previous page.
pub type PlayerEvents = mpsc::UnboundedSender<(u64, PlayerEvent)>;

/// The requests of the backend to the window that hosts the page. The window handles them in
/// order.
#[derive(Debug)]
pub enum HostRequest {
    /// Closes the current page, if there is one, and opens a new page with the SDK at `volume`
    /// percent (the SDK always starts at the volume of its creation).
    Start {
        generation: u64,
        volume: Option<u8>,
        events: PlayerEvents,
    },
    /// An access token for the current page.
    Token(String),
    /// Sets the volume of the SDK (percent) on the page itself, without a Web API request. While
    /// the music plays here, this is sufficient.
    Volume(u8),
    /// A player command for the SDK (see [`PageControl`]).
    Control(PageControl),
    Stop,
}

/// The backend's end of the connection with the page's host.
#[derive(Clone)]
pub struct WebPlayerHost {
    inner: Arc<HostInner>,
}

struct HostInner {
    requests: mpsc::UnboundedSender<HostRequest>,
    /// The host looks at its requests only when it runs: wake it up.
    wake: Box<dyn Fn() + Send + Sync>,
}

impl WebPlayerHost {
    /// A host and the requests that it receives. The function calls `wake` after each request (to
    /// make the window run its loop).
    pub fn new(
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (Self, mpsc::UnboundedReceiver<HostRequest>) {
        let (requests, receiver) = mpsc::unbounded_channel();
        let inner = HostInner {
            requests,
            wake: Box::new(wake),
        };
        (
            Self {
                inner: Arc::new(inner),
            },
            receiver,
        )
    }

    /// Sends `request`. Returns `false` if the host is gone.
    pub fn send(&self, request: HostRequest) -> bool {
        let sent = self.inner.requests.send(request).is_ok();
        (self.inner.wake)();
        sent
    }
}

impl fmt::Debug for WebPlayerHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WebPlayerHost")
    }
}

impl PartialEq for WebPlayerHost {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

/// The query string of the page for an SDK that starts at `volume` percent.
pub fn page_query(volume: Option<u8>) -> String {
    volume.map_or_else(String::new, |v| format!("?volume={}", v.min(100)))
}

/// True if `url` is the player page (`https://melodia.localhost/`, see `embedded.rs`). The main
/// frame must not go to a different page, because the host gives the access token to the page
/// that shows.
pub fn is_page_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("melodia.localhost")
            && url.port().is_none()
    })
}

/// Reads a message that the page posted (`window.ipc.postMessage`).
pub fn parse_page_message(body: &str) -> Result<FromPage, serde_json::Error> {
    serde_json::from_str(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_messages_use_the_page_format() {
        let ready = parse_page_message(r#"{"type":"ready","device_id":"abc"}"#).unwrap();
        assert_eq!(
            ready,
            FromPage::Ready {
                device_id: "abc".into()
            }
        );
        let state = parse_page_message(
            r#"{"type":"state","paused":false,"position":1200,"track_uri":"spotify:track:1"}"#,
        )
        .unwrap();
        assert!(matches!(
            state,
            FromPage::State(PageState {
                paused: false,
                position: 1200,
                next_uris: None,
                ..
            })
        ));
        let full = parse_page_message(
            r#"{"type":"state","paused":true,"position":0,"track_uri":"spotify:track:1",
                "track":{"type":"track","uri":"spotify:track:1","name":"Under Pressure"},
                "linked_from":null,"next_uris":["spotify:track:2"],"here":true}"#,
        )
        .unwrap();
        let FromPage::State(full) = full else {
            panic!("{full:?}")
        };
        assert_eq!(full.next_uris, Some(vec!["spotify:track:2".to_owned()]));
        assert_eq!(full.here, Some(true));
        let track = crate::spotify::models::parse_item(full.track.unwrap()).unwrap();
        assert_eq!(track.name, "Under Pressure");
        assert_eq!(
            parse_page_message(
                r#"{"type":"control_failed","action":"seek","position":5000,"message":"no"}"#
            )
            .unwrap(),
            FromPage::ControlFailed {
                action: "seek".into(),
                position: Some(5000),
                message: "no".into()
            }
        );
        assert_eq!(
            parse_page_message(r#"{"type":"need_token"}"#).unwrap(),
            FromPage::NeedToken
        );
        assert_eq!(
            parse_page_message(r#"{"type":"ready_to_close"}"#).unwrap(),
            FromPage::ReadyToClose
        );
        assert_eq!(
            parse_page_message(r#"{"type":"inactive"}"#).unwrap(),
            FromPage::Inactive
        );
        assert!(parse_page_message("not json").is_err());
    }

    #[test]
    fn only_the_player_page_counts_as_the_page() {
        assert!(is_page_url("https://melodia.localhost/"));
        assert!(is_page_url("https://melodia.localhost/?volume=35"));
        for url in [
            "http://melodia.localhost/",
            "https://melodia.localhost:8080/",
            "https://melodia.localhost.example.com/",
            "https://example.com/?https://melodia.localhost/",
            "https://sdk.scdn.co/embedded/index.html",
            "about:blank",
            "",
        ] {
            assert!(!is_page_url(url), "{url}");
        }
    }

    #[test]
    fn the_page_starts_at_the_saved_volume() {
        assert_eq!(page_query(Some(35)), "?volume=35");
        assert_eq!(page_query(Some(140)), "?volume=100");
        assert_eq!(page_query(None), "");
        assert!(PLAYER_PAGE.contains("URLSearchParams(location.search)"));
    }

    #[test]
    fn page_controls_call_the_page_and_are_read_back() {
        assert_eq!(
            PageControl::Pause.script(),
            r#"window.melodia.control("pause")"#
        );
        assert_eq!(
            PageControl::Seek(5000).script(),
            r#"window.melodia.control("seek", 5000)"#
        );
        for control in [
            PageControl::Resume,
            PageControl::Pause,
            PageControl::FadeIn,
            PageControl::Next,
            PageControl::Previous,
            PageControl::Seek(42),
        ] {
            assert_eq!(
                PageControl::parse(control.action(), Some(42)),
                Some(control)
            );
            assert!(PLAYER_PAGE.contains(&format!("{}: () =>", control.action())));
        }
        assert_eq!(PageControl::parse("seek", None), None);
        assert_eq!(PageControl::parse("shuffle", None), None);
    }

    #[test]
    fn requests_reach_the_host_and_wake_it() {
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = woken.clone();
        let (host, mut requests) = WebPlayerHost::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        assert!(host.send(HostRequest::Token("t".into())));
        assert!(matches!(requests.try_recv(), Ok(HostRequest::Token(t)) if t == "t"));
        assert_eq!(woken.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(host, host.clone());

        drop(requests);
        assert!(!host.send(HostRequest::Stop));
    }

    #[test]
    fn page_sends_every_message_the_app_understands() {
        for needle in [
            "name: \"Melodia\",",
            "type: \"need_token\"",
            "type: \"ready\"",
            "type: \"not_ready\"",
            "type: \"error\"",
            "type: \"state\"",
            "type: \"inactive\"",
            "type: \"log\"",
            "onSpotifyWebPlaybackSDKReady",
            "https://sdk.scdn.co/spotify-player.js",
            "window.melodia",
            "setVolume(percent)",
            "gainOf(percent)",
            "control(action, position)",
            "pauseWithFade(player)",
            "resumeWithFade(player)",
            "type: \"control_failed\"",
            "next_uris:",
            "linked_from:",
            "here,",
            "getCurrentState()",
        ] {
            assert!(PLAYER_PAGE.contains(needle), "player.html lacks {needle}");
        }
        assert!(PLAYER_PAGE.contains(DEVICE_NAME));
    }
}
