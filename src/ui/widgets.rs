//! Small building blocks shared by the views.

use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Galley, Id, Painter, Pos2, Rect, Response, Role,
    Sense, Stroke, StrokeKind, TextFormat, Ui, Vec2, WidgetInfo, pos2, vec2,
};

use super::anim::{self, ease_out_cubic, lerp, lerp_color};
use super::backdrop;
use super::dnd;
use super::icons;
use super::textures::Cover;
use super::theme;

/// Lays out one line of text, cut with "…" to `max_width`.
pub fn single_line(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::truncate_at_width(max_width.max(1.0));
    painter.layout_job(job)
}

/// As [`single_line`], with extra letter spacing (for small caps labels).
pub fn spaced_line(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    spacing: f32,
) -> Arc<Galley> {
    let job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    painter.layout_job(job)
}

/// Paints a galley with its top edge at `top`, aligned horizontally.
pub fn paint_galley(
    painter: &Painter,
    galley: Arc<Galley>,
    x: f32,
    top: f32,
    align: Align,
    alpha: f32,
) -> Rect {
    let size = galley.size();
    let left = match align {
        Align::Min => x,
        Align::Center => x - size.x / 2.0,
        Align::Max => x - size.x,
    };
    let rect = Rect::from_min_size(pos2(left, top), size);
    let mut painter = painter.clone();
    painter.multiply_opacity(alpha);
    painter.galley(rect.min, galley, theme::TEXT);
    rect
}

/// Gives painted content to screen readers (and UI tests).
pub fn a11y_label(ui: &Ui, rect: Rect, id: Id, text: &str) {
    let response = ui.interact(rect, id, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
}

/// Shows the full `text` in a tooltip while the pointer is over `rect`, if `galley` (its line,
/// painted there) was cut with «…». Spotify's guidelines tell that the user must be able to read a
/// cut name in full. A hover area does not take the clicks or the hover of the items under it.
pub fn full_text_tooltip(ui: &Ui, rect: Rect, id: Id, galley: &Galley, text: &str) {
    if galley.elided {
        ui.interact(rect, id.with("full-text"), Sense::hover())
            .on_hover_text(text);
    }
}

/// The width of the «E» badge of explicit content.
pub const EXPLICIT_BADGE: f32 = 14.0;
/// The gap between a title and its «E» badge.
pub const EXPLICIT_BADGE_GAP: f32 = 6.0;

/// The space that a title must leave on its line for the «E» badge, if `explicit`.
pub fn explicit_room(explicit: bool) -> f32 {
    if explicit {
        EXPLICIT_BADGE + EXPLICIT_BADGE_GAP
    } else {
        0.0
    }
}

/// The «E» badge of explicit content (as Spotify marks it), with its left edge at `left` and its
/// vertical center on `center_y`.
pub fn explicit_badge(ui: &Ui, painter: &Painter, left: f32, center_y: f32, id: Id) {
    let rect = Rect::from_min_size(
        pos2(left, center_y - EXPLICIT_BADGE / 2.0),
        Vec2::splat(EXPLICIT_BADGE),
    );
    painter.rect_filled(rect, 2.0, theme::TEXT_DIM);
    let galley = single_line(painter, "E", theme::strong(9.5), theme::BG, f32::INFINITY);
    painter.galley(rect.center() - galley.size() / 2.0, galley, theme::BG);
    ui.interact(rect, id.with("explicit"), Sense::hover())
        .on_hover_text("Explicit")
        .widget_info(|| WidgetInfo::labeled(Role::Label, true, "Explicit"));
}

/// A painted area that the user can click, with animated hover and press states. `paint` gets the
/// painter, the content rect and the progress of hover (0..1) and press (0..1). Tab goes to it,
/// and Enter or Space pushes it. Then a ring shows that it has the keyboard focus.
pub fn custom_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    label: &str,
    enabled: bool,
    paint: impl FnOnce(&Painter, Rect, f32, f32),
) -> Response {
    // Square buttons are circles. Other buttons are rounded rectangles.
    let ring_radius = if (rect.width() - rect.height()).abs() < 0.5 {
        rect.height() / 2.0
    } else {
        theme::RADIUS_MEDIUM.min(rect.height() / 2.0)
    };
    custom_button_with_ring(ui, rect, id, label, enabled, ring_radius, paint)
}

