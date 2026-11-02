//! Vector icons drawn with the painter: sharp at all scales and easy to animate. Coordinates are
//! relative to the icon size, with the center at `center`.

use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{
    Color32, ColorImage, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureFilter, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};

use super::anim::{ease_in_quad, ease_out_cubic, lerp, lerp_color};
use super::theme;
use super::widgets::{ROW_COVER, ROW_GLYPH_PAD};

fn at(center: Pos2, size: f32, (x, y): (f32, f32)) -> Pos2 {
    center + vec2(x, y) * size
}

/// Play (`play_t = 1`) that changes into pause (`play_t = 0`).
///
/// The two shapes are two quads: the pause bars and the two halves of the play triangle. Thus an
/// interpolation of their corners changes one shape into the other.
pub fn play_pause(painter: &Painter, center: Pos2, size: f32, play_t: f32, color: Color32) {
    const PAUSE: [[(f32, f32); 4]; 2] = [
        [(-0.30, -0.36), (-0.08, -0.36), (-0.08, 0.36), (-0.30, 0.36)],
        [(0.08, -0.36), (0.30, -0.36), (0.30, 0.36), (0.08, 0.36)],
    ];
    // Moved right, so that the triangle looks centered. The tip is two points very near each
    // other, to prevent an edge with zero length. The first half goes a little under the second
    // half (to x = 0.13, on the same edges). With the two anti-aliased edges at exactly 0.11, a
    // faint line showed there.
    const PLAY: [[(f32, f32); 4]; 2] = [
        [(-0.25, -0.40), (0.13, -0.189), (0.13, 0.189), (-0.25, 0.40)],
        [(0.11, -0.20), (0.46, -0.004), (0.46, 0.004), (0.11, 0.20)],
    ];
    let t = play_t.clamp(0.0, 1.0);
    for (pause, play) in PAUSE.iter().zip(PLAY.iter()) {
        let points = pause
            .iter()
            .zip(play.iter())
            .map(|(a, b)| at(center, size, (lerp(a.0, b.0, t), lerp(a.1, b.1, t))))
            .collect();
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
    }
}

/// Skip next (`forward`) or previous: a triangle and a bar.
pub fn skip(painter: &Painter, center: Pos2, size: f32, forward: bool, color: Color32) {
    let dir = if forward { 1.0 } else { -1.0 };
    let mut triangle: Vec<Pos2> = [(-0.30, -0.32), (0.17, 0.0), (-0.30, 0.32)]
        .iter()
        .map(|&(x, y)| at(center, size, (x * dir, y)))
        .collect();
    if !forward {
        triangle.reverse(); // keep the winding for anti-aliasing
    }
    painter.add(Shape::convex_polygon(triangle, color, Stroke::NONE));
    let bar = Rect::from_min_max(
        at(center, size, (if forward { 0.19 } else { -0.31 }, -0.32)),
        at(center, size, (if forward { 0.31 } else { -0.19 }, 0.32)),
    );
    painter.rect_filled(bar, size * 0.04, color);
}

pub fn search(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new(size * 0.11, color);
    painter.circle_stroke(at(center, size, (-0.07, -0.07)), size * 0.29, stroke);
    painter.line_segment(
        [
            at(center, size, (0.15, 0.15)),
            at(center, size, (0.38, 0.38)),
        ],
        stroke,
    );
}

pub fn close(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new(size * 0.11, color);
    let d = 0.28;
    painter.line_segment(
        [at(center, size, (-d, -d)), at(center, size, (d, d))],
        stroke,
    );
    painter.line_segment(
        [at(center, size, (-d, d)), at(center, size, (d, -d))],
        stroke,
    );
}

/// An arrow that points down (or up).
pub fn arrow(painter: &Painter, center: Pos2, size: f32, down: bool, color: Color32) {
    let stroke = Stroke::new(size * 0.11, color);
    let s = if down { 1.0 } else { -1.0 };
    let tip = at(center, size, (0.0, 0.3 * s));
    painter.line_segment([at(center, size, (0.0, -0.3 * s)), tip], stroke);
    painter.add(Shape::line(
        vec![
            at(center, size, (-0.24, 0.06 * s)),
            tip,
            at(center, size, (0.24, 0.06 * s)),
        ],
        stroke,
    ));
}

/// A chevron that points right (or left), for «Forward» and «Back».
pub fn chevron(painter: &Painter, center: Pos2, size: f32, forward: bool, color: Color32) {
    let stroke = Stroke::new(size * 0.12, color);
    let s = if forward { 1.0 } else { -1.0 };
    painter.add(Shape::line(
        vec![
            at(center, size, (-0.12 * s, -0.3)),
            at(center, size, (0.16 * s, 0.0)),
            at(center, size, (-0.12 * s, 0.3)),
        ],
        stroke,
    ));
}

