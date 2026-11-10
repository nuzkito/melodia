//! Left sidebar: the user's playlists and their folders, one level at a time, with a breadcrumb
//! title. The user drops tracks and albums on a playlist to add them. The user drags playlists
//! and folders to change their order or to move them between folders. New playlists and folders
//! get a card with a name field at the top of the level. The README tells what the user
//! sees («Playlists», «Folders»).

use std::collections::HashMap;

use egui::{
    Align, Color32, Context, CursorIcon, Id, Key, Layout, Margin, Order, Pos2, Rect, Role, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};

use super::anim::{self, bump, ease_in_out_cubic, ease_in_quad, ease_out_cubic, lerp, lerp_color};
use super::backdrop;
use super::context_menu::{ContextMenu, Target};
use super::devices::text_row;
use super::dnd::{self, DragItem, DragOrigin, Dragged, DraggedFolder};
use super::icons;
use super::textures::{THUMB_PX, TextureCache};
use super::theme;
use super::widgets::{self, pill_button, single_line};
use crate::backend::Command;
use crate::model::{
    Folder, Library, LibraryEdit, LibraryItem, LibraryKey, Playlist, PlaylistEdit, PlaylistsStatus,
};

const TITLE_HEIGHT: f32 = widgets::LIST_TITLE_HEIGHT;
const ROW_HEIGHT: f32 = widgets::ROW_HEIGHT;
/// The height of the field that replaces a name.
const RENAME_FIELD_HEIGHT: f32 = 22.0;
/// The distance from the top of the columns to the center of their header row. This row has the
/// «New playlist» button here, and the search row in the middle column. The two rows have the same
/// center, so they align.
pub const HEADER_CENTER_Y: f32 = TITLE_HEIGHT / 2.0 - 2.0;
/// The seconds for a level to slide in when the user opens or leaves a folder.
const LEVEL_SLIDE: f32 = 0.28;
/// The seconds for an item dropped on a folder to fly into it.
const SWALLOW: f32 = 0.34;
/// The seconds for a folder (or the breadcrumb) to pop after it receives an item.
const TOOK_POP: f32 = 0.4;
/// The seconds for the rows to make space for the card of a new item (or to close the gap that it
/// leaves)…
const NEW_CARD_SHIFT: f32 = 0.2;
/// …and the seconds for the card to fade in when there is space (or to fade out before the gap
/// closes).
const NEW_CARD_FADE: f32 = 0.2;
/// The seconds for a row to glide to its position in all other cases.
const ROW_GLIDE: f32 = 0.32;
const MENU_WIDTH: f32 = 180.0;

#[derive(Default)]
pub struct PlaylistsView {
    /// The playlists that recently received a track from here (id → time), for the ✓.
    added_at: HashMap<String, f64>,
    /// The playlist that the user clicked in this frame, to show its tracks.
    clicked: Option<Playlist>,
    /// The drop slot was on the screen in the last frame.
    slot_shown: bool,
    /// The card of a new item was in the list in the last frame.
    new_card_shown: bool,
    /// The folder shown (`None`: the first level).
    folder: Option<String>,
    /// The direction of the last slide of the list: 1 into a folder, -1 out of it.
    nav_dir: f32,
    /// The playlists that the app asked Spotify to create and that Spotify does not list yet,
    /// oldest first.
    creating: Vec<Creating>,
    /// The key of the next item.
    next_creating: u64,
    /// The position for the dragged item when it was last between rows. The slot stays there
    /// while the item is over a folder.
    last_drop: Option<usize>,
    /// The folder rows drawn in the last frame. They tell if a dragged item is over a folder
    /// before the rows move.
    folder_rects: Vec<(String, Rect)>,
    /// An item dropped on a folder, which flies into it.
    swallow: Option<Swallow>,
    /// The menu of a folder (right click), and true if it is open (it fades out after it closes).
    menu: Option<FolderMenu>,
    menu_open: bool,
    /// The menu of a playlist (right click), with «Share».
    playlist_menu: ContextMenu,
    /// The name that the user edits in a row. A field replaces the name.
    naming: Option<Naming>,
}

/// The item that the field of a row gives a name to.
#[derive(Clone, PartialEq, Eq)]
enum Named {
    /// The folder with this ID, with a new name.
    Folder(String),
    /// A new folder…
    NewFolder,
    /// …or a new playlist. Their card shows at the top of the level until the app creates it.
    NewPlaylist,
}

/// The name that the user edits in a row: the name of a folder, or of a new item.
struct Naming {
    named: Named,
    name: String,
    /// The field should get the focus (and select the name) when the click that opened it ends.
    focus: bool,
    /// The time that the field opened. Each time, a new card fades in.
    since: f64,
    /// The seconds that a new card waits before it fades in, while the rows make space for it.
    /// This is 0 if the card opens again while it is still there and fades out.
    delay: f32,
    /// The time that the user dropped a new card, and its opacity at that time. The card fades out
    /// (it does not accept input), and then the rows close the gap.
    closing: Option<(f64, f32)>,
}

impl Naming {
    fn new(named: Named, now: f64, delay: f32) -> Self {
        Self {
            named,
            name: String::new(),
            focus: true,
            since: now,
            delay,
            closing: None,
        }
    }

    /// The card of a new item (not a folder with a new name).
    fn is_new(&self) -> bool {
        !matches!(self.named, Named::Folder(_))
    }

    /// The user gives the folder `id` a new name.
    fn renames(&self, id: &str) -> bool {
        matches!(&self.named, Named::Folder(folder) if folder == id)
    }

    /// The opacity of the card of a new item. It fades in after the rows make space for it, and
    /// fades out when the user drops it. `None` after it fades out.
    fn card_alpha(&self, ctx: &Context) -> Option<f32> {
        match self.closing {
            None => {
                let t = (anim::elapsed_since(ctx, self.since) - self.delay) / NEW_CARD_FADE;
                Some(ease_out_cubic(t))
            }
            Some((at, from)) => {
                // Same speed as a full fade: from half opacity, half the time.
                let t = anim::elapsed_since(ctx, at) / (NEW_CARD_FADE * from);
                (from > 0.0 && t < 1.0).then_some(from * (1.0 - t))
            }
        }
    }
}

/// A playlist that the app asked Spotify to create and that Spotify does not list yet. Its card
/// stays at the top of the level where the user created it, with «New playlist» under its name.
struct Creating {
    /// The key of its row.
    key: u64,
    name: String,
    /// The folder that showed when the user created it (`None`: the first level).
    level: Option<String>,
    /// Its ID, after Spotify creates it. The card goes away when the list has the playlist, and
    /// its row replaces the card.
    id: Option<String>,
}

struct Swallow {
    /// The cover of the floating copy when the user dropped it…
    from: Rect,
    /// …and the cover of the folder that it went into.
    to: Rect,
    since: f64,
    thumb: Thumb,
}

/// The cover of a dragged item.
#[derive(Clone)]
enum Thumb {
    Cover(Option<String>),
    Folder(Color32),
}

struct FolderMenu {
    id: String,
    pos: Pos2,
    since: f64,
}

/// An item of the level that shows.
#[derive(Clone, Copy)]
enum Shown<'a> {
    Playlist(&'a Playlist),
    Folder(&'a Folder),
}

impl Shown<'_> {
    fn key(&self) -> LibraryKey {
        match self {
            Self::Playlist(playlist) => LibraryKey::Playlist(playlist.id.clone()),
            Self::Folder(folder) => LibraryKey::Folder(folder.id.clone()),
        }
    }
}

/// A row of the list: an item (its index), the slot for the dragged item, the card of a new item,
/// or the card of a playlist that Spotify creates (its index in [`PlaylistsView::creating`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    Item(usize),
    Slot,
    NewCard,
    Creating(usize),
}

pub struct PlaylistsData<'a> {
    pub status: &'a PlaylistsStatus,
    /// The folders and the order of the playlists.
    pub library: &'a Library,
    /// The playlist with its tracks on the screen.
    pub selected: Option<&'a str>,
    pub accent: Color32,
}

impl PlaylistsView {
    /// The app adds a track that the user dropped on the playlist `id`. Its row shows a ✓.
    pub fn on_added(&mut self, id: &str, time: f64) {
        self.added_at.insert(id.to_owned(), time);
    }

