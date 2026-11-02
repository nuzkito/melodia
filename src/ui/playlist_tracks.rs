//! The tracks of the playlist that the user clicked in the sidebar, in the middle column. A drag
//! changes their order or adds tracks from other areas. The ✕ removes them. A double click on the
//! name changes the name of the playlist.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{
    Color32, Context, CursorIcon, Id, Key, OpenUrl, Pos2, Rect, Sense, Ui, UiBuilder, pos2, vec2,
};

use super::anim::{self, ROW_APPEAR_DELAY, ROW_APPEAR_DURATION, ease_out_cubic};
use super::context_menu::{self, Choice, ContextMenu, Target};
use super::detail;
use super::dnd::{self, DragItem, DragOrigin, Dragged};
use super::icons;
use super::queue_keys::QueueKeys;
use super::textures::{THUMB_PX, TextureCache};
use super::theme;
use super::track_row::{ROW_HEIGHT, RowStyle, track_row};
use super::widgets::{self, LIST_TITLE_HEIGHT};
use crate::backend::{Command, PLAYLIST_NOT_READABLE};
use crate::model::{PlaybackSnapshot, Playlist, PlaylistEdit, Track, find_near};

#[derive(Default)]
pub struct PlaylistTracksView {
    open: Option<OpenPlaylist>,
    /// The playlist that just closed, shown while the panel fades out.
    closing: Option<OpenPlaylist>,
    /// The tracks of the playlists that the user opened before (id → tracks). When the user opens
    /// one again, they show immediately while they load again.
    cache: HashMap<String, Vec<Track>>,
    /// The tracks that the user recently added to the queue (uri → time), for the ✓ feedback.
    added_at: HashMap<String, f64>,
}

struct OpenPlaylist {
    playlist: Playlist,
    tracks: Option<Vec<Track>>,
    /// A request for the tracks is in progress.
    loading: bool,
    error: Option<String>,
    /// The arrival time of the tracks, for the entrance animation.
    arrived_at: f64,
    /// A key for each track, for the animations of its row (a track can be in the playlist more
    /// than one time).
    keys: QueueKeys,
    /// The drop slot was on the screen in the last frame.
    slot_shown: bool,
    /// The new name that the user types, while the user edits the name.
    renaming: Option<String>,
    /// The name field should get the focus when the click that opened it ends.
    focus_rename: bool,
    /// The menu of a track, which opens with a right click.
    menu: ContextMenu,
}

/// A row of the list: a track (its index) or the slot for the dragged track.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    Track(usize),
    Slot,
}

/// A dragged track that the open playlist accepts.
struct Dragging {
    dragged: Arc<Dragged>,
    pointer: Pos2,
    /// The user moves the track at this index of this list. `None` for a track from a different
    /// area, which the drop adds.
    from: Option<usize>,
}

pub struct PlaylistTracksData<'a> {
    pub playback: &'a PlaybackSnapshot,
    pub accent: Color32,
}

