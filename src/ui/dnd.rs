//! Drag and drop of tracks, albums, playlists and folders between the views: the payload, the
//! floating copy, the drop slot and the automatic scroll. It uses the `DragAndDrop` payload of
//! egui. egui removes the payload when the user releases the button or pushes Escape.

use std::sync::Arc;

use egui::style::ScrollAnimation;
use egui::{
    Area, Color32, Context, Id, Order, Pos2, Rect, Sense, Shadow, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};

use super::anim::{self, ease_out_cubic, lerp, lerp_color};
use super::icons;
use super::textures::{Cover, THUMB_PX, TextureCache};
use super::theme;
use super::widgets::{
    self, ROW_COVER, ROW_COVER_INSET, ROW_HEIGHT, a11y_label, paint_cover, single_line,
};
use crate::model::{Playlist, Release, Track};

/// A drag this near to the top or bottom edge of a list scrolls it…
const AUTOSCROLL_BAND: f32 = 56.0;
/// …at a maximum of this number of points per second.
const AUTOSCROLL_SPEED: f32 = 700.0;

/// The source of a dragged item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOrigin {
    Search,
    /// The track at this index of the full queue (previous tracks, the current track, then the
    /// upcoming tracks) when the drag started.
    Queue(usize),
    /// The playlist or folder at this index of the level of the sidebar that shows.
    Playlists(usize),
    /// The track at this index of the playlist panel.
    PlaylistTracks(usize),
    /// A track of the album that shows.
    Album,
    /// An album or single of the artist that shows.
    Artist,
}

/// The dragged item.
#[derive(Debug, Clone)]
pub enum DragItem {
    Track(Track),
    Playlist(Playlist),
    /// An album or single of an artist. When the user drops it on the queue, all its tracks go
    /// there.
    Release(Release),
    /// A folder of the sidebar: it can go only to a different position in the sidebar.
    Folder(DraggedFolder),
}

/// What the floating copy of a dragged folder shows.
#[derive(Debug, Clone)]
pub struct DraggedFolder {
    pub id: String,
    pub name: String,
    /// "Folder · 3 playlists".
    pub subtitle: String,
    /// The color of its tile.
    pub color: Color32,
}

impl DragItem {
    pub fn track(&self) -> Option<&Track> {
        match self {
            Self::Track(track) => Some(track),
            Self::Playlist(_) | Self::Release(_) | Self::Folder(_) => None,
        }
    }

    fn cover(&self) -> Option<&str> {
        match self {
            Self::Track(track) => track.cover_small.as_deref(),
            Self::Playlist(playlist) => playlist.cover.as_deref(),
            Self::Release(release) => release.cover.as_deref(),
            Self::Folder(_) => None,
        }
    }

    fn title(&self) -> &str {
        match self {
            Self::Track(track) => &track.name,
            Self::Playlist(playlist) => &playlist.name,
            Self::Release(release) => &release.name,
            Self::Folder(folder) => &folder.name,
        }
    }

