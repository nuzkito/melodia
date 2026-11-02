# Interface (`ui/`)

Read this file before you change `ui/`. Back to [AGENTS.md](../AGENTS.md).

- The egui painter draws almost all items, over a few widgets such as text fields and scroll areas.
- The window has no system frame. `window_frame.rs` draws the drag area, the buttons and the resize
  borders.

## Layout

The layout is in `MusicApp::main_screen`.

- Left: the playlists and folders (`playlists.rs`), one level at a time, with a breadcrumb.
- Center: the search box, and below it one of these:
  - the search results (`search.rs`)
  - the tracks of the open playlist (`playlist_tracks.rs`)
  - an album (`album.rs`), opened by a click on its name under a track (`album::link`)
  - an artist (`artist.rs`), opened the same way (`artist::links`). Albums and artists keep their
    state in a `detail::DetailPanel`.
  - the shortcut guide (`guide.rs`), when nothing else shows
  «Back» and «Forward» are left of the search box (`navigation.rs`).
- Right: the queue (`queue.rs`) with the card of the playing track (`player_controls.rs`). While the
  music plays on another device (`MusicApp::elsewhere`), the column shows where the music plays and a
  «Play here» button (`QueueView::elsewhere`).
- Top right: the device picker (`devices.rs`) and the ☰ menu (`account_menu.rs`). The «Changelog»
  row opens `changelog.rs`, which shows the entries of `CHANGELOG.md`, newest first. The build embeds
  that file.

## Views

- Each view is an `XxxView` with its own state (`Default`). It gets borrowed data in an
  `XxxData<'a>`, gets events through `on_*` methods, and puts its commands in `actions`.

## Reusable parts

Use these before you make a new widget:

- `theme.rs`: colors, fonts, radii.
- `widgets.rs`: `custom_button`, `icon_button`, `pill_button`, `text_link`, `text_field`,
  `list_title`, `empty_state`, `skeleton_rows`, `scroll_hints`, `a11y_label`,
  `full_text_tooltip` and `explicit_badge`. Also `ROW_HEIGHT` and `ROW_COVER`.
- `track_row.rs`: the track row (▶, ♥, ＋, ✕). A right click returns `RowActions::menu_at`.
- `context_menu.rs`: the right-click menu of a track or an album. Each view owns a `ContextMenu` and
  turns its `Choice` into commands. `context_menu::command` has the shared commands.
- `detail.rs`: a panel of the center column for one loaded item (cache, Esc, placeholders, links).
- `icons.rs`: vector icons that you can animate.
- `dnd.rs`: drag and drop (floating track, gap, auto-scroll, slide after the drop).
- `queue_keys.rs`: stable keys, so repeated rows slide into position.
- `toasts.rs`: notices.
- `backdrop.rs`: the tinted background. `backdrop::framed` draws a popup with its border. Gradient
  rules are in [design.md](design.md), «Background and surfaces».

## Animations

- Animate only with `ui/anim.rs`. Durations are in seconds, as written.
- The `animation_time` setting of egui only turns animations off. It is 0 in the tests, so all
  changes there are instant.

## Keyboard

- `custom_button` and the widgets that use it take the focus with Tab. Enter pushes them. Space stays
  play and pause. A focused button draws a ring (`focus_ring`). For shapes that are not a circle or a
  rounded rectangle, use `custom_button_with_ring`.
- Only the keyboard gives these buttons the focus. Thus the ring never shows after a click.
- Track and playlist rows show the same ring. Enter plays or opens them.
- Areas for the pointer only (window edges, drag handles, sliders) use `Sense::CLICK | Sense::DRAG`
  without `FOCUSABLE`, so Tab skips them.
- The search box consumes ↑, ↓ and Enter before the text field sees them. It keeps the focus
  (`SearchView::keyboard`).
- Search results are artists, albums and tracks (`Hit`). Artists and albums use `artist::card_row`.
- Shortcuts: `MusicApp::shortcuts`. The table is in [features.md](features.md), «Keyboard shortcuts».
  Update that table when a shortcut changes.

## Accessibility

- Each interactive element and each important text needs an English accessible label. Use the
  `label` of `custom_button`, `icon_button` or `text_field`, or `a11y_label`.
- The UI tests find elements by these labels.