impl PlaylistTracksView {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The playlist that shows, if there is one.
    pub fn open_id(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.playlist.id.as_str())
    }

    /// Shows the tracks of `playlist`, or hides them if they already show.
    pub fn toggle(&mut self, playlist: Playlist, time: f64, actions: &mut Vec<Command>) {
        if self.open_id() == Some(playlist.id.as_str()) {
            self.close();
        } else {
            self.open(playlist, time, actions);
        }
    }

    /// Shows the tracks of `playlist`, and loads them again.
    pub fn open(&mut self, playlist: Playlist, time: f64, actions: &mut Vec<Command>) {
        let tracks = self.cache.get(&playlist.id).cloned();
        let error = (!playlist.editable).then(|| PLAYLIST_NOT_READABLE.to_owned());
        let loading = playlist.editable;
        if loading {
            actions.push(Command::LoadPlaylistTracks {
                playlist: playlist.clone(),
            });
        }
        self.closing = None;
        self.open = Some(OpenPlaylist {
            playlist,
            tracks,
            loading,
            error,
            arrived_at: time,
            keys: QueueKeys::default(),
            slot_shown: false,
            renaming: None,
            focus_rename: false,
            menu: ContextMenu::default(),
        });
    }

    pub fn close(&mut self) {
        if let Some(open) = self.open.take() {
            self.closing = Some(open);
        }
    }

    /// True if `playlist` has the track `uri`, from the tracks read when it opened. `None` if
    /// they are not known, or only partly known.
    pub fn has(&self, playlist: &Playlist, uri: &str) -> Option<bool> {
        let (tracks, total) = match &self.open {
            Some(open) if open.playlist.id == playlist.id => {
                (open.tracks.as_ref()?, open.playlist.total)
            }
            _ => (self.cache.get(&playlist.id)?, playlist.total),
        };
        let found = tracks.iter().any(|track| track.uri == uri);
        (found || tracks.len() as u32 >= total).then_some(found)
    }

    /// The tracks of `id` arrived, with the number of its entries now.
    pub fn on_tracks(&mut self, id: &str, tracks: Vec<Track>, total: u32, time: f64) {
        self.cache.insert(id.to_owned(), tracks.clone());
        if let Some(open) = self.open.as_mut().filter(|open| open.playlist.id == id) {
            open.playlist.total = total;
            // They already show (from the cache): no new entrance animation.
            if open.tracks.as_ref() != Some(&tracks) {
                if open.tracks.is_none() {
                    open.arrived_at = time;
                }
                open.tracks = Some(tracks);
            }
            open.loading = false;
            open.error = None;
        }
    }

    /// The tracks of `id` arrived, but they are older than an edit that already shows. The view
    /// ignores them, and the load stops.
    pub fn on_outdated(&mut self, id: &str) {
        if let Some(open) = self.open.as_mut().filter(|open| open.playlist.id == id) {
            open.loading = false;
        }
    }

    /// The UI sends an edit of `playlist` (as it was before the edit). The edit shows immediately.
    pub fn on_edit(&mut self, playlist: &Playlist, edit: &PlaylistEdit) {
        let apply = |tracks: &mut Vec<Track>| {
            // At the end of a playlist that shows only partly: not visible.
            let unseen_end = matches!(
                edit,
                PlaylistEdit::Insert { index: None, .. } | PlaylistEdit::Append { .. }
            ) && (tracks.len() as u32) < playlist.total;
            !unseen_end && edit.apply(tracks)
        };
        if let Some(tracks) = self.cache.get_mut(&playlist.id) {
            apply(tracks);
        }
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.playlist.id == playlist.id)
        {
            if let Some(tracks) = &mut open.tracks {
                apply(tracks);
            }
            open.playlist.total = edit.total_after(open.playlist.total);
        }
    }

    /// The name of the playlist `id` changes to `name`. The new name shows immediately.
    pub fn on_rename(&mut self, id: &str, name: &str) {
        if let Some(open) = self.open.as_mut().filter(|open| open.playlist.id == id) {
            name.clone_into(&mut open.playlist.name);
        }
    }

    pub fn on_failed(&mut self, id: &str, message: String) {
        if let Some(open) = self.open.as_mut().filter(|open| open.playlist.id == id) {
            open.loading = false;
            open.error = Some(message);
        }
    }

    /// The playlists loaded again. The playlist that shows can be different (it got a track) or
    /// gone.
    pub fn on_playlists(&mut self, playlists: &[Playlist], actions: &mut Vec<Command>) {
        let Some(open) = &mut self.open else {
            return;
        };
        match playlists.iter().find(|p| p.id == open.playlist.id) {
            None => self.close(),
            Some(playlist) => {
                if playlist.total != open.playlist.total && playlist.editable {
                    open.loading = true;
                    actions.push(Command::LoadPlaylistTracks {
                        playlist: playlist.clone(),
                    });
                }
                open.playlist = playlist.clone();
            }
        }
    }

    /// The panel over `rect`, faded and slid in by `open_t` (0..1). Returns true if a dragged
    /// track is over it and the user can drop it there.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        open_t: f32,
        data: &PlaylistTracksData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        self.added_at.retain(|_, at| time - *at < 2.0);
        // Esc during a name change only keeps the name as it was.
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.renaming.is_none())
            && detail::escape_closes(&ctx)
        {
            self.close();
        }
        if open_t <= 0.001 {
            self.closing = None;
            return false;
        }
        // The panel fades out: it shows, but the user cannot use it.
        let interactive = self.open.is_some();
        let Some(open) = self.open.as_mut().or(self.closing.as_mut()) else {
            return false;
        };
        let rect = rect.translate(vec2(0.0, -8.0 * (1.0 - open_t)));
        let mut panel_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("playlist-tracks")
                .max_rect(rect.expand2(vec2(8.0, 0.0))),
        );
        panel_ui.multiply_opacity(open_t);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), LIST_TITLE_HEIGHT));
        header_row(
            &mut panel_ui,
            header,
            open,
            interactive,
            data.accent,
            time,
            actions,
        );
        let list = widgets::list_rows(rect, header);
        let mut list_ui = panel_ui.new_child(
            UiBuilder::new()
                .id_salt("playlist-tracks-list")
                .max_rect(list),
        );
        list_ui.set_clip_rect(list.intersect(panel_ui.clip_rect()));
        if !interactive {
            list_ui.disable();
            open.menu.close();
        }
        let over = track_list(
            &mut list_ui,
            open,
            interactive,
            &mut self.added_at,
            data,
            textures,
            actions,
        );
        if let Some((choice, index, Target::Track(track))) = open.menu.show(&ctx, data.accent) {
            menu_choice(
                open,
                choice,
                index,
                track,
                &mut self.added_at,
                time,
                actions,
            );
        }
        over
    }
}