/// A folder, filled: its back with the tab, and the front flap. The flap tilts open as `open`
/// goes from 0 to 1, and shows a sheet in the folder.
pub fn folder(painter: &Painter, center: Pos2, size: f32, open: f32, color: Color32) {
    let open = open.clamp(0.0, 1.0);
    let corner = size * 0.06;
    let back = color.gamma_multiply(0.72);
    let tab = Rect::from_min_max(
        at(center, size, (-0.42, -0.36)),
        at(center, size, (-0.04, -0.16)),
    );
    painter.rect_filled(tab, corner, back);
    let body = Rect::from_min_max(
        at(center, size, (-0.42, -0.26)),
        at(center, size, (0.42, 0.32)),
    );
    painter.rect_filled(body, corner, back);
    if open > 0.0 {
        // A sheet comes up out of it.
        let rise = 0.08 * open;
        let sheet = Rect::from_min_max(
            at(center, size, (-0.32, -0.22 - rise)),
            at(center, size, (0.32, 0.2)),
        );
        let paper = lerp_color(color, Color32::WHITE, 0.75).gamma_multiply(open);
        painter.rect_filled(sheet, corner, paper);
    }
    // The flap: its top edge goes down and leans forward.
    let top = lerp(-0.14, 0.0, open);
    let lean = 0.1 * open;
    let flap = vec![
        at(center, size, (-0.42 + lean, top)),
        at(center, size, (0.42 + lean, top)),
        at(center, size, (0.42, 0.32)),
        at(center, size, (-0.42, 0.32)),
    ];
    painter.add(Shape::convex_polygon(flap, color, Stroke::NONE));
}

/// The cover of a folder in the rows: a rounded tile tinted with `color`, with the folder on it
/// (`open` as in [`folder`]). It fits in [`glyph_bounds`], at `alpha`.
pub fn folder_tile(painter: &Painter, rect: Rect, color: Color32, open: f32, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let tile = lerp_color(theme::SURFACE_HOVER, color, 0.3).gamma_multiply(alpha);
    painter.rect_filled(rect, theme::RADIUS_SMALL, tile);
    let glyph = lerp_color(color, Color32::WHITE, 0.15).gamma_multiply(alpha);
    // The closed folder goes from x -0.42 to 0.42 and from y -0.36 to 0.32 of its size.
    let bounds = glyph_bounds(rect);
    let size = (bounds.width() / 0.84).min(bounds.height() / 0.68);
    let center = bounds.center() + vec2(0.0, 0.02 * size);
    folder(painter, center, size, open, glyph);
}

/// The area for the glyph of a cover with no image (a folder, or the note of a playlist): `cover`
/// less [`ROW_GLYPH_PAD`] on all sides, scaled with the cover (it becomes smaller while it flies
/// into a folder). The glyph fits in this area, centered, and keeps its proportions.
pub fn glyph_bounds(cover: Rect) -> Rect {
    let pad = ROW_GLYPH_PAD * cover.width() / ROW_COVER;
    cover.shrink(pad)
}

/// The outline of a folder with a plus inside: «New folder».
pub fn folder_plus(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new(size * 0.09, color);
    // Sufficiently tall for the plus to keep some space above and below.
    let outline = [
        (-0.44, -0.38),
        (-0.12, -0.38),
        (-0.02, -0.27),
        (0.44, -0.27),
        (0.44, 0.38),
        (-0.44, 0.38),
    ]
    .map(|p| at(center, size, p));
    painter.add(Shape::closed_line(outline.to_vec(), stroke));
    // At this small size, a stroked plus falls between pixels and is not symmetrical. Thus its bars
    // are filled on full pixels, the same on the two sides.
    let ppp = painter.pixels_per_point();
    let width = (size * 0.09 * ppp).round().max(1.0);
    let mut length = (size * 0.36 * ppp).round();
    if (length - width) % 2.0 != 0.0 {
        length += 1.0;
    }
    let snap = |v: f32| {
        if width % 2.0 == 0.0 {
            v.round()
        } else {
            v.floor() + 0.5
        }
    };
    let plus = at(center, size, (0.0, 0.055));
    let (x, y) = (snap(plus.x * ppp), snap(plus.y * ppp));
    for (w, h) in [(length, width), (width, length)] {
        let bar = Rect::from_center_size(pos2(x, y), vec2(w, h));
        painter.rect_filled(Rect::from_min_max(bar.min / ppp, bar.max / ppp), 0.0, color);
    }
}

/// A plus sign that changes into a check mark as `check_t` goes from 0 to 1.
pub fn plus_to_check(painter: &Painter, center: Pos2, size: f32, check_t: f32, color: Color32) {
    let t = check_t.clamp(0.0, 1.0);
    let stroke_width = size * 0.11;
    if t < 1.0 {
        let angle = t * FRAC_PI_2;
        let (sin, cos) = angle.sin_cos();
        let arm = size * 0.32 * (1.0 - t);
        let stroke = Stroke::new(stroke_width, color.gamma_multiply(1.0 - t));
        for (dx, dy) in [(cos, sin), (-sin, cos)] {
            let d = vec2(dx, dy) * arm;
            painter.line_segment([center - d, center + d], stroke);
        }
    }
    if t > 0.0 {
        // Draw the check gradually along its two segments.
        let points = [(-0.30, 0.02), (-0.08, 0.24), (0.32, -0.22)].map(|p| at(center, size, p));
        let first = (t * 2.0).min(1.0);
        let second = (t * 2.0 - 1.0).clamp(0.0, 1.0);
        let stroke = Stroke::new(stroke_width, color);
        painter.line_segment([points[0], points[0].lerp(points[1], first)], stroke);
        if second > 0.0 {
            painter.line_segment([points[1], points[1].lerp(points[2], second)], stroke);
        }
    }
}

