# AGENTS.md

Rules for coding agents that work on Melodia, a Spotify desktop player in Rust (egui). The final
product is a Windows app. The [README](README.md) tells how the app must behave. The details of
each subsystem are in [docs/](docs/) (see [More docs](#more-docs)).

## Rules

- **Nothing runs on the host.** Run cargo, tests, linters, scripts and the app only in the containers
  of `compose.yml`. On the host, read files and use the Docker CLI only.
- **A change is complete only when `test` and `lint` pass.** If you change `#[cfg(windows)]` code
  (`src/webplayer/embedded.rs`, parts of `src/main.rs`), also run clippy for Windows.
- **If you change the layout or the interactions**, run `smoke`. If the layout changes, update the
  coordinates in `docker/smoke-test.sh`. Examine `dist/screenshots/`. In the final message, link the
  screenshots that show the change (for example `dist/screenshots/03-queue.png`), with a short note
  for each. If no screenshot shows the change, say so, and consider a new step in `smoke-test.sh`.
- **Language.** All app text is in English: interface, notices, errors (`#[error(...)]`,
  `ApiError::user_message`), the OAuth page and the demo data. Code, comments, logs, test names and
  docs are also in English. In the interface, quotes are “”. In comments, quote interface texts with
  «» (for example «Play in this app»).
- **Docs.** If a change affects what the user sees, update the README (a short summary for people).
  If the architecture or a convention changes, update this file or the matching file in
  [docs/](docs/).

## Commands

From an agent, always use `-T` and `-e CARGO_TERM_COLOR=never`:

```sh
docker compose run --rm -T -e CARGO_TERM_COLOR=never test    # all tests (cargo nextest)
docker compose run --rm -T -e CARGO_TERM_COLOR=never lint    # fmt --check and clippy -D warnings
docker compose run --rm -T smoke                              # demo app on Xvfb → dist/screenshots/
docker compose run --rm -T dev cargo fmt --all                # format the code
# One test file, filtered by name
docker compose run --rm -T -e CARGO_TERM_COLOR=never test cargo nextest run --locked --test ui volume
# Clippy for Windows (checks the #[cfg(windows)] code)
docker compose run --rm -T -e CARGO_TERM_COLOR=never build-dev cargo clippy --locked --target x86_64-pc-windows-gnu -- -D warnings
```

The other services (Windows build, Wine, landing page, browsers) and the Docker details are in
[commands.md](docs/commands.md).

## Architecture

- `src/ui/` (egui) is the interface. `src/backend/` does all I/O on a Tokio thread. They communicate
  only through messages: the interface sends `Command`s and receives `Event`s
  (`backend/protocol.rs`). The interface never does I/O and never blocks. The only exception is the
  idle report of the smoke test ([testing-and-dev.md](docs/testing-and-dev.md#the-smoke-test)).
- **Two backends, one protocol**: `live.rs` (Spotify) and `demo.rs` (fake data, for `--demo`, the
  smoke test and the screenshots). The `match`es on `Command` are exhaustive. Each new feature must
  also work in the demo.
- **The app never polls.** Apart from Spotify, it asks only the update server, once at start.
- **Optimistic edits.** The interface applies each `QueueEdit` and `PlaylistEdit` at once, with the
  same pure function as the backend (`apply`, in `model.rs`).
- **The app's own queue** is the most delicate part. Read [queue.md](docs/queue.md) before you
  change it.
- Almost all files start with a `//!` comment that tells their role. Read it before you change the
  file. The folders and subsystems are in [code-map.md](docs/code-map.md). A new file needs a `//!`
  comment. Add it to the code map only for a new folder or subsystem.

## Conventions

- Rust, egui/eframe, Tokio, reqwest with rustls. `Cargo.lock` is versioned. All commands use
  `--locked`.
- `rustfmt` with defaults. Clippy with `-D warnings` and the pedantic lints of `Cargo.toml`. To
  silence a lint, use `#[expect(…)]`, never `#[allow(…)]`.
- Do not `panic` on Spotify data. Use tolerant parsing and classified errors (`ApiError`).
- Use `thiserror` for errors. Error messages start in lowercase, because other sentences include
  them. Use `capitalize` when a message shows alone.
- Lock `std::sync::Mutex`es with `util::lock`. Write configuration files with
  `config::write_private_file`.
- Getters lend their data (slices). A caller that needs a copy makes it.
- Interface: use the existing widgets (`theme.rs`, `widgets.rs`, `track_row.rs`, `detail.rs`) before
  you make new ones. Animate only with `ui/anim.rs`. Each interactive element needs an English
  accessible label, because the UI tests find elements by it.
- Test names are English sentences that describe the behavior. Test data uses real songs (Queen…)
  and Spanish names (Nuria, Pablo).

## Comments and docs

- A comment tells what the code cannot tell: the reason (often a Spotify limitation), or what the
  code must not break. Each new file needs a `//!` comment that tells its role.
- Keep comments short: a `//!` header in a few lines, a doc comment in one to four lines. Put longer
  explanations in [docs/](docs/), and refer to them («see `docs/queue.md`»).
- No noise: no comments that repeat the name, describe the next line, or describe a change ("now…",
  "fixed…").
- When behavior changes, update or delete the comments that describe it. `grep` the old name in
  `src/`, `tests/`, `docs/`, `docker/`, `player.html` and `frame_fade.js`. An incorrect comment is
  worse than no comment.
- Write comments and docs in Simplified Technical English (ASD-STE100): at most 20 words per
  sentence, one topic per sentence, present tense, active voice, no contractions or idioms, one term
  for one thing. Use "must" for a requirement and "can" for a possibility.
- Wrap comments and docs at 100 columns.
- Before a change is complete, read the comments in the diff again with these rules.

## More docs

Read the file for the code that you change.

| File | Read it when you |
| --- | --- |
| [new-feature.md](docs/new-feature.md) | add a feature: the steps from the Web API to the interface |
| [backend.md](docs/backend.md) | change `backend/`, `spotify/` or `auth/`: messages, requests, other devices, errors, 429, cache, scopes |
| [queue.md](docs/queue.md) | change `playback/`, the queue in `live.rs`, `QueueEdit` or `PlaylistEdit` |
| [playlists-liked.md](docs/playlists-liked.md) | change the playlist features or the hearts (`ui/liked.rs`) |
| [webplayer.md](docs/webplayer.md) | change `webplayer/`, playback control, volume or the reports of the player page |
| [interface.md](docs/interface.md) | change `ui/`: layout, keyboard, accessibility |
| [design.md](docs/design.md) | change the look: colors, typography, spacing, components, animation timings |
| [testing-and-dev.md](docs/testing-and-dev.md) | write tests, check a Web API limit, use development variables, or change the distributed files |
| [commands.md](docs/commands.md) | need a Docker service that is not above, or the containers fail |
| [code-map.md](docs/code-map.md) | look for the file that does something |
