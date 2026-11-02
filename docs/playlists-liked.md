# Playlists and «Liked Songs»

Read this file before you change the playlist features (`playlist_sync.rs`, `playlist_library.rs`,
`ui/playlist_adds.rs`) or the hearts (`ui/liked.rs`). Back to [AGENTS.md](../AGENTS.md).

## «Liked Songs» (`ui/liked.rs`)

- Each track row and the card of the playing track have a ♥ (`liked::heart_button`).
- The state flows like this. `MusicApp` shares `LikedSongs` through the memory of egui (`publish`).
  The rows read it with `liked::state`. At the end of the frame, `collect` makes the commands
  `Command::CheckLiked` (at most one every 0.2 s) and `Command::SetLiked`.
- `dispatch` applies `SetLiked` at once. Answers to earlier checks (`Event::LikedStates`) do not
  replace the user's changes. `Event::LikedChanged` always applies, so it undoes a refused change.
- In `live.rs`, the changes are chained (`liked_chain`), so they reach Spotify in order.
- Without the library scopes (`has_library_scopes`), the backend sends `Event::LikedNeedsReauth`.
  A click then logs in again and saves the track after the login (`liked_after_login`).
- The endpoints are `GET /me/library/contains`, `PUT /me/library` and `DELETE /me/library`. They take
  at most 40 URIs (`LIBRARY_BATCH`). Since February 2026, they replace `/me/tracks`.

## Playlists

- The app edits only the playlists that the user owns or collaborates on (`Playlist::editable`).
  Other playlists show dimmed. Their tracks are read from `/playlists/{id}/items`, the 2026 format.
  The old format is still accepted.
- Reads and edits of one playlist are chained (`chain_playlist_task`), so they reach Spotify in
  order.
- `PlaylistMirror` (`playlist_sync.rs`) converts interface indices into Spotify positions. It can
  remove a repeated track only by removing all its copies by URI, then adding the other copies
  again. It cannot add local files again (`PlanError::LocalFile`).
- If Spotify refuses an edit, the app shows a notice and reads the playlist again. Right after an
  edit, `GET /me/playlists` can return the old count or name. The app uses the expected value once
  (`Expected`).

### Duplicates

- `MusicApp::dispatch` holds each `PlaylistEdit::Insert` until it knows whether the playlist has the
  track. It knows this from the tracks read when the playlist opened (`PlaylistTracksView::has`), or
  from the tracks the user added since then.
- If it does not know, it sends `Command::FindInPlaylist`. The backend answers after the earlier
  edits (`Event::FoundInPlaylist`). If the track is there, a dialog (`egui::Modal`) asks the user.
  «Add again» sends the edit.
- For an album dropped on a playlist, the UI sends `Command::FindAlbumInPlaylist`. The answer is
  `Event::FoundAlbumInPlaylist`. If no track is there, the app appends all tracks, in requests of
  100 (`ApiStep::Append`). If some are there, the dialog offers «Skip repeated» and «Add all».

### Folders and order

- The API cannot order the library playlists and cannot see folders. The app keeps its own tree,
  `Library` (`model/library.rs`). `PlaylistLibrary` (`backend/playlist_library.rs`) stores it in
  `settings.json`, under `playlist_library`, for each account.
- An old flat `playlist_order` is read once and replaced by the tree. In the demo, the tree stays
  until logout. Until the user arranges the playlists, the tree follows Spotify's order and nothing
  is saved.
- `Library::arrange` makes the tree match Spotify's list. Deleted playlists leave the tree. A new
  playlist goes after the playlist before it, in the same folder (as `arrange_playlists` does).
  `Event::Playlists` always has the depth-first order of the tree. The backend sends `Event::Library`
  just before it. The UI ignores a library that is older than its last edit.
- Each change is a `LibraryEdit`. `MusicApp::send` applies it at once. The backend applies it with
  the same `Library::apply`. The UI creates the folder IDs (`folder-N`, `Library::next_folder_id`).
- When Spotify creates a playlist, the backend puts it in the folder that the user chose
  (`place_new`). It sends `Event::PlaylistCreated`, with `None` as the ID if the creation failed. It
  then loads the playlists again. If Spotify does not list the new playlist yet, the backend puts it
  first, once (`Expected::created`).
- A new item shows a card at the top of the level (`PlaylistsView::naming`). A new playlist keeps
  its card in `PlaylistsView::creating` until `Event::PlaylistCreated` gives its ID. Then the row
  replaces the card without a fade-in (`settle_created`).
- The sidebar (`ui/playlists.rs`) shows one level (`PlaylistsView::folder`). A drop target is the
  middle of a folder row, from the last drawn frame (`folder_rects`). The slot stays at `last_drop`,
  so the rows do not move under the pointer. A dragged folder is `DragItem::Folder`, which the queue
  ignores.

Playlist edits are optimistic and counted. See [queue.md](queue.md), «Optimistic changes and old
states».
