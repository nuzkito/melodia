//! Right column: the queue (previous tracks, the current track and the upcoming tracks), with
//! drag and drop. The current track is a card with the playback controls. The card grows from its
//! row and stays pinned to an edge while the list scrolls. While the music plays on a different
//! device, the column only tells where.

use std::collections::HashSet;
use std::sync::Arc;

use egui::{
    Align, Color32, Galley, Id, Layout, Pos2, Rect, Sense, Shadow, Stroke, StrokeKind, Ui,
    UiBuilder, pos2, vec2,
};

use super::album;
use super::anim::{self, ease_out_cubic, lerp, lerp_color};
use super::artist;
use super::dnd::{self, DragItem, DragOrigin, Dragged};
use super::icons;
use super::liked;
use super::player_controls::{SeekBar, VolumeSlider, transport};
use super::queue_keys::QueueKeys;
use super::textures::{Cover, THUMB_PX, TextureCache};
use super::theme;
use super::track_row::{ROW_COVER, ROW_COVER_INSET, ROW_HEIGHT, RowStyle, track_row};
use super::widgets::{
    self, a11y_label, custom_button, custom_button_with_ring, paint_cover, single_line,
};
use crate::backend::{Command, QueueAt};
use crate::model::{
    Device, PlaybackSnapshot, QueueEdit, QueuePosition, QueueSnapshot, Track, find_near,
    previous_count_after_move,
};
use crate::util::format_duration;

const TITLE_HEIGHT: f32 = 40.0;
const CARD_HEIGHT: f32 = 204.0;
const CARD_COVER: f32 = 64.0;
/// The space for the ♥ at the right of the card's title.
const CARD_HEART_ROOM: f32 = 34.0;
/// The distance that the card's title moves right for the equalizer before it.
const CARD_EQUALIZER_ROOM: f32 = 22.0;
/// The seconds for rows to glide to a new position, and for a track to change from a row into
/// the card or back.
const GLIDE: f32 = 0.32;
/// The visible part of the last played track above the card, when the list scrolls to a new
/// current track. It is sufficient to show that there is history, and the upcoming tracks get
/// the remaining space of the list.
const PREVIOUS_PEEK: f32 = 36.0;
/// The seconds to wait for added tracks to show in the queue (a playlist arrives when the backend
/// has read it).
const ARRIVAL_WAIT: f64 = 6.0;
/// The seconds that the notice of tracks added out of view stays.
const NOTICE_SECS: f64 = 3.0;

#[derive(Default)]
pub struct QueueView {
    /// The key of the current track that the list last scrolled to.
    scrolled_to: Option<u64>,
    /// A key for each entry of the full queue, for the animations of its row.
    keys: QueueKeys,
    /// The drop slot was on the screen in the last frame.
    slot_shown: bool,
    /// The card's own controls.
    seek: SeekBar,
    volume: VolumeSlider,
    /// The last current track's card (see [`LeavingCard`]).
    leaving: Option<LeavingCard>,
    /// The keys of the full queue in the last frame.
    last_keys: Vec<u64>,
    /// The tracks that the user adds, until they show (see [`Arrival`]).
    arrival: Option<Arrival>,
    /// The added tracks that went out of view (see [`AddedNotice`]).
    notice: Option<AddedNotice>,
}

/// The tracks that the user adds (a drop, a playlist, «＋»). They are the entries that show in the
/// queue with a key that is not in `known`.
struct Arrival {
    known: HashSet<u64>,
    /// The time that the queue started to wait for them.
    since: Option<f64>,
}

/// The added tracks that went out of view. A pill at that edge of the list tells this for some
/// time. A click on the pill scrolls to them.
struct AddedNotice {
    /// Their keys (see [`QueueKeys`]).
    keys: Vec<u64>,
    /// They are below the visible part of the list (if false, above it).
    below: bool,
    since: f64,
    fading_out: bool,
}

impl AddedNotice {
    fn text(&self) -> String {
        let place = if self.below { "below" } else { "above" };
        match self.keys.len() {
            1 => format!("Added {place}"),
            n => format!("{n} tracks added {place}"),
        }
    }
}

pub struct QueueData<'a> {
    pub queue: &'a QueueSnapshot,
    pub playback: &'a PlaybackSnapshot,
    /// The different device where the music plays, if it is not in this app.
    pub elsewhere: Option<&'a Device>,
    /// The command of «Play here» while the music is on a different device (nothing if this app
    /// cannot play).
    pub play_here: Option<Command>,
    pub accent: Color32,
}

/// The part of the queue that has a row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Previous,
    Current,
    Next,
}

enum RowKind<'a> {
    /// `index` is its position in the full queue (see [`whole_queue`]). `section` is where it
    /// shows, which changes while the user drags a track.
    Track {
        track: &'a Track,
        section: Section,
        index: usize,
    },
    /// The position for the dragged track.
    Slot,
}

struct Row<'a> {
    /// The animation id. When the queue changes, rows glide to their new position.
    id: Id,
    kind: RowKind<'a>,
    height: f32,
}

/// The position of an entry of the queue after its row glides into position.
struct Placed {
    /// Its key (see [`QueueKeys`]).
    key: u64,
    rect: Rect,
    /// It is the card of the current track, which always stays on the screen.
    card: bool,
}

impl Placed {
    /// Half of it or more shows in `view`.
    fn in_sight(&self, view: Rect) -> bool {
        let shown = self.rect.bottom().min(view.bottom()) - self.rect.top().max(view.top());
        self.card || shown >= self.rect.height() / 2.0
    }
}

/// A drag in progress, as the queue sees it.
struct Dragging {
    dragged: Arc<Dragged>,
    pointer: Pos2,
    /// The pointer is over the queue.
    over: bool,
    /// The dragged track is the track at this index of the full queue (see [`whole_queue`]): the
    /// user moves it. `None` for a queued track that cannot move (see [`can_move`]): the user can
    /// only copy it to a playlist.
    lifted: Option<usize>,
}

/// The position for a dragged item.
#[derive(Clone, Copy)]
enum DropAt {
    /// A queued track moves from `from` to `to` of the full queue.
    Move { from: usize, to: usize },
    /// All other items go to `next[index]`.
    Next(usize),
}

/// A row of the list: a track (its index in the full queue) or the slot for the dragged item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    Track(usize),
    Slot,
}

impl Dragging {
    fn comes_from_outside(&self) -> bool {
        !matches!(self.dragged.origin, DragOrigin::Queue(_))
    }

    /// The pointer is over the queue, and the queue accepts the dragged item: any item from
    /// outside, or a queued track that can move.
    fn over_target(&self) -> bool {
        self.over && (self.comes_from_outside() || self.lifted.is_some())
    }

