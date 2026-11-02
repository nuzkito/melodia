//! The player page in a WebView2, in a hidden window of the app.

use std::borrow::Cow;
use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE,
    VK_MEDIA_PREV_TRACK,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, SW_HIDE,
    SW_SHOWNOACTIVATE, ShowWindow, WM_CLOSE, WM_HOTKEY, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use wry::http::{Request, Response, StatusCode, header};
use wry::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use wry::{
    NewWindowResponse, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows as _,
    WebViewId,
};

use super::{
    FromPage, HostRequest, PLAYER_PAGE, PlayerEvent, PlayerEvents, is_page_url, page_query,
};

/// A custom protocol serves the page. On Windows, `melodia://localhost/` becomes
/// `https://melodia.localhost/` (a secure context, which protected media needs).
const PAGE_URL: &str = "melodia://localhost/";

/// The defaults of wry, plus these: media can start without a click, and a hidden page must not
/// be throttled or suspended. Also, audio plays in the browser process (a direct child of the app),
/// not in a separate audio service process. Thus capture tools that follow the process tree of the
/// app (OBS) can find it. The media keys are not the keys of Chromium (see [`MEDIA_KEYS`]).
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion,AudioServiceOutOfProcess,HardwareMediaKeyHandling \
    --autoplay-policy=no-user-gesture-required \
    --disable-background-timer-throttling \
    --disable-renderer-backgrounding \
    --disable-backgrounding-occluded-windows";

/// Makes the frame of the SDK fade when a different device pauses it (`frame_fade.js`).
const FRAME_FADE: &str = include_str!("frame_fade.js");

/// The media keys that the app takes (`RegisterHotKey`, so they work with the focus on any
/// window), as `(hotkey id, virtual key, action of `window.melodia.mediaKey`)`. [`BROWSER_ARGS`]
/// turns off the handling of these keys in Chromium, because the SDK would pause and play without
/// the fade.
const MEDIA_KEYS: [(i32, u16, &str); 3] = [
    (1, VK_MEDIA_PLAY_PAUSE, "play_pause"),
    (2, VK_MEDIA_NEXT_TRACK, "next"),
    (3, VK_MEDIA_PREV_TRACK, "previous"),
];

/// The media keys that the user pushed and that the page did not get yet: one bit for each entry
/// of [`MEDIA_KEYS`]. The procedure of the window sets them, and `pump` reads them.
static PRESSED_KEYS: AtomicU8 = AtomicU8::new(0);

/// Wakes the interface when the user pushes a media key, so that `pump` runs.
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Hosts the player page for the backend. It runs in the UI thread: call
/// [`EmbeddedPlayer::pump`] in each frame.
pub struct EmbeddedPlayer {
    requests: mpsc::UnboundedReceiver<HostRequest>,
    context: WebContext,
    /// `MELODIA_WEB_DEBUG=1`: shows the page and its DevTools.
    debug: bool,
    page: Option<Page>,
    /// Set when the page tells that the window can close it (`FromPage::ReadyToClose`).
    ready_to_close: Arc<AtomicBool>,
}

struct Page {
    /// Dropped before its window.
    webview: WebView,
    _window: HostWindow,
    events: PlayerEvents,
    generation: u64,
}