    /// Spotify answered the creation of the playlist `name`: it created it as `id`, or not
    /// (`None`: its card goes away).
    pub fn on_created(&mut self, name: &str, id: Option<String>) {
        let waiting = |c: &Creating| c.id.is_none();
        let Some(index) = self
            .creating
            .iter()
            .position(|c| waiting(c) && c.name == name)
            .or_else(|| self.creating.iter().position(waiting))
        else {
            return;
        };
        match id {
            Some(id) => self.creating[index].id = Some(id),
            None => {
                self.creating.remove(index);
            }
        }
    }

    /// The cards of the playlists that Spotify created go away when the list has them. Each row
    /// replaces its card, without a fade-in.
    fn settle_created(&mut self, ctx: &Context, playlists: &[Playlist], library: &Library) {
        self.creating.retain(|creating| {
            let Some(id) = &creating.id else {
                return true;
            };
            if !playlists.iter().any(|p| &p.id == id) {
                return true;
            }
            let key = LibraryKey::Playlist(id.clone());
            if let Some(level) = library.parent_of(&key) {
                anim::settle_change(ctx, row_id(level, &key).with("born"), None::<u64>);
            }
            false
        });
    }

    /// The playlist that the user clicked after the last call.
    pub fn take_clicked(&mut self) -> Option<Playlist> {
        self.clicked.take()
    }

    /// Shows the folder `folder` (`None`: the first level). The list slides in from the side of
    /// `dir` (1: deeper).
    fn go_to(&mut self, folder: Option<String>, dir: f32) {
        if self.folder != folder {
            self.folder = folder;
            self.nav_dir = dir;
            self.last_drop = None;
            self.folder_rects.clear();
            self.menu_open = false;
            self.playlist_menu.close();
            self.naming = None;
        }
    }

    /// Draws the sidebar. Returns true if a dragged item is over a position where the user can
    /// drop it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        data: &PlaylistsData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        self.added_at.retain(|_, at| time - *at < 2.0);

        let loaded = match data.status {
            PlaylistsStatus::Loaded(playlists) => {
                self.settle_created(&ctx, playlists, data.library);
                true
            }
            _ => {
                self.creating.clear();
                false
            }
        };
        // The folder that shows can be gone (deleted, or moved by the user).
        if loaded
            && let Some(folder) = &self.folder
            && data.library.folder(folder).is_none()
        {
            self.go_to(None, -1.0);
        }
        if !loaded
            || self.naming.as_ref().is_some_and(|n| match &n.named {
                Named::Folder(id) => data.library.folder(id).is_none(),
                Named::NewFolder | Named::NewPlaylist => false,
            })
        {
            self.naming = None;
        }