    /// The result of a drop at `at`.
    fn drop_command(&self, at: DropAt) -> Option<Command> {
        Some(match (&self.dragged.item, at) {
            (DragItem::Playlist(playlist), DropAt::Next(index)) => Command::QueuePlaylist {
                playlist: playlist.clone(),
                index,
            },
            (DragItem::Track(track), DropAt::Move { from, to }) => {
                Command::EditQueue(QueueEdit::Move {
                    from,
                    to,
                    uri: track.uri.clone(),
                })
            }
            (DragItem::Track(track), DropAt::Next(index)) => {
                Command::EditQueue(QueueEdit::Insert {
                    index,
                    track: track.clone(),
                })
            }
            (DragItem::Release(release), DropAt::Next(index)) => Command::QueueAlbum {
                id: release.id.clone(),
                name: release.name.clone(),
                at: QueueAt::Next(index),
            },
            (DragItem::Playlist(_) | DragItem::Release(_), DropAt::Move { .. })
            | (DragItem::Folder(_), _) => return None,
        })
    }
}

impl QueueView {
    /// The user asked to add a track to the queue in a different way (the «＋» of a track). If it
    /// goes out of view, a notice tells this.
    pub fn expect_arrival(&mut self) {
        self.arrival = Some(Arrival {
            known: self.last_keys.iter().copied().collect(),
            since: None,
        });
    }

    /// Draws the queue. Returns true if a dragged item is over it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        data: &QueueData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let queue = data.queue;
        let time = ctx.input(|i| i.time);
        self.keys
            .update(whole_queue(queue).map(|track| track.uri.as_str()), time);
        // A folder of playlists does not go to the queue.
        let dragged = dnd::current(&ctx).filter(|d| !matches!(d.item, DragItem::Folder(_)));
        let dragging = match (dragged, ctx.pointer_latest_pos()) {
            (Some(dragged), Some(pointer)) => {
                let lifted = match (dragged.origin, dragged.track()) {
                    (DragOrigin::Queue(index), Some(track)) => {
                        let all: Vec<Track> = whole_queue(queue).cloned().collect();
                        find_near(&all, &track.uri, index).filter(|index| can_move(queue, *index))
                    }
                    _ => None,
                };
                Some(Dragging {
                    over: rect.expand(16.0).contains(pointer),
                    dragged,
                    pointer,
                    lifted,
                })
            }
            _ => None,
        };
        let over = dragging.as_ref().is_some_and(Dragging::over_target);