    fn subtitle(&self) -> String {
        match self {
            Self::Track(track) => track.subtitle(),
            Self::Playlist(playlist) => format!("Playlist · {}", playlist.count_line()),
            Self::Release(release) => release.summary(),
            Self::Folder(folder) => folder.subtitle.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dragged {
    pub item: DragItem,
    pub origin: DragOrigin,
    /// The point where the user took the row, from its top-left corner.
    pub grab: Vec2,
    /// The size of the row. The floating copy keeps it.
    pub size: Vec2,
    /// It is the card of the current track, and it floats as that card.
    pub card: bool,
    /// The distance from its top to the point that gives the drop position.
    anchor: f32,
    /// The UI time when the drag started.
    pub since: f64,
}

impl Dragged {
    pub fn track(&self) -> Option<&Track> {
        self.item.track()
    }

    /// The top edge of the floating copy when the pointer is at `pointer_y`.
    pub fn top(&self, pointer_y: f32) -> f32 {
        pointer_y - self.grab.y
    }

    /// The point of the floating copy that gives the drop position. It is its vertical center, or
    /// the middle of the top part of a card (with the height of a row). Thus a card goes to the
    /// same position as a row.
    pub fn landing_y(&self, pointer_y: f32) -> f32 {
        self.top(pointer_y) + self.anchor
    }
}

/// Starts a drag of `item`, taken at `pointer` on its row `row`. The floating copy is at least as
/// large as a track row.
pub fn start(ctx: &Context, item: DragItem, origin: DragOrigin, row: Rect, pointer: Pos2) {
    let size = vec2(
        row.width().clamp(300.0, 420.0),
        row.height().max(ROW_HEIGHT),
    );
    set_payload(ctx, item, origin, row, size, false, size.y / 2.0, pointer);
}

/// Starts a drag of the card `card` of the current track, taken at `pointer`. It floats as a card
/// of the same size, and goes to the same position as a track row of height `row_height`.
pub fn start_card(
    ctx: &Context,
    track: Track,
    origin: DragOrigin,
    card: Rect,
    row_height: f32,
    pointer: Pos2,
) {
    let size = vec2(card.width().clamp(300.0, 420.0), card.height());
    let item = DragItem::Track(track);
    set_payload(
        ctx,
        item,
        origin,
        card,
        size,
        true,
        row_height / 2.0,
        pointer,
    );
}

#[expect(clippy::too_many_arguments)]
fn set_payload(
    ctx: &Context,
    item: DragItem,
    origin: DragOrigin,
    row: Rect,
    size: Vec2,
    card: bool,
    anchor: f32,
    pointer: Pos2,
) {
    egui::DragAndDrop::set_payload(
        ctx,
        Dragged {
            item,
            origin,
            grab: (pointer - row.min).min(size),
            size,
            card,
            anchor,
            since: ctx.input(|i| i.time),
        },
    );
}

pub fn current(ctx: &Context) -> Option<Arc<Dragged>> {
    egui::DragAndDrop::payload(ctx)
}

/// Takes the dragged item when the user drops it.
pub fn take(ctx: &Context) -> Option<Arc<Dragged>> {
    egui::DragAndDrop::take_payload(ctx)
}

pub fn is_dragging(ctx: &Context) -> bool {
    egui::DragAndDrop::has_payload_of_type::<Dragged>(ctx)
}

/// The pointer button went up in this frame: a drag ends here.
pub fn released(ctx: &Context) -> bool {
    ctx.input(|i| i.pointer.any_released())
}

/// Draws the floating copy of the dragged row under the pointer. It lifts off with a small pop.
/// Over a position where the user can drop it, its border gets the accent color.
pub fn paint_ghost(ctx: &Context, textures: &mut TextureCache, accent: Color32, over_target: bool) {
    let (Some(dragged), Some(pointer)) = (current(ctx), ctx.pointer_latest_pos()) else {
        return;
    };
    let elapsed = anim::elapsed_since(ctx, dragged.since);
    anim::keep_animating(ctx, elapsed, 0.2);
    let lift = ease_out_cubic(elapsed / 0.2);
    let target = anim::tween_bool(ctx, Id::unique("drag-ghost-target"), over_target, 0.15);
    let time = ctx.input(|i| i.time);
    let cover = dragged
        .item
        .cover()
        .and_then(|url| textures.get(url, THUMB_PX, time));

    let size = dragged.size * lerp(0.97, 1.0, lift);
    // Kept in the window horizontally (a queued track dragged to the playlists, on the left edge,
    // would leave it).
    let screen = ctx.content_rect();
    let left = (pointer.x - dragged.grab.x)
        .min(screen.right() - size.x - 8.0)
        .max(screen.left() + 8.0);
    let top_left = pos2(left, dragged.top(pointer.y));
    Area::new(Id::unique("drag-ghost"))
        .order(Order::Tooltip)
        .interactable(false)
        .constrain(false)
        .fixed_pos(top_left)
        .show(ctx, |ui| {
            ui.set_opacity(0.97 * lift);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let painter = ui.painter();
            painter.add(
                Shadow {
                    offset: [0, 14],
                    blur: 32,
                    spread: 0,
                    color: Color32::from_black_alpha(150),
                }
                .as_shape(rect, 12.0),
            );
            let border = lerp_color(theme::BORDER, accent.gamma_multiply(0.9), target);
            if dragged.card {
                paint_card(ui, rect, &dragged.item, cover.as_ref(), accent, target);
                return;
            }
            painter.rect(
                rect,
                theme::RADIUS_MEDIUM,
                theme::SURFACE_HOVER,
                Stroke::new(1.0 + target, border),
                StrokeKind::Inside,
            );
            let thumb = Rect::from_min_size(
                pos2(
                    rect.left() + ROW_COVER_INSET,
                    rect.center().y - ROW_COVER / 2.0,
                ),
                Vec2::splat(ROW_COVER),
            );
            match &dragged.item {
                DragItem::Folder(folder) => {
                    icons::folder_tile(painter, thumb, folder.color, 0.0, 1.0);
                }
                DragItem::Playlist(_) => {
                    widgets::paint_playlist_cover(ui, thumb, cover.as_ref(), 1.0);
                }
                _ => paint_cover(ui, thumb, cover.as_ref(), theme::RADIUS_SMALL, 1.0),
            }
            let left = thumb.right() + widgets::ROW_TEXT_GAP;
            let width = (rect.right() - 14.0 - left).max(20.0);
            let title = single_line(
                painter,
                dragged.item.title(),
                theme::regular(theme::FONT_BODY),
                theme::TEXT,
                width,
            );
            let subtitle = single_line(
                painter,
                &dragged.item.subtitle(),
                theme::regular(theme::FONT_SMALL),
                theme::TEXT_DIM,
                width,
            );
            let top = rect.center().y - (title.size().y + 2.0 + subtitle.size().y) / 2.0;
            let title_height = title.size().y;
            painter.galley(pos2(left, top), title, theme::TEXT);
            painter.galley(
                pos2(left, top + title_height + 2.0),
                subtitle,
                theme::TEXT_DIM,
            );
        });
}

/// The floating copy of the card of the current track: similar to the card, with its cover, title
/// and an empty seek bar (its controls stay on the card).
fn paint_card(
    ui: &Ui,
    rect: Rect,
    item: &DragItem,
    cover: Option<&Cover>,
    accent: Color32,
    target: f32,
) {
    let painter = ui.painter();
    let card = rect.shrink2(vec2(2.0, 3.0));
    painter.rect_filled(card, theme::RADIUS_LARGE, theme::BG);
    let border = lerp_color(
        accent.gamma_multiply(0.38),
        accent.gamma_multiply(0.9),
        target,
    );
    painter.rect(
        card,
        theme::RADIUS_LARGE,
        accent.gamma_multiply(0.16),
        Stroke::new(1.0 + target, border),
        StrokeKind::Inside,
    );
    let pad = 14.0;
    let thumb = Rect::from_min_size(card.min + vec2(pad, pad), Vec2::splat(64.0));
    paint_cover(ui, thumb, cover, theme::RADIUS_SMALL, 1.0);
    let left = thumb.right() + widgets::ROW_TEXT_GAP;
    let width = (card.right() - pad - left).max(40.0);
    let title_color = lerp_color(theme::TEXT, accent, 0.85);
    let title = single_line(
        painter,
        item.title(),
        theme::strong(theme::FONT_TITLE),
        title_color,
        width,
    );
    let subtitle = single_line(
        painter,
        &item.subtitle(),
        theme::regular(theme::FONT_BODY),
        theme::TEXT_DIM,
        width,
    );
    let top = thumb.center().y - (title.size().y + 4.0 + subtitle.size().y) / 2.0;
    let title_height = title.size().y;
    painter.galley(pos2(left, top), title, title_color);
    painter.galley(
        pos2(left, top + title_height + 4.0),
        subtitle,
        theme::TEXT_DIM,
    );
    let seek = Rect::from_center_size(
        pos2(card.center().x, thumb.bottom() + 33.0),
        vec2(card.width() - 2.0 * (pad + 2.0), 4.0),
    );
    painter.rect_filled(seek, 2.0, theme::TEXT_FAINT.gamma_multiply(0.35));
}

/// Scrolls the list of `ui` (in a `ScrollArea`), with the content `content`, while the pointer, at
/// `pointer_y`, is near its top or bottom edge (and there is more content in that direction).
pub fn autoscroll(ui: &mut Ui, content: Rect, pointer_y: f32) {
    let viewport = ui.clip_rect();
    let ctx = ui.ctx().clone();
    let dt = ctx.input(|i| i.stable_dt).min(0.05);
    let up = (viewport.top() + AUTOSCROLL_BAND - pointer_y) / AUTOSCROLL_BAND;
    let down = (pointer_y - viewport.bottom() + AUTOSCROLL_BAND) / AUTOSCROLL_BAND;
    let speed = if up > 0.0 && content.top() < viewport.top() - 0.5 {
        up.min(1.0)
    } else if down > 0.0 && content.bottom() > viewport.bottom() + 0.5 {
        -down.min(1.0)
    } else {
        0.0
    };
    if speed != 0.0 {
        ui.scroll_with_delta_animation(
            vec2(0.0, speed * AUTOSCROLL_SPEED * dt),
            ScrollAnimation::none(),
        );
        anim::request_frame(&ctx);
    }
}

/// Makes the row `row` (with its offset tweened under `row.with("y")`) start at `drop_y`, from
/// the top of its list, and glide into its position.
pub fn glide_from_drop(ctx: &Context, row: Id, drop_y: f32) {
    anim::set_tween_value(ctx, row.with("y"), drop_y);
    anim::since_change(ctx, row.with("seen"), (), false);
}

/// The gap that opens in a list where the dragged item would go.
pub fn drop_slot(ui: &Ui, rect: Rect, id: Id, over: bool, alpha: f32, accent: Color32) {
    let ctx = ui.ctx();
    let hot = anim::tween_bool(ctx, id.with("hot"), over, 0.15);
    let slot = rect.shrink2(vec2(4.0, 4.0));
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(alpha);
    let border = lerp_color(theme::TEXT_FAINT.gamma_multiply(0.4), accent, hot);
    painter.rect(
        slot,
        theme::RADIUS_MEDIUM,
        accent.gamma_multiply(0.09 * hot),
        Stroke::new(1.0 + 0.5 * hot, border),
        StrokeKind::Inside,
    );
    // An accent bar shows the exact position.
    let bar = Rect::from_min_size(
        pos2(slot.left() + 1.5, slot.top() + 8.0),
        vec2(3.0, slot.height() - 16.0),
    );
    painter.rect_filled(bar, 1.5, accent.gamma_multiply(hot));
    a11y_label(ui, slot, id.with("label"), "It will go here");
}
