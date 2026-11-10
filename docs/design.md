# Design guide

The style guide for the interface components. It describes what `src/ui/` does now. If a value here
and the code differ, the code is right, and you must update this file. [interface.md](interface.md)
tells how the interface is organized. Back to [AGENTS.md](../AGENTS.md).

## Principles

- **Dark only.** `theme::install` sets the dark theme. There is no light mode.
- **The app paints its items** with the egui painter. Standard widgets are only for text fields and
  scroll areas. Use a `widgets.rs` component before you draw a new one.
- **The accent tints the app.** It comes from the current cover, or Spotify green by default. Do not
  hard-code a color that must follow the accent.
- **Quiet at rest, responsive on hover.** Controls are dim until the pointer or the keyboard reaches
  them. Hover takes 0.16 to 0.18 s, press 0.08 s and focus 0.12 s.
- **Each interactive element has an English accessible label.**
- **All user text is in English.** In comments, interface texts use «». In the interface, quotes use
  “”.

## Foundations

### Color (`theme.rs`)

| Token | RGB | Use |
| --- | --- | --- |
| `BG` | 11, 11, 14 | Window base, dark glyphs on light buttons |
| `SURFACE` | 23, 23, 29 | Windows, boxed fields at rest |
| `SURFACE_HOVER` | 32, 32, 40 | Toasts, tooltips, cover placeholder, fields with focus |
| `BORDER` | 42, 42, 52 | 1 px borders, separators |
| `TEXT` | 242, 242, 247 | Titles, track names, active icons |
| `TEXT_DIM` | 166, 166, 180 | Secondary lines, icons at rest, links |
| `TEXT_FAINT` | 128, 128, 142 | Durations, hints, placeholders. The dimmest allowed text color |
| `SPOTIFY_GREEN` | 30, 215, 96 | Default accent, success notices |
| `DANGER` | 255, 110, 110 | Error notices |
| `FOLDER_GRAY` | 132, 132, 146 | Folders with no cover color |
| `FOCUS_RING` | 206, 206, 210 at about 85 % | Keyboard focus ring |

Translucent whites lift a surface over the tinted background: rows on hover `white_alpha(10)`, icon
circles `white_alpha(14)`, bar tracks `white_alpha(28)`, skeleton bars `white_alpha(14)` with a
pulse. The close button is red on hover (`232, 17, 35`). It is the only hard-coded exception.

### Typography

Segoe UI on Windows. Inter, or DejaVu Sans, on Linux. CJK and symbol fonts are fallbacks.

The app maps the system fonts into memory (`theme::map_font`) and does not read them. The CJK
fallbacks are more than 30 MB, and a mapping loads only the glyphs that the UI draws.

| Constant | Size | Use |
| --- | --- | --- |
| `FONT_SMALL` | 12 | Durations, counters, badges, secondary lines |
| `FONT_BODY` | 14 | Body text, track titles, buttons, fields |
| `FONT_TITLE` | 16 | Titles, semibold |
| `FONT_HERO` | 28 | Login headline |

- Text is one line, cut with «…» (`widgets::single_line`). Paragraphs (dialogs, toasts, the subtitle of
  an empty state) wrap.
- A cut name shows its full text in a tooltip (`widgets::full_text_tooltip`). Spotify's guidelines
  require it.

### Radii and spacing

- Radii: `RADIUS_SMALL` 4 (covers, tooltips, bubbles), `RADIUS_MEDIUM` 6 (rows, boxed fields, menu
  items, focus rings), `RADIUS_LARGE` 8 (dialogs, popovers, toasts).
- Item spacing is 8 × 6. Button padding is 12 × 6.
- Track rows (`ROW_HEIGHT`) are 68 px high. The cover (`ROW_COVER`) is 52 px, inset by 8 px. The text
  starts 14 px right of the cover (`ROW_TEXT_GAP`). Tracks, releases and playlists use these values.
- A list title row (`LIST_TITLE_HEIGHT`) is 40 px high.
- The window is 1120 × 720, with a minimum of 880 × 600. The top drag band is 40 px high. Window
  buttons are 46 × 34 px.

### Elevation

- Windows: `offset 12, blur 32, black α150`. Popups and toasts: `offset 8, blur 24, α140`.
- Modal backdrop: black α140. Borders: 1 px `BORDER`, drawn in the shape.

