# «Play in this app» (`webplayer/`, Windows only)

Read this file before you change `webplayer/`. Also read it before you change the backend code that
controls the music in this app: volume, pause and resume, and the reports of the page. Back to
[AGENTS.md](../AGENTS.md).

## Host and protocol

- The Web Playback SDK runs in `player.html`. The page runs in a WebView2 (crate `wry`), in a hidden
  window of the same process.
- WebView2 must run on the window thread. Thus `eframe::App::logic` (`main.rs`) calls
  `EmbeddedPlayer::pump()` each frame. WebView2 runs the page in its own processes, so a page crash
  does not stop the app.
- The backend and the page communicate through `WebPlayerHost`:
  - `HostRequest` values go to the page: `Start`, `Token`, `Volume`, `Control` and `Stop`.
  - The page sends `PlayerEvent` values back as `FromPage` JSON messages
    (`window.ipc.postMessage`).
  - `generation` lets the backend ignore messages from earlier pages.
  - `DEVICE_NAME` must match the device name in `player.html`.
- The host gives the access token to the page that shows. Thus the main frame must stay on the
  player page (`is_page_url`). The navigation handler blocks all other addresses, and the page can
  open no windows. The iframes of the SDK can navigate. The host accepts IPC messages only from
  the player page.
- Outside Windows there is no host. The state is `Unavailable`. The tests use `fake_host`
  (`tests/backend_live.rs`), which has the same protocol.

## Close and resume position

- If the page stops without a message, Spotify keeps an older position. So `App::logic` does this
  when the user closes the window:
  1. It cancels the close.
  2. It calls `EmbeddedPlayer::prepare_to_close`. If the music plays here, the page pauses it, so
     the SDK reports the position. The page then answers `ready_to_close`.
  3. It closes the window when the page answers, or after `CLOSE_GRACE` (3 s).
- As a fallback, the backend saves the position in `settings.json` (`ResumePoint`) every 5 s and at
  each pause or seek. The fallback covers a missing network or a killed app.
- At start, after `adopt_quietly`, the app seeks to the saved position if the last music was in this
  app and the track is the same (`go_back_to_saved_position`).

## Reports of the page

- Each `FromPage::State` says whether the music plays here (`PageState::here`).
- The backend uses only reports of music that plays here (`on_page_state`). It treats them as
  Spotify reports for the web player device. A report without `here` is trusted only while the
  music already plays here (`page_reports_playback`).
- The SDK sometimes reports what another device plays. That is not documented and not reliable, so
  the backend never uses it.
- A report with `here: false` or `inactive` makes the backend ask Spotify where the music went
  (`music_not_here`). It does not ask when the music just moved here (`moving_here`, 5 s), because
  the reports can be older than the move.
- A report confirms the last command and the place of the music. No request is then needed, unless
  the page describes a track that Melodia does not know (`known_track`).
- Each report contains the current track in the Web API format (`parse_item` reads it) and the URIs
  of the next tracks (`next_uris`, kept in `page_next`).
- While the app shows Spotify's queue, `follow_queue_from_page` compares the next tracks with that
  queue. If they match, nothing is read. If not, the queue is read once for that mismatch
  (`page_next_mismatch`). With the app's own queue, `check_queue` trusts the page more than
  `SpotifyPlan`.

## Volume

- The SDK sets the volume on the page (`HostRequest::Volume`, `player.setVolume`). The Web API is not
  used.
- The SDK volume is a linear gain. The page converts the percentage to `(p/100)⁴` (`gainOf`,
  `VOLUME_CURVE`), so the sound matches Spotify's apps. The rest of the code uses 0 to 100.
- The volume task still saves the value.
- For the web player device, the UI always shows the volume that Melodia set (`hold_volume`).
- While the page is open, the volume goes only to the page. It goes to Spotify only when there is no
  page, and never while the music plays on a different device.

## Playback control and fades

- The SDK does resume, pause, next, previous and seek for the music that plays here
  (`control_on_page`). The report of the page confirms each command. If the SDK fails, the page
  answers `control_failed`, and the command goes through the Web API (`control_through_api`).
- Only resume and pause fade, over `FADE_MS` (150 ms). A new fade cancels the one in progress.
- The app gets the media keys. Chromium does not, because `BROWSER_ARGS` disables
  `HardwareMediaKeyHandling`. `RegisterHotKey` (`MEDIA_KEYS` in `embedded.rs`) sets a bit in
  `PRESSED_KEYS`. `pump` passes the key to `window.melodia.mediaKey`, and play and pause use the
  fading `togglePlay`.
- The page replaces the SDK's `pause`, `resume` and `togglePlay` with fading versions
  (`fadeEveryPlayAndPause`). A resume that the page did not ask for, such as from a phone, fades in
  (`fadeInExternalResume`).
- A pause or play from another device reaches the SDK iframe directly. `frame_fade.js`, an
  initialization script for each frame, fades the media elements out on `pause()`. It cancels the
  pause on `play()`. `load()` and the end of a track stop the sound at once. The script does not use
  the SDK code. If it stops fading, check that the SDK plays through media elements.
- Before a resume that sends the full list, the backend sends `PageControl::FadeIn` (`armFadeIn`).
  The sound stays silent until the music plays, then it fades in. The silence ends after 5 s.

## Debugging

- `MELODIA_WEB_DEBUG=1` shows the window with DevTools.
- `MELODIA_WEB_PROBE=1` opens the page at start, without a session. `windows-smoke` uses it.
- The log records the page messages as `[web player]` lines.