/// Does the action that the user selected in the menu of `track`, at `index` of the list when it
/// opened. «Play» plays only `track`, as its ▶ does.
fn menu_choice(
    open: &mut OpenPlaylist,
    choice: Choice,
    index: usize,
    track: Track,
    added_at: &mut HashMap<String, f64>,
    time: f64,
    actions: &mut Vec<Command>,
) {
    // The list can load again after the menu opened.
    let tracks = open.tracks.as_deref().unwrap_or_default();
    let found = find_near(tracks, &track.uri, index);
    match choice {
        Choice::Play => actions.push(Command::PlayTrack { track }),
        Choice::PlayNext | Choice::PlayLast => {
            if choice == Choice::PlayLast {
                added_at.insert(track.uri.clone(), time);
            }
            actions.extend(context_menu::command(choice, Target::Track(track)));
        }
        Choice::Remove => {
            if let Some(index) = found
                && open.playlist.editable
            {
                open.keys.removed(index, time);
                actions.push(Command::EditPlaylist {
                    playlist: open.playlist.clone(),
                    edit: PlaylistEdit::Remove {
                        index,
                        uri: track.uri,
                    },
                });
            }
        }
    }
}

/// Each row has a key from its entry (see [`QueueKeys`]). Thus, when the order of the playlist
/// changes, each row glides to its new position.
fn row_id(key: u64) -> Id {
    Id::unique(("playlist-row", key))
}

/// The dragged track, if the list accepts it. The list moves its own tracks and adds the other
/// tracks, but only to a playlist of the user (`editable`).
fn accepted_drag(ctx: &Context, tracks: &[Track], editable: bool) -> Option<Dragging> {
    let (dragged, pointer) = (dnd::current(ctx)?, ctx.pointer_latest_pos()?);
    let track = dragged.track().filter(|_| editable)?;
    let from = match dragged.origin {
        // The list does not accept a track of this list that is not in it now.
        DragOrigin::PlaylistTracks(index) => Some(find_near(tracks, &track.uri, index)?),
        _ => None,
    };
    Some(Dragging {
        dragged,
        pointer,
        from,
    })
}

