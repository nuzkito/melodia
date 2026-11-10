# Tests, Web API limits, development configuration and packaging

Read this file when you write tests, check a Web API limit, use development variables, or change the
distributed files. Back to [AGENTS.md](../AGENTS.md).

## Tests

| What | Where | How |
| --- | --- | --- |
| Pure logic | `#[cfg(test)] mod tests` in the same file | Compact helpers. In `model.rs` the queue is `"ab\|c\|de"` (previous\|current\|next) |
| Spotify client | `tests/spotify_api.rs` | `wiremock` checks the method, path, parameters and body of each request |
| Real backend | `tests/backend_live.rs` | `wiremock` replaces Spotify. `config(&server, dir)` and `save_session` set up the session. `Timings` at 0.2× (`timings()`). Wait for a timer with `timings().tick * 6 / 5` or `after_follow_ups()`. `Backend::wait_for(predicate)` keeps all events, because their order is not fixed. Use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` |
| Demo backend | `tests/backend_demo.rs` | `demo::run` with `Timings` at 0.1×. To check that a command changed nothing, send `RefreshDevices` and wait for `Event::Devices`. Do not use a sleep |
| Login | `tests/oauth_flow.rs` | The full flow with a simulated browser |
| Interface | `tests/ui.rs` | `egui_kittest` with the real `MusicApp` (1120×900) and no backend. The test sends `Event`s (`fixture.send`) and checks the `Command`s (`fixture.commands()`). It finds elements by label (`click`, `has`, `count`, `center_of`) and drags with mouse events (`drag`, `drop_at`). Start points: `Fixture::logged_in()`, `queue_fixture()`, `with_playlists(…)`, `open_favorites(…)` |
| Full app | `docker/smoke-test.sh` | Demo mode in Xvfb, with clicks at fixed coordinates of the 1120×720 window. Three parts run at the same time (see [The smoke test](#the-smoke-test)) |

- Test names are English sentences that describe the behavior, such as
  `queue_tracks_can_be_reordered_by_dragging`. Test data uses real songs (Queen…) and Spanish names
  (Nuria, Pablo).
- If you change the layout, update the coordinates in `smoke-test.sh`. Then check
  `dist/screenshots/`.
- The smoke test also opens the app with a local server that reports version 99.0.0. That makes the
  `27-update-*` screenshots. A third app makes the `og-*` screenshots, at 1200×630, the Open Graph
  size.
- To update the README screenshots (`docs/screenshots/`) and `web/player.png`, copy them from
  `dist/screenshots/`. `player.png` in both folders comes from `og-artist.png`.

## The smoke test

- `smoke-test.sh` builds the app. Then it runs three parts at the same time: `main` (steps 01 to
  26), `update` (27a and 27b) and `og`. Each part has its own Xvfb display and its own app. The
  script prints the output of each part at the end. To run one part, give its name: `bash
  docker/smoke-test.sh main` (build first).
- `scrot` saves PPM images, because the PNG compression is slow. At the end, all cores convert them
  to PNG.
- The script does not wait a fixed time before a screenshot. It waits until the app is idle
  (`settle`). With `MELODIA_IDLE_REPORT=1`, the app has the egui plugin `ui::IdleReport`. The script
  presses F18 (`XF86Launch9`) as a marker. The app removes the key from its input. When the app is
  idle, it writes `idle <n>` to stdout. This is the only I/O of the interface.
- The app is idle when all these conditions are true:
  - A frame after the marker frame is done. Thus the marker frame is on the screen.
  - No repaint is due in less than 0.5 s. The search waits 0.35 s before its request, and the hearts
    wait 0.2 s.
  - The demo has finished all commands and the tasks that they spawned (`WorkCounter`).
- The cursor of a text field does not blink with the report, because the blink repaints the frame.
- A demo task that sends an event later must hold `EventSink::work_guard()` until it sends the
  event. Otherwise the screenshot can come before the event.
- The live backend does not count its work. Thus the report works only in demo mode.
- Under load, X can lose the marker. After 1 s, the script sends it again and prints a line.
- `shot NAME SECONDS` waits the extra time before it waits for the idle state. Use it only for a
  change that depends on the clock, such as the end of a track. `shot_now NAME SECONDS` does not
  wait for the idle state. It shows work in progress (`23a2`).

## Web API limits

The user guide, [features.md](features.md), describes the limits that the user sees. The limits below
also shape the code:

- There are no change notifications. The player page reports the changes instead. The API cannot stop
  playback or clear Spotify's queue. It can only pause.
- The February 2026 changes are in the models, which accept the new and the old format:
  - `GET /playlists/{id}/items`: each track is in `item`. Before, it was in `track`.
  - `GET /me/playlists`: the track count is in `items`. Before, it was in `tracks`.
  - In development mode, search returns 10 results per request (`SEARCH_PAGE_SIZE`). The app pages
    with `offset`, up to 1000 (`MAX_OFFSET`). The first page asks for `track,album,artist`. Later pages
    ask only for `track`.
  - The album list of an artist refuses a `limit` above 10 («Invalid limit»). Constant:
    `ARTIST_ALBUMS_PAGE_SIZE`.
- The API cannot play or add local files to a playlist.
- `GET /albums` was removed, so the API cannot read several albums in one request. An artist's album
  list has no `album_group`. Thus:
  - `Command::LoadArtist` reads only the list (`include_groups=album,single`, sorted by `album_type`).
  - The tracks of a release come from `GET /albums/{id}`, when the user opens it (`LoadAlbum`), drops
    it on the queue, or selects it in the menu (`QueueAlbum`). `QueueAlbum` works like
    `QueuePlaylist`. Its `QueueAt` gives the position: at the drop, at the end, or `Now`, which plays the
    first track and inserts the others after it.
- The app's own limits: 100 upcoming tracks (`QueueSnapshot::MAX_NEXT`), 500 tracks for each playlist,
  and 50 tracks of history. An artist has no limit. The app reads all pages, 10 at a time.

## Development configuration

These variables are for development. The README describes the user variables in «Configuration».

| Variable | Purpose |
| --- | --- |
| `SPOTIFY_CLIENT_ID` at build time | Embeds the Client ID in the executable (`option_env!`) |
| `MELODIA_CALLBACK_BIND` | The address for the OAuth redirect. The `app` service uses `0.0.0.0`, so Windows can reach it |
| `MELODIA_WEB_PROBE=1` | Opens the player page at start, without a session. The Windows smoke test uses it |
| `MELODIA_NO_ANIMATIONS=1` | Makes all animations instant. `smoke-test.sh` uses it |
| `MELODIA_IDLE_REPORT=1` | Demo only. Tells the smoke test when the app is idle (see [The smoke test](#the-smoke-test)) |
| `MELODIA_UPDATE_URL` at build time | The update manifest built into the executable (`config::BUILT_IN_UPDATE_URL`). Without it, the development server is used (`DEV_UPDATE_URL`). The `build-dev` and `build-release` services set `https://melodia.nuzkito.es/version.json` when it is empty. At run time, the `app` service sets `http://web:80/version.json` |
| `HOST_UID` / `HOST_GID` | The UID and GID of the user in the images |