/// [`custom_button`] with the corner radius of its focus ring, for shapes that the default does
/// not fit (pills, covers…).
pub fn custom_button_with_ring(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    label: &str,
    enabled: bool,
    ring_radius: f32,
    paint: impl FnOnce(&Painter, Rect, f32, f32),
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let mut response = ui.interact(rect, id, sense);
    if enabled {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    }
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let hover = anim::tween_bool(
        ui.ctx(),
        id.with("hover"),
        enabled && response.hovered(),
        0.16,
    );
    let press = anim::tween_bool(
        ui.ctx(),
        id.with("press"),
        enabled && response.is_pointer_button_down_on(),
        0.08,
    );
    // Only the keyboard gives these buttons the focus (a click does not). Thus the ring never shows
    // after a mouse click.
    let focus = anim::tween_bool(
        ui.ctx(),
        id.with("focus"),
        enabled && response.has_focus(),
        0.12,
    );
    let mut painter = ui.painter().clone();
    if !enabled {
        painter.multiply_opacity(0.35);
    }
    paint(&painter, rect, hover, press);
    focus_ring(&painter, rect, ring_radius, focus);
    response
}

/// The ring around the element with the keyboard focus, just outside `rect`, faded in by `t`
/// (0..1).
pub fn focus_ring(painter: &Painter, rect: Rect, radius: f32, t: f32) {
    if t <= 0.0 {
        return;
    }
    painter.rect_stroke(
        rect.expand(1.0),
        radius + 1.0,
        Stroke::new(2.0, theme::FOCUS_RING.gamma_multiply(t)),
        StrokeKind::Outside,
    );
}

/// Round icon button: on hover, a faint circle appears and the icon becomes brighter.
pub fn icon_button(
    ui: &mut Ui,
    center: Pos2,
    diameter: f32,
    id: Id,
    label: &str,
    enabled: bool,
    paint_icon: impl FnOnce(&Painter, Pos2, Color32),
) -> Response {
    icon_button_hover(
        ui,
        center,
        diameter,
        id,
        label,
        enabled,
        |painter, center, color, _| paint_icon(painter, center, color),
    )
}

/// [`icon_button`] with an icon that also gets the hover animation (0 to 1).
pub fn icon_button_hover(
    ui: &mut Ui,
    center: Pos2,
    diameter: f32,
    id: Id,
    label: &str,
    enabled: bool,
    paint_icon: impl FnOnce(&Painter, Pos2, Color32, f32),
) -> Response {
    let rect = Rect::from_center_size(center, Vec2::splat(diameter));
    let response = custom_button(
        ui,
        rect,
        id,
        label,
        enabled,
        |painter, rect, hover, press| {
            let radius = diameter / 2.0 * (1.0 - 0.06 * press);
            painter.circle_filled(
                rect.center(),
                radius,
                Color32::from_white_alpha((14.0 * hover) as u8),
            );
            let color = lerp_color(theme::TEXT_DIM, theme::TEXT, hover);
            paint_icon(painter, rect.center(), color, hover);
        },
    );
    response.on_hover_text(label)
}

/// Rounded "pill" button with text, for example "Connect to Spotify".
pub fn pill_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    text: &str,
    fill: Color32,
    text_color: Color32,
    enabled: bool,
) -> Response {
    custom_button_with_ring(
        ui,
        rect,
        id,
        text,
        enabled,
        rect.height() / 2.0,
        |painter, rect, hover, press| {
            // Only the width of the pill changes (wider on hover, narrower while pushed). The text
            // keeps its size and position.
            let width = rect.width() * (1.0 + 0.05 * hover - 0.04 * press);
            let pill = Rect::from_center_size(rect.center(), vec2(width, rect.height()));
            let fill = lerp_color(fill, Color32::WHITE, 0.12 * hover);
            painter.rect_filled(pill, pill.height() / 2.0, fill);
            let galley = single_line(
                painter,
                text,
                theme::strong(theme::FONT_BODY),
                text_color,
                rect.width() - 24.0,
            );
            // The line box of the fonts has more space below the baseline than above the capitals.
            // Thus a centered box puts the text approximately one pixel too low. Move it up to
            // center the letters.
            let pos = (rect.center() - galley.size() / 2.0 - vec2(0.0, 1.0)).round();
            painter.galley(pos, galley, text_color);
        },
    )
}

