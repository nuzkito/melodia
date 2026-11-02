//! The guide in the middle column while nothing is open there: the keyboard shortcuts and a hint
//! about the drag of tracks. Keep it the same as `MusicApp::shortcuts` and the README table.

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Color32, Id, Painter, Rect, Stroke, StrokeKind, TextFormat, Ui, pos2, vec2};

use super::theme;
use super::widgets::{a11y_label, paint_galley, single_line};

/// A piece of the keys column: a key cap or the text between keys.
enum Part {
    Key(&'static str),
    Text(&'static str),
}
use Part::{Key, Text};

const SHORTCUTS: &[(&[Part], &str)] = &[
    (&[Key("Space")], "Play or pause"),
    (
        &[Key("Ctrl"), Text("+"), Key("Shift"), Text("+"), Key("→")],
        "Next song",
    ),
    (
        &[Key("Ctrl"), Text("+"), Key("Shift"), Text("+"), Key("←")],
        "Previous song",
    ),
    (
        &[Key("Ctrl"), Text("+"), Key("←"), Text("/"), Key("→")],
        "Back / forward",
    ),
    (
        &[Key("Ctrl"), Text("+"), Key("↑"), Text("/"), Key("↓")],
        "Volume up / down",
    ),
    (
        &[Key("Ctrl"), Text("+"), Key("Wheel")],
        "Volume, over the slider",
    ),
    (
        &[Key("Ctrl"), Text("+"), Key("F"), Text("or"), Key("/")],
        "Go to search",
    ),
    (
        &[Key("↑"), Text("/"), Key("↓"), Text("+"), Key("Enter")],
        "Play a search result",
    ),
    (&[Key("Esc")], "Cancel search or drag"),
];

const SHORTCUTS_TITLE: &str = "Keyboard shortcuts";
const DRAG_TITLE: &str = "Drag and drop";
const DRAG_HINT: &str = "Drag a song from the search results or a playlist to the queue to \
    put it exactly where you want, or drop it on a playlist to add it.";

const MAX_WIDTH: f32 = 380.0;
const TITLE_HEIGHT: f32 = 30.0;
const ROW_HEIGHT: f32 = 34.0;
const KEY_HEIGHT: f32 = 24.0;
/// The maximum width of a key cap or of a text between keys.
const MAX_PART_WIDTH: f32 = 150.0;
/// The space between a part and the next part, and between the keys and the action.
const PART_GAP: f32 = 5.0;
const KEYS_TO_ACTION: f32 = 14.0;
const SECTION_GAP: f32 = 28.0;

/// Paints the guide centered in `rect`, faded by `alpha` (0..1).
pub fn show(ui: &Ui, rect: Rect, alpha: f32) {
    if alpha <= 0.01 {
        return;
    }
    let mut painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.multiply_opacity(alpha);
    let width = rect.width().min(MAX_WIDTH);
    let left = rect.center().x - width / 2.0;

    let hint = hint_galley(&painter, width);
    let height =
        2.0 * TITLE_HEIGHT + SHORTCUTS.len() as f32 * ROW_HEIGHT + SECTION_GAP + hint.size().y;
    // A little above the middle, which looks centered. Never above the top.
    let mut y = rect.top() + ((rect.height() - height) * 0.42).max(0.0);

    title(ui, &painter, left, y, SHORTCUTS_TITLE);
    y += TITLE_HEIGHT;
    // The actions align after the longest shortcut.
    let keys_width = SHORTCUTS
        .iter()
        .map(|(keys, _)| keys_width(&painter, keys))
        .fold(0.0, f32::max)
        + KEYS_TO_ACTION;
    for (i, (keys, action)) in SHORTCUTS.iter().enumerate() {
        let row = Rect::from_min_size(pos2(left, y), vec2(width, ROW_HEIGHT));
        paint_keys(&painter, row, keys);
        let galley = single_line(
            &painter,
            action,
            theme::regular(theme::FONT_BODY),
            theme::TEXT_DIM,
            width - keys_width,
        );
        let top = row.center().y - galley.size().y / 2.0;
        paint_galley(&painter, galley, left + keys_width, top, Align::Min, 1.0);
        a11y_label(ui, row, Id::unique(("guide-shortcut", i)), action);
        y += ROW_HEIGHT;
    }

    y += SECTION_GAP;
    title(ui, &painter, left, y, DRAG_TITLE);
    y += TITLE_HEIGHT;
    let hint_rect = Rect::from_min_size(pos2(left, y), hint.size());
    painter.galley(hint_rect.min, hint, theme::TEXT_FAINT);
    a11y_label(ui, hint_rect, Id::unique("guide-drag-hint"), DRAG_HINT);
}

fn title(ui: &Ui, painter: &Painter, left: f32, top: f32, text: &str) {
    let galley = single_line(
        painter,
        text,
        theme::strong(theme::FONT_TITLE),
        theme::TEXT,
        MAX_WIDTH,
    );
    let rect = paint_galley(painter, galley, left, top, Align::Min, 1.0);
    a11y_label(ui, rect, Id::unique(("guide-title", text)), text);
}

fn hint_galley(painter: &Painter, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(
        DRAG_HINT.to_owned(),
        TextFormat {
            font_id: theme::regular(theme::FONT_BODY),
            color: theme::TEXT_FAINT,
            line_height: Some(20.0),
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::wrap_at_width(width.max(80.0));
    painter.layout_job(job)
}

fn key_galley(painter: &Painter, label: &str) -> std::sync::Arc<egui::Galley> {
    single_line(
        painter,
        label,
        theme::strong(theme::FONT_SMALL),
        theme::TEXT,
        MAX_PART_WIDTH,
    )
}

fn text_galley(painter: &Painter, text: &str) -> std::sync::Arc<egui::Galley> {
    single_line(
        painter,
        text,
        theme::regular(theme::FONT_SMALL),
        theme::TEXT_FAINT,
        MAX_PART_WIDTH,
    )
}

fn cap_width(galley: &egui::Galley) -> f32 {
    (galley.size().x + 16.0).max(KEY_HEIGHT)
}

/// The width of the key caps of one shortcut, with the texts between them.
fn keys_width(painter: &Painter, parts: &[Part]) -> f32 {
    let width: f32 = parts
        .iter()
        .map(|part| match part {
            Key(label) => cap_width(&key_galley(painter, label)),
            Text(text) => text_galley(painter, text).size().x,
        })
        .map(|width| width + PART_GAP)
        .sum();
    (width - PART_GAP).max(0.0)
}

/// The key caps of one shortcut, from left to right, with their vertical center in `row`.
fn paint_keys(painter: &Painter, row: Rect, parts: &[Part]) {
    let mut x = row.left();
    for part in parts {
        match part {
            Key(label) => {
                let galley = key_galley(painter, label);
                let cap_width = cap_width(&galley);
                let cap = Rect::from_min_size(
                    pos2(x, row.center().y - KEY_HEIGHT / 2.0),
                    vec2(cap_width, KEY_HEIGHT),
                );
                painter.rect(
                    cap,
                    theme::RADIUS_SMALL,
                    Color32::from_white_alpha(10),
                    Stroke::new(1.0, theme::BORDER),
                    StrokeKind::Inside,
                );
                // A brighter bottom edge, as on a physical key.
                painter.hline(
                    cap.x_range().shrink(theme::RADIUS_SMALL),
                    cap.bottom() - 0.5,
                    Stroke::new(1.0, Color32::from_white_alpha(28)),
                );
                painter.galley(cap.center() - galley.size() / 2.0, galley, theme::TEXT);
                x = cap.right() + PART_GAP;
            }
            Text(text) => {
                let galley = text_galley(painter, text);
                let top = row.center().y - galley.size().y / 2.0;
                painter.galley(pos2(x, top), galley.clone(), theme::TEXT_FAINT);
                x += galley.size().x + PART_GAP;
            }
        }
    }
}