/// The page of `playlist` in Spotify's web player.
fn web_url(playlist: &Playlist) -> String {
    format!("https://open.spotify.com/playlist/{}", playlist.id)
}

/// The tracks. If `interactive` (not in a fade-out) and the playlist is of the user, the user can
/// change their order and remove them. The list adds tracks from other areas at the drop
/// position. Returns true if a dragged track is over it and the user can drop it there.
fn track_list(
    ui: &mut Ui,
    open: &mut OpenPlaylist,
    interactive: bool,
    added_at: &mut HashMap<String, f64>,
    data: &PlaylistTracksData<'_>,
    textures: &mut TextureCache,
    actions: &mut Vec<Command>,
) -> bool {
    let ctx = ui.ctx().clone();
    let time = ctx.input(|i| i.time);
    let rect = ui.max_rect();
    let editable = interactive && open.playlist.editable;
    // When a track is over an empty playlist, the list (with only the slot) replaces the empty
    // state.
    let empty_target = open.error.is_none()
        && open.tracks.as_ref().is_some_and(Vec::is_empty)
        && accepted_drag(&ctx, &[], editable)
            .is_some_and(|d| d.from.is_none() && rect.contains(d.pointer));
    let tracks = match (&open.tracks, &open.error) {
        (_, Some(error)) => {
            let bottom = widgets::empty_state(
                ui,
                rect,
                Id::unique("playlist-tracks-error"),
                "Its songs can't be shown",
                error,
                1.0,
                |p, c, color| icons::music_note(p, c, 24.0, color),
            );
            // Spotify does not let the app list the playlist of a different user here, but its web
            // player shows it.
            if error == PLAYLIST_NOT_READABLE {
                let button =
                    Rect::from_center_size(pos2(rect.center().x, bottom + 36.0), vec2(190.0, 38.0));
                let response = widgets::pill_button(
                    ui,
                    button,
                    Id::unique("playlist-open-in-spotify"),
                    "Open in Spotify",
                    data.accent.gamma_multiply(0.22),
                    theme::TEXT,
                    true,
                );
                if response.clicked() {
                    ctx.open_url(OpenUrl::new_tab(web_url(&open.playlist)));
                }
            }
            return false;
        }
        (None, None) => {
            widgets::skeleton_rows(ui, rect, time, 8);
            return false;
        }
        (Some(tracks), None) if tracks.is_empty() && !empty_target => {
            widgets::empty_state(
                ui,
                rect,
                Id::unique("playlist-tracks-empty"),
                "Empty playlist",
                "Drag songs onto the playlist to add them.",
                1.0,
                |p, c, color| icons::music_note(p, c, 24.0, color),
            );
            return false;
        }
        (Some(tracks), None) => tracks,
    };
    let playlist = &open.playlist;
    let keys = open
        .keys
        .update(tracks.iter().map(|t| t.uri.as_str()), time)
        .to_vec();

    let dragging = accepted_drag(&ctx, tracks, editable);
    let over = dragging.as_ref().is_some_and(|d| rect.contains(d.pointer));
    let from = dragging.as_ref().and_then(|d| d.from);
    // The slot opens where the track would go. While the user drags a track of the list to a
    // different area, the slot stays at the old position of the track. A track from a different
    // area opens it only while it is over the list.
    let slot_shown = from.is_some() || over;
    let slot_id = Id::unique("playlist-drop-slot");
    let slot_alpha = anim::tween_bool(&ctx, slot_id.with("alpha"), slot_shown, 0.2);
    let slot_appeared = slot_shown && !open.slot_shown;
    open.slot_shown = slot_shown;

    let current_uri = data.playback.track_uri();
    let playing = anim::tween_bool(
        &ctx,
        Id::unique("playlist-tracks-playing"),
        data.playback.is_playing,
        0.4,
    );
    let elapsed = anim::elapsed_since(&ctx, open.arrived_at);
    anim::keep_animating(&ctx, elapsed, ROW_APPEAR_DELAY * 10.0 + ROW_APPEAR_DURATION);
    let mut play = None;
    let mut add = None;
    let mut menu_at = None;
    let scrolled = egui::ScrollArea::vertical()
        .id_salt(("playlist-tracks-scroll", playlist.id.as_str()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            // One more row while a track from a different area opens a slot.
            let rows = tracks.len() + usize::from(over && from.is_none());
            let (area, _) =
                ui.allocate_exact_size(vec2(width, rows as f32 * ROW_HEIGHT + 8.0), Sense::hover());

            // The position of the dragged track: before the first other track with its middle below
            // the middle of the floating copy.
            let others = tracks.len() - usize::from(from.is_some());
            let drop_to = dragging.as_ref().filter(|_| over).map(|d| {
                let center = d.dragged.landing_y(d.pointer.y) - area.top();
                (0..others)
                    .filter(|i| center > (*i as f32 + 0.5) * ROW_HEIGHT)
                    .count()
            });
            let mut entries: Vec<Entry> = (0..tracks.len())
                .filter(|i| from != Some(*i))
                .map(Entry::Track)
                .collect();
            if let Some(at) = drop_to.or(from) {
                entries.insert(at, Entry::Slot);
            }

            if let Some(d) = dragging.as_ref().filter(|_| over) {
                dnd::autoscroll(ui, area, d.pointer.y);
                if let Some(to) = drop_to
                    && dnd::released(&ctx)
                    && dnd::take(&ctx).is_some()
                {
                    let key = match (d.from, d.dragged.track()) {
                        (Some(from), _) if from == to => Some(keys[from]),
                        (Some(from), _) => {
                            actions.push(Command::EditPlaylist {
                                playlist: playlist.clone(),
                                edit: PlaylistEdit::Move {
                                    from,
                                    to,
                                    uri: tracks[from].uri.clone(),
                                },
                            });
                            open.keys.moved(from, to, time)
                        }
                        (None, Some(track)) => {
                            actions.push(Command::EditPlaylist {
                                playlist: playlist.clone(),
                                edit: PlaylistEdit::Insert {
                                    index: Some(to),
                                    track: track.clone(),
                                },
                            });
                            Some(open.keys.inserted(to, &track.uri, time))
                        }
                        (None, None) => None,
                    };
                    // It glides from the drop point to its position.
                    if let Some(key) = key {
                        dnd::glide_from_drop(
                            &ctx,
                            row_id(key),
                            d.dragged.top(d.pointer.y) - area.top(),
                        );
                    }
                }
            }

            for (place, entry) in entries.iter().enumerate() {
                let target = place as f32 * ROW_HEIGHT;
                let (id, index) = match entry {
                    Entry::Track(index) => (row_id(keys[*index]), Some(*index)),
                    Entry::Slot => (slot_id, None),
                };
                if *entry == Entry::Slot && slot_appeared {
                    anim::set_tween_value(&ctx, id.with("y"), target);
                }
                let offset = anim::tween(&ctx, id.with("y"), target, 0.32);
                let appear = ease_out_cubic(anim::stagger(
                    elapsed,
                    place,
                    ROW_APPEAR_DELAY,
                    ROW_APPEAR_DURATION,
                ));
                let rect = Rect::from_min_size(
                    pos2(area.left(), area.top() + offset + (1.0 - appear) * 14.0),
                    vec2(width, ROW_HEIGHT),
                );
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let Some(i) = index else {
                    dnd::drop_slot(ui, rect, slot_id, over, slot_alpha, data.accent);
                    continue;
                };
                let track = &tracks[i];
                let cover = track
                    .cover_small
                    .as_deref()
                    .and_then(|url| textures.get(url, THUMB_PX, time));
                let added = added_at
                    .get(&track.uri)
                    .map(|at| anim::elapsed_since(&ctx, *at));
                let row = track_row(
                    ui,
                    rect,
                    id,
                    track,
                    cover.as_ref(),
                    RowStyle {
                        alpha: appear,
                        current: current_uri == Some(track.uri.as_str()),
                        playing,
                        pause_button: false,
                        draggable: true,
                        can_add: true,
                        // The user removes it from its menu (right click), not with a ✕.
                        remove_from: None,
                        locked: false,
                        added,
                        highlighted: false,
                        menu_open: open.menu.is_open_for(i),
                        accent: data.accent,
                        time,
                    },
                );
                if row.play {
                    play = Some(track.clone());
                }
                if let Some(pos) = row.menu_at {
                    menu_at = Some((i, pos));
                }
                if row.add {
                    add = Some(track.clone());
                }
                if let Some(pointer) = row.drag_from {
                    dnd::start(
                        &ctx,
                        DragItem::Track(track.clone()),
                        DragOrigin::PlaylistTracks(i),
                        rect,
                        pointer,
                    );
                }
            }
        });
    widgets::scroll_hints(ui, &scrolled, (0.0, 0.0));
    if let Some((index, pos)) = menu_at {
        let target = Target::Track(tracks[index].clone());
        open.menu.open(&ctx, target, index, pos, editable);
    }
    if let Some(track) = play {
        actions.push(Command::PlayTrack { track });
    }
    if let Some(track) = add {
        added_at.insert(track.uri.clone(), time);
        actions.push(Command::AddToQueue { track });
    }
    over
}