        let title = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE_HEIGHT));
        let new_center = pos2(title.right() - 10.0, title.top() + HEADER_CENTER_Y);
        let folder_center = new_center - vec2(32.0, 0.0);
        let detail_right = if loaded {
            folder_center.x - 16.0
        } else {
            title.right()
        };
        let moving = dnd::current(&ctx).filter(|d| {
            moving_key(d)
                .is_some_and(|key| data.library.parent_of(&key) == Some(self.folder.as_deref()))
        });
        let over_crumb = self.breadcrumb(ui, title, detail_right, data, moving.as_deref(), actions);
        if loaded {
            self.new_button(ui, new_center);
            self.folder_button(ui, folder_center);
        }

        // The user drags a track or an album: the playlists light up as positions to drop it.
        let dragging_track = dnd::current(&ctx).is_some_and(|d| addable(&d.item));
        let glow = anim::tween_bool(&ctx, Id::unique("playlists-drop-glow"), dragging_track, 0.2);
        if glow > 0.001 {
            ui.painter().rect(
                rect.expand2(vec2(10.0, 8.0)),
                theme::RADIUS_LARGE,
                data.accent.gamma_multiply(0.03 * glow),
                Stroke::new(1.0, data.accent.gamma_multiply(0.3 * glow)),
                StrokeKind::Inside,
            );
        }

        // When the user opens or leaves a folder, the new level slides in.
        let level = anim::since_change(&ctx, Id::unique("playlists-level"), &self.folder, false);
        anim::keep_animating(&ctx, level, LEVEL_SLIDE);
        let slide = ease_out_cubic(level / LEVEL_SLIDE);
        let content = Rect::from_min_max(
            pos2(rect.left() - 6.0, title.bottom() + 6.0),
            pos2(rect.right() + 6.0, rect.bottom()),
        );
        let over_list = match data.status {
            PlaylistsStatus::Loading => {
                widgets::skeleton_rows(ui, content, time, 6);
                false
            }
            PlaylistsStatus::NeedsReauth => {
                if empty_state_with_button(
                    ui,
                    content,
                    "No access to your playlists",
                    "Reconnect your account so the app can see your playlists and add songs to them.",
                    "Reconnect",
                    data.accent,
                    |p, c, color| icons::music_note(p, c, 26.0, color),
                ) {
                    actions.push(Command::Login { client_id: None });
                }
                false
            }
            PlaylistsStatus::Failed(error) => {
                if empty_state_with_button(
                    ui,
                    content,
                    "Could not load them",
                    error,
                    "Try again",
                    data.accent,
                    |p, c, color| icons::music_note(p, c, 26.0, color),
                ) {
                    actions.push(Command::RefreshPlaylists);
                }
                false
            }
            PlaylistsStatus::Loaded(playlists) => {
                let by_id: HashMap<&str, &Playlist> =
                    playlists.iter().map(|p| (p.id.as_str(), p)).collect();
                let shown: Vec<Shown<'_>> = data
                    .library
                    .level(self.folder.as_deref())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|item| match item {
                        LibraryItem::Playlist(id) => {
                            by_id.get(id.as_str()).map(|p| Shown::Playlist(p))
                        }
                        LibraryItem::Folder(folder) => Some(Shown::Folder(folder)),
                    })
                    .collect();
                let cards = self.naming.as_ref().is_some_and(Naming::is_new)
                    || self.creating.iter().any(|c| c.level == self.folder);
                if shown.is_empty() && !cards {
                    self.folder_rects.clear();
                    if empty_level(ui, content, self.folder.is_some(), data.accent) {
                        self.open_card(&ctx, Named::NewPlaylist);
                    }
                    false
                } else {
                    let shifted = content.translate(vec2(self.nav_dir * 28.0 * (1.0 - slide), 0.0));
                    let list = ListData {
                        shown: &shown,
                        by_id: &by_id,
                        slide,
                    };
                    self.list(ui, shifted, &list, data, textures, actions)
                }
            }
        };
        self.paint_swallow(ui, textures);
        self.folder_menu(&ctx, data, actions);
        // «Share» is its only choice, and the menu does it.
        let _ = self.playlist_menu.show(&ctx, data.accent);
        over_list || over_crumb
    }

    /// The title: «Playlists» on the first level. In a folder, the previous level and the folder
    /// («Festivals › Resurrection Fest»). A click on the previous level goes back to it. When the
    /// user drops the playlist or folder `moving` on it, it receives that item. Returns true if
    /// `moving` is over it.
    fn breadcrumb(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        detail_right: f32,
        data: &PlaylistsData<'_>,
        moving: Option<&Dragged>,
        actions: &mut Vec<Command>,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let path = self
            .folder
            .as_deref()
            .map(|id| data.library.path_to(id))
            .unwrap_or_default();
        let Some(current) = path.last() else {
            widgets::list_title(
                ui,
                rect,
                Id::unique("playlists-title"),
                "Playlists",
                "Playlists",
                "",
                detail_right,
            );
            return false;
        };
        let parent = path.len().checked_sub(2).map(|i| path[i]);
        let previous = parent.map_or("Playlists", |f| f.name.as_str());
        let accent = data.accent;
        let id = Id::unique("playlists-crumb");

        // It slides in with the list.
        let since = anim::since_change(&ctx, id.with("level"), &self.folder, false);
        anim::keep_animating(&ctx, since, LEVEL_SLIDE);
        let t = ease_out_cubic(since / LEVEL_SLIDE);
        let dx = self.nav_dir * 12.0 * (1.0 - t);
        let mut crumb_ui = ui.new_child(UiBuilder::new().id_salt(id).max_rect(rect));
        crumb_ui.multiply_opacity(t);

        let font = theme::strong(theme::FONT_TITLE);
        // The previous level has the same size as the folder: both are the title.
        let previous_font = font.clone();
        let width = (detail_right - rect.left()).max(80.0);
        // The previous level gets the space that the current level does not use (the current
        // level keeps half or more). But it never gets less than a third.
        let current_width = crumb_ui
            .painter()
            .layout_no_wrap(current.name.clone(), font.clone(), theme::TEXT)
            .size()
            .x;
        let previous_max = (width - 22.0 - current_width.min(width * 0.5)).max(width * 0.33);
        let previous_galley = single_line(
            crumb_ui.painter(),
            previous,
            previous_font.clone(),
            theme::TEXT_DIM,
            previous_max,
        );
        let size = previous_galley.size();
        let top = rect.center().y - size.y / 2.0 - 1.0;
        let previous_rect = Rect::from_min_size(pos2(rect.left() + dx, top), size);
        let button = previous_rect.expand2(vec2(6.0, 4.0));

        // The user can drag a playlist or folder of this level up to here.
        let pointer = ctx.pointer_latest_pos();
        let hot = moving.is_some() && pointer.is_some_and(|p| button.expand(6.0).contains(p));
        let glow = anim::tween_bool(&ctx, id.with("target"), moving.is_some(), 0.2);
        let hot_t = anim::tween_bool(&ctx, id.with("hot"), hot, 0.12);
        let took = anim::since_trigger(&ctx, id.with("took"), false);
        anim::keep_animating(&ctx, took, TOOK_POP);
        let flash = if took < TOOK_POP {
            1.0 - took / TOOK_POP
        } else {
            0.0
        };
        let response = widgets::custom_button(
            &mut crumb_ui,
            button,
            id,
            &format!("Back to {previous}"),
            true,
            |painter, button, hover, _press| {
                let fill = 0.08 * glow + 0.14 * hot_t + 0.3 * flash;
                if fill > 0.0 {
                    painter.rect(
                        button,
                        theme::RADIUS_SMALL,
                        accent.gamma_multiply(fill),
                        Stroke::new(
                            1.0 + 0.5 * hot_t,
                            accent.gamma_multiply(0.35 * glow + 0.55 * hot_t),
                        ),
                        StrokeKind::Inside,
                    );
                }
                let color = lerp_color(theme::TEXT_DIM, theme::TEXT, hover.max(hot_t));
                let galley = single_line(
                    painter,
                    previous,
                    previous_font.clone(),
                    color,
                    previous_max,
                );
                painter.galley(previous_rect.min, galley, color);
            },
        );
        let response = if previous_galley.elided {
            response.on_hover_text(previous)
        } else {
            response
        };
        if response.clicked() {
            self.go_to(parent.map(|f| f.id.clone()), -1.0);
        }

        let chevron = pos2(previous_rect.right() + 11.0, rect.center().y - 1.5);
        icons::chevron(crumb_ui.painter(), chevron, 12.0, true, theme::TEXT_FAINT);
        let current_left = previous_rect.right() + 22.0;
        let current_galley = single_line(
            crumb_ui.painter(),
            &current.name,
            font,
            theme::TEXT,
            (detail_right - current_left).max(30.0),
        );
        let current_rect = Rect::from_min_size(
            pos2(
                current_left,
                rect.center().y - current_galley.size().y / 2.0 - 2.0,
            ),
            current_galley.size(),
        );
        widgets::full_text_tooltip(
            &crumb_ui,
            current_rect,
            id.with("current"),
            &current_galley,
            &current.name,
        );
        crumb_ui
            .painter()
            .galley(current_rect.min, current_galley, theme::TEXT);
        widgets::a11y_label(
            &crumb_ui,
            current_rect,
            Id::unique("playlists-title"),
            &current.name,
        );

        // The user dropped an item on it: the item goes to the previous level, immediately after
        // this folder.
        if hot
            && dnd::released(&ctx)
            && let Some(dragged) = moving
            && let Some(key) = moving_key(dragged)
            && dnd::take(&ctx).is_some()
        {
            let to = parent.map(|f| f.id.clone());
            let index = data
                .library
                .level(to.as_deref())
                .and_then(|items| {
                    items.iter().position(
                        |item| matches!(item, LibraryItem::Folder(f) if f.id == current.id),
                    )
                })
                .map(|at| at + 1);
            actions.push(Command::EditLibrary(LibraryEdit::Move {
                item: key,
                to,
                index,
            }));
            anim::since_trigger(&ctx, id.with("took"), true);
            if let Some(pointer) = pointer {
                let to = Rect::from_center_size(previous_rect.left_center(), Vec2::splat(24.0));
                self.start_swallow(&ctx, dragged, pointer, to);
            }
        }
        hot
    }

    /// ＋ adds the card of a new playlist at the top of the level.
    fn new_button(&mut self, ui: &mut Ui, center: Pos2) {
        let response = widgets::icon_button(
            ui,
            center,
            28.0,
            Id::unique("new-playlist"),
            "New playlist",
            true,
            |painter, center, color| {
                let stroke = Stroke::new(1.6, color);
                for arm in [vec2(5.0, 0.0), vec2(0.0, 5.0)] {
                    painter.line_segment([center - arm, center + arm], stroke);
                }
            },
        );
        if response.clicked() {
            self.open_card(ui.ctx(), Named::NewPlaylist);
        }
    }

    /// Adds the card of a new folder at the top of the level.
    fn folder_button(&mut self, ui: &mut Ui, center: Pos2) {
        let response = widgets::icon_button(
            ui,
            center,
            28.0,
            Id::unique("new-folder"),
            "New folder",
            true,
            |painter, center, color| icons::folder_plus(painter, center, 18.0, color),
        );
        if response.clicked() {
            self.open_card(ui.ctx(), Named::NewFolder);
        }
    }

    /// Adds the card of a new item (`named`) at the top of the level, with the focus on the field
    /// for its name. If the card is already there, it only gets the focus. A card of the other
    /// type changes into this one at the same position.
    fn open_card(&mut self, ctx: &Context, named: Named) {
        let now = ctx.input(|i| i.time);
        match &mut self.naming {
            Some(naming) if naming.named == named => {
                naming.focus = true;
                // The card fades out: the rows still keep space for it.
                if naming.closing.take().is_some() {
                    naming.since = now;
                    naming.delay = 0.0;
                }
            }
            // The rows already keep space for it.
            Some(naming) if naming.is_new() => *naming = Naming::new(named, now, 0.0),
            _ => self.naming = Some(Naming::new(named, now, NEW_CARD_SHIFT)),
        }
    }

    /// Keeps the name that the user edits as it was. The card of a new item first fades out.
    /// Then the rows below close the gap.
    fn cancel_naming(&mut self, ctx: &Context) {
        match &mut self.naming {
            Some(naming) if naming.is_new() => {
                if naming.closing.is_none() {
                    let alpha = naming.card_alpha(ctx).unwrap_or(0.0);
                    naming.closing = Some((ctx.input(|i| i.time), alpha));
                    naming.focus = false;
                }
            }
            _ => self.naming = None,
        }
    }

    fn list(
        &mut self,
        ui: &mut Ui,
        content: Rect,
        list: &ListData<'_, '_>,
        data: &PlaylistsData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) -> bool {
        let shown = list.shown;
        let accent = data.accent;
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        let dragged = dnd::current(&ctx);
        // A drag removes the focus from the name field, which then closes. The card of a new item
        // would only be in the way.
        if dragged.is_some() && self.naming.as_ref().is_some_and(Naming::is_new) {
            self.naming = None;
        }
        // The card of a new item, and its opacity. It stays (and the rows keep space for it) until
        // it fades out.
        let mut new_card = self
            .naming
            .as_ref()
            .filter(|n| n.is_new())
            .map(|n| n.card_alpha(&ctx));
        if new_card == Some(None) {
            self.naming = None;
            new_card = None;
        }
        let new_card = new_card.flatten();
        let closing = self.naming.as_ref().is_some_and(|n| n.closing.is_some());
        if new_card.is_some_and(|alpha| alpha < 1.0 || closing) {
            ctx.request_repaint();
        }
        // The rows make space for it (or close the gap) a bit faster than they glide in other
        // cases.
        let new_card_moved = new_card.is_some() != self.new_card_shown;
        self.new_card_shown = new_card.is_some();
        let level = self.folder.clone();
        // The playlists that Spotify creates on this level, newest first. Their cards go above the
        // rows (under the card of a new item).
        let creating: Vec<usize> = (0..self.creating.len())
            .rev()
            .filter(|i| self.creating[*i].level == level)
            .collect();
        let cards_above = creating.len() as f32 * ROW_HEIGHT;
        let glide = if new_card_moved {
            NEW_CARD_SHIFT
        } else {
            ROW_GLIDE
        };
        // A track or album that can be added to a playlist.
        let dragged_track = dragged
            .as_ref()
            .map(|d| &d.item)
            .filter(|item| addable(item));
        let pointer = ctx.pointer_latest_pos();
        let pointer_in = pointer.filter(|p| content.contains(*p));
        let mut over_target = false;

        // The user drags an item of the list: it moves to where the user drops it.
        let from = dragged.as_ref().and_then(|d| moving_from(d, shown));
        if from.is_none() {
            self.last_drop = None;
        }
        let moving_folder = from.and_then(|i| match shown[i] {
            Shown::Folder(folder) => Some(folder.id.as_str()),
            Shown::Playlist(_) => None,
        });
        // When the item is over the middle of a folder, it goes into that folder.
        let into = match (from, pointer_in) {
            (Some(_), Some(p)) => self
                .folder_rects
                .iter()
                .find(|(id, rect)| {
                    Some(id.as_str()) != moving_folder
                        && rect.shrink2(vec2(0.0, ROW_HEIGHT * 0.2)).contains(p)
                })
                .map(|(id, rect)| (id.clone(), *rect)),
            _ => None,
        };
        let over = from.is_some() && pointer_in.is_some();
        // The slot opens where the item would go. While the item is away (over the queue or a
        // folder), the slot stays at its last position.
        let slot_shown = from.is_some();
        let slot_id = Id::unique("playlists-drop-slot");
        let slot_alpha = anim::tween_bool(&ctx, slot_id.with("alpha"), slot_shown, 0.2);
        let slot_appeared = slot_shown && !self.slot_shown;
        self.slot_shown = slot_shown;
        let mut folder_rects = Vec::new();
        let mut open_folder = None;

        let mut list_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("playlists-list")
                .max_rect(content)
                .layout(Layout::top_down(Align::Min)),
        );
        list_ui.set_clip_rect(content.intersect(ui.clip_rect()));
        list_ui.multiply_opacity(list.slide);
        let scrolled = egui::ScrollArea::vertical()
            .id_salt(("playlists-scroll", &level))
            .auto_shrink([false, false])
            .show(&mut list_ui, |ui| {
                let width = ui.available_width();
                let (area, _) = ui.allocate_exact_size(
                    vec2(
                        width,
                        (shown.len() + creating.len() + usize::from(new_card.is_some())) as f32
                            * ROW_HEIGHT
                            + 8.0,
                    ),
                    Sense::hover(),
                );
                let viewport = ui.clip_rect();

                // Near the top or bottom edge, the list scrolls by itself.
                if let Some(pointer) = pointer_in
                    && (dragged_track.is_some() || from.is_some())
                {
                    dnd::autoscroll(ui, area, pointer.y);
                }

                // The position of the dragged item: before the first other item with its middle
                // below the middle of the floating copy.
                let drop_to = match (&dragged, pointer_in) {
                    (Some(_), Some(_)) if over && into.is_some() => self.last_drop.or(from),
                    (Some(d), Some(pointer)) if over => {
                        let center = d.landing_y(pointer.y) - area.top() - cards_above;
                        let at = (0..shown.len() - 1)
                            .filter(|i| center > (*i as f32 + 0.5) * ROW_HEIGHT)
                            .count();
                        self.last_drop = Some(at);
                        Some(at)
                    }
                    _ => None,
                };
                let mut entries: Vec<Entry> = (0..shown.len())
                    .filter(|i| from != Some(*i))
                    .map(Entry::Item)
                    .collect();
                if let Some(at) = drop_to.or(from) {
                    entries.insert(at.min(entries.len()), Entry::Slot);
                }
                // The app creates new items at the top: their cards are there.
                for (place, index) in creating.iter().enumerate() {
                    entries.insert(place, Entry::Creating(*index));
                }
                if new_card.is_some() {
                    entries.insert(0, Entry::NewCard);
                }
                if let (Some(from), Some(d), Some(pointer)) = (from, &dragged, pointer_in)
                    && dnd::released(&ctx)
                {
                    let item = shown[from].key();
                    if let Some((folder, folder_rect)) = &into {
                        if dnd::take(&ctx).is_some() {
                            actions.push(Command::EditLibrary(LibraryEdit::Move {
                                item,
                                to: Some(folder.clone()),
                                index: None,
                            }));
                            anim::since_trigger(&ctx, took_id(folder), true);
                            self.start_swallow(&ctx, d, pointer, thumb_rect(*folder_rect));
                        }
                    } else if let Some(to) = drop_to
                        && dnd::take(&ctx).is_some()
                    {
                        if from != to {
                            actions.push(Command::EditLibrary(LibraryEdit::Move {
                                item: item.clone(),
                                to: level.clone(),
                                index: Some(to),
                            }));
                        }
                        // It glides from the drop point to its position.
                        dnd::glide_from_drop(
                            &ctx,
                            row_id(level.as_deref(), &item),
                            d.top(pointer.y) - area.top(),
                        );
                    }
                }

                for (place, entry) in entries.iter().enumerate() {
                    let target_y = place as f32 * ROW_HEIGHT;
                    let (id, index) = match entry {
                        Entry::Item(index) => {
                            (row_id(level.as_deref(), &shown[*index].key()), Some(*index))
                        }
                        Entry::Slot => (slot_id, None),
                        Entry::NewCard => (Id::unique("playlists-new-card"), None),
                        Entry::Creating(index) => (
                            Id::unique(("playlists-creating", self.creating[*index].key)),
                            None,
                        ),
                    };
                    if (*entry == Entry::Slot && slot_appeared) || *entry == Entry::NewCard {
                        anim::set_tween_value(&ctx, id.with("y"), target_y);
                    }
                    let offset = anim::tween(&ctx, id.with("y"), target_y, glide);
                    // New rows (a level that opens, a new folder) fade in one after the other. The
                    // card of a new item fades in and out by itself. The card of a playlist that
                    // Spotify creates replaces it without a change.
                    let appear = match (entry, new_card) {
                        (Entry::NewCard, Some(alpha)) => alpha,
                        (Entry::Creating(_), _) => 1.0,
                        _ => {
                            let born = anim::since_change(&ctx, id.with("born"), None::<u64>, true);
                            let appear_for =
                                place as f32 * anim::ROW_APPEAR_DELAY + anim::ROW_APPEAR_DURATION;
                            anim::keep_animating(&ctx, born, appear_for);
                            ease_out_cubic(anim::stagger(
                                born,
                                place,
                                anim::ROW_APPEAR_DELAY,
                                anim::ROW_APPEAR_DURATION,
                            ))
                        }
                    };
                    let rect = Rect::from_min_size(
                        pos2(area.left(), area.top() + offset),
                        vec2(width, ROW_HEIGHT),
                    );
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    // New rows move up into position. The card of a new item moves down into
                    // position, from the header above it.
                    let rise = if *entry == Entry::NewCard { -6.0 } else { 6.0 };
                    let shown_rect = rect.translate(vec2(0.0, rise * (1.0 - appear)));
                    let Some(i) = index else {
                        match entry {
                            Entry::NewCard => {
                                self.new_card(ui, shown_rect, id, appear, data, actions);
                            }
                            Entry::Creating(index) => {
                                creating_card(ui, shown_rect, id, &self.creating[*index], accent);
                            }
                            Entry::Slot | Entry::Item(_) => dnd::drop_slot(
                                ui,
                                rect,
                                slot_id,
                                over && into.is_none(),
                                slot_alpha,
                                accent,
                            ),
                        }
                        continue;
                    };
                    match shown[i] {
                        Shown::Playlist(playlist) => {
                            // The track goes here if the user drops it.
                            let target = dragged_track.is_some()
                                && pointer
                                    .is_some_and(|p| rect.contains(p) && viewport.contains(p));
                            over_target |= target && playlist.editable;
                            // The ✓ shows after the add (the app can ask the user first).
                            if target
                                && dnd::released(&ctx)
                                && dnd::take(&ctx).is_some()
                                && let Some(item) = dragged_track
                            {
                                actions.extend(add_command(playlist, item));
                            }
                            let cover = playlist
                                .cover
                                .as_deref()
                                .and_then(|url| textures.get(url, THUMB_PX, time));
                            let added = self
                                .added_at
                                .get(&playlist.id)
                                .map(|at| anim::elapsed_since(&ctx, *at));
                            let row = RowState {
                                target,
                                dragging_track: dragged_track.is_some(),
                                selected: data.selected == Some(playlist.id.as_str()),
                                added,
                                appear,
                                new: false,
                                naming: false,
                                menu_open: self.playlist_menu.is_open_for(i),
                            };
                            let row = playlist_row(
                                ui,
                                shown_rect,
                                id,
                                playlist,
                                cover.as_ref(),
                                row,
                                accent,
                            );
                            if row.clicked {
                                self.clicked = Some(playlist.clone());
                            }
                            if let Some(pos) = row.menu_at {
                                let target = Target::playlist(playlist);
                                self.playlist_menu.open(&ctx, target, i, pos, false);
                            }
                            if let Some(grab) = row.drag_from {
                                dnd::start(
                                    &ctx,
                                    DragItem::Playlist(playlist.clone()),
                                    DragOrigin::Playlists(i),
                                    rect,
                                    grab,
                                );
                            }
                        }
                        Shown::Folder(folder) => {
                            folder_rects.push((folder.id.clone(), rect));
                            let hot = into.as_ref().is_some_and(|(id, _)| id == &folder.id);
                            let color = folder_color(folder, list.by_id, textures, time);
                            let state = FolderRowState {
                                hot,
                                dragging_track: dragged_track.is_some(),
                                holds_selected: data
                                    .selected
                                    .is_some_and(|id| folder.holds_playlist(id)),
                                renaming: self
                                    .naming
                                    .as_ref()
                                    .is_some_and(|n| n.renames(&folder.id)),
                                is_new: false,
                                appear,
                            };
                            let row = folder_row(ui, shown_rect, id, folder, color, &state, accent);
                            if state.renaming {
                                self.name_field(
                                    ui,
                                    shown_rect,
                                    row.title,
                                    Some(folder),
                                    data,
                                    actions,
                                );
                            }
                            if row.clicked {
                                open_folder = Some(folder.id.clone());
                            }
                            if let Some(pos) = row.menu_at {
                                self.menu = Some(FolderMenu {
                                    id: folder.id.clone(),
                                    pos,
                                    since: time,
                                });
                                self.menu_open = true;
                            }
                            if let Some(grab) = row.drag_from {
                                dnd::start(
                                    &ctx,
                                    DragItem::Folder(DraggedFolder {
                                        id: folder.id.clone(),
                                        name: folder.name.clone(),
                                        subtitle: folder.count_line(),
                                        color,
                                    }),
                                    DragOrigin::Playlists(i),
                                    rect,
                                    grab,
                                );
                            }
                        }
                    }
                }
            });
        widgets::scroll_hints(&list_ui, &scrolled, (0.0, 0.0));
        self.folder_rects = folder_rects;
        if let Some(folder) = open_folder {
            self.go_to(Some(folder), 1.0);
        }
        over_target || over
    }

    /// The cover of `dragged`, dropped with the pointer at `pointer`, flies into `to` while it
    /// becomes smaller and fades.
    fn start_swallow(&mut self, ctx: &Context, dragged: &Dragged, pointer: Pos2, to: Rect) {
        let thumb = match &dragged.item {
            DragItem::Folder(folder) => Thumb::Folder(folder.color),
            DragItem::Playlist(playlist) => Thumb::Cover(playlist.cover.clone()),
            DragItem::Track(_) | DragItem::Release(_) => return,
        };
        let top_left = pos2(pointer.x - dragged.grab.x, dragged.top(pointer.y));
        let from = Rect::from_min_size(
            top_left
                + vec2(
                    widgets::ROW_COVER_INSET,
                    dragged.size.y / 2.0 - widgets::ROW_COVER / 2.0,
                ),
            Vec2::splat(widgets::ROW_COVER),
        );
        self.swallow = Some(Swallow {
            from,
            to,
            since: ctx.input(|i| i.time),
            thumb,
        });
    }

    fn paint_swallow(&mut self, ui: &Ui, textures: &mut TextureCache) {
        let Some(swallow) = &self.swallow else {
            return;
        };
        let ctx = ui.ctx();
        let t = anim::elapsed_since(ctx, swallow.since) / SWALLOW;
        if t >= 1.0 {
            self.swallow = None;
            return;
        }
        ctx.request_repaint();
        let eased = ease_in_out_cubic(t);
        let end = Rect::from_center_size(swallow.to.center(), swallow.to.size() * 0.3);
        let rect = Rect::from_min_max(
            swallow.from.min.lerp(end.min, eased),
            swallow.from.max.lerp(end.max, eased),
        );
        let alpha = 1.0 - 0.9 * ease_in_quad(t);
        match &swallow.thumb {
            Thumb::Cover(url) => {
                let time = ctx.input(|i| i.time);
                let cover = url
                    .as_deref()
                    .and_then(|url| textures.get(url, THUMB_PX, time));
                widgets::paint_playlist_cover(ui, rect, cover.as_ref(), alpha);
            }
            Thumb::Folder(color) => icons::folder_tile(ui.painter(), rect, *color, 0.0, alpha),
        }
    }

    /// The card of a new item, at the top of the level: a folder or a playlist with no name yet.
    /// The line under it says «New folder» or «New playlist». The card has the field for its name.
    fn new_card(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        id: Id,
        appear: f32,
        data: &PlaylistsData<'_>,
        actions: &mut Vec<Command>,
    ) {
        let Some(naming) = &self.naming else {
            return;
        };
        let closing = naming.closing.is_some();
        let title = if naming.named == Named::NewPlaylist {
            let state = RowState {
                target: false,
                dragging_track: false,
                selected: false,
                added: None,
                appear,
                new: true,
                naming: true,
                menu_open: false,
            };
            let playlist = new_playlist(&naming.name);
            playlist_row(ui, rect, id, &playlist, None, state, data.accent).title
        } else {
            let folder = Folder {
                id: String::new(),
                name: naming.name.clone(),
                items: Vec::new(),
            };
            let state = FolderRowState {
                hot: false,
                dragging_track: false,
                holds_selected: false,
                renaming: true,
                is_new: true,
                appear,
            };
            folder_row(
                ui,
                rect,
                id,
                &folder,
                theme::FOLDER_GRAY,
                &state,
                data.accent,
            )
            .title
        };
        let mut field_ui = ui.new_child(UiBuilder::new().id_salt("new-card-field"));
        field_ui.multiply_opacity(appear);
        if closing {
            // It is not dimmed: it only fades out with the card.
            field_ui.visuals_mut().disabled_alpha = 1.0;
            field_ui.disable();
        }
        self.name_field(&mut field_ui, rect, title, None, data, actions);
    }

    /// The field with the name of `folder` (`None`: a new item), in its row (`row`) on the line of
    /// its name (`title`, where it was drawn). The line under it stays. Enter, ✓ or a click on a
    /// different area changes the name (or creates the item). Esc (or an empty name) keeps it as
    /// it was (or removes the new item). Then the card of a new playlist waits for Spotify to
    /// create it.
    fn name_field(
        &mut self,
        ui: &mut Ui,
        row: Rect,
        title: Rect,
        folder: Option<&Folder>,
        data: &PlaylistsData<'_>,
        actions: &mut Vec<Command>,
    ) {
        let accent = data.accent;
        let Some(renaming) = self.naming.as_mut() else {
            return;
        };
        // A new card that fades out shows only its field.
        let closing = renaming.closing.is_some();
        let named = renaming.named.clone();
        let (id, label, accept_label, hint) = match named {
            Named::Folder(_) => (
                Id::unique("folder-rename"),
                "New name of the folder",
                "Rename folder",
                "Folder name",
            ),
            Named::NewFolder => (
                Id::unique("folder-new-name"),
                "Name of the new folder",
                "Create folder",
                "Folder name",
            ),
            Named::NewPlaylist => (
                Id::unique("new-playlist-name"),
                "Name of the new playlist",
                "Create playlist",
                "Playlist name",
            ),
        };
        // If the field asks for the focus during the click, egui removes it again.
        let clicking = ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released());
        // (When the user drops a new card, its focus request stops.)
        if renaming.focus && !clicking {
            renaming.focus = false;
            ui.memory_mut(|m| m.request_focus(id));
            // Select the full name, so that the user can type a new name in its place.
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::default(),
                    egui::text::CCursor::new(renaming.name.chars().count()),
                )));
            state.store(ui.ctx(), id);
            ui.ctx().request_repaint();
        } else if renaming.focus {
            ui.ctx().request_repaint();
        }
        // The distance from the tile and the card's right edge is the same as the distance from
        // the tile to its left edge. Its text starts at the position of the name.
        let card = row_card(row);
        let gap = thumb_rect(row).left() - card.left();
        let field = Rect::from_min_max(
            pos2(
                title.left() - widgets::COMPACT_FIELD_PAD,
                title.center().y - RENAME_FIELD_HEIGHT / 2.0,
            ),
            pos2(
                card.right() - gap,
                title.center().y + RENAME_FIELD_HEIGHT / 2.0,
            ),
        );
        let response = widgets::text_field(
            ui,
            field,
            id,
            label,
            &mut renaming.name,
            hint,
            widgets::FieldStyle::Compact,
            accent,
        );
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let escape = (response.has_focus() || response.lost_focus())
            && ui.input(|i| i.key_pressed(Key::Escape));
        let valid = !renaming.name.trim().is_empty();
        let accept = widgets::icon_button(
            ui,
            pos2(field.right() - 12.0, field.center().y),
            20.0,
            id.with("accept"),
            accept_label,
            valid,
            |painter, center, color| {
                let color = if valid {
                    accent
                } else {
                    color.gamma_multiply(0.5)
                };
                icons::plus_to_check(painter, center, 13.0, 1.0, color);
            },
        );
        if closing {
            return;
        }
        if escape {
            self.cancel_naming(ui.ctx());
        } else if enter && !valid {
            // There is no name yet: continue to type.
            ui.memory_mut(|m| m.request_focus(id));
        } else if response.lost_focus() || accept.clicked() {
            let name = renaming.name.trim().to_owned();
            match (named, folder) {
                (Named::Folder(_), Some(folder)) if valid && name != folder.name => {
                    actions.push(Command::EditLibrary(LibraryEdit::RenameFolder {
                        id: folder.id.clone(),
                        name,
                    }));
                }
                (Named::Folder(_), _) => {}
                // There is no name: remove the new item.
                _ if !valid => {
                    self.cancel_naming(ui.ctx());
                    return;
                }
                (Named::NewPlaylist, _) => {
                    actions.push(Command::CreatePlaylist {
                        name: name.clone(),
                        folder: self.folder.clone(),
                    });
                    // Its card stays until Spotify creates the playlist.
                    self.creating.push(Creating {
                        key: self.next_creating,
                        name,
                        level: self.folder.clone(),
                        id: None,
                    });
                    self.next_creating += 1;
                }
                (Named::NewFolder, _) => {
                    let id = data.library.next_folder_id();
                    // Its card changes into the folder at the same position. It does not fade in
                    // again.
                    let key = LibraryKey::Folder(id.clone());
                    anim::settle_change(
                        ui.ctx(),
                        row_id(self.folder.as_deref(), &key).with("born"),
                        None::<u64>,
                    );
                    actions.push(Command::EditLibrary(LibraryEdit::CreateFolder {
                        id,
                        name,
                        parent: self.folder.clone(),
                    }));
                }
            }
            self.naming = None;
        }
    }

    /// The menu of a folder, which opens with a right click on it. «Rename folder» changes its
    /// name into a field. «Delete folder» puts its contents in its position.
    fn folder_menu(&mut self, ctx: &Context, data: &PlaylistsData<'_>, actions: &mut Vec<Command>) {
        let open = anim::tween_bool(ctx, Id::unique("folder-menu-open"), self.menu_open, 0.16);
        let Some(menu) = &self.menu else {
            return;
        };
        let folder = data.library.folder(&menu.id);
        if folder.is_none() || (!self.menu_open && open <= 0.001) {
            self.menu = None;
            self.menu_open = false;
            return;
        }
        let (id, pos, since) = (menu.id.clone(), menu.pos, menu.since);
        let interactive = self.menu_open;
        let accent = data.accent;
        let (mut rename, mut delete) = (false, false);
        let area = egui::Area::new(Id::unique("folder-menu"))
            .order(Order::Foreground)
            .fixed_pos(pos + vec2(0.0, 4.0 - (1.0 - open) * 8.0))
            .constrain(true)
            .show(ctx, |ui| {
                ui.set_opacity(open);
                if !interactive {
                    ui.disable();
                }
                egui::Frame::new()
                    .corner_radius(theme::RADIUS_LARGE)
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        backdrop::framed(ui, Margin::same(8), accent, |ui| {
                            ui.set_width(MENU_WIDTH);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let id = Id::unique("folder-menu");
                            rename = text_row(
                                ui,
                                MENU_WIDTH,
                                id.with("rename"),
                                "Rename folder",
                                accent,
                            )
                            .clicked();
                            delete = text_row(
                                ui,
                                MENU_WIDTH,
                                id.with("delete"),
                                "Delete folder",
                                accent,
                            )
                            .clicked();
                        });
                    });
            });
        if !interactive {
            return;
        }
        if rename && let Some(folder) = folder {
            let mut naming = Naming::new(Named::Folder(id), ctx.input(|i| i.time), 0.0);
            naming.name.clone_from(&folder.name);
            self.naming = Some(naming);
            self.menu_open = false;
        } else if delete {
            actions.push(Command::EditLibrary(LibraryEdit::DeleteFolder { id }));
            self.menu_open = false;
        } else {
            // The click that opened it does not close it.
            let menu = area.response.rect;
            let clicked_outside = ctx.input(|i| {
                i.time > since
                    && i.pointer.any_click()
                    && i.pointer.interact_pos().is_some_and(|p| !menu.contains(p))
            });
            if clicked_outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.menu_open = false;
            }
        }
    }
}