### Motion (`ui/anim.rs`)

Animate only with the `anim` helpers. The durations scale with the egui `animation_time`, which is 0
in the tests.

- The default easing is ease-out cubic. `ease_out_back` is for pop-ins (toasts). `ease_in_out_cubic`
  is for morphs (play ↔ pause, 0.28 s).
- Lists appear in a stagger: `ROW_APPEAR_DELAY` 0.018 s between rows, and `ROW_APPEAR_DURATION` 0.18 s
  for each row.
- An animating item requests frames (`keep_animating`, `request_frame*`) and stops when the animation
  ends. The app is idle otherwise.

## Background and surfaces (`backdrop.rs`)

The background is `BG`, with two soft glows. The accent glow sits at 22 % of the width and 42 % of
the height. Its complementary color (hue + 180°) glows in the bottom right corner. Dialogs and menus
use `backdrop::framed`, with the colors swapped: the complementary color is the main color.

Rules for glows and gradients:

- Make one texture, and tint it.
- Add light as a color with alpha 0. Do not put a translucent color on a flat color.
- The falloff must have no corners.

If you break these rules, the 8-bit rounding shows as rings. Overlays that fade into the background
use `backdrop::color_at`.

## Components

### Buttons

All painted buttons use `widgets::custom_button`. It gives hover, press and focus animations, a hand
cursor, an accessible label, 0.35 opacity when disabled, and the focus ring.

| Component | Function | Look |
| --- | --- | --- |
| Icon button | `icon_button` / `icon_button_hover` | Round, 30 px (46 px for previous and next). A faint circle on hover. On press, the circle is 6 % smaller. It has a tooltip. |
| Pill button | `pill_button` | Fully rounded, semibold. On hover 5 % wider, with a 12 % lighter fill. On press 4 % narrower. It is the primary action, 36 px high in dialogs. |
| Text link | `text_link` | Invisible over painted text (an artist or album name). On hover, an underline in `TEXT_DIM`. |
| Text button | `text_button` | Text only. Changes from `TEXT_FAINT` to `TEXT` on hover. |
| Play and pause | `player_controls::transport` | A 68 px `TEXT` circle with a drop shadow. It grows 5 % on hover. A click squashes it and it springs back (0.55 s). |
| Previous and next | same | 46 px icon buttons, 80 px from the center. The glyph moves 5 px on click. |
| Window buttons | `window_frame::controls` | 46 × 34 px, top right: minimize, maximize or restore, close (red on hover). |

For shapes that are not a circle or a rounded rectangle, use `custom_button_with_ring`.

### Focus ring

`focus_ring` draws a 2 px `FOCUS_RING` stroke, 1 px outside the element, with the element's radius
plus 1. It fades in during 0.12 s. Areas for the pointer only use `Sense::CLICK | Sense::DRAG`
without `FOCUSABLE`.

### Text fields (`widgets::text_field`)

- **Boxed** (forms): `SURFACE` fill, 1 px `BORDER`, `RADIUS_MEDIUM`, 14 px left padding. On focus, the
  fill moves toward `SURFACE_HOVER` and the border toward the accent (80 %), in 0.2 s.
- **Search**: a translucent white pill with a 20 px magnifier. At rest it is 4 px shorter and
  `white_alpha(5)`. On focus it grows to full height and `white_alpha(20)`. The box consumes ↑, ↓ and
  Enter, so it keeps the focus.

Both styles have a `TEXT_FAINT` placeholder and an accessible label.

### Track row (`track_row.rs`)

68 px high, with a `RADIUS_MEDIUM` highlight inset 2 × 3 px.

- **Cover** (52 px): nothing covers it. Spotify's guidelines forbid overlays and controls on artwork.
- **Title** in `FONT_BODY` and `TEXT`, with the «E» badge if the track is explicit.
- **Subtitle** «artist · album» in `FONT_SMALL` and `TEXT_DIM`. Each artist and the album are text
  links.
- **Duration** in `FONT_SMALL` and `TEXT_FAINT`, right aligned. On hover it fades out, and the action
  buttons replace it from right to left, 34 px apart: ✕ (remove), ＋ (add, then ✓ for about 1.4 s),
  ♥ (like), ▶ (play, or ❚❚ for the playing track). The ♥ stays visible for liked tracks.