/// The size of the pencil next to the name of a playlist that the user can rename, and the gap
/// between them.
const PENCIL_SIZE: f32 = 16.0;
const PENCIL_GAP: f32 = 8.0;

/// The name of the playlist and the number of its tracks (or its owner). Esc or a second click on
/// it in the sidebar closes it. If `interactive` (not in a fade-out), a double click on the name
/// of a playlist of the user changes the name into a field to rename it. A click on the pencil
/// next to the name on hover does the same.
fn header_row(
    ui: &mut Ui,
    rect: Rect,
    open: &mut OpenPlaylist,
    interactive: bool,
    accent: Color32,
    time: f64,
    actions: &mut Vec<Command>,
) {
    let refreshing = open.loading && open.tracks.is_some();
    let dots_width = if refreshing { 26.0 } else { 0.0 };
    let title_right = rect.right() - dots_width;
    let title_id = Id::unique("playlist-tracks-title");
    if interactive && open.playlist.editable && open.renaming.is_some() {
        rename_field(ui, rect, title_right, open, accent, actions);
    } else {
        let playlist = &open.playlist;
        let detail = if playlist.editable {
            String::new()
        } else {
            format!("By {}", playlist.owner)
        };
        let renamable = interactive && playlist.editable;
        // Space for the pencil, so that a long name never goes under it.
        let pencil_room = if renamable {
            PENCIL_GAP + PENCIL_SIZE
        } else {
            0.0
        };
        let title = widgets::list_title(
            ui,
            rect,
            title_id,
            &playlist.name,
            &format!("Songs in “{}”", playlist.name),
            &detail,
            title_right - pencil_room,
        );
        if renamable {
            let pencil = Rect::from_center_size(
                pos2(
                    title.right() + PENCIL_GAP + PENCIL_SIZE / 2.0,
                    title.center().y + 1.0,
                ),
                vec2(PENCIL_SIZE, PENCIL_SIZE),
            );
            let name_response = ui
                .interact(title, title_id.with("rename"), Sense::CLICK) // pointer only
                .on_hover_cursor(CursorIcon::Text)
                .on_hover_text("Double-click to rename");
            let pencil_response = ui
                .interact(pencil, title_id.with("pencil"), Sense::CLICK) // pointer only
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Rename");
            // The pencil shows only while the pointer is over the name or the pencil. Thus the
            // user knows that the name is editable.
            let hovered = name_response.hovered() || pencil_response.hovered();
            let shown = anim::tween_bool(ui.ctx(), title_id.with("pencil-shown"), hovered, 0.15);
            if shown > 0.0 {
                let color = if pencil_response.hovered() {
                    theme::TEXT
                } else {
                    theme::TEXT_FAINT
                };
                icons::pencil(
                    ui.painter(),
                    pencil.center(),
                    PENCIL_SIZE,
                    color.gamma_multiply(shown),
                );
                widgets::a11y_label(ui, pencil, title_id.with("pencil-label"), "Rename playlist");
            }
            // egui counts a click just before (on the playlist, to open it) as part of a triple
            // click.
            if name_response.double_clicked()
                || name_response.triple_clicked()
                || pencil_response.clicked()
            {
                open.renaming = Some(playlist.name.clone());
                open.focus_rename = true;
            }
        }
    }
    if refreshing {
        // While they load again, a few dots show next to the count.
        icons::loading_dots(
            ui.painter(),
            pos2(rect.right() - 8.0, rect.center().y - 1.0),
            7.0,
            time,
            theme::TEXT_FAINT,
        );
        anim::request_frame(ui.ctx());
    }
}