        let title = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE_HEIGHT));
        Self::title(ui, title, queue, dragging.is_none(), actions);

        // The user drags a search result or a playlist: the queue lights up as the position to
        // drop it.
        let comes_from_outside = dragging.as_ref().is_some_and(Dragging::comes_from_outside);
        let glow = anim::tween_bool(&ctx, Id::unique("queue-drop-glow"), comes_from_outside, 0.2);
        if glow > 0.001 {
            let hot = anim::tween_bool(&ctx, Id::unique("queue-drop-hot"), over, 0.15);
            let strength = glow * lerp(0.35, 1.0, hot);
            ui.painter().rect(
                rect.expand2(vec2(14.0, 8.0)),
                theme::RADIUS_LARGE,
                data.accent.gamma_multiply(0.05 * strength),
                Stroke::new(1.0, data.accent.gamma_multiply(0.5 * strength)),
                StrokeKind::Inside,
            );
        }

        let content = Rect::from_min_max(
            pos2(rect.left() - 8.0, title.bottom() + 6.0),
            pos2(rect.right() + 8.0, rect.bottom()),
        );
        if let Some(device) = data.elsewhere {
            self.slot_shown = false;
            self.last_keys.clear();
            Self::elsewhere(ui, content, device, dragging.as_ref(), data, actions);
        } else if queue.previous.is_empty() && queue.current.is_none() && queue.next.is_empty() {
            self.slot_shown = false;
            self.last_keys.clear();
            self.empty(ui, content, dragging.as_ref(), data, actions);
        } else {
            self.list(ui, content, data, textures, actions, dragging.as_ref());
        }
        over
    }

    /// «Queue» and «Clear» (while there is something to clear and the user drags nothing).
    fn title(
        ui: &mut Ui,
        rect: Rect,
        queue: &QueueSnapshot,
        can_clear: bool,
        actions: &mut Vec<Command>,
    ) {
        let has_tracks =
            !queue.previous.is_empty() || queue.current.is_some() || !queue.next.is_empty();
        if can_clear && has_tracks {
            let label = "Clear";
            let button = Rect::from_min_max(
                pos2(rect.right() - 64.0, rect.top() + 6.0),
                pos2(rect.right() + 6.0, rect.bottom() - 8.0),
            );
            let response = custom_button(
                ui,
                button,
                Id::unique("clear-queue"),
                "Clear the queue",
                true,
                |painter, r, hover, _| {
                    let color = lerp_color(theme::TEXT_FAINT, theme::TEXT, hover);
                    let galley = single_line(
                        painter,
                        label,
                        theme::regular(theme::FONT_SMALL),
                        color,
                        r.width(),
                    );
                    painter.galley(
                        pos2(
                            r.right() - galley.size().x - 6.0,
                            r.center().y - galley.size().y / 2.0 + 1.0,
                        ),
                        galley,
                        color,
                    );
                },
            )
            .on_hover_text("Removes every song and stops playback");
            if response.clicked() {
                actions.push(Command::ClearQueue);
            }
        }

        let painter = ui.painter();
        let galley = single_line(
            painter,
            "Queue",
            theme::strong(theme::FONT_TITLE),
            theme::TEXT,
            200.0,
        );
        let pos = pos2(rect.left(), rect.center().y - galley.size().y / 2.0 - 2.0);
        let title_rect = Rect::from_min_size(pos, galley.size());
        painter.galley(pos, galley, theme::TEXT);
        a11y_label(ui, title_rect, Id::unique("queue-title"), "Queue");
    }

    /// Nothing is in the queue: an empty card (with the volume) and a hint under it, or the
    /// position to drop the dragged item.
    fn empty(
        &mut self,
        ui: &mut Ui,
        content: Rect,
        dragging: Option<&Dragging>,
        data: &QueueData<'_>,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let accent = data.accent;
        let card = Rect::from_min_size(content.min, vec2(content.width(), CARD_HEIGHT));
        let style = CardStyle {
            alpha: 1.0,
            morph: 1.0,
            pinned: false,
            playing: 0.0,
            time: ctx.input(|i| i.time),
        };
        self.current_card(
            ui,
            card,
            Id::unique("empty-card"),
            None,
            None,
            data,
            style,
            actions,
        );
        let rect = Rect::from_min_max(pos2(content.left(), card.bottom() + 8.0), content.max);
        let Some(dragging) = dragging.filter(|d| d.comes_from_outside()) else {
            widgets::empty_state(
                ui,
                rect,
                Id::unique("queue-empty"),
                "The queue is empty",
                "Search for a song and press play, or drag a song or a playlist here.",
                1.0,
                |p, c, color| icons::music_note(p, c, 26.0, color),
            );
            return;
        };
        Self::drop_zone(ui, rect, dragging, accent, actions);
    }

    /// The music plays on `device`: no card and no queue, only where it plays and «Play here». An
    /// item that the user drops here plays in this app.
    fn elsewhere(
        ui: &mut Ui,
        content: Rect,
        device: &Device,
        dragging: Option<&Dragging>,
        data: &QueueData<'_>,
        actions: &mut Vec<Command>,
    ) {
        if let Some(dragging) = dragging.filter(|d| d.comes_from_outside()) {
            Self::drop_zone(ui, content, dragging, data.accent, actions);
            return;
        }
        let bottom = widgets::empty_state(
            ui,
            content,
            Id::unique("queue-elsewhere"),
            &format!("Playing on {}", device.name),
            "Play a song here, or bring the music back.",
            1.0,
            |p, c, color| icons::device(p, c, 26.0, &device.kind, color),
        );
        if let Some(command) = &data.play_here {
            let button =
                Rect::from_center_size(pos2(content.center().x, bottom + 36.0), vec2(140.0, 38.0));
            let response = widgets::pill_button(
                ui,
                button,
                Id::unique("play-here"),
                "Play here",
                data.accent.gamma_multiply(0.22),
                theme::TEXT,
                true,
            );
            if response.clicked() {
                actions.push(command.clone());
            }
        }
    }

    /// The position to drop a search result, an album or a playlist over an empty queue, to play
    /// it.
    fn drop_zone(
        ui: &mut Ui,
        rect: Rect,
        dragging: &Dragging,
        accent: Color32,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let hot = anim::tween_bool(&ctx, Id::unique("queue-empty-hot"), dragging.over, 0.15);
        let zone = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(rect.width() - 16.0, 120.0));
        let painter = ui.painter();
        painter.rect(
            zone,
            theme::RADIUS_LARGE,
            accent.gamma_multiply(0.06 + 0.08 * hot),
            Stroke::new(1.0 + hot, accent.gamma_multiply(0.4 + 0.5 * hot)),
            StrokeKind::Inside,
        );
        let center = zone.center() - vec2(0.0, 14.0);
        painter.circle_filled(center, 16.0, accent.gamma_multiply(0.2 + 0.2 * hot));
        icons::plus_to_check(painter, center, 16.0, 0.0, accent);
        let text = "Drop it here to play it";
        let galley = single_line(
            painter,
            text,
            theme::regular(theme::FONT_BODY),
            theme::TEXT,
            zone.width(),
        );
        let text_rect = Rect::from_center_size(center + vec2(0.0, 32.0), galley.size());
        painter.galley(
            text_rect.min,
            galley,
            lerp_color(theme::TEXT_DIM, theme::TEXT, hot),
        );
        a11y_label(ui, text_rect, Id::unique("queue-drop-here"), text);

        if dragging.over && dnd::released(&ctx) && dnd::take(&ctx).is_some() {
            actions.extend(dragging.drop_command(DropAt::Next(0)));
        }
    }

    fn list(
        &mut self,
        ui: &mut Ui,
        content: Rect,
        data: &QueueData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
        dragging: Option<&Dragging>,
    ) {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        let queue = data.queue;
        let all: Vec<&Track> = whole_queue(queue).collect();
        let keys = self
            .keys
            .update(all.iter().map(|t| t.uri.as_str()), time)
            .to_vec();
        self.last_keys.clone_from(&keys);
        let previous_count = queue.previous.len();
        let has_current = queue.current.is_some();
        let fixed = queue.fixed();
        let lifted = dragging.and_then(|d| d.lifted);
        let over = dragging.is_some_and(Dragging::over_target);
        // Only a track of the queue can move. Items from outside go to `next`.
        let from_queue = dragging.is_some_and(|d| !d.comes_from_outside());
        // The slot opens where the track would go. While the user drags a queued track to a
        // different area, the slot stays at the old position of the track.
        let slot_shown = dragging.is_some() && (over || lifted.is_some());
        let slot_alpha = anim::tween_bool(&ctx, Id::unique("queue-slot"), slot_shown, 0.2);
        let slot_id = Id::unique("queue-drop-slot");
        let slot_appeared = slot_shown && !self.slot_shown;
        self.slot_shown = slot_shown;
        // The card of the current track leaves a gap of its own size.
        let slot_height = if has_current && lifted == Some(previous_count) {
            CARD_HEIGHT
        } else {
            ROW_HEIGHT
        };

        // The list before the drag (without the dragged track). The position of the floating copy
        // over it gives the drop position.
        let section_of = |index: usize, previous_count: usize| {
            if index < previous_count {
                Section::Previous
            } else if has_current && index == previous_count {
                Section::Current
            } else {
                Section::Next
            }
        };
        let before: Vec<(Entry, Section)> = (0..all.len())
            .filter(|index| Some(*index) != lifted)
            .map(|index| (Entry::Track(index), section_of(index, previous_count)))
            .collect();
        let before = layout(
            &before,
            &all,
            &keys,
            previous_count,
            has_current,
            (slot_id, slot_height),
        );

        let playing = anim::tween_bool(
            &ctx,
            Id::unique("queue-playing"),
            data.playback.is_playing,
            0.4,
        );
        // Its key: the same track played again is a different entry.
        let current_key = has_current.then(|| keys[previous_count]);
        let scroll_to_current = current_key != self.scrolled_to && dragging.is_none();
        if dragging.is_none() {
            self.scrolled_to = current_key;
        }
        let accent = data.accent;

        let mut list_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("queue-list")
                .max_rect(content)
                .layout(Layout::top_down(Align::Min)),
        );
        list_ui.set_clip_rect(content.intersect(ui.clip_rect()));
        // The part of each edge that the pinned card of the current track uses.
        let mut pinned_insets = (0.0, 0.0);
        let scrolled = egui::ScrollArea::vertical()
            .id_salt("queue-scroll")
            .auto_shrink([false, false])
            .show(&mut list_ui, |ui| {
                let width = ui.available_width();
                let list_top = ui.cursor().top();

                // The position of the dragged item: before the first track with its middle below
                // the drop point of the floating copy (see `Dragged::landing_y`). Items from
                // outside go only between the upcoming tracks. Nothing goes between the current
                // track and a track that the app already sent to Spotify.
                let drop_at = match dragging {
                    Some(d) if over => {
                        let center = d.dragged.landing_y(d.pointer.y) - list_top;
                        let mut y = 0.0;
                        let mut above = 0;
                        for row in &before {
                            if let RowKind::Track { section, .. } = row.kind
                                && (from_queue || section == Section::Next)
                                && center > y + row.height / 2.0
                            {
                                above += 1;
                            }
                            y += row.height;
                        }
                        match lifted {
                            Some(from) => {
                                let mut to = above;
                                let current_at = previous_count - usize::from(from < previous_count);
                                if has_current && to > current_at && to <= current_at + fixed {
                                    to = current_at + fixed + 1;
                                }
                                Some(DropAt::Move { from, to })
                            }
                            // A queued track that is not in the queue now.
                            None if from_queue => None,
                            None => Some(DropAt::Next(above.max(fixed))),
                        }
                    }
                    _ => None,
                };

                // The list after the drop, with the slot in place of the dragged item. The tracks
                // that the item passes already show on their new side.
                let mut entries: Vec<Entry> = (0..all.len())
                    .filter(|index| Some(*index) != lifted)
                    .map(Entry::Track)
                    .collect();
                let mut new_previous_count = previous_count;
                match (drop_at, lifted) {
                    (Some(DropAt::Move { from, to }), _) => {
                        entries.insert(to, Entry::Slot);
                        new_previous_count = previous_count_after_move(previous_count, has_current, from, to);
                    }
                    (Some(DropAt::Next(index)), _) => {
                        let at = previous_count + usize::from(has_current) + index;
                        entries.insert(at.min(entries.len()), Entry::Slot);
                    }
                    (None, Some(from)) if slot_shown => entries.insert(from, Entry::Slot),
                    (None, _) => {}
                }
                let visible: Vec<(Entry, Section)> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, entry)| (*entry, section_of(i, new_previous_count)))
                    .collect();
                let rows = layout(
                    &visible,
                    &all,
                    &keys,
                    previous_count,
                    has_current,
                    (slot_id, slot_height),
                );

                let mut total = rows.iter().map(|r| r.height).sum::<f32>() + 16.0;
                // Space at the end, so that the card can always go up to the top of the list,
                // also with few upcoming tracks.
                let card_top = rows
                    .iter()
                    .position(|row| {
                        matches!(row.kind, RowKind::Track { index, .. } if has_current && index == previous_count)
                    })
                    .map(|at| rows[..at].iter().map(|r| r.height).sum::<f32>());
                if let Some(card_top) = card_top {
                    let view = ui.clip_rect().height();
                    total = total.max((card_top - PREVIOUS_PEEK).max(0.0) + view);
                }
                let (area, _) = ui.allocate_exact_size(vec2(width, total), Sense::hover());

                if let Some(d) = dragging
                    && let Some(at) = drop_at
                    && dnd::released(&ctx)
                    && dnd::take(&ctx).is_some()
                {
                    actions.extend(d.drop_command(at));
                    // Something new: a notice tells if it goes out of view.
                    if matches!(at, DropAt::Next(_)) {
                        self.arrival = Some(Arrival {
                            known: keys.iter().copied().collect(),
                            since: None,
                        });
                    }
                    // A dropped track glides from the drop point to its position (a playlist
                    // arrives when the backend has it). The keys follow the edit, so that the
                    // moved copy of a repeated track is the copy that moves.
                    let key = match (at, d.dragged.track()) {
                        (DropAt::Move { from, to }, Some(_)) => self.keys.moved(from, to, time),
                        (DropAt::Next(index), Some(track)) => {
                            let at = previous_count + usize::from(has_current) + index;
                            Some(self.keys.inserted(at, &track.uri, time))
                        }
                        _ => None,
                    };
                    if let Some(key) = key {
                        dnd::glide_from_drop(
                            &ctx,
                            row_id(key),
                            d.dragged.top(d.pointer.y) - area.top(),
                        );
                    }
                }

                // Near the top or bottom edge, the list scrolls by itself.
                if let Some(d) = dragging.filter(|_| over) {
                    dnd::autoscroll(ui, area, d.pointer.y);
                }

                let mut y = 0.0;
                let mut current_target = None;
                let mut card = None;
                for row in &rows {
                    let target = y;
                    y += row.height;
                    if row.id == slot_id && slot_appeared {
                        anim::set_tween_value(&ctx, row.id.with("y"), target);
                    }
                    let offset = anim::tween(&ctx, row.id.with("y"), target, GLIDE);
                    let seen = anim::since_change(&ctx, row.id.with("seen"), (), true);
                    anim::keep_animating(&ctx, seen, anim::ROW_APPEAR_DURATION);
                    let appear = ease_out_cubic(seen / anim::ROW_APPEAR_DURATION);
                    let rect = Rect::from_min_size(
                        pos2(area.left(), area.top() + offset),
                        vec2(width, row.height),
                    );
                    match &row.kind {
                        RowKind::Slot => {
                            dnd::drop_slot(ui, rect, slot_id, over, slot_alpha, accent);
                        }
                        RowKind::Track {
                            track,
                            section,
                            index,
                        } => {
                            let is_current = has_current && *index == previous_count;
                            // The track that starts grows from a row into the card. The track that
                            // ends becomes a row again. This takes the same time as the glide of
                            // the rows around it, so they move together.
                            let morph = anim::tween(
                                &ctx,
                                row.id.with("card"),
                                if is_current { 1.0 } else { 0.0 },
                                GLIDE,
                            );
                            let dim = anim::tween(
                                &ctx,
                                row.id.with("dim"),
                                if *section == Section::Previous {
                                    0.7
                                } else {
                                    1.0
                                },
                                GLIDE,
                            );
                            let alpha = appear * dim;
                            let rect = Rect::from_min_size(
                                rect.min,
                                vec2(width, lerp(ROW_HEIGHT, CARD_HEIGHT, morph)),
                            );
                            if is_current {
                                // The card at the top, under a small part of the last played
                                // track.
                                let peek = PREVIOUS_PEEK.min(target);
                                current_target = Some(Rect::from_min_size(
                                    pos2(area.left(), area.top() + target - peek),
                                    vec2(width, CARD_HEIGHT + peek),
                                ));
                                // Draw it last: when it is pinned, it goes over the other rows.
                                card = Some((*track, rect, row.id, alpha, morph));
                                continue;
                            }
                            if !ui.is_rect_visible(rect) {
                                continue;
                            }
                            let cover = track
                                .cover_small
                                .as_deref()
                                .and_then(|url| textures.get(url, THUMB_PX, time));
                            if morph > 0.001 {
                                // It still changes back into a row.
                                paint_face(
                                    ui,
                                    rect,
                                    &Face {
                                        id: row.id,
                                        track: Some(track),
                                        cover: cover.as_ref(),
                                        morph,
                                        alpha,
                                        lift: 0.0,
                                        playing,
                                        accent,
                                        time,
                                    },
                                );
                                self.leaving_controls(
                                    ui,
                                    rect,
                                    row.id,
                                    track,
                                    data,
                                    CardStyle {
                                        alpha,
                                        morph,
                                        pinned: false,
                                        playing,
                                        time,
                                    },
                                );
                                continue;
                            }
                            // A previous or upcoming track: the current track is the card.
                            let position = if *index < previous_count {
                                QueuePosition::Previous(*index)
                            } else {
                                QueuePosition::Next(*index - previous_count - usize::from(has_current))
                            };
                            let upcoming = match position {
                                QueuePosition::Next(next) => Some(next),
                                QueuePosition::Previous(_) => None,
                            };
                            let locked = upcoming.is_some_and(|next| next < fixed);
                            let row_actions = track_row(
                                ui,
                                rect,
                                row.id,
                                track,
                                cover.as_ref(),
                                RowStyle {
                                    alpha,
                                    current: false,
                                    playing,
                                    pause_button: false,
                                    // A locked track can go only to a playlist.
                                    draggable: true,
                                    can_add: false,
                                    remove_from: (!locked).then_some("the queue"),
                                    locked,
                                    added: None,
                                    highlighted: false,
                                    menu_open: false,
                                    accent,
                                    time,
                                },
                            );
                            if row_actions.play {
                                actions.push(Command::PlayFromQueue {
                                    position,
                                    uri: track.uri.clone(),
                                });
                            }
                            if row_actions.remove {
                                self.keys.removed(*index, time);
                                let uri = track.uri.clone();
                                actions.push(Command::EditQueue(match position {
                                    QueuePosition::Next(index) => QueueEdit::Remove { index, uri },
                                    QueuePosition::Previous(index) => {
                                        QueueEdit::RemovePrevious { index, uri }
                                    }
                                }));
                            }
                            if let Some(grab) = row_actions.drag_from {
                                dnd::start(
                                    &ctx,
                                    DragItem::Track((*track).clone()),
                                    DragOrigin::Queue(*index),
                                    rect,
                                    grab,
                                );
                            }
                        }
                    }
                }
                if let Some((track, rect, id, alpha, morph)) = card {
                    // Its position, kept in the visible part of the list.
                    let viewport = ui.clip_rect();
                    let top = rect
                        .top()
                        .min(viewport.bottom() - rect.height())
                        .max(viewport.top());
                    let pinned = (top - rect.top()).abs() > 0.5;
                    if pinned && top > rect.top() {
                        pinned_insets.0 = rect.height();
                    } else if pinned {
                        pinned_insets.1 = rect.height();
                    }
                    let card_rect = Rect::from_min_size(pos2(rect.left(), top), rect.size());
                    if ui.is_rect_visible(card_rect) {
                        let cover = track
                            .cover_small
                            .as_deref()
                            .and_then(|url| textures.get(url, THUMB_PX, time));
                        let grab = self.current_card(
                            ui,
                            card_rect,
                            id,
                            Some(track),
                            cover.as_ref(),
                            data,
                            CardStyle {
                                alpha,
                                morph,
                                pinned,
                                playing,
                                time,
                            },
                            actions,
                        );
                        if let Some(grab) = grab {
                            dnd::start_card(
                                &ctx,
                                track.clone(),
                                DragOrigin::Queue(previous_count),
                                card_rect,
                                ROW_HEIGHT,
                                grab,
                            );
                        }
                    }
                }
                // A new current track goes to the top, so that the upcoming tracks show below it.
                if scroll_to_current && let Some(target) = current_target {
                    ui.scroll_to_rect(target, Some(Align::Min));
                }

                // The position of each track after the change, and the part of the list that
                // shows them (not under the pinned card).
                let mut y = 0.0;
                let placed: Vec<Placed> = rows
                    .iter()
                    .filter_map(|row| {
                        let top = y;
                        y += row.height;
                        match row.kind {
                            RowKind::Track { index, .. } => Some(Placed {
                                key: keys[index],
                                rect: Rect::from_min_size(
                                    pos2(area.left(), area.top() + top),
                                    vec2(width, row.height),
                                ),
                                card: has_current && index == previous_count,
                            }),
                            RowKind::Slot => None,
                        }
                    })
                    .collect();
                let viewport = ui.clip_rect();
                let view = Rect::from_min_max(
                    pos2(viewport.left(), viewport.top() + pinned_insets.0),
                    pos2(viewport.right(), viewport.bottom() - pinned_insets.1),
                );
                self.watch_arrival(&placed, view, time);
                if let Some(target) = self.added_notice(ui, view, &placed, accent, time) {
                    ui.scroll_to_rect(target, Some(Align::Center));
                }
            });
        widgets::scroll_hints(&list_ui, &scrolled, pinned_insets);
    }

    /// Finds the tracks that the user adds in `placed`. If none of them is visible in `view`, it
    /// starts the notice.
    fn watch_arrival(&mut self, placed: &[Placed], view: Rect, time: f64) {
        let Some(arrival) = &mut self.arrival else {
            return;
        };
        let since = *arrival.since.get_or_insert(time);
        let added: Vec<&Placed> = placed
            .iter()
            .filter(|p| !arrival.known.contains(&p.key))
            .collect();
        if added.is_empty() {
            if time - since > ARRIVAL_WAIT {
                self.arrival = None;
            }
            return;
        }
        self.arrival = None;
        if added.iter().any(|p| p.in_sight(view)) {
            return;
        }
        self.notice = Some(AddedNotice {
            keys: added.iter().map(|p| p.key).collect(),
            below: added[0].rect.top() >= view.center().y,
            since: time,
            fading_out: false,
        });
    }

    /// The pill that tells that added tracks went out of view, at the edge of `view` where they
    /// are. It slides in from that edge, and its arrow moves up and down towards them. It fades
    /// out after some time or when they show. Returns their position when the user clicks it.
    fn added_notice(
        &mut self,
        ui: &mut Ui,
        view: Rect,
        placed: &[Placed],
        accent: Color32,
        time: f64,
    ) -> Option<Rect> {
        let notice = self.notice.as_mut()?;
        let ctx = ui.ctx().clone();
        let targets: Vec<&Placed> = placed
            .iter()
            .filter(|p| notice.keys.contains(&p.key))
            .collect();
        if targets.is_empty()
            || targets.iter().any(|p| p.in_sight(view))
            || time - notice.since > NOTICE_SECS
        {
            notice.fading_out = true;
        }
        let id = Id::unique("queue-added-notice");
        let shown = anim::tween_bool(&ctx, id.with("shown"), !notice.fading_out, 0.22);
        if notice.fading_out && shown <= 0.001 {
            self.notice = None;
            return None;
        }
        if !notice.fading_out {
            // The arrow continues to move until the pill goes.
            anim::request_frame(&ctx);
        }
        let text = notice.text();
        let below = notice.below;
        let galley = single_line(
            ui.painter(),
            &text,
            theme::strong(theme::FONT_SMALL),
            theme::TEXT,
            view.width() - 60.0,
        );
        let size = vec2(galley.size().x + 50.0, 30.0);
        let slide = (1.0 - ease_out_cubic(shown)) * 16.0;
        let center = if below {
            pos2(view.center().x, view.bottom() - 12.0 - size.y / 2.0 + slide)
        } else {
            pos2(view.center().x, view.top() + 12.0 + size.y / 2.0 - slide)
        };
        let pill = Rect::from_center_size(center, size);
        let mut pill_ui = ui.new_child(UiBuilder::new().max_rect(pill));
        pill_ui.multiply_opacity(shown);
        let response = custom_button_with_ring(
            &mut pill_ui,
            pill,
            id,
            &text,
            !notice.fading_out,
            pill.height() / 2.0,
            |painter, rect, hover, _| {
                let radius = rect.height() / 2.0;
                painter.add(
                    Shadow {
                        offset: [0, 6],
                        blur: 18,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    }
                    .as_shape(rect, radius),
                );
                painter.rect(
                    rect,
                    radius,
                    lerp_color(theme::SURFACE, theme::SURFACE_HOVER, hover),
                    Stroke::new(1.0, accent.gamma_multiply(0.7)),
                    StrokeKind::Inside,
                );
                let bob = (time * std::f64::consts::TAU * 1.5).sin().abs() as f32 * 3.0;
                let arrow = pos2(
                    rect.left() + 18.0,
                    rect.center().y + if below { bob - 1.5 } else { 1.5 - bob },
                );
                icons::arrow(painter, arrow, 16.0, below, accent);
                painter.galley(
                    pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0),
                    galley,
                    theme::TEXT,
                );
            },
        );
        if !response.clicked() {
            return None;
        }
        notice.fading_out = true;
        targets.first().map(|p| p.rect)
    }

    /// The controls of a card that changes back into a row fade out as they were when it was the
    /// current track. They do not respond.
    fn leaving_controls(
        &self,
        ui: &mut Ui,
        rect: Rect,
        id: Id,
        track: &Track,
        data: &QueueData<'_>,
        style: CardStyle,
    ) {
        let (progress_ms, is_playing) = match self.leaving {
            Some(leaving) if leaving.id == id => (leaving.progress_ms, leaving.is_playing),
            _ => (track.duration_ms, false),
        };
        // Measure it now, so that the position stays the same.
        let playback = PlaybackSnapshot {
            track: Some(track.clone()),
            is_playing,
            progress_ms,
            measured_at: std::time::Instant::now(),
            ..data.playback.clone()
        };
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect));
        ui.disable();
        let card = rect.shrink2(vec2(2.0, 3.0));
        let parts = CardControls {
            seek: &mut SeekBar::default(),
            volume: &mut VolumeSlider::default(),
        };
        parts.show(
            &mut ui,
            rect,
            card,
            id,
            &playback,
            data,
            &style,
            &mut Vec::new(),
        );
    }

    /// The card of the current track: cover, title, the seek bar, the transport buttons and the
    /// volume. The user drags it by its top part (cover and title). Returns the point where the
    /// drag starts. Without a track (empty queue), it shows a placeholder, the user cannot drag
    /// it, and only the volume works.
    #[expect(clippy::too_many_arguments)]
    fn current_card(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        id: Id,
        track: Option<&Track>,
        cover: Option<&Cover>,
        data: &QueueData<'_>,
        style: CardStyle,
        actions: &mut Vec<Command>,
    ) -> Option<Pos2> {
        let ctx = ui.ctx().clone();
        let accent = data.accent;
        // Without a track, the user cannot seek or skip, whatever Spotify tells.
        let idle;
        let playback = if track.is_some() {
            data.playback
        } else {
            idle = PlaybackSnapshot {
                track: None,
                is_playing: false,
                progress_ms: 0,
                ..data.playback.clone()
            };
            &idle
        };
        let card = rect.shrink2(vec2(2.0, 3.0));
        // It covers the rows below it, also for the pointer (and only for the pointer: it is not a
        // Tab stop).
        ui.interact(card, id.with("card"), Sense::CLICK);
        let lift = anim::tween_bool(&ctx, id.with("pinned"), style.pinned, 0.18);
        let (name, details) = match track {
            Some(track) => (track.name.clone(), track.subtitle()),
            None => (EMPTY_TITLE.to_owned(), EMPTY_SUBTITLE.to_owned()),
        };
        a11y_label(ui, card, id.with("label"), &format!("{name} — {details}"));
        let layout = paint_face(
            ui,
            rect,
            &Face {
                id,
                track,
                cover,
                morph: style.morph,
                alpha: style.alpha,
                lift,
                playing: style.playing,
                accent,
                time: style.time,
            },
        );

        let thumb = layout.thumb;
        let grab = if track.is_some() {
            let handle = Rect::from_min_max(card.min, pos2(card.right(), thumb.bottom() + 8.0));
            let mut response = ui.interact(handle, id.with("drag"), Sense::CLICK | Sense::DRAG);
            if !dnd::is_dragging(&ctx) {
                response = response.on_hover_cursor(egui::CursorIcon::Grab);
            }
            // The point where the button went down, as for the other rows.
            response
                .drag_started()
                .then(|| {
                    ctx.input(|i| i.pointer.press_origin())
                        .or(response.interact_pointer_pos())
                })
                .flatten()
        } else {
            None
        };
        // Over the handle, so that it gets the clicks.
        if let Some(track) = track {
            artist::links(
                ui,
                id,
                track,
                layout.card_subtitle,
                theme::regular(theme::FONT_BODY),
            );
            album::link(
                ui,
                id,
                track,
                layout.card_subtitle,
                theme::regular(theme::FONT_BODY),
            );
            // After the links, so that it is over them and also shows there.
            if let Some(galley) = &layout.card_subtitle_galley {
                widgets::full_text_tooltip(
                    ui,
                    layout.card_subtitle,
                    id.with("subtitle"),
                    galley,
                    &details,
                );
            }
            // Level with the cover, at the right edge. It comes in with the other parts of the
            // card while the card grows from a row.
            let reveal = ease_out_cubic((style.morph - 0.4) / 0.6);
            if liked::can_like(track) && reveal > 0.0 {
                let center = pos2(
                    card.right() - 14.0 - CARD_HEART_ROOM / 2.0,
                    thumb.center().y,
                );
                liked::heart_button(ui, center, id, track, 1.0, style.alpha * reveal, accent);
            }
        }

        let parts = CardControls {
            seek: &mut self.seek,
            volume: &mut self.volume,
        };
        parts.show(ui, rect, card, id, playback, data, &style, actions);
        if track.is_some() {
            self.leaving = Some(LeavingCard {
                id,
                progress_ms: playback.progress_at(std::time::Instant::now()),
                is_playing: playback.is_playing,
            });
        }
        grab
    }
}

