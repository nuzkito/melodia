# The app's own queue and optimistic edits

This is the most delicate part of the app. Read this file before you change `playback/`, the queue
in `live.rs`, `QueueEdit` or `PlaylistEdit`. Back to [AGENTS.md](../AGENTS.md).

## The app's own queue

The Web API cannot change the order of Spotify's queue, cannot insert at a position, and cannot show
previous tracks. A new list (`PUT /me/player/play`) restarts the track. The full list of limits is
in [testing-and-dev.md](testing-and-dev.md), «Web API limits».

So the app does this (`playback/queue.rs` and `live.rs`):

- **Spotify's queue, until the user changes something.** `queue_owned = false`. The app reads
  `GET /me/player/queue` and removes the repeated tracks that Spotify adds (`without_padding`). It
  reads the queue when the session starts, when the music comes back here, and when the page's next
  tracks do not match the queue (`queue_stale`). It never reads on a timer. While nothing plays, or
  the music is on another device, the queue is empty.
- **Owned queue.** At the first change, `queue_owned = true`. `SpotifyPlan` records what Spotify
  would play by itself. A change of order **sends nothing** to Spotify.
- **Lead time.** `ENQUEUE_LEAD_MS` (3 s) before each track ends, `check_queue` adds the next track
  to Spotify's queue, if Spotify would play a different track. That track is then locked (`next_locked`,
  🔒). The locked track and the playing track cannot move (`QueueSnapshot::fixed()`).
- **End of Spotify's list.** Spotify does not play tracks added after its last track. So the app
  starts the next track itself (`list_ended_before_the_queue`, `TrackEnd::PlayNext`).
- **Follow.** `follow` and `apply_follow` interpret each track change. An unexpected track
  (`Follow::Lost`, for example a track started on another device) returns the queue to Spotify's
  queue (`give_queue_back`).
- **Full list.** `order_from` (`Request::Order`) sends the full list only when the track changes in
  all cases: ▶ on a track, «next» after a change of order, or a resume that does not match the plan.
  Then shuffle is turned off.
- **Empty queue.** `finish_queue` pauses Spotify and hides the track (`dismissed`). «Clear» does the
  same.
- **History.** `PlayHistory` keeps 50 tracks. It follows the track changes that the app sees. The
  app does not use the recently played endpoint and does not send local files (`queue_uris`).
- The user guide explains this behavior («How the queue keeps your order», in
  [features.md](features.md)). Update it when the behavior changes.

## Saved queue

- The queue survives a restart (`backend/saved_queue.rs`). After each handled message, `save_queue`
  compares the history, `upcoming` and `queue_owned` with the last saved values. When they changed,
  `spawn_save_task` writes `queue.json`. The task deletes the file when the user logs out or the
  queue is empty. If the saved types change so that old files cannot load, increment `FORMAT`.
- At restore (`restore_queue`), the queue shows at once, before Spotify says what plays. The queue
  stays owned, with `SpotifyPlan::restored()`. The plan is empty, so it does not match. The first
  resume therefore sends the full list (`resume_with_order`).
- After that, the app follows the queue as usual. The same track is the current track
  (`same_as_before`). Any other track is a track change (`follow`, or `observe`, which makes the
  saved current track a previous track). Music on another device removes the queue (`go_elsewhere`).
- The demo does not save the queue.

## Optimistic changes and old states

- The interface applies each `QueueEdit` and `PlaylistEdit` at once, in `dispatch`, with the same
  pure function as the backend (`apply`, in `model.rs`). It counts the edits that it sends
  (`queue_edits_sent`, `playlist_edits_sent`).
- The backend counts the edits that it receives, including edits it cannot apply. It returns the
  count in `QueueSnapshot::edits` and in `Event::PlaylistTracks { edits, .. }`. The interface ignores
  data with fewer edits. Thus a dropped track never jumps back for a moment.
- Edits identify an item by its index **and** its URI (`find_near`), because a list can change during
  a drag and a track can repeat.
- A new playlist edit follows the same pattern. Add a pure function in `model.rs` with tests. The
  interface and both backends must apply it, and it must be counted.