/// The field with the new name of the open playlist, in place of its name (up to `right`). Enter,
/// ✓ or a click on a different area changes the name. Esc (or an empty name) keeps it as it was.
fn rename_field(
    ui: &mut Ui,
    rect: Rect,
    right: f32,
    open: &mut OpenPlaylist,
    accent: Color32,
    actions: &mut Vec<Command>,
) {
    let Some(name) = open.renaming.as_mut() else {
        return;
    };
    let id = Id::unique("playlist-rename");
    // If the field asks for the focus during the click, egui removes it again.
    let clicking = ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released());
    if open.focus_rename && !clicking {
        open.focus_rename = false;
        ui.memory_mut(|m| {
            m.request_focus(id);
        });
        // Select the full name, so that the user can type a new name in its place.
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::default(),
                egui::text::CCursor::new(name.chars().count()),
            )));
        state.store(ui.ctx(), id);
        ui.ctx().request_repaint();
    } else if open.focus_rename {
        ui.ctx().request_repaint();
    }
    // Its text is at the position of the name, with the same alignment.
    let field = Rect::from_min_max(
        pos2(rect.left() - 8.0, rect.top() + 1.0),
        pos2(right, rect.bottom() - 5.0),
    );
    let response = widgets::text_field(
        ui,
        field,
        id,
        "New playlist name",
        name,
        "Playlist name",
        widgets::FieldStyle::Boxed,
        accent,
    );
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let escape =
        (response.has_focus() || response.lost_focus()) && ui.input(|i| i.key_pressed(Key::Escape));
    let valid = !name.trim().is_empty();
    let accept = widgets::icon_button(
        ui,
        pos2(field.right() - 19.0, field.center().y),
        28.0,
        id.with("accept"),
        "Save name",
        valid,
        |painter, center, color| {
            let color = if valid {
                accent
            } else {
                color.gamma_multiply(0.5)
            };
            icons::plus_to_check(painter, center, 16.0, 1.0, color);
        },
    );
    if escape {
        open.renaming = None;
    } else if enter && !valid {
        // There is no name yet: continue to type.
        ui.memory_mut(|m| m.request_focus(id));
    } else if response.lost_focus() || accept.clicked() {
        let name = name.trim().to_owned();
        if valid && name != open.playlist.name {
            actions.push(Command::RenamePlaylist {
                playlist: open.playlist.clone(),
                name,
            });
        }
        open.renaming = None;
    }
}
