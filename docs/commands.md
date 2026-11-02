# Docker commands and environment

This file describes all Docker services and the development environment. Read this file when you
need a service that is not in the short list in [AGENTS.md](../AGENTS.md). Read it also when the
containers do not work as expected. Back to [AGENTS.md](../AGENTS.md).

## Services

The images come from `docker/Dockerfile`. It has these stages: `dev`, `windows`, `wine` and
`browser`. The code is mounted at `/app`.

| Command | What it does |
| --- | --- |
| `docker compose run --rm test` | Runs `cargo nextest run --locked --all-targets`: all tests. It lists only failures and slow tests. The settings are in `.config/nextest.toml` |
| `docker compose run --rm lint` | Runs `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings` |
| `docker compose run --rm smoke` | Starts the app in demo mode on Xvfb. It drives the app with `xdotool` and saves the screenshots to `dist/screenshots/` |
| `docker compose run --rm build-dev` | Quick build for `x86_64-pc-windows-gnu` with the `fast` profile: release without LTO, incremental. It makes only the `.exe` and `WebView2Loader.dll`. A change rebuilds in seconds, not in about 2 minutes. Use this service also for clippy on the Windows code |
| `docker compose run --rm build-release` | Optimized build for `x86_64-pc-windows-gnu`. It puts the `.exe`, `WebView2Loader.dll`, the installer, the zip and `SHA256SUMS.txt` in `dist/`. Use it to publish |
| `docker compose run --rm windows-smoke` | Starts `dist/melodia.exe` with Wine and saves the screenshots. Run `build-dev` or `build-release` first |
| `docker compose run --rm --service-ports app` | Runs the Linux version on the Windows desktop through WSLg, with the `fast` profile. To use demo mode, set `MELODIA_DEMO=1` before the command |
| `docker compose run --rm dev` | Starts an interactive shell with the toolchain |
| `docker compose up -d web` | Serves the landing page (`web/`) on `http://127.0.0.1:8897`. The page is read-only, so an edit shows after a reload. The service also serves `web/version.json`, the update manifest for development. The `app` service starts it by itself |
| `docker compose up -d browser` | Starts Chromium and Firefox with Puppeteer. They open the page at `http://web:80`. `docker compose exec -T browser node docker/web-screenshots.js` checks the page and saves `dist/screenshots/web-*.png` |

The `Makefile` has shortcuts: `make build-dev`, `make build-release`, `make smoke`, `make test`,
`make lint`, `make demo` and `make web`. Run `make` alone to list them.

## More examples

```sh
# The unit tests of one module
docker compose run --rm -T -e CARGO_TERM_COLOR=never test cargo nextest run --locked --lib playback::queue
# Add a dependency (all commands use --locked, so Cargo.lock must be up to date)
docker compose run --rm -T dev cargo add <crate>
```

## Environment

- The cargo and build caches are in Docker volumes (`target-linux` and `target-windows`). Thus the
  `target/` folder of the host stays empty. The first build takes several minutes.
- The containers use the UID and GID of the host. Thus the files that they create, such as
  `Cargo.lock` and `dist/`, belong to the user. If the UID or GID is not 1000, export `HOST_UID`
  and `HOST_GID`. Then rebuild the images with `docker compose build`.
- If you change the `Dockerfile`, rebuild the affected image.
- The development variables (`MELODIA_*`) are in [testing-and-dev.md](testing-and-dev.md).
