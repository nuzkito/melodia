#!/usr/bin/env bash
# GUI smoke test: runs the app in demo mode on a virtual X display (Xvfb with software
# rendering), drives it with xdotool and saves screenshots in dist/screenshots/. It fails if the
# app crashes or does not show its window.
set -euo pipefail

OUT="${OUT:-dist/screenshots}"
SCREEN="${SCREEN:-1280x800}"
# The pause before each screenshot. Animations are off (MELODIA_NO_ANIMATIONS), so it must only
# cover a few frames. Longer waits below are for the simulated latencies of the demo (login
# 0.7 s, search 0.35 s, a 0.5 s playback tick).
WAIT="${WAIT:-0.3}"
mkdir -p "$OUT"
rm -f "$OUT"/*.png

cargo build --locked --quiet
BIN=target/debug/melodia

export DISPLAY=:99
Xvfb "$DISPLAY" -screen 0 "${SCREEN}x24" -nolisten tcp >/dev/null 2>&1 &
XVFB_PID=$!
APP_PID=""
UPDATES_PID=""
cleanup() {
    [[ -n "$APP_PID" ]] && kill "$APP_PID" 2>/dev/null || true
    [[ -n "$UPDATES_PID" ]] && kill "$UPDATES_PID" 2>/dev/null || true
    kill "$XVFB_PID" 2>/dev/null || true
}
trap cleanup EXIT
for _ in $(seq 1 50); do xdotool getmouselocation >/dev/null 2>&1 && break; sleep 0.1; done

# Starts the app in demo mode, with a new configuration, and waits for its window. Extra
# `VAR=value` arguments go to its environment. Here, nothing listens on the default update URL.
# Thus the app shows no new version, if no argument is MELODIA_UPDATE_URL.
launch() {
    env MELODIA_DEMO=1 MELODIA_NO_ANIMATIONS=1 MELODIA_CONFIG_DIR="$(mktemp -d)" \
        RUST_LOG="${RUST_LOG:-warn}" "$@" "$BIN" &
    APP_PID=$!
    WINDOW=""
    for _ in $(seq 1 60); do
        WINDOW="$(xdotool search --name '^Melodia' 2>/dev/null | head -n1 || true)"
        [[ -n "$WINDOW" ]] && break
        sleep 0.5
    done
    if [[ -z "$WINDOW" ]]; then
        echo "The window did not appear" >&2
        exit 1
    fi
    # There is no window manager: give the keyboard focus to the window explicitly.
    xdotool windowfocus --sync "$WINDOW" || true
    # The window geometry, to click relative to it.
    eval "$(xdotool getwindowgeometry --shell "$WINDOW")"
}

launch

alive() {
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        echo "The app exited unexpectedly" >&2
        exit 1
    fi
}

click() { xdotool mousemove $((X + $1)) $((Y + $2)) click 1; }
move() { xdotool mousemove $((X + $1)) $((Y + $2)); }
# Drags from ($1,$2) to ($3,$4) and does not release the button.
drag() {
    xdotool mousemove $((X + $1)) $((Y + $2)) mousedown 1
    for step in 1 2 3 4 5 6 7 8 9 10; do
        sleep 0.04
        move $(($1 + ($3 - $1) * step / 10)) $(($2 + ($4 - $2) * step / 10))
    done
}
release() { xdotool mouseup 1; }
shot() {
    alive
    sleep "${2:-$WAIT}"
    scrot --overwrite --autoselect "$X,$Y,$WIDTH,$HEIGHT" "$OUT/$1.png"
    echo "  saved $OUT/$1.png"
}

echo "Window ${WIDTH}x${HEIGHT} at ${X},${Y}"
# The pointer starts at the center of the screen, over the login button. Move it away, so that
# the first screenshot shows the button at rest.
move 40 $((HEIGHT - 40))
shot 01-login 1

# The coordinates below are for the default 1120x720 window (see the layout constants in
# src/ui/*.rs). The playlists use x=32..243. The player column (search and its results) has its
# center at x=446. The queue starts at x=657. The content starts at y=38, just under the window
# buttons. When the track changes, the queue scrolls to put the card of the current track at its
# top (y=130..334). Its seek bar is at y=232 (x=667..1078), and its controls are at y=287 (◀◀
# 687, ▶ 743, ▶▶ 799, volume 871..1078). In the title bar, left of the window buttons
# (x=982..1120), the three-dot menu is at x=959 and the device picker is at x=913.

# Login button: on hover, it only becomes wider (the text does not move).
move $((WIDTH / 2)) $((HEIGHT / 2 + 19))
shot 01a-login-button-hover
click $((WIDTH / 2)) $((HEIGHT / 2 + 19))
shot 02-now-playing 1.5

# Next track (in the card). The history starts empty. Thus, until the track changes, the card is
# at the top of the list (y=88..285, controls at y=245).
click 799 245
shot 03-next-track 0.5

# On hover, a queued track shows its ▶, ♥ and ✕ buttons on the right. The upcoming tracks are
# under the card, with their centers at y=364, 432, 500…
move 870 364
shot 04-queue-track-hover

# Drag the first upcoming track two positions down: a slot opens where it goes.
drag 870 364 870 502
shot 05-queue-reorder-drag
release
shot 06-queue-reordered

# The album's name in the card («Rust», after "Amber Moon · ", under the title) is underlined on
# hover. A click opens the album in the middle column, with its year and its songs. Escape closes
# it.
move 855 190
shot 06a-card-album-hover
click 855 190
shot 06b-album-opened
xdotool key Escape

# The artist's name in the card («Amber Moon», before " · Rust") opens the artist in the
# middle column: each album and single with its songs. Escape closes it.
move 800 190
shot 06c-card-artist-hover
click 800 190
shot 06d-artist-opened 0.7
# A right click on their first release opens its menu (Play, Play next, Play last). Esc closes
# only the menu.
move 446 163
xdotool click 3
shot 06d2-artist-release-menu
xdotool key Escape
# Drag their first release (at y=163) to the queue, between the first two upcoming tracks. All its
# songs go there.
drag 446 163 870 398
shot 06e-artist-release-drag
release
shot 06f-artist-release-queued 0.7
xdotool key Escape

# Search: the results fill the middle column.
click 446 60
xdotool type --delay 60 "a"
shot 07-search 0.7

# ↓ highlights a result from the search box (the second result, after two pushes). ↑ two times
# goes back to the text, so the next steps find the list as it was.
xdotool key Down Down
shot 07a-search-result-highlighted
xdotool key Up Up

# Drag a result into the queue, between two upcoming tracks.
drag 442 270 870 466
shot 08-drag-search-result-to-queue
release
shot 09-search-result-in-queue

# The ＋ of the first result (x=598) adds it at the end of the queue, out of view. A notice at
# the bottom of the queue tells where it went.
click 598 134
shot 09a-search-result-added-below

# Clear the search with the ✕ at the right of the field (x=594): the hint comes back.
click 594 60
shot 10-search-cleared

# The playlists start under the folder «Festivals» (first row, y=118). Rows are 68 px apart.
# Drag «Calm» (fourth row) into the queue: all its tracks go there.
drag 130 322 870 466
shot 11-drag-playlist-to-queue
release
shot 12-playlist-queued

# Drag an upcoming track onto the first playlist («Favorites», second row): the app adds it to the
# playlist.
drag 870 432 130 186
shot 13-drag-queue-track-to-playlist
release
shot 14-queue-track-added-to-playlist

# A right click on «Calm» (fourth row) opens its menu, with only «Share». The row of «Share» is
# under the pointer (y=322+4+8+20). A click copies the link, and a notice tells it.
move 130 322
xdotool click 3
shot 14a-playlist-menu
click 200 354
shot 14b-playlist-link-copied

# Click «Calm» (fourth row): its tracks fill the middle column.
click 130 322
shot 15-playlist-opened

# A right click on its second track (on the album link) opens the track menu. Esc closes it, and
# the playlist stays open.
move 442 256
xdotool click 3
shot 15a-playlist-track-menu
xdotool key Escape
shot 15b-playlist-track-menu-closed

# Drag one of them into the queue, between two upcoming tracks.
drag 442 256 870 466
shot 16-drag-playlist-track-to-queue
release
shot 17-playlist-track-in-queue

# Drag that track two positions down in the playlist: its position changes.
drag 442 256 442 401
shot 17a-playlist-track-reorder-drag
release
shot 17b-playlist-tracks-reordered

# On hover, a track of the playlist shows its ▶, ♥ and ＋, but no ✕. Its menu (right click)
# removes it.
move 442 324
shot 17c-playlist-track-hover
xdotool click 3
shot 17c1-playlist-track-remove-menu
click 520 524
shot 17d-playlist-track-removed

# Drag the current track (by the cover and title of its card) into the open playlist, between its
# first two tracks. A slot opens there, and the app adds the track at that position (the queue
# keeps it).
drag 760 161 442 222
shot 17e-drag-current-track-to-playlist
release
shot 17f-current-track-added-to-playlist

# Drop the current track again, now on «Calm» in the sidebar. The playlist already has it, so a
# dialog asks if the app must add it again. Escape cancels it (and the playlist stays open).
drag 760 161 130 322
release
shot 17f1-already-in-playlist-dialog
xdotool key Escape
shot 17f2-already-in-playlist-cancelled

# Drag a previous track onto «Running», which is not open and does not have it. The track is the
# last previous track, partly visible above the card: «Harbor Song», the only one, because the
# history starts empty. The app adds it at the end of the playlist.
drag 870 108 130 254
shot 17g-drag-previous-track-to-running
release
shot 17h-previous-track-added-to-running

# On hover, the name of the open playlist («Calm», x=295) shows a pencil next to it: the user can
# edit the name.
move 295 112
shot 17i-playlist-title-hover

# Escape closes them: the hint comes back.
xdotool key Escape
shot 18-playlist-closed

# Drag «Calm» (fourth row) to the top of the list, above «Festivals». The pointer stays at the top
# edge of the folder row, because over its middle the playlist goes into the folder. A slot opens
# there, the other rows make space, and it stays first (the app keeps the order).
drag 130 322 130 90
shot 18a-playlists-reorder-drag
release
shot 18b-playlists-reordered

# Ctrl+← goes back to the playlist that showed before («Calm»), and Escape closes it again.
xdotool key ctrl+Left
shot 18c-navigation-back
xdotool key Escape

# A playlist of a different user («Summer Hits», now the sixth row). The app cannot show its
# songs, and «Open in Spotify» opens it in the web player.
click 130 458
shot 18d-someone-elses-playlist
xdotool key Escape

# Ctrl+Shift+→ skips to the next track. The card goes back to the top of the queue, where the
# steps below expect it.
xdotool key ctrl+shift+Right
sleep 0.4

# Near the end of the track, the app sends the next track to Spotify: it shows a padlock.
click 1065 232
shot 19-next-track-locked 1

# Pause: the button changes into "play".
click 743 287
shot 20-paused

# Volume at approximately a third.
click 928 287
shot 21-volume-lowered

# Device picker, in the title bar just left of the three-dot menu.
click 913 17
shot 22-device-picker

# The music goes to Nuria's phone (third row). The queue column only tells where it plays, with
# no player, and the picker shows the name of the phone.
click 733 228
shot 22b-playing-elsewhere 0.5

# «Play here» (under the message) brings back the music, the player and the queue.
click 872 494
shot 22c-back-here 1.5

# The three-dot menu next to the window buttons: the user and «Log out».
xdotool key Escape
click 958 17
shot 22a-account-menu

# «Changelog» (first row of the menu) opens its dialog.
click 880 130
shot 22d-changelog 0.5
xdotool key Escape

# A new playlist: ＋ adds its card at the top, with a field for its name and «New playlist» under
# it. After Enter, the card waits for Spotify (of the demo) to create it. Then it shows its song
# count.
xdotool key Escape
click 233 54
shot 23-new-playlist-field 0.5
xdotool type --delay 40 "Road trip"
shot 23a-new-playlist-name
xdotool key Return
shot 23a2-new-playlist-creating 0.1
shot 23b-new-playlist-created 1

# A folder: the button left of ＋ (x=201) adds the card of a new folder at the top («New folder»
# under the field for its name). Enter creates it there. «Road trip» is now the second row.
click 201 54
xdotool type --delay 40 "Concerts"
shot 23b2-new-folder-card
xdotool key Return
shot 23c-new-folder-created 0.5

# When dragged onto the folder, «Road trip» goes into it. While it is over the folder, the folder
# lights up and opens its lid.
drag 130 186 130 118
shot 23d-drag-playlist-onto-folder
release
shot 23e-playlist-in-folder

# A click on the folder shows its contents. The title becomes a breadcrumb.
click 130 118
shot 23f-folder-opened

# When dropped on the previous level of the breadcrumb («Playlists», x=32..100), it goes back
# there, immediately after the folder.
drag 130 118 64 56
shot 23g-drag-playlist-onto-breadcrumb
release
shot 23h-folder-emptied

# «Playlists» in the breadcrumb goes back to the first level.
click 64 56
shot 23i-back-to-first-level

# «Calm» (third row) also goes into the folder: the folder gets the color of its cover.
drag 130 254 130 118
release
shot 23i2-folder-colored

# A right click on the folder opens its menu (rename, delete).
move 130 118
xdotool click 3
shot 23j-folder-menu

# «Rename folder» (the first item) changes the name of the folder into a field, in its row. Esc
# keeps the name as it was.
click 150 144
shot 23k-folder-rename
xdotool key Escape

# Scroll the queue down: the card of the current track stays at the top, above the upcoming
# tracks.
move 870 366
for _ in 1 2 3 4 5 6; do xdotool click 5; sleep 0.05; done
shot 24-queue-scrolled-card-pinned

# With the pointer over the list (the step above), a thin scroll bar shows at the right edge of
# the queue, tinted with the accent color. Over the bar itself (x=1086..1096, its handle at
# approximately y=420..645), it becomes wider and a bit stronger.
move 1091 450
shot 24a-queue-scroll-bar-hover

# «Clear» empties the queue. An empty card stays, with the volume and the device picker, but no
# track to skip.
click 1070 58
shot 25-queue-cleared 0.5

# Tab moves the keyboard focus through the buttons: a ring shows its position.
xdotool key Tab Tab Tab
shot 26-focus-ring

# A new version: the app opens again, and checks a local server that tells about 99.0.0 (it
# checks 2 s after it starts). A notice tells this, and the lines of the three-dot menu become a
# gradient. An «Update» button appears left of the menu (it pushes the device picker left), and
# the menu offers the download under the version.
kill "$APP_PID"
wait "$APP_PID" 2>/dev/null || true
UPDATES_DIR="$(mktemp -d)"
echo '{"version": "99.0.0", "url": "https://github.com/nuzkito/melodia/releases"}' >"$UPDATES_DIR/version.json"
python3 -m http.server 18899 --bind 127.0.0.1 --directory "$UPDATES_DIR" \
    >"$UPDATES_DIR/requests.log" 2>&1 &
UPDATES_PID=$!
launch MELODIA_UPDATE_URL=http://127.0.0.1:18899/version.json
click $((WIDTH / 2)) $((HEIGHT / 2 + 19))
for _ in $(seq 1 40); do
    grep -q 'GET /version.json' "$UPDATES_DIR/requests.log" && break
    sleep 0.25
done
shot 27a-update-notice 0.5
click 958 17
shot 27b-update-in-menu

# The images of the README and the landing page (`og-*`), at 1200x630, the size of Open Graph
# images. The app opens again, and the window gets that size (there is no window manager to
# stop it). Then the columns change: the playlists use x=32..259, the middle column has its
# center at x=475 and the queue starts at x=699. The window buttons and the device picker keep
# their distance from the right edge.
kill "$APP_PID"
wait "$APP_PID" 2>/dev/null || true
launch
xdotool windowmove --sync "$WINDOW" 0 0 windowsize --sync "$WINDOW" 1200 630
eval "$(xdotool getwindowgeometry --shell "$WINDOW")"
move 40 $((HEIGHT - 40))
shot og-login 1
click $((WIDTH / 2)) $((HEIGHT / 2 + 19))
sleep 1.5
# Next track, as in step 03. Then the pointer goes to the empty part of the middle column.
click 842 245
move 475 560
shot og-player 0.5
# A search, as in step 07.
click 475 60
xdotool type --delay 60 "a"
move 475 560
shot og-search 2
click 636 60
# The artist of the current track, as in step 06d: the pointer stays on the name, which is
# underlined. The landing page uses this image.
click 842 190
shot og-artist 0.7
# Their first release dragged to the queue, as in step 06e.
drag 475 163 912 398
shot og-drag
xdotool key Escape
release
xdotool key Escape
# The shortcuts guide, with no menu open (the README shows it). The pointer leaves the queue.
move 475 560
shot og-shortcuts 0.5
# The device picker, as in step 22.
click 993 17
shot og-devices

echo "Smoke test passed"