/// A link over text that is already painted in `rect`, for example a name in the line under a
/// track. It is underlined on hover. `label` tells what it opens.
pub fn text_link(ui: &mut Ui, rect: Rect, id: Id, label: &str) -> Response {
    custom_button(ui, rect, id, label, true, |painter, rect, hover, _press| {
        if hover > 0.0 {
            let y = rect.bottom() - 1.0;
            painter.line_segment(
                [pos2(rect.left(), y), pos2(rect.right(), y)],
                Stroke::new(1.0, theme::TEXT_DIM.gamma_multiply(hover)),
            );
        }
    })
}

/// A button with only text, similar to a link ("Change Client ID", "Cancel"…).
pub fn text_button(ui: &mut Ui, id: Id, text: &str, font: FontId) -> Response {
    let galley = single_line(
        ui.painter(),
        text,
        font.clone(),
        theme::TEXT_DIM,
        f32::INFINITY,
    );
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(8.0, 6.0), Sense::hover());
    custom_button(ui, rect, id, text, true, |painter, rect, hover, _| {
        let color = lerp_color(theme::TEXT_FAINT, theme::TEXT, hover);
        let galley = single_line(painter, text, font, color, f32::INFINITY);
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    })
}

/// A rounded cover image, or a placeholder with a music note.
pub fn paint_cover(ui: &Ui, rect: Rect, cover: Option<&Cover>, radius: f32, alpha: f32) {
    paint_cover_over(ui, rect, cover, radius, alpha, |painter, rect, color| {
        icons::music_note(painter, rect.center(), rect.width() * 0.36, color);
    });
}

/// [`paint_cover`] for a playlist: its placeholder has the note of the logo.
pub fn paint_playlist_cover(ui: &Ui, rect: Rect, cover: Option<&Cover>, alpha: f32) {
    paint_cover_over(
        ui,
        rect,
        cover,
        theme::RADIUS_SMALL,
        alpha,
        |painter, rect, color| {
            icons::logo_note(painter, icons::glyph_bounds(rect), color);
        },
    );
}

/// The cover, over a placeholder (`glyph` on a plain tile) that stays under it while it fades in.
fn paint_cover_over(
    ui: &Ui,
    rect: Rect,
    cover: Option<&Cover>,
    radius: f32,
    alpha: f32,
    glyph: impl FnOnce(&egui::Painter, Rect, Color32),
) {
    if alpha <= 0.0 {
        return;
    }
    let painter = ui.painter();
    let fade_in = cover.map_or(0.0, |c| {
        let elapsed = anim::elapsed_since(ui.ctx(), c.loaded_at);
        anim::keep_animating(ui.ctx(), elapsed, 0.3);
        ease_out_cubic(elapsed / 0.3)
    });
    if fade_in < 1.0 {
        painter.rect_filled(rect, radius, theme::SURFACE_HOVER.gamma_multiply(alpha));
        glyph(painter, rect, theme::TEXT_FAINT.gamma_multiply(alpha));
    }
    if let Some(cover) = cover {
        egui::Image::from_texture(cover.texture)
            .corner_radius(radius)
            .tint(Color32::WHITE.gamma_multiply(alpha * fade_in))
            .paint_at(ui, rect);
    }
}

/// The look of a [`text_field`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FieldStyle {
    /// A boxed field, always visible (forms).
    Boxed,
    /// A thin boxed field that fits on a line of text (a name edited in a row). Its text starts
    /// `COMPACT_FIELD_PAD` from its left edge, and its right end leaves space for a small button.
    Compact,
    /// The search box: a faint pill with a magnifier. Its background becomes brighter while it has
    /// the focus.
    Search,
}

/// The space between the left edge of a [`FieldStyle::Compact`] field and its text, and the space
/// at its right end for a button.
pub const COMPACT_FIELD_PAD: f32 = 8.0;
pub const COMPACT_FIELD_BUTTON_ROOM: f32 = 26.0;