/// The controls of a card, with their own state. The current card uses the state of the view. A
/// card that changes back into a row gets a new state.
struct CardControls<'a> {
    seek: &'a mut SeekBar,
    volume: &'a mut VolumeSlider,
}

impl CardControls<'_> {
    /// The seek bar, the transport buttons and the volume of the card in
    /// `rect` (`card` is its visible shape).
    #[expect(clippy::too_many_arguments)]
    fn show(
        self,
        ui: &mut Ui,
        rect: Rect,
        card: Rect,
        id: Id,
        playback: &PlaybackSnapshot,
        data: &QueueData<'_>,
        style: &CardStyle,
        actions: &mut Vec<Command>,
    ) {
        let accent = data.accent;
        // The controls keep their position in the full card. While the card grows from a row, they
        // fade in and show when the card uncovers them (and the opposite when it becomes a row).
        let reveal = ease_out_cubic((style.morph - 0.4) / 0.6);
        if reveal <= 0.0 {
            return;
        }
        let full =
            Rect::from_min_size(rect.min, vec2(rect.width(), CARD_HEIGHT)).shrink2(vec2(2.0, 3.0));
        let mut controls =
            ui.new_child(UiBuilder::new().id_salt(id.with("controls")).max_rect(full));
        controls.set_clip_rect(card.intersect(ui.clip_rect()));
        controls.multiply_opacity(style.alpha * reveal);
        let pad = 14.0;
        let seek = Rect::from_min_size(
            pos2(
                full.left() + pad + 2.0,
                full.top() + pad + CARD_COVER + 16.0,
            ),
            vec2(full.width() - 2.0 * (pad + 2.0), 34.0),
        );
        self.seek.show(
            &mut controls,
            seek,
            id.with("seek"),
            playback,
            accent,
            actions,
        );
        let controls_y = seek.bottom() + 30.0;
        let transport_x = full.left() + pad + 78.0;
        transport(
            &mut controls,
            pos2(transport_x, controls_y),
            0.7,
            id.with("transport"),
            playback,
            actions,
        );
        // The volume fills the remaining part of the row, after the "next" button (it goes to 72
        // px right of the transport center at this scale). In a narrow queue, only the speaker
        // fits: it goes in the gap between that button and the right edge.
        let next_right = transport_x + 72.0;
        // Aligned with the right end of the seek bar.
        let volume_right = seek.right();
        let volume_left = if volume_right - (next_right + 20.0) >= VolumeSlider::MIN_WIDTH {
            next_right + 20.0
        } else {
            next_right
        };
        let volume = Rect::from_min_max(
            pos2(volume_left, controls_y - 16.0),
            pos2(volume_right, controls_y + 16.0),
        );
        self.volume.show(
            &mut controls,
            volume,
            id.with("volume"),
            playback,
            accent,
            actions,
        );
    }
}