- **States**: hover `white_alpha(10)`, also when the row or its buttons have the focus. The **playing
  track** has a 12 % accent tint, a title blended 85 % toward the accent, and an animated equalizer
  (12 × 11 px). **Locked** rows show a padlock. Rows can be dragged if they are enabled.
- Enter, or a double click, plays the row.

### Lists

- **Title row** (`list_title`): semibold `FONT_TITLE` on the left. An optional count or duration is
  on the right, in `FONT_SMALL` and `TEXT_FAINT`.
- **Cards** (`artist::card_row`): artists and albums in search and on artist pages. Same size as a
  track row.
- **Scroll hints** (`scroll_hints`): a 28 px fade along each edge that has more content. The scroll
  bars take the accent tint (`tint_scroll_bars`).
- **Skeleton rows** (`skeleton_rows`): a cover square and two bars, with `white_alpha(14)` and a
  pulse, while a list loads.
- **Empty state** (`empty_state`): a 60 px circle with an icon, a semibold title and a `TEXT_FAINT`
  subtitle, wrapped at 340 px. It sits a little above the middle.
- **Drag and drop** (`dnd.rs`): the track floats under the pointer. A gap opens where it would go.
  The list scrolls near the edges. The row slides into place after the drop.

### Playlist folders (`playlists.rs`, `icons::folder_tile`)

- **Row**: the same as a playlist row. It shows “Folder · N playlists” or “Folder · Empty”. A
  `TEXT_DIM` chevron slides in on hover.
- **Tile**: a `RADIUS_SMALL` square. Its fill is `SURFACE_HOVER` blended 30 % toward the folder
  color. The color is the accent of the first playlist's cover, or `FOLDER_GRAY` without one. The
  folder glyph (`icons::folder`) is 32 × 25.9 px. The tile fades in during 0.3 s when the cover loads.
  The lid opens on hover, and fully while an item can be dropped in.
  - On a drop, the tile pops (12 %, 0.4 s), and the dropped cover flies in and fades out.
- **Breadcrumb** (in a folder): the previous level in `TEXT_DIM` (a «Back to …» button), a chevron,
  and the folder name in `TEXT`. While an item is dragged, the previous level shows an accent pill.
- **Levels**: opening or leaving a folder slides the list 28 px while it fades in (0.28 s).
- **Folder menu** (right click): `backdrop::framed`, 180 px wide, with `devices::text_row`s. A
  playlist uses the menu of `context_menu.rs`.

### Explicit badge (`widgets::explicit_badge`)

A 14 px `TEXT_DIM` square with a 9.5 px semibold «E» in `BG`. It sits 6 px right of the title of an
explicit track. Its label and tooltip are “Explicit”.

### Spotify logo (`spotify_logo.rs`, login screen)

The white full logo from Spotify's guidelines, unchanged (`assets/spotify-full-logo-white.svg`). It
is 70 px wide, centered on the line of the footer text «Playing music requires Spotify Premium.»
It keeps the clear space of half the icon height. The demo does not show it.

### Covers (`widgets::paint_cover`)

A rounded image. While it loads, or when it is missing, a `SURFACE_HOVER` square with a `TEXT_FAINT`
music note shows. The image fades in during 0.3 s. Playlists use the logo note (`icons::logo_note`).
A cover without an image shows its glyph in the area of `icons::glyph_bounds`: the cover less
`ROW_GLYPH_PAD` (10 px) on each side. The glyph keeps its proportions and is centered.

| Glyph | Cover | Glyph area | Glyph size |
| --- | --- | --- | --- |
| Playlist note | 52 × 52 | 32 × 32 | 20.5 × 32 |
| Folder | 52 × 52 | 32 × 32 | 32 × 25.9 |

### Seek bar and volume slider (`player_controls.rs`)

- A rounded track of `white_alpha(28)`, 4 px thick (6 px on hover). The fill is `TEXT`, and the accent on
  hover. A `TEXT` knob with a soft shadow shows on hover or while dragging.
- **Seek bar**: elapsed and total times below it, in `FONT_SMALL` and `TEXT_FAINT`. A hover bubble
  shows the time under the pointer. It is not a Tab stop.
- **Volume**: a speaker button (mute, with an animated level). A drag shows a percentage bubble. Ctrl
  with the wheel changes the volume in 5 % steps. Under 96 px, only the speaker shows. When the device
  does not allow volume changes, the control is disabled (0.35 opacity).