pub fn music_note(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    painter.circle_filled(at(center, size, (-0.14, 0.24)), size * 0.15, color);
    let stem = Rect::from_min_max(
        at(center, size, (-0.03, -0.36)),
        at(center, size, (0.04, 0.26)),
    );
    painter.rect_filled(stem, 0.0, color);
    let flag = vec![
        at(center, size, (0.04, -0.36)),
        at(center, size, (0.30, -0.24)),
        at(center, size, (0.30, -0.10)),
        at(center, size, (0.04, -0.20)),
    ];
    painter.add(Shape::convex_polygon(flag, color, Stroke::NONE));
}

/// The mask of the logo's note, cropped to the note.
#[derive(Clone)]
struct NoteMask {
    texture: TextureHandle,
    /// The part of the texture that the note uses (0..1).
    uv: Rect,
}

/// The eighth note of the logo in `color`, at the largest size that fits in `bounds`, centered.
/// The function renders its mask one time, the first time that it is necessary.
pub fn logo_note(painter: &Painter, bounds: Rect, color: Color32) {
    /// The pixels of the full icon for the mask. This is sufficient for the largest cover.
    const MASK_PX: usize = 96;
    let ctx = painter.ctx();
    let id = egui::Id::unique("logo-note-mask");
    let mask = ctx.data(|d| d.get_temp::<NoteMask>(id)).unwrap_or_else(|| {
        let rgba = crate::icon::render_note(MASK_PX as u32);
        // The bounding box of the note, one pixel wider for its soft edge.
        let (mut min, mut max) = ([MASK_PX; 2], [0; 2]);
        for (i, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] > 0 {
                let (x, y) = (i % MASK_PX, i / MASK_PX);
                min = [min[0].min(x), min[1].min(y)];
                max = [max[0].max(x + 1), max[1].max(y + 1)];
            }
        }
        let image = ColorImage::from_rgba_unmultiplied([MASK_PX, MASK_PX], &rgba);
        // Mipmaps: on the cover of a row, the note is drawn at a fraction of its size.
        let options = TextureOptions {
            mipmap_mode: Some(TextureFilter::Linear),
            ..TextureOptions::LINEAR
        };
        let px = MASK_PX as f32;
        let mask = NoteMask {
            texture: ctx.load_texture("logo-note-mask", image, options),
            uv: Rect::from_min_max(
                pos2(min[0] as f32 / px, min[1] as f32 / px),
                pos2(max[0] as f32 / px, max[1] as f32 / px),
            ),
        };
        ctx.data_mut(|d| d.insert_temp(id, mask.clone()));
        mask
    });
    let note = mask.uv.size();
    let scale = (bounds.width() / note.x).min(bounds.height() / note.y);
    painter.image(
        mask.texture.id(),
        Rect::from_center_size(bounds.center(), note * scale),
        mask.uv,
        color,
    );
}

/// The device icon for the Spotify device type.
pub fn device(painter: &Painter, center: Pos2, size: f32, kind: &str, color: Color32) {
    let stroke = Stroke::new(size * 0.09, color);
    match kind.to_ascii_lowercase().as_str() {
        "computer" => {
            let screen = Rect::from_min_max(
                at(center, size, (-0.40, -0.30)),
                at(center, size, (0.40, 0.18)),
            );
            painter.rect_stroke(screen, size * 0.06, stroke, egui::StrokeKind::Middle);
            painter.line_segment(
                [
                    at(center, size, (-0.20, 0.36)),
                    at(center, size, (0.20, 0.36)),
                ],
                stroke,
            );
            painter.line_segment(
                [at(center, size, (0.0, 0.18)), at(center, size, (0.0, 0.36))],
                stroke,
            );
        }
        "smartphone" | "tablet" => {
            let body = Rect::from_min_max(
                at(center, size, (-0.22, -0.40)),
                at(center, size, (0.22, 0.40)),
            );
            painter.rect_stroke(body, size * 0.08, stroke, egui::StrokeKind::Middle);
            painter.circle_filled(at(center, size, (0.0, 0.27)), size * 0.04, color);
        }
        _ => speaker(painter, center, size, color),
    }
}