/// What [`PlaylistsView::list`] draws.
struct ListData<'a, 'b> {
    shown: &'b [Shown<'a>],
    by_id: &'b HashMap<&'a str, &'a Playlist>,
    /// The progress of the slide-in of the level (0..1).
    slide: f32,
}

/// True if the user can drop `item` on a playlist to add it there.
fn addable(item: &DragItem) -> bool {
    matches!(item, DragItem::Track(_) | DragItem::Release(_))
}

/// The command that adds `item` (a track or an album) to `playlist`. For an album, the app first
/// asks which of its tracks the playlist has.
fn add_command(playlist: &Playlist, item: &DragItem) -> Option<Command> {
    match item {
        DragItem::Release(release) => Some(Command::FindAlbumInPlaylist {
            playlist: playlist.clone(),
            id: release.id.clone(),
            name: release.name.clone(),
        }),
        DragItem::Track(track) => Some(Command::EditPlaylist {
            playlist: playlist.clone(),
            edit: PlaylistEdit::Insert {
                index: None,
                track: track.clone(),
            },
        }),
        DragItem::Playlist(_) | DragItem::Folder(_) => None,
    }
}

/// The playlist or folder that the user drags from the sidebar, if it is one.
fn moving_key(dragged: &Dragged) -> Option<LibraryKey> {
    if !matches!(dragged.origin, DragOrigin::Playlists(_)) {
        return None;
    }
    match &dragged.item {
        DragItem::Playlist(playlist) => Some(LibraryKey::Playlist(playlist.id.clone())),
        DragItem::Folder(folder) => Some(LibraryKey::Folder(folder.id.clone())),
        DragItem::Track(_) | DragItem::Release(_) => None,
    }
}