- Both show a sent value for up to 2.5 s, until Spotify reports it.

### Bubbles and tooltips

`SURFACE_HOVER` fill, `RADIUS_SMALL`, `FONT_SMALL` `TEXT`. Padding 14 × 8 px for a time, 12 × 6 px for a
percentage. They rise 4 px while they fade in (0.15 s).

### Toasts (`toasts.rs`)

Centered, 26 px above the bottom edge, stacked with 10 px between them. At most 4 show.

- `SURFACE_HOVER` fill, 1 px `BORDER`, `RADIUS_LARGE`, popup shadow. `FONT_BODY` wrapped at 440 px.
  Padding is 22 px on the left and 26 px on the right.
- A 4 px dot on the left: the accent (info), `SPOTIFY_GREEN` (success) or `DANGER` (error).
- A toast pops in with `ease_out_back` (rises 18 px), stays for 3.4 s, and fades out in 0.28 s. A
  repeated message restarts the timer and does not add a toast. Messages are short English sentences
  that start with a capital letter.

### Menus and popovers (`account_menu.rs`, `devices.rs`)

- `backdrop::framed` with an 8 px margin. The account menu is 240 px wide and opens 6 px below its
  button. It slides 8 px in while it fades in.
- Items are `devices::text_row`s, with 2 px between them. A 1 px `BORDER` line, inset 8 px,
  divides the groups. The last line is small, faint text: the app name and the version.
- A click outside, or Esc, closes the menu.

When a newer version exists:

- The three ☰ lines are 1.5 px thick, with a gradient from the accent to its complementary color. The
  gradient turns one full turn every 5 s (`UPDATE_CYCLE`). It does not move without animations.
- The menu shows “Download Melodia X.Y.Z” under the version line, in the accent color.
- An “Update” pill (`update_button`) shows left of ☰. It uses semibold `FONT_SMALL` in the accent
  color, with 10 × 4 px of padding, and a fill of the accent at 16 % (28 % on hover).

### Dialogs (`changelog.rs`)

- `egui::Modal` with a black α140 backdrop and `backdrop::framed` (20 px margin, `RADIUS_LARGE`).
- 400 px wide and 380 px high at most, with an internal scroll area. The confirm action is a 36 px
  `pill_button`.
- Esc, a click outside, or Enter (when no item has the focus) closes it. The top band of the backdrop
  still drags the window (`window_frame::drag_through_backdrop`).

### Window frame (`window_frame.rs`)

- A 40 px band at the top drags the window. A double click maximizes it. The edges are 5 px and the
  corners 8 px, and they resize the window.
- Draw the drag area before the content. Then the widgets on top keep their clicks.

### Icons (`icons.rs`)

Vector icons drawn with the painter. Each icon is relative to its size and centered on a point. They
stay sharp at all scales and can be animated. Strokes are about 11 % of the icon size. Usual sizes are
15 px (＋, search, row ▶), 16 px (new folder), 18 px (✕, speaker) and 22 px (skip). Add icons here,
not as font glyphs.

### App icon (`assets/icon.svg`, `icon.rs`)

A dark circle with a faint green glow. An eighth note and a thin border have a gradient from purple
(`icon::PURPLE`, the complementary color of the app's green) through blue to green. The login screen
turns the gradient one full turn every 5 s (`LOGO_TURN_PERIOD`), and it stays still without
animations.

## Accessibility

- Each interactive element and each important text has an accessible label. Sliders report their
  value (`WidgetInfo::slider`, «Volume», «Position 1:12 of 3:45»).
- Painted rows give «Name — artist · album», and a selected state when highlighted.
- Secondary text must not be dimmer than `TEXT_FAINT`.

## Add or change a component

1. Look first in `widgets.rs`, `track_row.rs`, `detail.rs` and `backdrop.rs`. Extend the existing
   code. [interface.md](interface.md), «Reusable parts», lists the same files.
2. Take colors, fonts and radii from `theme.rs`. Take the accent as a parameter.
3. Use `custom_button` for interactive areas.
4. Animate only with `ui/anim.rs`. Stop the frame requests when the animation ends.
5. Add the accessible label and a UI test in `tests/ui.rs` that finds the element by it.
6. Update this file when you add a component, change a value, or add a color, size or radius.