- `MELODIA_CONFIG_DIR` sets the configuration folder. It holds `settings.json`, `session.json`,
  `cache.json`, `queue.json` and `melodia.log`. The app clears the log when it is over 2 MB. The `app`
  service keeps the folder in the `app-config` volume.
- With `app`, the browser does not open by itself. On the login screen, use «Copy link» and paste it
  into the Windows browser. Port 8898 is published, so the response reaches the container.

## Packaging

- The version is only in `Cargo.toml`. `docker/build-windows.sh` uses it for the file names. If you
  change it, update the file names in the README.
- `web/version.json` is the latest published version, not the version of this build. Installed apps
  compare it with their own version (`update.rs`). Increase it when the new version can be downloaded.
  `"url"` is required. It must be an `https` page on `nuzkito.es` (or a subdomain) or in the GitHub
  repository (`github.com/nuzkito/melodia/…`). The app ignores other addresses
  (`is_trusted_download`). The landing page shows the version on its download button
  (`web/main.js`).
- In development, the `web` service serves the file. nginx reads it on each request, so a restart is
  not needed. The `app` service starts `web` by itself. To see the update notice, raise the version and
  open the app again.
- `build.rs` embeds the icon and the version information in the `.exe`, for Windows builds only.
- The `.exe` may use only Windows DLLs and `WebView2Loader.dll`, which lies next to it.
  `build-windows.sh` checks this.
- When the distributed files change, update the portable zip in `build-windows.sh` and
  `installer/windows/installer.nsi`. In the zip, the `.exe` is `melodia-<version>-portable.exe`.
- To change the icon, edit `assets/icon.svg`. Then run
  `docker compose run --rm dev cargo run --example gen_icon` to rebuild `assets/icon.ico`.
