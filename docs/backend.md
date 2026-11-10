# Backend architecture

This file describes how the interface and the backend communicate, and how the real backend
(`backend/live.rs`) works. Read it before you change `backend/`, `spotify/` or `auth/`. Back to
[AGENTS.md](../AGENTS.md).

## Messages

- `backend::spawn` starts a thread with a Tokio runtime. The interface sends `Command` values and
  receives `Event` values (`backend/protocol.rs`). The interface never does I/O and never blocks.
- The protocol has two backends: `live.rs` (Spotify) and `demo.rs` (fake data). The `match`
  statements on `Command` must cover all variants. Thus a new command does not compile until both
  backends handle it. Each new feature must also work in the demo.
- A demo task that sends an event after its command ends must hold `EventSink::work_guard()`. The
  smoke test waits for this work (see [testing-and-dev.md](testing-and-dev.md#the-smoke-test)).
- Once at start, 2 s after `spawn`, a task checks for a new version (`backend/updates.rs`). It
  sends `Event::UpdateAvailable` if the manifest has a newer version. Neither `live::run` nor
  `demo::run` starts this check. Their tests never run it.
- Each frame, `MusicApp::show` (`ui/app.rs`) processes the events and draws the views. The views
  put their commands in `actions`. `dispatch` applies the results that the interface can apply at
  once, and then it sends each command.
- On exit, `MusicApp::shut_down` closes the command channel. It waits up to 3 s (`SHUTDOWN_WAIT`)
  for the backend thread to end. Thus the backend writes its data before the process ends.

### Cover art

- Views call `TextureCache::get(url, px, time)`. The cache requests missing images with
  `Command::LoadImage`. The backend answers with `Event::ImageLoaded` or `Event::ImageFailed`.
- All images use `THUMB_PX` (96). Each image has an accent color. The app loads no large cover.

## Real backend (`backend/live.rs`)

### State task

- One task owns the state. It handles commands, login and playlist results, web player messages,
  the requests for what plays, the queue check and a 5 s tick. The tick saves the position. It
  sends no request.
- Thus two commands never run at the same time. Slow or independent work runs in separate tasks,
  and those tasks send their results back.

### Timings

- `Timings` (`backend/timings.rs`) holds the waits of the backend. Put a new delay in `Timings`,
  not in a `const`.
- `tests/backend_live.rs` runs the timings at 0.2× (`Timings::scaled`). `tests/backend_demo.rs`
  runs the demo timings at 0.1×.

### Requests to Spotify

- The Web API sends no change notifications, and the backend never polls. While the music plays
  here, the player page reports each change. See [webplayer.md](webplayer.md).
- The backend sends `GET /me/player` one request at a time (`fetch_state_at`). It sends one when
  the session starts, when the page says that the music is not here, after a move to this app, and
  when a prediction expires without a confirmation. It retries a failed request up to 3 times
  (`FETCH_RETRIES`).
- The backend reads `GET /me/player/devices` only when the device picker is open, every 10 s, and
  after a transfer. It also reads the list when no device is active.
- A request that starts the music goes to the player of this app, if that player is ready
  (`web_device_to_start`). Melodia never starts music on a different device.
- After a transfer that Spotify accepts, the backend shows the new device for 3 s
  (`transfer_hold`). Spotify shows the change late, so the backend corrects older answers in that
  time.

### Music on a different device

- When the music plays on a different device, `go_elsewhere` runs. It removes the queue and the
  history, hides the track, and stops all reads.
- In that state, the backend ignores `Resume`, `Pause`, `Next`, `Previous` and `Seek`. It only saves
  the volume. A command that starts a track sends it to the player of this app, and that ends the
  state.
- A page report with `here: true` calls `come_back_here`. The backend then reads Spotify's queue
  again.
- Without the player of this app (outside Windows, or if it failed), all music plays on a different
  device.

### Predictions, errors and 429

- **Predictions**: each command shows its expected result at once (`playback::predict`). The
  backend ignores states that still show the old value until Spotify or the page confirms the
  result, or 2.5 s pass (`Expectation`).
- **Errors**: `ApiError` arrives classified. `report()` shows it as a notice. If the error is an
  authentication failure, the backend logs the user out. On a 401 response, `SpotifyClient::send`
  refreshes the token and sends the request once more.
- **429** (`spotify/rate_limit.rs`): after a 429, `SpotifyClient` sends no request until the wait
  ends. Each request during the wait fails at once with `ApiError::RateLimited`. The wait is the
  longer of `Retry-After` and an exponential backoff. The backoff starts at 2 s, doubles for each
  consecutive 429, and stops at 120 s. A successful response resets it. Nothing retries a 429
  automatically.

### Cache

- **Cache** (`response_cache.rs`): the backend reuses successful answers and does not ask Spotify
  again:
  - searches, for 24 h. The cache keeps the full JSON, and the parser reads it each time.
  - albums and artists, for 24 h
  - playlist tracks, for 10 min
- The cached tracks of a playlist count only if the backend read them after the last edit of that
  playlist, and the playlist has the same `total` and `snapshot`.
- The cache is saved to `cache.json`, for the Spotify account. It is saved every 30 s
  (`CACHE_SAVE_EVERY`), before a login or logout, and when the backend stops. The backend loads it
  after a login, only if the file belongs to that account.
- If you change the saved types so that old files cannot load, increment `FORMAT`.

### Scopes

- The scopes are in `auth::SCOPES`. Old sessions can lack new scopes. The backend checks this
  (`has_streaming_scope`, `has_library_scopes`), and the interface then offers «Reconnect».
- To add a scope, add it to `SCOPES`. Handle the missing scope in the same way, without breaking
  existing sessions.