/// A text field drawn as a rounded surface. The border lights up when it has the focus. `label` is
/// its accessible name.
#[expect(clippy::too_many_arguments)]
pub fn text_field(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    label: &str,
    text: &mut String,
    hint: &str,
    style: FieldStyle,
    accent: Color32,
) -> Response {
    let ctx = ui.ctx().clone();
    let focused = ui.memory(|m| m.has_focus(id));
    let focus = anim::tween_bool(&ctx, id.with("focus"), focused, 0.2);
    let hovered = ui.rect_contains_pointer(rect) && !dnd::is_dragging(&ctx);
    let hover = anim::tween_bool(&ctx, id.with("hover"), hovered, 0.16);
    let painter = ui.painter();
    let text_left = match style {
        FieldStyle::Boxed => {
            painter.rect(
                rect,
                theme::RADIUS_MEDIUM,
                lerp_color(theme::SURFACE, theme::SURFACE_HOVER, focus * 0.6),
                Stroke::new(
                    1.0,
                    lerp_color(theme::BORDER, accent.gamma_multiply(0.8), focus),
                ),
                StrokeKind::Inside,
            );
            rect.left() + 14.0
        }
        FieldStyle::Compact => {
            painter.rect(
                rect,
                theme::RADIUS_SMALL,
                lerp_color(theme::SURFACE, theme::SURFACE_HOVER, focus * 0.6),
                Stroke::new(
                    1.0,
                    lerp_color(theme::BORDER, accent.gamma_multiply(0.8), focus),
                ),
                StrokeKind::Inside,
            );
            rect.left() + COMPACT_FIELD_PAD
        }
        FieldStyle::Search => {
            // A bit shorter at rest. It grows to its full size when it has the focus.
            let shape = rect.shrink2(vec2(0.0, lerp(4.0, 0.0, focus)));
            let radius = shape.height() / 2.0;
            // Translucent also with the focus, so that the tinted backdrop shows through. The focus
            // only makes it lighter.
            let rest = 5.0 + 5.0 * hover;
            painter.rect(
                shape,
                radius,
                Color32::from_white_alpha(lerp(rest, 20.0, focus) as u8),
                Stroke::new(
                    1.0,
                    Color32::from_white_alpha((6.0 * hover * (1.0 - focus)) as u8),
                ),
                StrokeKind::Inside,
            );
            let rest_icon = lerp_color(theme::TEXT_FAINT, theme::TEXT_DIM, hover * 0.5);
            icons::search(
                painter,
                pos2(rect.left() + 20.0, rect.center().y),
                lerp(15.0, 17.0, focus),
                lerp_color(rest_icon, theme::TEXT, focus),
            );
            rect.left() + 38.0
        }
    };
    let right_room = match style {
        FieldStyle::Compact => COMPACT_FIELD_BUTTON_ROOM,
        FieldStyle::Boxed | FieldStyle::Search => 38.0,
    };
    let edit_rect = Rect::from_min_max(
        pos2(text_left, rect.top()),
        pos2(rect.right() - right_room, rect.bottom()),
    );
    let (font, hint_color) = match style {
        FieldStyle::Boxed | FieldStyle::Compact => {
            (theme::regular(theme::FONT_BODY), theme::TEXT_FAINT)
        }
        FieldStyle::Search => (
            theme::regular(theme::FONT_BODY),
            lerp_color(
                theme::TEXT_FAINT.gamma_multiply(0.85),
                theme::TEXT_FAINT,
                hover.max(focus),
            ),
        ),
    };
    let edit = egui::TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .font(font)
        .text_color(theme::TEXT)
        .hint_text(egui::RichText::new(hint).color(hint_color))
        .align(Align2::LEFT_CENTER)
        .desired_width(edit_rect.width());
    let response = ui.put(edit_rect, edit);
    ui.ctx()
        .accesskit_node_builder(id, |node| node.set_label(label));
    response
}

/// The height of a row in the lists of tracks, releases and playlists, and the size of its cover,
/// `ROW_COVER_INSET` from its left edge. All rows have the size of the album rows.
pub const ROW_HEIGHT: f32 = 68.0;
pub const ROW_COVER: f32 = 52.0;
pub const ROW_COVER_INSET: f32 = 8.0;
/// The space between the cover of a row and its text.
pub const ROW_TEXT_GAP: f32 = 14.0;
/// The space from the edges of a cover with no image (a folder tile, a playlist placeholder) to
/// the area for its glyph. It is the same for all glyphs.
pub const ROW_GLYPH_PAD: f32 = 10.0;

/// The height of the title row of the lists («Playlists», «Queue», the results…).
pub const LIST_TITLE_HEIGHT: f32 = 40.0;