/// The index of the dragged item in the level that shows, if the item is in it.
fn moving_from(dragged: &Dragged, shown: &[Shown<'_>]) -> Option<usize> {
    let key = moving_key(dragged)?;
    let DragOrigin::Playlists(index) = dragged.origin else {
        return None;
    };
    // The list can load again during the drag.
    shown
        .get(index)
        .filter(|item| item.key() == key)
        .map(|_| index)
        .or_else(|| shown.iter().position(|item| item.key() == key))
}

/// Each row has a key from its level and item. Thus, when the order of the list changes, each row
/// glides to its new position. When a level opens again, its rows come in.
fn row_id(level: Option<&str>, key: &LibraryKey) -> Id {
    Id::unique(("playlist-row", level, key))
}

/// The pop of a folder that just received an item.
fn took_id(folder: &str) -> Id {
    Id::unique(("folder-took", folder))
}

/// The position of the cover in a row.
fn thumb_rect(row: Rect) -> Rect {
    Rect::from_min_size(
        pos2(
            row.left() + widgets::ROW_COVER_INSET,
            row.center().y - widgets::ROW_COVER / 2.0,
        ),
        vec2(widgets::ROW_COVER, widgets::ROW_COVER),
    )
}

/// The color of a folder: the color of the cover of its first playlist, or gray if there is no
/// cover.
fn folder_color(
    folder: &Folder,
    playlists: &HashMap<&str, &Playlist>,
    textures: &mut TextureCache,
    time: f64,
) -> Color32 {
    folder
        .first_playlist()
        .and_then(|id| playlists.get(id))
        .and_then(|playlist| playlist.cover.as_deref())
        .and_then(|url| textures.get(url, THUMB_PX, time))
        .and_then(|cover| cover.accent)
        .unwrap_or(theme::FOLDER_GRAY)
}

/// There is nothing to show on this level. Returns true if the user clicked «Create a playlist».
fn empty_level(ui: &mut Ui, rect: Rect, in_folder: bool, accent: Color32) -> bool {
    if in_folder {
        empty_state_with_button(
            ui,
            rect,
            "This folder is empty",
            "Drop playlists on it from the level above, or create a new one here.",
            "Create a playlist",
            accent,
            |p, c, color| icons::folder(p, c, 26.0, 0.0, color),
        )
    } else {
        empty_state_with_button(
            ui,
            rect,
            "You have no playlists",
            "Create your first one to start saving songs.",
            "Create a playlist",
            accent,
            |p, c, color| icons::music_note(p, c, 26.0, color),
        )
    }
}

/// A message with a button under it. Returns true if the user clicked the button.
fn empty_state_with_button(
    ui: &mut Ui,
    rect: Rect,
    title: &str,
    subtitle: &str,
    button: &str,
    accent: Color32,
    paint_icon: impl FnOnce(&egui::Painter, Pos2, Color32),
) -> bool {
    let area = Rect::from_min_size(rect.min, vec2(rect.width(), rect.height().min(320.0)));
    let bottom = widgets::empty_state(
        ui,
        area,
        Id::unique("playlists-message"),
        title,
        subtitle,
        1.0,
        paint_icon,
    );
    let button_rect = Rect::from_center_size(
        pos2(area.center().x, bottom + 32.0),
        vec2((rect.width() - 24.0).min(190.0), 36.0),
    );
    pill_button(
        ui,
        button_rect,
        Id::unique("playlists-message-button"),
        button,
        accent.gamma_multiply(0.22),
        theme::TEXT,
        true,
    )
    .clicked()
}

struct RowState {
    /// A dragged track is over this row.
    target: bool,
    /// The user drags a track to some position.
    dragging_track: bool,
    /// Its tracks show.
    selected: bool,
    /// The seconds after the user dropped a track here, for the ✓.
    added: Option<f32>,
    /// The progress of its fade-in (0..1).
    appear: f32,
    /// Spotify did not create it yet: «New playlist» shows under its name, and it does nothing.
    new: bool,
    /// The user edits its name: the field is on its line in place of the name.
    naming: bool,
    /// Its menu (right click) is open: it stays lit, as on hover.
    menu_open: bool,
}

struct RowActions {
    /// The user clicked it: show its tracks.
    clicked: bool,
    /// The user right-clicked here: its menu opens.
    menu_at: Option<Pos2>,
    /// The position of its name (or of the name to come, while the user types it).
    title: Rect,
    /// The user started to drag it, from this point.
    drag_from: Option<egui::Pos2>,
}

/// The card that the highlight of a row fills, in `rect`.
fn row_card(rect: Rect) -> Rect {
    rect.shrink2(vec2(2.0, 3.0))
}

/// The highlight of playlist and folder rows: hover (or focus), the accent bar of the selected
/// row, and the accent border of a target.
fn row_background(
    painter: &egui::Painter,
    rect: Rect,
    hover: f32,
    focus: f32,
    selected: f32,
    hot: f32,
    accent: Color32,
) {
    let background = row_card(rect);
    if hover > 0.0 || selected > 0.0 {
        painter.rect_filled(
            background,
            theme::RADIUS_MEDIUM,
            Color32::from_white_alpha((10.0 * hover + 12.0 * selected) as u8),
        );
    }
    widgets::focus_ring(painter, background, theme::RADIUS_MEDIUM, focus);
    if selected > 0.0 {
        // A short accent bar on the left edge marks the open playlist.
        let height = 22.0 * ease_out_cubic(selected);
        let bar = Rect::from_center_size(
            pos2(background.left() + 1.5, rect.center().y),
            vec2(3.0, height),
        );
        painter.rect_filled(bar, 1.5, accent.gamma_multiply(selected));
    }
    if hot > 0.0 {
        painter.rect(
            background,
            theme::RADIUS_MEDIUM,
            accent.gamma_multiply(0.14 * hot),
            Stroke::new(1.0 + 0.5 * hot, accent.gamma_multiply(0.9 * hot)),
            StrokeKind::Inside,
        );
    }
}

/// The name and the line under it, from `left`, with the width `width`. A tooltip shows a cut name
/// in full. Returns the position of the name.
#[expect(clippy::too_many_arguments)]
fn row_text(
    ui: &Ui,
    id: Id,
    painter: &egui::Painter,
    rect: Rect,
    left: f32,
    width: f32,
    name: &str,
    subtitle: &str,
    title_color: Color32,
) -> Rect {
    let title = single_line(
        painter,
        name,
        theme::regular(theme::FONT_BODY),
        title_color,
        width,
    );
    let sub = single_line(
        painter,
        subtitle,
        theme::regular(theme::FONT_SMALL),
        theme::TEXT_DIM,
        width,
    );
    let block = title.size().y + 3.0 + sub.size().y;
    let top = rect.center().y - block / 2.0;
    let title_rect = Rect::from_min_size(pos2(left, top), title.size());
    widgets::full_text_tooltip(ui, title_rect, id.with("title"), &title, name);
    painter.galley(title_rect.min, title, title_color);
    painter.galley(pos2(left, title_rect.bottom() + 3.0), sub, theme::TEXT_DIM);
    title_rect
}

/// The point where the button went down. Thus the row stays under the pointer at the point where
/// the user took it.
fn drag_start(ctx: &Context, response: &egui::Response) -> Option<Pos2> {
    response
        .drag_started()
        .then(|| {
            ctx.input(|i| i.pointer.press_origin())
                .or(response.interact_pointer_pos())
        })
        .flatten()
}

/// One playlist: cover, name and count. A click on it shows its tracks. The user can also drag it
/// into the queue or to a different position in the list.
fn playlist_row(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    playlist: &Playlist,
    cover: Option<&super::textures::Cover>,
    state: RowState,
    accent: Color32,
) -> RowActions {
    let ctx = ui.ctx().clone();
    let dragging = dnd::is_dragging(&ctx);
    let sense = if state.new {
        Sense::hover()
    } else {
        Sense::click_and_drag()
    };
    let mut response = ui.interact(rect, id, sense);
    if !dragging && !state.new {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
        if !playlist.editable {
            response = response.on_hover_text(format!(
                "It belongs to {}: from here, Spotify only lets you use your own playlists and the ones you collaborate on.",
                playlist.owner
            ));
        }
    }
    let subtitle = if state.new {
        "New playlist".to_owned()
    } else if playlist.editable {
        playlist.count_line()
    } else {
        format!("By {}", playlist.owner)
    };
    response.widget_info(|| {
        WidgetInfo::labeled(
            Role::Label,
            true,
            if playlist.name.is_empty() {
                // A new playlist, before it has a name.
                format!("{subtitle}, no name yet")
            } else {
                format!("{} — {}", playlist.name, subtitle)
            },
        )
    });

    // Tab goes to the row and Enter opens it (egui changes Enter into a click).
    let focused = response.has_focus();
    let hover = anim::tween_bool(
        &ctx,
        id.with("hover"),
        !dragging && (response.contains_pointer() || focused || state.menu_open),
        0.18,
    );
    let focus = anim::tween_bool(&ctx, id.with("focus"), focused, 0.12);
    let hot = anim::tween_bool(
        &ctx,
        id.with("target"),
        state.target && playlist.editable,
        0.12,
    );
    // While the user drags a track, the playlists that cannot receive it become dim.
    let dim = anim::tween_bool(
        &ctx,
        id.with("dim"),
        state.dragging_track && !playlist.editable,
        0.2,
    );
    let alpha = lerp(1.0, 0.35, dim) * state.appear;
    let selected = anim::tween_bool(&ctx, id.with("selected"), state.selected, 0.18);

    let mut painter = ui.painter().clone();
    painter.multiply_opacity(alpha);
    row_background(&painter, rect, hover, focus, selected, hot, accent);

    let thumb = thumb_rect(rect);
    widgets::paint_playlist_cover(ui, thumb, cover, alpha);

    // ＋ while a track is over it, then ✓ after the drop.
    let check = state.added.map_or(0.0, |t| {
        if t < 1.4 {
            ease_out_cubic(t / 0.25)
        } else {
            1.0 - ease_out_cubic((t - 1.4) / 0.25)
        }
    });
    if state.added.is_some_and(|t| t < 1.7) {
        ctx.request_repaint();
    }
    let badge = hot.max(check);
    let badge_width = if badge > 0.01 { 30.0 } else { 0.0 };

    let text_left = thumb.right() + widgets::ROW_TEXT_GAP;
    let text_width = (rect.right() - 10.0 - badge_width - text_left).max(30.0);
    let title_color = lerp_color(theme::TEXT, accent, 0.6 * hot);
    // While the user types the name, the layout includes the name but it is not visible. The
    // field is on its line, above it.
    let title_color = if state.naming {
        Color32::TRANSPARENT
    } else {
        title_color
    };
    let title = row_text(
        ui,
        id,
        &painter,
        rect,
        text_left,
        text_width,
        &playlist.name,
        &subtitle,
        title_color,
    );

    if badge > 0.01 {
        let center = pos2(rect.right() - 22.0, rect.center().y);
        painter.circle_filled(center, 12.0 * badge, accent.gamma_multiply(0.25 * badge));
        icons::plus_to_check(&painter, center, 13.0, check, accent.gamma_multiply(badge));
    }

    let menu_at = (!dragging && response.secondary_clicked())
        .then(|| response.interact_pointer_pos().or(ctx.pointer_latest_pos()))
        .flatten();
    RowActions {
        clicked: response.clicked(),
        menu_at,
        title,
        drag_from: drag_start(&ctx, &response),
    }
}

/// A playlist that Spotify did not create yet, with the name `name`: empty and of the user.
fn new_playlist(name: &str) -> Playlist {
    Playlist {
        id: String::new(),
        name: name.to_owned(),
        owner: String::new(),
        total: 0,
        cover: None,
        editable: true,
        snapshot: None,
    }
}

/// The card of a playlist that Spotify creates: its row, with «New playlist» under its name.
fn creating_card(ui: &mut Ui, rect: Rect, id: Id, creating: &Creating, accent: Color32) {
    let state = RowState {
        target: false,
        dragging_track: false,
        selected: false,
        added: None,
        appear: 1.0,
        new: true,
        naming: false,
        menu_open: false,
    };
    let playlist = new_playlist(&creating.name);
    playlist_row(ui, rect, id, &playlist, None, state, accent);
}

struct FolderRowState {
    /// A dragged playlist or folder is over it, and goes into it on a drop.
    hot: bool,
    /// The user drags a track to some position (a track cannot go into a folder).
    dragging_track: bool,
    /// The open playlist is in it, at some level.
    holds_selected: bool,
    /// The user edits its name: the field is on its line in place of the name.
    renaming: bool,
    /// The card of a folder that the app did not create yet: «New folder» under its name.
    is_new: bool,
    /// The progress of its fade-in (0..1).
    appear: f32,
}

struct FolderRowActions {
    /// The user clicked it: show its contents.
    clicked: bool,
    /// The user right-clicked here: its menu opens.
    menu_at: Option<Pos2>,
    /// The position of its name (or of the name to come, while the user types it).
    title: Rect,
    /// The user started to drag it, from this point.
    drag_from: Option<Pos2>,
}

/// One folder: its tile (tinted with `color`), name and count. A click on it shows its contents.
/// Its lid opens on hover, and opens wide when a dragged item can go in. The user can drag it to a
/// different position in the list or into a different folder.
fn folder_row(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    folder: &Folder,
    color: Color32,
    state: &FolderRowState,
    accent: Color32,
) -> FolderRowActions {
    let ctx = ui.ctx().clone();
    let dragging = dnd::is_dragging(&ctx);
    let mut response = ui.interact(rect, id, Sense::click_and_drag());
    if !dragging {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    }
    let subtitle = if state.is_new {
        "New folder".to_owned()
    } else {
        folder.count_line()
    };
    response.widget_info(|| {
        WidgetInfo::labeled(
            Role::Label,
            true,
            if folder.name.is_empty() {
                // A new folder, before it has a name.
                format!("{subtitle}, no name yet")
            } else {
                format!("{} — {}", folder.name, subtitle)
            },
        )
    });

    let focused = response.has_focus();
    let hover = anim::tween_bool(
        &ctx,
        id.with("hover"),
        !dragging && (response.contains_pointer() || focused),
        0.18,
    );
    let focus = anim::tween_bool(&ctx, id.with("focus"), focused, 0.12);
    let hot = anim::tween_bool(&ctx, id.with("target"), state.hot, 0.12);
    let dim = anim::tween_bool(&ctx, id.with("dim"), state.dragging_track, 0.2);
    let alpha = lerp(1.0, 0.35, dim) * state.appear;
    // Its bar is fainter than the bar of a playlist: the open item is in it.
    let selected = 0.55 * anim::tween_bool(&ctx, id.with("selected"), state.holds_selected, 0.18);
    // Its color fades in when the cover of that color loads.
    let tint = anim::tween(
        &ctx,
        id.with("tint"),
        f32::from(color != theme::FOLDER_GRAY),
        0.3,
    );
    let color = lerp_color(theme::FOLDER_GRAY, color, tint);

    let mut painter = ui.painter().clone();
    painter.multiply_opacity(alpha);
    row_background(&painter, rect, hover, focus, selected, hot, accent);

    // The lid opens a little on hover and wide over a target. The folder pops when it receives
    // an item.
    let lid = (0.35 * hover).max(hot);
    let took = anim::since_trigger(&ctx, took_id(&folder.id), false);
    anim::keep_animating(&ctx, took, TOOK_POP);
    let pop = if took < TOOK_POP {
        1.0 + 0.12 * bump(took / TOOK_POP)
    } else {
        1.0
    };
    let thumb = thumb_rect(rect).scale_from_center(pop);
    icons::folder_tile(&painter, thumb, color, lid, 1.0);

    let text_left = thumb_rect(rect).right() + widgets::ROW_TEXT_GAP;
    let text_width = (rect.right() - 34.0 - text_left).max(30.0);
    let title_color = lerp_color(theme::TEXT, accent, 0.6 * hot);
    // While the user changes the name, the layout includes the name but it is not visible. The
    // field is on its line, above it.
    let title_color = if state.renaming {
        Color32::TRANSPARENT
    } else {
        title_color
    };
    let title = row_text(
        ui,
        id,
        &painter,
        rect,
        text_left,
        text_width,
        &folder.name,
        &subtitle,
        title_color,
    );
    // A chevron on hover: a click opens the folder.
    if hover > 0.0 && !state.renaming {
        icons::chevron(
            &painter,
            pos2(rect.right() - 18.0 + 3.0 * (1.0 - hover), rect.center().y),
            14.0,
            true,
            theme::TEXT_DIM.gamma_multiply(hover),
        );
    }

    let menu_at = response
        .secondary_clicked()
        .then(|| response.interact_pointer_pos().or(ctx.pointer_latest_pos()))
        .flatten();
    FolderRowActions {
        clicked: response.clicked(),
        menu_at,
        title,
        drag_from: drag_start(&ctx, &response),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card's opacity at `time` (seconds), at egui's default speed.
    fn alpha_at(naming: &Naming, time: f64) -> Option<f32> {
        let ctx = Context::default();
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let mut alpha = None;
        let mut output = ctx.run_ui(input, |ui| alpha = naming.card_alpha(ui.ctx()));
        output.textures_delta.clear();
        alpha
    }

    fn new_card() -> Naming {
        Naming::new(Named::NewFolder, 10.0, NEW_CARD_SHIFT)
    }

    #[test]
    fn a_new_folder_card_fades_in_once_the_rows_made_room_for_it() {
        let card = new_card();
        assert_eq!(
            alpha_at(&card, 10.1),
            Some(0.0),
            "the rows are still moving"
        );
        assert_eq!(alpha_at(&card, 10.2), Some(0.0));
        let mid = alpha_at(&card, 10.3).unwrap();
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(alpha_at(&card, 10.4), Some(1.0));
    }

    #[test]
    fn a_dropped_new_folder_card_fades_out_before_the_gap_closes() {
        let mut card = new_card();
        card.closing = Some((20.0, 1.0));
        let mid = alpha_at(&card, 20.1).unwrap();
        assert!((mid - 0.5).abs() < 1e-3);
        assert_eq!(alpha_at(&card, 20.2), None, "gone: the rows move up");

        // The user dropped it at half opacity: it fades out in half the time.
        card.closing = Some((20.0, 0.5));
        assert!(alpha_at(&card, 20.05).is_some());
        assert_eq!(alpha_at(&card, 20.1), None);
    }
}
