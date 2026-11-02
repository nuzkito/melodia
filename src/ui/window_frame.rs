//! The window has no system frame. The app draws its own title bar area (drag to move, double
//! click to maximize), the minimize / maximize / close buttons and the resize borders.

use egui::{
    Color32, CursorIcon, Id, Painter, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Ui,
    ViewportCommand, pos2, vec2,
};

use super::anim::lerp_color;
use super::theme;
use super::widgets::custom_button;

pub const BUTTON_WIDTH: f32 = 46.0;
pub const BUTTON_HEIGHT: f32 = 34.0;
/// The width of the three buttons in the top right corner.
pub const CONTROLS_WIDTH: f32 = BUTTON_WIDTH * 3.0;
/// The height of the band at the top that moves the window with a drag.
pub const DRAG_HEIGHT: f32 = 40.0;
const RESIZE_EDGE: f32 = 5.0;
const RESIZE_CORNER: f32 = 8.0;
const CLOSE_HOVER: Color32 = Color32::from_rgb(232, 17, 35);

fn is_maximized(ui: &Ui) -> bool {
    ui.input(|i| {
        let viewport = i.viewport();
        viewport.maximized.unwrap_or(false) || viewport.fullscreen.unwrap_or(false)
    })
}

/// The top band of the window: a drag moves the window, and a double click maximizes or restores
/// it. Call it before you draw the content, so that the widgets on top of it keep their clicks.
pub fn drag_area(ui: &mut Ui, rect: Rect) {
    let band = Rect::from_min_size(rect.min, vec2(rect.width(), DRAG_HEIGHT));
    // Without `Sense::FOCUSABLE`: the band, as the resize edges, is only for the pointer, not a
    // Tab stop.
    let response = ui.interact(band, Id::unique("window-drag"), Sense::CLICK | Sense::DRAG);
    if response.double_clicked() {
        let maximized = is_maximized(ui);
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if response.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Lets the user move the window while a modal dialog covers it. The backdrop of the dialog is
/// over [`drag_area`], so a drag of the top band of the backdrop moves the window.
pub fn drag_through_backdrop(ctx: &egui::Context, backdrop: &egui::Response) {
    if !backdrop.drag_started_by(egui::PointerButton::Primary) {
        return;
    }
    let in_band = backdrop.interact_pointer_pos().is_some_and(|pos| {
        let top = ctx.content_rect().top();
        (top..top + DRAG_HEIGHT).contains(&pos.y)
    });
    if in_band {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Minimize, maximize (or restore) and close, aligned with the top right corner of `rect`.
pub fn controls(ui: &mut Ui, rect: Rect) {
    let maximized = is_maximized(ui);
    let top_right = rect.right_top();
    let button = |i: usize| {
        Rect::from_min_size(
            pos2(top_right.x - BUTTON_WIDTH * (3 - i) as f32, top_right.y),
            vec2(BUTTON_WIDTH, BUTTON_HEIGHT),
        )
    };

    let minimize = window_button(ui, button(0), "Minimize", false, |p, c, color| {
        p.line_segment(
            [c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)],
            Stroke::new(1.0, color),
        );
    });
    if minimize.clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }

    let label = if maximized { "Restore" } else { "Maximize" };
    let maximize = window_button(ui, button(1), label, false, |p, c, color| {
        let stroke = Stroke::new(1.0, color);
        if maximized {
            let front = Rect::from_center_size(c + vec2(-1.0, 1.0), vec2(8.0, 8.0));
            p.rect_stroke(front, 1.0, stroke, StrokeKind::Middle);
            // Only the visible part of the window behind.
            let (l, t, r, b) = (
                front.left() + 2.0,
                front.top() - 2.0,
                front.right() + 2.0,
                front.bottom() - 2.0,
            );
            p.line_segment([pos2(l, front.top()), pos2(l, t)], stroke);
            p.line_segment([pos2(l, t), pos2(r, t)], stroke);
            p.line_segment([pos2(r, t), pos2(r, b)], stroke);
            p.line_segment([pos2(r, b), pos2(front.right(), b)], stroke);
        } else {
            let square = Rect::from_center_size(c, vec2(10.0, 10.0));
            p.rect_stroke(square, 1.0, stroke, StrokeKind::Middle);
        }
    });
    if maximize.clicked() {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }

    let close = window_button(ui, button(2), "Close window", true, |p, c, color| {
        let stroke = Stroke::new(1.0, color);
        let d = 5.0;
        p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
        p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
    });
    if close.clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
}

/// A button in the style of the window buttons.
pub(super) fn window_button(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    danger: bool,
    paint_icon: impl FnOnce(&Painter, Pos2, Color32),
) -> egui::Response {
    window_button_lit(ui, rect, label, danger, 0.0, paint_icon)
}

/// [`window_button`] kept lit by `highlight` (0..1), for example while its menu is open.
pub(super) fn window_button_lit(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    danger: bool,
    highlight: f32,
    paint_icon: impl FnOnce(&Painter, Pos2, Color32),
) -> egui::Response {
    let id = Id::unique("window-control").with(if danger { "close" } else { label });
    let response = custom_button(ui, rect, id, label, true, |painter, rect, hover, press| {
        let hover = hover.max(highlight);
        let fill = if danger {
            CLOSE_HOVER.gamma_multiply(hover * (1.0 - 0.25 * press))
        } else {
            Color32::from_white_alpha((14.0 * hover + 10.0 * press) as u8)
        };
        painter.rect_filled(rect, 0.0, fill);
        let color = if danger {
            lerp_color(theme::TEXT_DIM, Color32::WHITE, hover)
        } else {
            lerp_color(theme::TEXT_DIM, theme::TEXT, hover)
        };
        paint_icon(painter, rect.center(), color);
    });
    // Window buttons use the normal arrow, as the system buttons do.
    response.on_hover_cursor(CursorIcon::Default)
}

/// Invisible borders along the edges of `rect` that resize the window. Call it after you draw the
/// content, so that the borders are above it.
pub fn resize_borders(ui: &mut Ui, rect: Rect) {
    if is_maximized(ui) {
        return;
    }
    let (e, c) = (RESIZE_EDGE, RESIZE_CORNER);
    let zones = [
        (
            Rect::from_min_max(
                pos2(rect.left() + c, rect.top()),
                pos2(rect.right() - c, rect.top() + e),
            ),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(
                pos2(rect.left() + c, rect.bottom() - e),
                pos2(rect.right() - c, rect.bottom()),
            ),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(
                pos2(rect.left(), rect.top() + c),
                pos2(rect.left() + e, rect.bottom() - c),
            ),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(
                pos2(rect.right() - e, rect.top() + c),
                pos2(rect.right(), rect.bottom() - c),
            ),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
        (
            Rect::from_min_size(rect.left_top(), vec2(c, c)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        (
            Rect::from_min_size(rect.right_top() - vec2(c, 0.0), vec2(c, c)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            Rect::from_min_size(rect.left_bottom() - vec2(0.0, c), vec2(c, c)),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (
            Rect::from_min_size(rect.right_bottom() - vec2(c, c), vec2(c, c)),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
    ];
    for (i, (zone, direction, cursor)) in zones.into_iter().enumerate() {
        let response = ui
            .interact(zone, Id::unique("window-resize").with(i), Sense::DRAG)
            .on_hover_cursor(cursor);
        if response.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

/// A thin line around the window. The window has no system frame to separate it from the items
/// behind it.
pub fn paint_border(ui: &Ui, rect: Rect) {
    if !is_maximized(ui) {
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, theme::BORDER),
            StrokeKind::Inside,
        );
    }
}