impl EmbeddedPlayer {
    /// Keeps the WebView2 profile (cache, cookies) in `data_dir`.
    pub fn new(
        requests: mpsc::UnboundedReceiver<HostRequest>,
        data_dir: PathBuf,
        wake_on_media_key: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let _ = WAKE.set(Box::new(wake_on_media_key));
        Self {
            requests,
            context: WebContext::new(Some(data_dir)),
            debug: std::env::var("MELODIA_WEB_DEBUG").is_ok_and(|v| v == "1"),
            page: None,
            ready_to_close: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The app is about to close. Asks the page to pause the music if it plays here, so that
    /// Spotify gets the position. Returns `false` if there is no page to wait for. If there is a
    /// page, wait for [`Self::ready_to_close`].
    pub fn prepare_to_close(&mut self) -> bool {
        let Some(page) = &self.page else {
            return false;
        };
        self.ready_to_close.store(false, Ordering::SeqCst);
        match page
            .webview
            .evaluate_script("window.melodia.prepareToClose()")
        {
            Ok(()) => true,
            Err(error) => {
                log::warn!("[web player] Could not prepare the page to close: {error}");
                false
            }
        }
    }

    /// True if the page did its tasks before it closes.
    pub fn ready_to_close(&self) -> bool {
        self.ready_to_close.load(Ordering::SeqCst)
    }

    /// Handles the backend's requests. Call it from the thread of the app window. Its message loop
    /// also runs the window of the page.
    pub fn pump(&mut self) {
        let pressed = PRESSED_KEYS.swap(0, Ordering::SeqCst);
        for (index, (_, _, action)) in MEDIA_KEYS.iter().enumerate() {
            if pressed & (1 << index) != 0 {
                self.run_script(
                    &format!("window.melodia.mediaKey(\"{action}\")"),
                    "Could not pass a media key to the player",
                );
            }
        }
        while let Ok(request) = self.requests.try_recv() {
            match request {
                HostRequest::Start {
                    generation,
                    volume,
                    events,
                } => {
                    self.close();
                    self.open(generation, volume, events);
                }
                HostRequest::Token(token) => self.deliver_token(&token),
                HostRequest::Volume(percent) => self.set_volume(percent),
                HostRequest::Control(control) => {
                    self.run_script(&control.script(), "Could not control the player");
                }
                HostRequest::Stop => self.close(),
            }
        }
    }

    pub fn close(&mut self) {
        if let Some(page) = self.page.take() {
            drop(page.webview);
            let _ = page.events.send((page.generation, PlayerEvent::Closed));
        }
    }

    fn open(&mut self, generation: u64, volume: Option<u8>, events: PlayerEvents) {
        let window = match HostWindow::new(self.debug) {
            Ok(window) => window,
            Err(error) => {
                report_open_failure(
                    &events,
                    generation,
                    format!("Could not create the window: {error}"),
                );
                return;
            }
        };
        let ipc_events = events.clone();
        let ready_to_close = self.ready_to_close.clone();
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_custom_protocol("melodia".into(), serve_page)
            .with_https_scheme(true)
            .with_additional_browser_args(BROWSER_ARGS)
            .with_initialization_script_for_main_only(FRAME_FADE, false)
            // The main frame stays on the player page, and the page opens no windows. Only the
            // main frame triggers the navigation handler: the iframes of the SDK can navigate.
            .with_navigation_handler(|url| {
                let allowed = is_page_url(&url);
                if !allowed {
                    log::warn!("[web player] Blocked a navigation away from the player page");
                }
                allowed
            })
            .with_new_window_req_handler(|_, _| {
                log::warn!("[web player] Blocked a new window");
                NewWindowResponse::Deny
            })
            .with_ipc_handler(move |request| {
                forward_page_message(&ipc_events, &ready_to_close, generation, request);
            })
            .with_devtools(self.debug)
            // Never take the keyboard from the app.
            .with_focused(false)
            .with_background_color((11, 11, 14, 255))
            .with_url(format!("{PAGE_URL}{}", page_query(volume)))
            .build(&window);
        match webview {
            Ok(webview) => {
                if self.debug {
                    webview.open_devtools();
                }
                log::info!("[web player] WebView2 started");
                self.page = Some(Page {
                    webview,
                    _window: window,
                    events,
                    generation,
                });
            }
            Err(error) => report_open_failure(
                &events,
                generation,
                format!("Could not start WebView2 (is the runtime installed?): {error}"),
            ),
        }
    }

    fn deliver_token(&self, token: &str) {
        let literal = serde_json::to_string(token).unwrap_or_default();
        self.run_script(
            &format!("window.melodia.deliverToken({literal})"),
            "Could not deliver the token",
        );
    }

    fn set_volume(&self, percent: u8) {
        self.run_script(
            &format!("window.melodia.setVolume({})", percent.min(100)),
            "Could not set the volume",
        );
    }

    /// Runs `script` on the current page, if there is one. If it fails, it reports a `script`
    /// error that starts with `what`.
    fn run_script(&self, script: &str, what: &str) {
        let Some(page) = &self.page else {
            return;
        };
        if let Err(error) = page.webview.evaluate_script(script) {
            let error = FromPage::Error {
                kind: "script".into(),
                message: format!("{what}: {error}"),
            };
            let _ = page
                .events
                .send((page.generation, PlayerEvent::Message(error)));
        }
    }
}

impl Drop for EmbeddedPlayer {
    fn drop(&mut self) {
        self.close();
    }
}

/// A hidden top-level window of this thread that holds the page, as a separate browser window
/// would. The page in it stays "visible". A WebView2 that is hidden itself counts as a page in
/// the background, and Chromium pauses its media (the music stops after a few seconds).
struct HostWindow {
    hwnd: HWND,
}

impl HostWindow {
    fn new(shown: bool) -> std::io::Result<Self> {
        let class = to_wide_nul("MelodiaWebPlayer");
        let title = to_wide_nul("Melodia · web player");
        // SAFETY: plain Win32 calls with valid, NUL-terminated strings that live longer than the
        // calls. The class is registered one time (later calls fail without effect with
        // ERROR_CLASS_ALREADY_EXISTS).
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let window_class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(host_window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            RegisterClassExW(&window_class);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                640,
                480,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            if shown {
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            for (id, key, _) in MEDIA_KEYS {
                if RegisterHotKey(hwnd, id, MOD_NOREPEAT, u32::from(key)) == 0 {
                    log::warn!(
                        "[web player] Could not take a media key: {}",
                        std::io::Error::last_os_error()
                    );
                }
            }
            Ok(Self { hwnd })
        }
    }
}

impl HasWindowHandle for HostWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.hwnd as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: the window lives as long as `self`.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        // SAFETY: this thread created the window, and nothing destroyed it yet.
        unsafe {
            for (id, _, _) in MEDIA_KEYS {
                UnregisterHotKey(self.hwnd, id);
            }
            DestroyWindow(self.hwnd);
        }
    }
}

/// When the user closes the window (it shows only with `MELODIA_WEB_DEBUG=1`), it becomes hidden.
/// The music continues.
unsafe extern "system" fn host_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Windows calls it with a valid window of this class.
    unsafe {
        if message == WM_HOTKEY {
            let id = i32::try_from(wparam).unwrap_or(0);
            if let Some(index) = MEDIA_KEYS.iter().position(|(key_id, _, _)| *key_id == id) {
                PRESSED_KEYS.fetch_or(1 << index, Ordering::SeqCst);
                if let Some(wake) = WAKE.get() {
                    wake();
                }
            }
            return 0;
        }
        if message == WM_CLOSE {
            ShowWindow(hwnd, SW_HIDE);
            return 0;
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

fn to_wide_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn serve_page(_: WebViewId<'_>, request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let (status, content_type, body) = match request.uri().path() {
        "/" | "/index.html" => (StatusCode::OK, "text/html; charset=utf-8", PLAYER_PAGE),
        _ => (
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "Not found",
        ),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Cow::Borrowed(body.as_bytes()))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

/// Gives the messages of the page to the backend. Only the app's page can talk to Melodia, not the
/// iframes of the SDK.
fn forward_page_message(
    events: &PlayerEvents,
    ready_to_close: &AtomicBool,
    generation: u64,
    request: Request<String>,
) {
    if !is_page_url(&request.uri().to_string()) {
        return;
    }
    match super::parse_page_message(request.body()) {
        Ok(message) => {
            if message == FromPage::ReadyToClose {
                ready_to_close.store(true, Ordering::SeqCst);
            }
            let _ = events.send((generation, PlayerEvent::Message(message)));
        }
        Err(error) => log::warn!(
            "[web player] Unexpected message ({error}): {}",
            request.body()
        ),
    }
}

fn report_open_failure(events: &PlayerEvents, generation: u64, message: String) {
    let error = FromPage::Error {
        kind: "webview".into(),
        message,
    };
    let _ = events.send((generation, PlayerEvent::Message(error)));
    let _ = events.send((generation, PlayerEvent::Closed));
}
