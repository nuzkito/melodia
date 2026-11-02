# syntax=docker/dockerfile:1
#
# Images to build, test and package Melodia without installations on the host. The image does
# not include the source code: compose.yml mounts it at /app, and the cargo/target caches are in
# named volumes.

ARG RUST_VERSION=1.99
ARG DEBIAN_RELEASE=trixie

# ---------------------------------------------------------------------------
# base: Rust toolchain + unprivileged user with the same UID/GID as the host, so that the
# generated files (Cargo.lock, dist/) belong to you.
# ---------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-${DEBIAN_RELEASE} AS base
ARG UID=1000
ARG GID=1000
ENV CARGO_HOME=/home/dev/.cargo \
    CARGO_TERM_COLOR=always
RUN rustup component add rustfmt clippy \
 && groupadd --gid "${GID}" dev \
 && useradd --uid "${UID}" --gid "${GID}" --create-home --shell /bin/bash dev \
 && mkdir -p /app/target /app/dist "${CARGO_HOME}/registry" "${CARGO_HOME}/git" \
 && chown -R dev:dev /app "${CARGO_HOME}"
WORKDIR /app

# ---------------------------------------------------------------------------
# dev: tests, lint and the Linux version of the app. It includes the libraries that winit/glutin
# load at runtime (X11, Wayland, software OpenGL with Mesa), and Xvfb for the graphical smoke
# test.
# ---------------------------------------------------------------------------
FROM base AS dev
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      libx11-6 libx11-xcb1 libxcb1 libxcursor1 libxrandr2 libxi6 \
      libxkbcommon0 libxkbcommon-x11-0 \
      libwayland-client0 libwayland-cursor0 libwayland-egl1 \
      libgl1 libegl1 libgl1-mesa-dri libglx-mesa0 libegl-mesa0 \
      xvfb xdotool scrot fonts-inter \
 && rm -rf /var/lib/apt/lists/*
# cargo-nextest runs the tests of all binaries at the same time (cargo test runs one binary after
# the other). Thus `test` takes as long as the slowest test.
ARG NEXTEST_VERSION=0.9.150
RUN cargo install --locked --version "${NEXTEST_VERSION}" --root /usr/local cargo-nextest
# cargo-outdated lists the dependencies that have a newer version (`cargo outdated`).
ARG OUTDATED_VERSION=0.19.0
RUN cargo install --locked --version "${OUTDATED_VERSION}" --root /usr/local cargo-outdated
USER dev
CMD ["cargo", "nextest", "run", "--locked", "--all-targets"]

# ---------------------------------------------------------------------------
# windows: cross-compilation to x86_64-pc-windows-gnu with MinGW-w64, and the build of the
# installer with NSIS.
# ---------------------------------------------------------------------------
FROM base AS windows
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 nsis zip \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-pc-windows-gnu
USER dev
CMD ["bash", "docker/build-windows.sh"]

# ---------------------------------------------------------------------------
# wine: runs the Windows .exe with Wine on a virtual display, as a smoke
# test of the Windows build (optional).
# ---------------------------------------------------------------------------
FROM debian:trixie-slim AS wine
ARG UID=1000
ARG GID=1000
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      wine wine64 xvfb xdotool scrot procps \
      libgl1-mesa-dri libglx-mesa0 mesa-vulkan-drivers libvulkan1 \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid "${GID}" dev \
 && useradd --uid "${UID}" --gid "${GID}" --create-home --shell /bin/bash dev
USER dev
WORKDIR /app
CMD ["bash", "docker/windows-smoke-test.sh"]

# ---------------------------------------------------------------------------
# browser: Chromium and Firefox driven by Puppeteer, so that agents can check the landing page
# (web/) in real browsers. Only the `browser` service in compose.yml uses it. That service gets
# to the page at http://web:80 on the Compose network (docker/web-screenshots.js takes its
# screenshots).
# ---------------------------------------------------------------------------
FROM node:26-trixie-slim AS browser
ARG UID=1000
ARG GID=1000
RUN apt-get update \
 && apt-get install -y --no-install-recommends chromium firefox-esr fonts-liberation \
 && rm -rf /var/lib/apt/lists/* \
 && npm install -g puppeteer-core@25.16.0 \
 && npm cache clean --force \
 # The `node` user of the image already has UID/GID 1000: it gives them to `dev`.
 && userdel --remove node \
 && (groupdel node 2>/dev/null || true) \
 && groupadd --gid "${GID}" dev \
 && useradd --uid "${UID}" --gid "${GID}" --create-home --shell /bin/bash dev
ENV NODE_PATH=/usr/local/lib/node_modules \
    CHROME_PATH=/usr/bin/chromium \
    FIREFOX_PATH=/usr/bin/firefox-esr
USER dev
WORKDIR /app
# It stays up, so that scripts can run with `docker compose exec`.
CMD ["sleep", "infinity"]