/// What the card of a track last showed. Thus its controls can fade out as they were when it
/// stops as the current track.
#[derive(Clone, Copy)]
struct LeavingCard {
    /// The row id of the card.
    id: Id,
    progress_ms: u64,
    is_playing: bool,
}

const EMPTY_TITLE: &str = "Nothing playing";
const EMPTY_SUBTITLE: &str = "Pick a song to get started";

#[derive(Clone, Copy)]
struct CardStyle {
    /// The opacity of the full card.
    alpha: f32,
    /// 0 while it is still a row, 1 when it is the full card.
    morph: f32,
    /// Pinned to the top or the bottom of the list (it floats over the rows).
    pinned: bool,
    /// The motion (0..1) of the equalizer.
    playing: f32,
    time: f64,
}

/// The look of an entry of the queue, from a row (`morph` 0) to the card of the current track
/// (`morph` 1), without the controls of the card.
struct Face<'a> {
    /// The id of the row (or card). The hover areas of its texts make their ids from it.
    id: Id,
    /// `None`: the placeholder of the empty queue (always a card).
    track: Option<&'a Track>,
    cover: Option<&'a Cover>,
    morph: f32,
    alpha: f32,
    /// The shadow of the pinned card (0..1).
    lift: f32,
    /// The motion (0..1) of the equalizer.
    playing: f32,
    accent: Color32,
    time: f64,
}

