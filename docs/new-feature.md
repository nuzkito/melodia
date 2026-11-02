# How to add a feature

This file gives the steps to add a feature, from the Web API to the interface. Back to
[AGENTS.md](../AGENTS.md).

1. Read how related features work in the README. Find the limits of the Web API. The limits are in
   [testing-and-dev.md](testing-and-dev.md), «Web API limits».
2. Put the types in `model.rs`. They must not depend on Spotify's JSON. Put the pure logic in
   `model.rs` or in `playback/`. Add unit tests.
3. For a new endpoint, add a method in `spotify/client.rs`. Add tolerant models in
   `spotify/models.rs`. Use `#[serde(default)]`, so the models skip the data that they do not
   understand. Add the test in `tests/spotify_api.rs`.
4. Add a `Command` or an `Event` variant in `backend/protocol.rs`. Add a comment that tells what the
   variant means.
5. Change `live.rs` and `demo.rs`. The demo uses fake data. In `live.rs`, put slow work in a
   separate task. That task sends its result back to the task that owns the state. Add tests in
   `tests/backend_live.rs`. If the feature applies, add tests in `tests/backend_demo.rs` too.
6. In the interface, put the state in its view and the commands in `actions`. If the result is
   clear, add the optimistic update in `MusicApp::dispatch`. Add accessible labels. Use `anim` for
   the animations. Add tests in `tests/ui.rs`.
7. If a value must stay between sessions, add a field in `Settings` (`config.rs`). Use
   `#[serde(default, skip_serializing_if = …)]`, so that existing `settings.json` files still
   load. Write the field with `update_settings`. That function is atomic and locked.
8. If the feature needs a new Spotify scope, add the scope to `auth::SCOPES`. Then tell the old
   sessions. See «Scopes» in [backend.md](backend.md).