pub fn speaker(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let body = vec![
        at(center, size, (-0.38, -0.12)),
        at(center, size, (-0.20, -0.12)),
        at(center, size, (0.02, -0.34)),
        at(center, size, (0.02, 0.34)),
        at(center, size, (-0.20, 0.12)),
        at(center, size, (-0.38, 0.12)),
    ];
    painter.add(Shape::convex_polygon(body, color, Stroke::NONE));
    let stroke = Stroke::new(size * 0.08, color);
    for radius in [0.16, 0.30] {
        let points = (0..=8)
            .map(|i| {
                let angle = -0.9 + 1.8 * i as f32 / 8.0;
                at(
                    center,
                    size,
                    (0.06 + radius * angle.cos(), radius * angle.sin()),
                )
            })
            .collect();
        painter.add(Shape::line(points, stroke));
    }
}

/// A speaker with waves that follow `level` (0..1). Near 0, a cross appears.
pub fn volume(painter: &Painter, center: Pos2, size: f32, level: f32, color: Color32) {
    let body = vec![
        at(center, size, (-0.42, -0.13)),
        at(center, size, (-0.24, -0.13)),
        at(center, size, (-0.02, -0.34)),
        at(center, size, (-0.02, 0.34)),
        at(center, size, (-0.24, 0.13)),
        at(center, size, (-0.42, 0.13)),
    ];
    painter.add(Shape::convex_polygon(body, color, Stroke::NONE));
    let level = level.clamp(0.0, 1.0);
    let width = size * 0.08;
    for (i, radius) in [0.17_f32, 0.32].into_iter().enumerate() {
        // The first wave fades in just above 0, the second wave after approximately half.
        let alpha = ((level - 0.02 - i as f32 * 0.45) / 0.15).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            continue;
        }
        let points = (0..=8)
            .map(|k| {
                let angle = -0.85 + 1.7 * k as f32 / 8.0;
                at(
                    center,
                    size,
                    (0.04 + radius * angle.cos(), radius * angle.sin()),
                )
            })
            .collect();
        painter.add(Shape::line(
            points,
            Stroke::new(width, color.gamma_multiply(alpha)),
        ));
    }
    let muted = (1.0 - level / 0.03).clamp(0.0, 1.0);
    if muted > 0.0 {
        let stroke = Stroke::new(width, color.gamma_multiply(muted));
        let c = at(center, size, (0.25, 0.0));
        let d = size * 0.12;
        painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
        painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
    }
}

/// A pencil that points down and left ("edit").
pub fn pencil(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    // Along the pencil (towards the top right) and across it.
    let along = vec2(1.0, -1.0) / std::f32::consts::SQRT_2;
    let across = vec2(1.0, 1.0) / std::f32::consts::SQRT_2;
    let point = |t: f32, s: f32| center + (along * (t + 0.01) + across * s) * size;
    let stroke = Stroke::new(size * 0.09, color);
    let (half, tip, top) = (0.11, -0.16, 0.40);
    painter.add(Shape::closed_line(
        vec![
            point(-0.42, 0.0),
            point(tip, -half),
            point(top, -half),
            point(top, half),
            point(tip, half),
        ],
        stroke,
    ));
    painter.line_segment([point(tip, -half), point(tip, half)], stroke);
    painter.line_segment([point(0.26, -half), point(0.26, half)], stroke);
}

const HEART_ARC_POINTS: usize = 20;
/// The thickness of the heart's outline (times its size).
const HEART_STROKE: f32 = 0.09;

/// The outline of the heart, clockwise from the notch between the lobes to the tip and back. The
/// function also returns the length of the first half.
fn heart_outline(center: Pos2, size: f32) -> (Vec<Pos2>, usize) {
    // Two round lobes (circles that overlap at the notch) and two straight sides, tangent to them,
    // that meet at a sharp tip. The size is 0.8 × 0.72 of `size`.
    const RADIUS: f32 = 0.21;
    const LOBE: (f32, f32) = (0.19, -0.14); // center of the right lobe
    const TIP: (f32, f32) = (0.0, 0.37);
    let (dx, dy) = (TIP.0 - LOBE.0, TIP.1 - LOBE.1);
    // The point where the right side leaves the lobe, and the point where the lobes cross.
    let tangent = dy.atan2(dx) - (RADIUS / dx.hypot(dy)).acos();
    let notch = (-(RADIUS * RADIUS - LOBE.0 * LOBE.0).sqrt()).atan2(-LOBE.0);
    // The right half, from the notch over the lobe to the tip.
    let mut right: Vec<(f32, f32)> = (0..=HEART_ARC_POINTS)
        .map(|i| {
            let angle = lerp(notch, tangent, i as f32 / HEART_ARC_POINTS as f32);
            (LOBE.0 + RADIUS * angle.cos(), LOBE.1 + RADIUS * angle.sin())
        })
        .collect();
    right.push(TIP);
    // The full outline. The left half is a mirror of the right half (without the tip and the
    // notch again).
    let outline = right
        .iter()
        .chain(right[1..right.len() - 1].iter().rev())
        .enumerate()
        .map(|(i, &(x, y))| at(center, size, (if i < right.len() { x } else { -x }, y)))
        .collect();
    (outline, right.len())
}