/// The positions of the items that [`paint_face`] painted.
struct FaceLayout {
    thumb: Rect,
    /// The line under the title, when it is the full card ("artist · album", cut). Before that,
    /// [`Rect::NOTHING`].
    card_subtitle: Rect,
    /// That line as painted, to show it in full in a tooltip if it was cut.
    card_subtitle_galley: Option<Arc<Galley>>,
}

/// Paints the background, the cover and the texts of `face` in `rect`: a row, the card, or a
/// shape between them.
fn paint_face(ui: &mut Ui, rect: Rect, face: &Face<'_>) -> FaceLayout {
    let t = face.morph.clamp(0.0, 1.0);
    let accent = face.accent;
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(face.alpha);
    let card = rect.shrink2(vec2(2.0, 3.0));
    let radius = lerp(theme::RADIUS_MEDIUM, theme::RADIUS_LARGE, t);
    if face.lift > 0.0 {
        let shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha((140.0 * face.lift) as u8),
        };
        // When the card is pinned to an edge of the list, the edge cuts the shadow in a straight
        // line. Thus the shadow is shorter on that side, so that it fits.
        let margin = shadow.margin();
        let clip = painter.clip_rect();
        let mut caster = card;
        caster.min.y += (clip.top() - (card.top() - margin.top)).max(0.0);
        caster.max.y -= ((card.bottom() + margin.bottom) - clip.bottom()).max(0.0);
        if caster.height() > 0.0 {
            painter.add(shadow.as_shape(caster, radius));
        }
    }
    if t > 0.0 {
        painter.rect_filled(card, radius, theme::BG.gamma_multiply(t));
        // While it floats, only the shadow shows its edge, without a border. egui paints the
        // stroke in place of the fill, not over it, so the border fades into the tint. A fade to
        // transparent would leave a dark edge (the bare background) until the stroke goes away.
        let border = lerp(0.18, 0.12, face.lift);
        painter.rect(
            card,
            radius,
            accent.gamma_multiply(0.12 * t),
            Stroke::new(1.0, accent.gamma_multiply(border * t)),
            StrokeKind::Inside,
        );
    }

    // The cover slides and grows from its position in a row to its position in the card.
    let row_thumb = pos2(
        rect.left() + ROW_COVER_INSET,
        rect.top() + (ROW_HEIGHT - ROW_COVER) / 2.0,
    );
    let card_thumb = card.min + vec2(14.0, 14.0);
    let size = lerp(ROW_COVER, CARD_COVER, t);
    let thumb = Rect::from_min_size(
        pos2(
            lerp(row_thumb.x, card_thumb.x, t),
            lerp(row_thumb.y, card_thumb.y, t),
        ),
        vec2(size, size),
    );
    if face.track.is_some() {
        let mut thumb_ui = ui.new_child(UiBuilder::new().max_rect(thumb));
        thumb_ui.multiply_opacity(face.alpha);
        // Nothing goes over the artwork (Spotify's guidelines): the equalizer goes before the
        // title.
        paint_cover(&thumb_ui, thumb, face.cover, theme::RADIUS_SMALL, 1.0);
    } else {
        painter.rect_filled(thumb, theme::RADIUS_SMALL, Color32::from_white_alpha(10));
        icons::music_note(&painter, thumb.center(), 26.0, theme::TEXT_FAINT);
    }

    // The texts of the row change into the larger texts of the card.
    let (name, details) = match face.track {
        Some(track) => (track.name.as_str(), track.subtitle()),
        None => (EMPTY_TITLE, EMPTY_SUBTITLE.to_owned()),
    };
    let (title_color, subtitle_color) = if face.track.is_some() {
        let card_title = lerp_color(theme::TEXT, accent, 0.85);
        (lerp_color(theme::TEXT, card_title, t), theme::TEXT_DIM)
    } else {
        (theme::TEXT_DIM, theme::TEXT_FAINT)
    };
    let text_left = thumb.right() + widgets::ROW_TEXT_GAP;
    // A row keeps space on the right for the duration (or the ✕).
    let row_width = rect.right() - 12.0 - 46.0 - text_left;
    // The card keeps space on the right for the ♥ of a song.
    let heart_room = if face.track.is_some_and(liked::can_like) {
        CARD_HEART_ROOM
    } else {
        0.0
    };
    let card_width = card.right() - 14.0 - heart_room - text_left;
    let text_width = lerp(row_width, card_width, t).max(40.0);
    // A quick change at the halfway point, so the two sizes do not show one over the other.
    let swap = ((t - 0.4) / 0.2).clamp(0.0, 1.0);
    let mut card_subtitle = Rect::NOTHING;
    let mut card_subtitle_galley = None;
    let styles = [
        (
            1.0 - swap,
            theme::regular(theme::FONT_BODY),
            theme::regular(theme::FONT_SMALL),
            3.0,
        ),
        (
            swap,
            theme::strong(theme::FONT_TITLE),
            theme::regular(theme::FONT_BODY),
            4.0,
        ),
    ];
    // The equalizer of the card comes in before its title, which moves right to make space for
    // it.
    let playing_mark = if face.track.is_some() { t } else { 0.0 };
    let title_left = text_left + CARD_EQUALIZER_ROOM * playing_mark;
    let explicit = face.track.is_some_and(|track| track.explicit);
    for (weight, title_font, subtitle_font, gap) in styles {
        if weight <= 0.0 {
            continue;
        }
        let mut text = painter.clone();
        text.multiply_opacity(weight);
        let title_width =
            (text_width - (title_left - text_left) - widgets::explicit_room(explicit)).max(20.0);
        let title = single_line(&text, name, title_font, title_color, title_width);
        let subtitle = single_line(&text, &details, subtitle_font, subtitle_color, text_width);
        let title_height = title.size().y;
        let top = thumb.center().y - (title_height + gap + subtitle.size().y) / 2.0;
        let title_rect = Rect::from_min_size(pos2(title_left, top), title.size());
        if playing_mark > 0.0 {
            let mut bars = text.clone();
            bars.multiply_opacity(playing_mark);
            let size = vec2(lerp(12.0, 14.0, t), lerp(11.0, 13.0, t));
            let bars_rect =
                Rect::from_min_size(pos2(text_left, title_rect.center().y - size.y / 2.0), size);
            icons::equalizer(&bars, bars_rect, face.time, face.playing, accent);
            if face.playing > 0.0 {
                anim::request_frame_after(ui.ctx(), std::time::Duration::from_millis(33));
            }
        }
        let subtitle_pos = pos2(text_left, top + title_height + gap);
        let subtitle_rect = Rect::from_min_size(subtitle_pos, subtitle.size());
        if t >= 1.0 {
            card_subtitle = subtitle_rect;
            widgets::full_text_tooltip(ui, title_rect, face.id.with("title"), &title, name);
            card_subtitle_galley = Some(subtitle.clone());
        }
        text.galley(title_rect.min, title, title_color);
        if explicit {
            widgets::explicit_badge(
                ui,
                &text,
                title_rect.right() + widgets::EXPLICIT_BADGE_GAP,
                title_rect.center().y,
                face.id.with(weight > 0.5),
            );
        }
        text.galley(subtitle_pos, subtitle, subtitle_color);
    }

    // The duration of the row fades out.
    if let Some(track) = face.track
        && t < 1.0
    {
        let mut faded = painter.clone();
        faded.multiply_opacity(1.0 - t);
        let duration = single_line(
            &faded,
            &format_duration(track.duration_ms),
            theme::regular(theme::FONT_SMALL),
            theme::TEXT_FAINT,
            46.0,
        );
        faded.galley(
            pos2(
                rect.right() - 16.0 - duration.size().x,
                rect.top() + ROW_HEIGHT / 2.0 - duration.size().y / 2.0,
            ),
            duration,
            theme::TEXT_FAINT,
        );
    }
    FaceLayout {
        thumb,
        card_subtitle,
        card_subtitle_galley,
    }
}