/// The title row of a list: `title` on the left, and `detail` (faint) that ends at `detail_right`.
/// `label` is what screen readers say for the title. Returns the position of the title.
pub fn list_title(
    ui: &Ui,
    rect: Rect,
    id: Id,
    title: &str,
    label: &str,
    detail: &str,
    detail_right: f32,
) -> Rect {
    let painter = ui.painter();
    let detail_width = if detail.is_empty() {
        0.0
    } else {
        let galley = single_line(
            painter,
            detail,
            theme::regular(theme::FONT_SMALL),
            theme::TEXT_FAINT,
            rect.width() / 2.0,
        );
        let size = galley.size();
        let pos = pos2(detail_right - size.x, rect.center().y - size.y / 2.0 - 1.0);
        painter.galley(pos, galley, theme::TEXT_FAINT);
        a11y_label(
            ui,
            Rect::from_min_size(pos, size),
            id.with("detail"),
            detail,
        );
        size.x + 12.0
    };
    let galley = single_line(
        painter,
        title,
        theme::strong(theme::FONT_TITLE),
        theme::TEXT,
        (detail_right - rect.left() - detail_width).max(40.0),
    );
    let pos = pos2(rect.left(), rect.center().y - galley.size().y / 2.0 - 2.0);
    let title_rect = Rect::from_min_size(pos, galley.size());
    full_text_tooltip(ui, title_rect, id, &galley, title);
    painter.galley(pos, galley, theme::TEXT);
    a11y_label(ui, title_rect, id, label);
    title_rect
}

/// The area for the rows of a list under its title row. It is a little wider than the title, so
/// the hover highlight of the rows goes past it.
pub fn list_rows(rect: Rect, title: Rect) -> Rect {
    Rect::from_min_max(
        pos2(rect.left() - 8.0, title.bottom() + 6.0),
        pos2(rect.right() + 8.0, rect.bottom()),
    )
}

/// The distance into the list where the scroll hints get full strength.
const SCROLL_HINT_RAMP: f32 = 32.0;
/// The height of the shade at an edge with more content behind it.
const SCROLL_HINT_SHADE: f32 = 28.0;

/// Shows that a scrolled list continues past its top or bottom edge: a fade into the background
/// along that edge. `insets` (top, bottom) are the parts of each edge that a pinned item uses.
/// That edge gets no hint, because the hint would cut the shadow of the pinned item in a straight
/// line.
pub fn scroll_hints<R>(
    ui: &Ui,
    output: &egui::scroll_area::ScrollAreaOutput<R>,
    insets: (f32, f32),
) {
    let view = output.inner_rect;
    let offset = output.state.clamped_offset().y;
    let max_offset = (output.content_size.y - view.height()).max(0.0);
    let above = if insets.0 > 0.0 {
        0.0
    } else {
        (offset / SCROLL_HINT_RAMP).clamp(0.0, 1.0)
    };
    let below = if insets.1 > 0.0 {
        0.0
    } else {
        ((max_offset - offset) / SCROLL_HINT_RAMP).clamp(0.0, 1.0)
    };
    if above <= 0.0 && below <= 0.0 {
        return;
    }
    let view = Rect::from_min_max(
        pos2(view.left(), view.top() + insets.0),
        pos2(view.right(), view.bottom() - insets.1),
    );
    if view.height() < SCROLL_HINT_SHADE * 2.0 {
        return;
    }
    let painter = ui.painter().with_clip_rect(view.intersect(ui.clip_rect()));
    if above > 0.0 {
        let shade = Rect::from_min_size(view.min, vec2(view.width(), SCROLL_HINT_SHADE));
        paint_scroll_hint(&painter, shade, true, above);
    }
    if below > 0.0 {
        let shade = Rect::from_min_size(
            pos2(view.left(), view.bottom() - SCROLL_HINT_SHADE),
            vec2(view.width(), SCROLL_HINT_SHADE),
        );
        paint_scroll_hint(&painter, shade, false, below);
    }
}

