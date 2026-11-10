# What the app does

The user guide for the window, the search, the queue, the playlists, the folders and the music. Back
to [README.md](../README.md).

## The window

- The playlists are on the left. The search, or the open playlist, album or artist, is in the center.
- The queue is on the right. It starts with a card for the playing track, with the controls: play or
  pause, previous, next, the progress bar and the volume.
- The top right has the device selector and the ☰ menu.

## Search and tracks

- Search finds tracks, albums and artists. The best matches come first.
- More results load 10 at a time («Show more results»).
- ▶ or a double click plays a track now. The queue stays the same.
- Drag a track to the queue to put it at a specific place.
- The track menu (right click) has “Play”, “Play next”, “Play last”, “Like” or “Unlike”. In your own
  playlists, it also has “Remove from playlist”. The album menu has “Play”, “Play next” and “Play last”
  for all its tracks.

## Queue

- The queue shows the previous tracks, the playing track and the next tracks.
- Drag a track to change the order. ▶ jumps to a track. ✕ removes it.
- **Clear** removes all tracks and pauses the music.
- The queue stays after you close the app. It is removed when you log out.

## Albums and artists

- Click the name of an album or an artist under a track to open it.
- An artist page shows the albums and singles, the newest first.
- Drag a release to the queue or onto a playlist.

## Playlists

- The sidebar shows your playlists, and the playlists that you follow or collaborate on.
- Click ＋ to create a playlist. Double-click the name of your playlist to rename it.
- Next to the name of an open playlist or album, the app shows the total duration of its tracks.
- In an open playlist, drag tracks to change the order.
- Drag a track or an album onto a playlist to add it. If the playlist already has the track, a dialog
  asks first.
- Drag a playlist to the queue to add its tracks. The queue takes up to 100 tracks.
- Drag playlists to change their order.
- The app cannot show the tracks of playlists from other users, because Spotify does not let it read
  them. Click **Open in Spotify** to open them in the Spotify web player.

## Folders

- Click the button left of ＋ to create a folder.
- Drag playlists and folders onto a folder to move them in. Folders can contain folders.
- Right-click a folder to rename or delete it. Deleting a folder keeps its contents in its place.

## Liked Songs

- Click ♥ on a track to add it to your Liked Songs in Spotify. Click it again to remove it.

## Navigation

- ‹ and › next to the search box go back and forward through the playlists, albums, artists and
  searches that you opened.

## Menu and updates

- The ☰ menu shows the version, the changelog and “Log out”. The app keeps your session between starts.
- When the app opens, it checks for a newer version. If one exists, the “Update” button shows next to
  ☰, and it opens the download page.

## Music playback

Melodia plays the music with the official Spotify web player. The Web API does the other tasks:
search, the queue and the playlists. Spotify Premium is necessary.

### Play in this app

“Play in this app” is the first option in the device selector, and the default device. When the app
opens, the player is ready and paused, with the last volume.

- Volume, play, pause, next, previous and seek are instant. They do not send requests to Spotify.
- Play and pause fade in and out.
- If the last track played in this app, the app opens it at its last position.
- The first time, the device row says “Permission”. Click **Reconnect** to give the `streaming`
  permission.

The app runs the [Web Playback SDK](https://developer.spotify.com/documentation/web-playback-sdk) in a
hidden WebView2 window, which registers as the “Melodia” device. If playback fails, the device row
shows the reason. See [troubleshooting.md](troubleshooting.md).

### Music on other devices

You can send the music to Spotify on another device, such as your PC, your phone or a speaker. The
music can also go there when you start a track in Spotify on that device. Then:

- Melodia removes its queue and hides the player. The queue column shows “Playing on *device*”.
- Melodia does not control the music on that device, including with the keyboard shortcuts.
- The music returns to Melodia when you click **Play here**, select “Play in this app”, play a track,
  or drop a track on the queue.

### How the queue keeps your order

The Web API cannot change the order of Spotify's queue. A new list restarts the track and you hear a
cut. So Melodia keeps your order itself. Changing the order does not change the playback.

About 3 seconds before a track ends, the app adds the next track to Spotify's queue.

- In those last seconds, the next track shows 🔒. That track and the playing track cannot move.
- When the track changes in all cases (▶ on a track, “next” after a change of order, a resume), the
  app sends the full list. You hear no cut. The app also turns off **shuffle**.
- When the queue has no more tracks, the music stops. If Spotify continues by itself (autoplay), the
  app pauses it.
- The previous tracks are the tracks that played in Melodia, not Spotify's recently played tracks.
- Tracks that you add in the official Spotify app stay in Spotify's queue, and they can play first.
  The Web API cannot remove them.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Space` | Play or pause |
| `Ctrl+Shift+→` / `Ctrl+Shift+←` | Next or previous track |
| `Ctrl+←` / `Ctrl+→` | Back or forward. The side buttons of the mouse do the same |
| `Ctrl+↑` / `Ctrl+↓` | Volume up or down |
| `Ctrl` + scroll wheel over the volume | Volume up or down |
| `Ctrl+F` or `/` | Go to the search box |
| `↑` / `↓`, then `Enter` | In the search box: select a result, then play or open it |
| `Tab` / `Shift+Tab`, then `Enter` | Move between buttons, tracks and playlists. Then push or open the selected item |
| `Esc` | Clear the search. Close what is open in the center column or a menu. Cancel a drag or a dialog |

## Limitations

- The app cannot play local files, and it cannot add them to a playlist.
- A playlist shows up to 500 tracks.
- The queue shows up to 100 next tracks.
- The history keeps the last 50 tracks.