/// True if the entry at `index` of the full queue (see [`whole_queue`]) can move in it. In the
/// last seconds of a track, the app already sent the next track to Spotify. That track and the
/// current track cannot move.
fn can_move(queue: &QueueSnapshot, index: usize) -> bool {
    let previous_count = queue.previous.len();
    let has_current = queue.current.is_some();
    match index.checked_sub(previous_count + usize::from(has_current)) {
        Some(next) => next >= queue.fixed(),
        None => index < previous_count || queue.fixed() == 0,
    }
}

/// The previous tracks, the current track and the upcoming tracks, in order.
fn whole_queue(queue: &QueueSnapshot) -> impl Iterator<Item = &Track> {
    queue
        .previous
        .iter()
        .chain(&queue.current)
        .chain(&queue.next)
}

/// The rows of `entries` (tracks of `all`, with the keys `keys`, and the slot, each with its
/// section, in order). The card of the current track is `all[previous_count]`, if there is one.
/// `slot` is the id and height of the slot.
fn layout<'a>(
    entries: &[(Entry, Section)],
    all: &[&'a Track],
    keys: &[u64],
    previous_count: usize,
    has_current: bool,
    (slot_id, slot_height): (Id, f32),
) -> Vec<Row<'a>> {
    entries
        .iter()
        .map(|(entry, section)| match entry {
            Entry::Track(index) => {
                let is_card = has_current && *index == previous_count;
                Row {
                    id: row_id(keys[*index]),
                    kind: RowKind::Track {
                        track: all[*index],
                        section: *section,
                        index: *index,
                    },
                    height: if is_card { CARD_HEIGHT } else { ROW_HEIGHT },
                }
            }
            Entry::Slot => Row {
                id: slot_id,
                kind: RowKind::Slot,
                height: slot_height,
            },
        })
        .collect()
}

/// Each row has a key from its entry (see [`QueueKeys`]). Thus, when the queue moves, each row
/// glides to its new position and does not jump. This also works when a track is in the queue
/// more than one time.
fn row_id(key: u64) -> Id {
    Id::unique(("queue-row", key))
}
