# Code map

The folders and the subsystems. The `//!` comment at the start of each file tells its role. Read it
before you change the file. Back to [AGENTS.md](../AGENTS.md).

```
src/
├── main.rs, lib.rs   window and startup; the crate is also a library
├── config.rs, model.rs, update.rs, util.rs, icon.rs
├── spotify/          Web API client, errors and models, rate limit
├── auth/             OAuth with PKCE and the session
├── playback/         pure logic: history, own queue, predictions
├── backend/          all I/O, on a Tokio thread (protocol.rs: Command and Event)
│   ├── live.rs       real backend (Spotify)
│   └── demo.rs       demo backend (fake data)
├── ui/               egui interface
└── webplayer/        «Play in this app»: protocol, SDK page, WebView2 (Windows only)
tests/                integration and UI tests
docker/               Dockerfile, build and smoke-test scripts, landing page nginx config
installer/windows/    NSIS installer
docs/screenshots/     README screenshots
web/                  landing page and update manifest (version.json)
```