/// «Liked Songs»: a heart, outlined, that fills as `fill_t` goes to 1.
pub fn heart(painter: &Painter, center: Pos2, size: f32, fill_t: f32, color: Color32) {
    let (outline, _) = heart_outline(center, size);
    let fill = color.gamma_multiply(fill_t.clamp(0.0, 1.0));
    outlined_polygon(painter, &outline, size * HEART_STROKE, color, fill);
}

/// The breaks in each half of the shattered heart, in addition to the crack down the middle. Each
/// cut goes from a point on the outline (a fractional index into it) to a corner of the crack (an
/// index into it), in order along the outline. The outline of the right half goes from the notch
/// to the tip. The outline of the left half goes from the tip to the notch. Each half breaks into
/// four pieces of almost the same size.
const RIGHT_CUTS: [(f32, usize); 3] = [(6.5, 3), (13.0, 2), (20.0, 2)];
const LEFT_CUTS: [(f32, usize); 3] = [(0.7, 3), (5.0, 2), (11.0, 1)];

/// The pieces of the heart, clockwise, each with the side of its half (1 right, -1 left). They are
/// the two halves of a zigzag crack and, if `shattered`, the pieces of [`RIGHT_CUTS`] and
/// [`LEFT_CUTS`]. The cuts move in by half the width of the outline. Thus the drawn pieces touch
/// and do not overlap.
fn heart_pieces(center: Pos2, size: f32, shattered: bool) -> Vec<(Vec<Pos2>, f32)> {
    cut_heart(center, size, shattered, size * HEART_STROKE / 2.0)
}

/// [`heart_pieces`], with the cuts moved in by `pull`.
fn cut_heart(center: Pos2, size: f32, shattered: bool, pull: f32) -> Vec<(Vec<Pos2>, f32)> {
    let (outline, right_len) = heart_outline(center, size);
    let (notch, tip) = (outline[0], outline[right_len - 1]);
    // Straight teeth, sufficiently wide to see at this size.
    let teeth: Vec<Pos2> = [(0.2, 1.0), (0.45, -1.0), (0.7, 1.0)]
        .iter()
        .map(|&(t, side)| pos2(center.x + side * 0.08 * size, lerp(notch.y, tip.y, t)))
        .collect();
    let crack_down: Vec<Pos2> = std::iter::once(notch)
        .chain(teeth.iter().copied())
        .chain(std::iter::once(tip))
        .collect();
    let crack_up: Vec<Pos2> = crack_down.iter().rev().copied().collect();
    let mut left_outline = outline[right_len - 1..].to_vec();
    left_outline.push(notch);
    let halves = [
        (&outline[..right_len], crack_up, RIGHT_CUTS, 1.0),
        (&left_outline[..], crack_down, LEFT_CUTS, -1.0),
    ];
    let mut pieces = Vec::new();
    for (outer, crack, cuts, side) in halves {
        // The ends of the half are also cuts: at the start of its outline (the end of its crack)
        // and at its end (the start of the crack).
        let mut bounds = vec![(0.0, crack.len() - 1)];
        if shattered {
            bounds.extend(cuts);
        }
        bounds.push(((outer.len() - 1) as f32, 0));
        let point = |at: f32| {
            let i = (at.floor() as usize).min(outer.len() - 1);
            let j = (i + 1).min(outer.len() - 1);
            outer[i].lerp(outer[j], at.fract())
        };
        for pair in bounds.windows(2) {
            let ((from, crack_from), (to, crack_to)) = (pair[0], pair[1]);
            // Each point, and true if the edge from it is on the outline of the heart (not moved
            // in) or false if it is a cut (moved in).
            let mut piece = vec![(point(from), true)];
            let inner = (from.floor() as usize + 1)..=(to.ceil() as usize - 1);
            piece.extend(outer[inner].iter().map(|p| (*p, true)));
            piece.push((point(to), false));
            // Up the crack, without the points that the outline already has.
            let first = usize::from(to == (outer.len() - 1) as f32);
            let last = crack_from - usize::from(from == 0.0);
            piece.extend(crack[crack_to + first..=last].iter().map(|p| (*p, false)));
            let (points, on_outline): (Vec<Pos2>, Vec<bool>) = piece.into_iter().unzip();
            let pulls: Vec<f32> = on_outline
                .iter()
                .map(|&o| if o { 0.0 } else { pull })
                .collect();
            pieces.push((offset_edges(&points, &pulls), side));
        }
    }
    pieces
}

/// A piece of the broken heart turns a little around the tip and moves to the side with its half
/// (quickly at first, so the crack shows immediately). `side` is -1 for the left half and 1 for
/// the right half.
fn open_half(point: Pos2, pivot: Pos2, size: f32, side: f32, split: f32) -> Pos2 {
    let (sin, cos) = (side * split * 0.12).sin_cos();
    let v = point - pivot;
    pivot
        + vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
        + vec2(side * split.sqrt() * 0.07 * size, 0.0)
}

