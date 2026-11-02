#!/usr/bin/env bash
# Smoke test of the Windows build: runs dist/melodia.exe in demo mode with Wine on a virtual
# display and saves screenshots in dist/screenshots/. First, build the executable with
# `docker compose run --rm build-dev` or `build-release`.
#
# Wine is not Windows: it only shows that the executable starts and renders. (The DirectX 12
# emulation of Wine fails here, so the test also checks the automatic change to OpenGL.)
set -euo pipefail

EXE=dist/melodia.exe
OUT=dist/screenshots
if [[ ! -f "$EXE" ]]; then
    echo "Missing $EXE: run 'docker compose run --rm windows' first" >&2
    exit 1
fi
mkdir -p "$OUT"

export DISPLAY=:98 WINEDEBUG=-all WINEPREFIX="$HOME/.wine"
Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
XVFB_PID=$!
cleanup() {
    wineserver -k 2>/dev/null || true
    kill "$XVFB_PID" 2>/dev/null || true
}
trap cleanup EXIT
sleep 1

echo "Preparing Wine…"
wineboot --init >/dev/null 2>&1 || true
# The app imports the WebView2 loader: it must be next to the app.
cp "$EXE" dist/WebView2Loader.dll "$HOME/"
# MELODIA_WEB_PROBE: also opens the hidden player page («Play in this app») immediately.
MELODIA_DEMO=1 MELODIA_WEB_PROBE=1 wine "$HOME/melodia.exe" >/dev/null 2>&1 &

find_window() { xdotool search --name '^Melodia' 2>/dev/null | tail -n1 || true; }
WINDOW=""
for _ in $(seq 1 120); do
    WINDOW="$(find_window)"
    [[ -n "$WINDOW" ]] && break
    sleep 0.5
done
if [[ -z "$WINDOW" ]]; then
    echo "The window did not appear" >&2
    exit 1
fi
# If the GPU renderer fails, the app starts again with OpenGL: wait, and look for the window again.
sleep 8
WINDOW="$(find_window)"
eval "$(xdotool getwindowgeometry --shell "$WINDOW")"
scrot --overwrite "$OUT/windows-01-login.png"

# «Enter the demo» (see ui/login.rs layout).
xdotool mousemove $((X + WIDTH / 2)) $((Y + HEIGHT / 2 + 19)) click 1
sleep 6
scrot --overwrite "$OUT/windows-02-now-playing.png"
echo "  saved $OUT/windows-01-login.png and $OUT/windows-02-now-playing.png"

LOG="$(find "$WINEPREFIX/drive_c/users" -path '*Melodia*' -name 'melodia.log' | head -n1)"
[[ -n "$LOG" ]] && grep -E "starting|restarting|Renderer:|ERROR|web probe" "$LOG" | cut -c1-200 || true

if ! pgrep -f melodia.exe >/dev/null; then
    echo "The app is not running" >&2
    exit 1
fi

# «Play in this app»: the page must report its state, and the app must not crash. Wine has no
# WebView2 runtime, so the expected answer is a clean "webview" error (on Windows, the page
# loads).
if ! grep -q "\[web probe\].*webview" "${LOG:-/dev/null}"; then
    echo "The in-app web player did not report its state" >&2
    exit 1
fi

echo "Windows smoke test passed"