/// One edge of [`scroll_hints`]: a straight fade from the background at the edge to nothing,
/// farther into the list. The background has soft glows, so the function samples its color along
/// the edge.
fn paint_scroll_hint(painter: &Painter, shade: Rect, top: bool, alpha: f32) {
    const ROWS: usize = 8;
    const COLUMNS: usize = 16;
    let ctx = painter.ctx();
    let edge_y = if top { shade.top() } else { shade.bottom() };
    let inner_y = if top { shade.bottom() } else { shade.top() };
    let mut mesh = egui::Mesh::default();
    for column in 0..=COLUMNS {
        let x = lerp(shade.left(), shade.right(), column as f32 / COLUMNS as f32);
        let background = backdrop::color_at(ctx, pos2(x, edge_y));
        for row in 0..=ROWS {
            // t = 0 at the edge, 1 at the inner end of the shade.
            let t = row as f32 / ROWS as f32;
            let color = background.gamma_multiply(alpha * (1.0 - t) * (1.0 - t));
            mesh.colored_vertex(pos2(x, lerp(edge_y, inner_y, t)), color);
        }
    }
    let index = |column: usize, row: usize| (column * (ROWS + 1) + row) as u32;
    for column in 0..COLUMNS {
        for row in 0..ROWS {
            let (a, b) = (index(column, row), index(column, row + 1));
            let (c, d) = (index(column + 1, row), index(column + 1, row + 1));
            mesh.add_triangle(a, b, d);
            mesh.add_triangle(a, d, c);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Placeholder rows down from the top of `rect` (`max_rows` at most) while a list loads. They
/// pulse, so they ask for the next frame.
pub fn skeleton_rows(ui: &Ui, rect: Rect, time: f64, max_rows: usize) {
    let rows = ((rect.height() / ROW_HEIGHT).ceil() as usize).min(max_rows);
    for i in 0..rows {
        let row = Rect::from_min_size(
            pos2(rect.left(), rect.top() + i as f32 * ROW_HEIGHT),
            vec2(rect.width(), ROW_HEIGHT),
        );
        skeleton_row(ui.painter(), row, time, i);
    }
    anim::request_frame(ui.ctx());
}

/// Placeholder bars that show while results load.
fn skeleton_row(painter: &Painter, rect: Rect, time: f64, index: usize) {
    let pulse = ((time as f32 * 3.0 - index as f32 * 0.35).sin() * 0.5 + 0.5) * 0.6 + 0.4;
    let color = Color32::from_white_alpha((14.0 * pulse) as u8);
    let thumb = Rect::from_min_size(
        pos2(
            rect.left() + ROW_COVER_INSET,
            rect.center().y - ROW_COVER / 2.0,
        ),
        Vec2::splat(ROW_COVER),
    );
    painter.rect_filled(thumb, theme::RADIUS_SMALL, color);
    let text_left = thumb.right() + ROW_TEXT_GAP;
    let width = rect.right() - text_left - 60.0;
    let widths = [lerp(0.45, 0.7, ((index * 37) % 10) as f32 / 10.0), 0.35];
    for (line, fraction) in widths.into_iter().enumerate() {
        let top = rect.center().y - 14.0 + line as f32 * 18.0;
        let bar = Rect::from_min_size(pos2(text_left, top), vec2(width * fraction, 10.0));
        painter.rect_filled(bar, 5.0, color);
    }
}

/// Centered icon, title and subtitle for empty states. Returns the bottom of the subtitle, to put
/// something under it.
pub fn empty_state(
    ui: &Ui,
    rect: Rect,
    id: Id,
    title: &str,
    subtitle: &str,
    alpha: f32,
    paint_icon: impl FnOnce(&Painter, Pos2, Color32),
) -> f32 {
    let elapsed = anim::since_change(ui.ctx(), id, (title, subtitle), true);
    anim::keep_animating(ui.ctx(), elapsed, 0.4);
    let t = ease_out_cubic(elapsed / 0.4) * alpha;
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(t);
    let center = rect.center() - vec2(0.0, 40.0 - (1.0 - t) * 8.0);
    painter.circle_filled(center, 30.0, Color32::from_white_alpha(10));
    paint_icon(&painter, center, theme::TEXT_DIM);
    let title_galley = single_line(
        &painter,
        title,
        theme::strong(theme::FONT_TITLE),
        theme::TEXT,
        rect.width() - 32.0,
    );
    let title_rect = paint_galley(
        &painter,
        title_galley,
        center.x,
        center.y + 46.0,
        Align::Center,
        1.0,
    );
    let mut job = LayoutJob::single_section(
        subtitle.to_owned(),
        TextFormat {
            font_id: theme::regular(theme::FONT_BODY),
            color: theme::TEXT_FAINT,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::wrap_at_width((rect.width() - 64.0).clamp(80.0, 340.0));
    job.halign = Align::Center;
    let galley = painter.layout_job(job);
    let bottom = title_rect.bottom() + 6.0 + galley.size().y;
    painter.galley(
        pos2(center.x, title_rect.bottom() + 6.0),
        galley,
        theme::TEXT_FAINT,
    );
    a11y_label(ui, title_rect, id.with("title"), title);
    bottom
}