/// The heart, with a zigzag crack down the middle. The distance between its halves is `split` (0
/// is [`heart`]).
pub fn heart_split(
    painter: &Painter,
    center: Pos2,
    size: f32,
    fill_t: f32,
    color: Color32,
    split: f32,
) {
    let split = split.clamp(0.0, 1.0);
    if split <= 0.0 {
        return heart(painter, center, size, fill_t, color);
    }
    let tip = at(center, size, (0.0, 0.37));
    let fill = color.gamma_multiply(fill_t.clamp(0.0, 1.0));
    for (half, side) in heart_pieces(center, size, false) {
        let half: Vec<Pos2> = half
            .iter()
            .map(|p| open_half(*p, tip, size, side, split))
            .collect();
        outlined_polygon(painter, &half, size * HEART_STROKE, color, fill);
    }
}

/// A shattered heart, filled with `color`. It starts as [`heart_split`] with `split`, but in
/// pieces. At `t` (0 to 1), the pieces fly away, turn, and fade out.
pub fn heart_shards(
    painter: &Painter,
    center: Pos2,
    size: f32,
    color: Color32,
    split: f32,
    t: f32,
) {
    let t = t.clamp(0.0, 1.0);
    let fade = 1.0 - ease_in_quad(t);
    if fade <= 0.0 {
        return;
    }
    let color = color.gamma_multiply(fade);
    for piece in flying_pieces(center, size, split, t) {
        outlined_polygon(painter, &piece, size * HEART_STROKE, color, color);
    }
}

/// The positions of the pieces of [`heart_shards`] at `t`.
fn flying_pieces(center: Pos2, size: f32, split: f32, t: f32) -> Vec<Vec<Pos2>> {
    // The speed and spin of each piece of a half, in the order of [`heart_pieces`]. They are not
    // random, so the burst is always the same.
    const PIECES: [(f32, f32); 4] = [(1.0, 2.5), (0.7, -3.0), (1.25, 1.5), (0.85, -2.0)];
    let tip = at(center, size, (0.0, 0.37));
    let pieces = heart_pieces(center, size, true);
    pieces
        .into_iter()
        .enumerate()
        .map(|(k, (piece, side))| {
            let (speed, spin) = PIECES[k % PIECES.len()];
            // The start position of the piece: with its half, at its distance from the middle.
            let piece: Vec<Pos2> = piece
                .iter()
                .map(|p| open_half(*p, tip, size, side, split))
                .collect();
            let middle =
                piece.iter().fold(Vec2::ZERO, |sum, p| sum + p.to_vec2()) / piece.len() as f32;
            // Away from the middle of the heart, and to the side of its half.
            let away = (middle - center.to_vec2()).normalized() + vec2(side * 0.5, 0.0);
            let flight = away * size * 0.8 * speed * ease_out_cubic(t) + vec2(0.0, size * t * t);
            let (sin, cos) = (spin * t).sin_cos();
            piece
                .iter()
                .map(|p| {
                    let v = p.to_vec2() - middle;
                    (middle + vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos) + flight).to_pos2()
                })
                .collect()
        })
        .collect()
}

/// Fills `polygon` with `fill` and outlines it with a line of thickness `width`. It uses one
/// anti-aliased mesh that covers each pixel one time. Where the line and the fill have the same
/// color, the result is one even color, also when it is translucent. `polygon` can be concave,
/// but it must not cross itself.
fn outlined_polygon(
    painter: &Painter,
    polygon: &[Pos2],
    width: f32,
    stroke: Color32,
    fill: Color32,
) {
    let feather = 1.0 / painter.pixels_per_point();
    let (outer, inner) = (width / 2.0 + feather / 2.0, width / 2.0 - feather / 2.0);
    // From the outside in: the soft outer edge, the line, and the start of the fill. A filled shape
    // needs no inner edge. At sharp corners, the rings in it would overlap and show brighter areas
    // when translucent.
    let mut rings = vec![
        (offset(polygon, -outer), Color32::TRANSPARENT),
        (offset(polygon, -inner), stroke),
    ];
    if fill != stroke {
        rings.push((offset(polygon, inner), stroke));
        rings.push((offset(polygon, outer), fill));
    }
    let n = polygon.len() as u32;
    let mut mesh = Mesh::default();
    for (ring, color) in &rings {
        for point in ring {
            mesh.colored_vertex(*point, *color);
        }
    }
    for r in 0..rings.len() as u32 - 1 {
        let (a, b) = (r * n, (r + 1) * n);
        for i in 0..n {
            let j = (i + 1) % n;
            mesh.add_triangle(a + i, b + i, b + j);
            mesh.add_triangle(a + i, b + j, a + j);
        }
    }
    // The fill covers the innermost ring.
    if fill.a() > 0 {
        let (last, color) = &rings[rings.len() - 1];
        let first = (rings.len() as u32 - 1) * n;
        debug_assert_eq!(*color, fill);
        for triangle in triangulate(last).chunks(3) {
            mesh.add_triangle(
                first + triangle[0],
                first + triangle[1],
                first + triangle[2],
            );
        }
    }
    painter.add(Shape::mesh(mesh));
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

/// Two times the signed area of `polygon`: positive when clockwise on the screen.
fn signed_area(polygon: &[Pos2]) -> f32 {
    let n = polygon.len();
    (0..n)
        .map(|i| cross(polygon[i].to_vec2(), polygon[(i + 1) % n].to_vec2()))
        .sum()
}

/// `polygon` with each edge moved inward by its own distance (`amounts[i]` for the edge from point
/// `i` to the next point). Each corner goes where its two moved edges meet.
fn offset_edges(polygon: &[Pos2], amounts: &[f32]) -> Vec<Pos2> {
    let n = polygon.len();
    let sign = signed_area(polygon).signum();
    let inward = |a: Pos2, b: Pos2| {
        let edge = (b - a).normalized();
        vec2(-edge.y, edge.x) * sign
    };
    (0..n)
        .map(|i| {
            let before = (i + n - 1) % n;
            let (prev, next) = (polygon[before], polygon[(i + 1) % n]);
            let (n1, n2) = (inward(prev, polygon[i]), inward(polygon[i], next));
            let (a1, a2) = (amounts[before], amounts[i]);
            // The point where the two moved edges meet: d·n1 = a1 and d·n2 = a2.
            let det = cross(n1, n2);
            if det.abs() < 1e-3 {
                return polygon[i] + n1 * a1.max(a2);
            }
            let d = vec2(a1 * n2.y - a2 * n1.y, n1.x * a2 - n2.x * a1) / det;
            // Very sharp corners would go far out. A corner that slides along an edge that does
            // not move does not go past the other end of that edge.
            let mut limit = 3.0 * a1.max(a2);
            if a1 == 0.0 {
                limit = limit.min(0.9 * (polygon[i] - prev).length());
            }
            if a2 == 0.0 {
                limit = limit.min(0.9 * (next - polygon[i]).length());
            }
            polygon[i]
                + if d.length() > limit {
                    d.normalized() * limit
                } else {
                    d
                }
        })
        .collect()
}

/// `polygon` moved `amount` inward (outward if negative) along the normals of its edges, with
/// limited miters at the corners.
fn offset(polygon: &[Pos2], amount: f32) -> Vec<Pos2> {
    let n = polygon.len();
    let sign = signed_area(polygon).signum();
    let inward = |a: Pos2, b: Pos2| {
        let edge = (b - a).normalized();
        vec2(-edge.y, edge.x) * sign
    };
    (0..n)
        .map(|i| {
            let (prev, next) = (polygon[(i + n - 1) % n], polygon[(i + 1) % n]);
            let (n1, n2) = (inward(prev, polygon[i]), inward(polygon[i], next));
            let sum = n1 + n2;
            if sum.length() < 1e-3 {
                return polygon[i] + n1 * amount;
            }
            let direction = sum.normalized();
            let scale = (1.0 / direction.dot(n1).max(0.3)).min(3.0);
            polygon[i] + direction * amount * scale
        })
        .collect()
}

/// Triangles (indices into `polygon`) that cover a polygon that does not cross itself. The
/// function uses ear clipping.
fn triangulate(polygon: &[Pos2]) -> Vec<u32> {
    let sign = signed_area(polygon).signum();
    let mut left: Vec<usize> = (0..polygon.len()).collect();
    let mut triangles = Vec::with_capacity(3 * polygon.len());
    let turn = |a: Pos2, b: Pos2, c: Pos2| cross(b - a, c - b) * sign;
    while left.len() > 3 {
        let m = left.len();
        let corner = |i: usize| (left[(i + m - 1) % m], left[i], left[(i + 1) % m]);
        let ear = (0..m).find(|&i| {
            let (a, b, c) = corner(i);
            let (pa, pb, pc) = (polygon[a], polygon[b], polygon[c]);
            turn(pa, pb, pc) > 0.0
                && left.iter().all(|&k| {
                    k == a || k == b || k == c || {
                        let p = polygon[k];
                        // Outside, or on an edge.
                        turn(pa, pb, p) <= 0.0 || turn(pb, pc, p) <= 0.0 || turn(pc, pa, p) <= 0.0
                    }
                })
        });
        // This occurs only with degenerate shapes: remove the flattest corner.
        let i = ear.unwrap_or_else(|| {
            (0..m)
                .min_by(|&x, &y| {
                    let (a, b, c) = corner(x);
                    let (d, e, f) = corner(y);
                    let flat = |a: usize, b: usize, c: usize| {
                        turn(polygon[a], polygon[b], polygon[c]).abs()
                    };
                    flat(a, b, c).total_cmp(&flat(d, e, f))
                })
                .unwrap_or(0)
        });
        let (a, b, c) = corner(i);
        if ear.is_some() {
            triangles.extend([a as u32, b as u32, c as u32]);
        }
        left.remove(i);
    }
    if let [a, b, c] = left[..] {
        triangles.extend([a as u32, b as u32, c as u32]);
    }
    triangles
}

pub fn lock(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let body = Rect::from_min_max(
        at(center, size, (-0.30, -0.04)),
        at(center, size, (0.30, 0.38)),
    );
    painter.rect_filled(body, size * 0.08, color);
    let radius = 0.18;
    let mut shackle = vec![at(center, size, (-radius, -0.04))];
    shackle.extend((0..=10).map(|i| {
        let angle = std::f32::consts::PI * (1.0 - i as f32 / 10.0);
        at(
            center,
            size,
            (radius * angle.cos(), -0.20 - radius * angle.sin()),
        )
    }));
    shackle.push(at(center, size, (radius, -0.04)));
    painter.add(Shape::line(shackle, Stroke::new(size * 0.1, color)));
}

/// Three animated bars ("now playing"). `active` (0..1) fades the motion in and out. Thus, on a
/// pause, the bars stop smoothly.
pub fn equalizer(painter: &Painter, rect: Rect, time: f64, active: f32, color: Color32) {
    let bar_width = rect.width() / 5.0;
    for (i, (speed, phase)) in [(7.1, 0.0), (9.3, 1.7), (6.2, 3.1)].into_iter().enumerate() {
        let wave = (((time * speed + phase) as f32).sin() * 0.5 + 0.5).powf(0.8);
        let level = lerp(0.18, 0.25 + 0.75 * wave, active.clamp(0.0, 1.0));
        let x = rect.left() + bar_width * (i as f32 * 2.0);
        let bar = Rect::from_min_max(
            pos2(x, rect.bottom() - rect.height() * level),
            pos2(x + bar_width, rect.bottom()),
        );
        painter.rect_filled(bar, bar_width * 0.4, color);
    }
}

/// The bars of the app's logo. They move with the music when `playing` (0..1).
pub fn app_bars(painter: &Painter, rect: Rect, time: f64, playing: f32, color: Color32) {
    const HEIGHTS: [f32; 4] = [0.55, 1.0, 0.78, 0.45];
    let bar_width = rect.width() / 7.0;
    for (i, height) in HEIGHTS.into_iter().enumerate() {
        let wave = ((time * (6.0 + i as f64 * 1.7) + i as f64).sin() as f32) * 0.5 + 0.5;
        let level = lerp(height, 0.3 + 0.7 * wave, playing.clamp(0.0, 1.0));
        let h = rect.height() * level;
        let x = rect.left() + bar_width * (i as f32 * 2.0);
        let bar = Rect::from_min_max(
            pos2(x, rect.center().y - h / 2.0),
            pos2(x + bar_width, rect.center().y + h / 2.0),
        );
        painter.rect_filled(bar, bar_width / 2.0, color);
    }
}

/// Three dots that pulse in sequence (loading indicator).
pub fn loading_dots(painter: &Painter, center: Pos2, size: f32, time: f64, color: Color32) {
    for i in 0..3 {
        let phase = (time as f32 * 5.0 - i as f32 * 0.7).rem_euclid(TAU);
        let wave = phase.sin() * 0.5 + 0.5;
        let offset = vec2((i as f32 - 1.0) * size * 0.9, -wave * size * 0.25);
        painter.circle_filled(
            center + offset,
            size * 0.22,
            color.gamma_multiply(0.45 + 0.55 * wave),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pieces_of_a_shattered_heart_make_up_the_whole_heart() {
        let (center, size) = (pos2(50.0, 50.0), 100.0);
        let (outline, _) = heart_outline(center, size);
        let whole = signed_area(&outline);
        // Without the space for the line, the pieces fill the heart exactly, and no piece is very
        // thin.
        let pieces = cut_heart(center, size, true, 0.0);
        assert_eq!(pieces.len(), 8);
        let areas: Vec<f32> = pieces.iter().map(|(p, _)| signed_area(p)).collect();
        let sum: f32 = areas.iter().sum();
        assert!((sum - whole).abs() < whole * 1e-4, "{sum} of {whole}");
        assert!(
            areas.iter().all(|&a| a > whole * 0.04),
            "{areas:?} of {whole}"
        );
        // With the space, each piece is still a clockwise shape.
        let drawn = heart_pieces(center, size, true);
        assert!(drawn.iter().all(|(p, _)| signed_area(p) > whole * 0.01));
    }

    #[test]
    fn the_cracked_heart_breaks_into_its_two_halves() {
        let (center, size) = (pos2(50.0, 50.0), 100.0);
        let halves = heart_pieces(center, size, false);
        assert_eq!(
            halves.iter().map(|(_, side)| *side).collect::<Vec<_>>(),
            [1.0, -1.0]
        );
        // The right half stays right of the leftmost tooth of the crack. The left half stays left
        // of its rightmost tooth.
        assert!(halves[0].0.iter().all(|p| p.x > center.x - 0.09 * size));
        assert!(halves[1].0.iter().all(|p| p.x < center.x + 0.09 * size));
    }
}
