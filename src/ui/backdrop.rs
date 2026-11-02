//! The window background: a dark base with a soft glow of the accent color and, in the opposite
//! corner, a glow of its complementary color. [`color_at`] tells its color at each point, so that
//! overlays can fade into it. [`panel`] gives dialogs and menus the same look, with the colors
//! swapped, and [`framed`] puts it under their contents.
//!
//! The two glows use one texture, made one time ([`falloff`]). They add light to the base and do
//! not cover it, so they show no rings (see «Large soft gradients» in
//! `docs/interface.md`).

use egui::ecolor::HsvaGamma;
use egui::epaint::RectShape;
use egui::{
    Color32, ColorImage, Context, Frame, Id, Margin, Pos2, Rect, Shape, Stroke, StrokeKind,
    TextureHandle, TextureId, TextureOptions, Ui, Vec2, pos2, vec2,
};

use super::theme;

/// The position of a glow, its reach and the light that it adds.
struct Glow {
    center: Pos2,
    /// The end of the straight segments of [`falloff`]. With their corners rounded, the glow goes
    /// [`GLOW_REACH`] times as far.
    radius: f32,
    /// The light that it adds to the base at its center: an additive color.
    light: Color32,
}

impl Glow {
    /// The square that the texture of the glow covers.
    fn square(&self) -> Rect {
        Rect::from_center_size(self.center, Vec2::splat(2.0 * self.radius * GLOW_REACH))
    }
}

/// What the last [`paint`] drew, kept so that [`color_at`] can sample it.
#[derive(Clone, Copy)]
struct Painted {
    rect: Rect,
    accent: Color32,
}

fn memory_id() -> Id {
    Id::unique("backdrop")
}

fn glows(rect: Rect, accent: Color32) -> [Glow; 2] {
    let size = rect.width().max(rect.height());
    [
        Glow {
            center: pos2(
                rect.left() + rect.width() * 0.22,
                rect.top() + rect.height() * 0.42,
            ),
            radius: size * 0.75,
            light: light(accent, 0.2),
        },
        // The opposite corner gets the complementary color, so the background does not have only
        // one tint.
        Glow {
            center: rect.right_bottom(),
            radius: size * 0.6,
            light: light(complementary(accent), 0.14),
        },
    ]
}

/// `color` with its hue rotated half a turn.
pub(super) fn complementary(color: Color32) -> Color32 {
    let mut hsva = HsvaGamma::from(color);
    hsva.h = (hsva.h + 0.5).fract();
    hsva.into()
}

/// The light that changes the base into `color` laid over it with `opacity`, as an additive color
/// (alpha 0). A channel where `color` is darker than the base stays as the base is.
fn light(color: Color32, opacity: f32) -> Color32 {
    let add = |c: u8, base: u8| {
        ((f32::from(c) - f32::from(base)) * opacity)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_premultiplied(
        add(color.r(), theme::BG.r()),
        add(color.g(), theme::BG.g()),
        add(color.b(), theme::BG.b()),
        0,
    )
}

/// The ring of each glow where its strength decreases to [`GLOW_MID_STRENGTH`], as a fraction of
/// the radius. An intermediate ring gives a softer, more natural falloff.
const GLOW_MID_RING: f64 = 0.45;
const GLOW_MID_STRENGTH: f64 = 0.42;

/// Half the width, in radii, of the window where [`falloff`] averages its straight segments. A
/// comparison on screenshots with the segments alone showed that it rounds their corners and
/// keeps their look, with the same radii.
const GLOW_SOFTNESS: f64 = 0.1;

/// The reach of a glow, in radii.
const GLOW_REACH: f32 = 1.0 + GLOW_SOFTNESS as f32;

/// The strength of a glow at `r` radii from its center. It has straight segments from 1 at the
/// center, through [`GLOW_MID_STRENGTH`] at [`GLOW_MID_RING`], to 0 at the radius. The function
/// averages them over ±[`GLOW_SOFTNESS`] around `r`, with weights that decrease linearly to 0 at
/// the ends. Where the segments are straight, this changes nothing. It rounds their corners,
/// which the eye would see as rings. Also the rate of the bend of the curve changes gradually.
fn falloff(r: f32) -> f32 {
    if r >= GLOW_REACH {
        return 0.0;
    }
    // The average of a function over that window is the second difference of its second
    // integral. In f64, because that difference loses precision.
    let (r, h) = (f64::from(r), GLOW_SOFTNESS);
    let average = (segments_second_integral(r + h) - 2.0 * segments_second_integral(r)
        + segments_second_integral(r - h))
        / (h * h);
    average.clamp(0.0, 1.0) as f32
}

/// The second integral from 0 to `x` of the straight segments of [`falloff`]. For negative `x`, it
/// is a mirror, so that the center stays flat.
fn segments_second_integral(x: f64) -> f64 {
    const STOPS: [(f64, f64); 3] = [(0.0, 1.0), (GLOW_MID_RING, GLOW_MID_STRENGTH), (1.0, 0.0)];
    let x = x.abs();
    // The two integrals up to the start of each segment.
    let (mut first, mut second) = (0.0, 0.0);
    for pair in STOPS.windows(2) {
        let [(r0, s0), (r1, s1)] = [pair[0], pair[1]];
        let slope = (s1 - s0) / (r1 - r0);
        let d = x.min(r1) - r0;
        second += first * d + s0 * d * d / 2.0 + slope * d * d * d / 6.0;
        first += s0 * d + slope * d * d / 2.0;
        if x <= r1 {
            return second;
        }
    }
    // After the last stop, the segments are 0: the first integral does not change.
    second + first * (x - 1.0)
}

/// The pixels on each side of the glow texture. [`falloff`] has no corners, so linear filtering
/// fills the space between texels, for all glow sizes.
const GLOW_TEXTURE_PX: usize = 256;

/// The texture of all glows: white with the strength of [`falloff`], from the center of the square
/// out to [`GLOW_REACH`] at its edges. A tint with the light of a glow gives that glow. The
/// function makes it only one time.
fn glow_texture(ctx: &Context) -> TextureId {
    let id = Id::unique("backdrop-glow");
    if let Some(texture) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return texture.id();
    }
    let half = GLOW_TEXTURE_PX as f32 / 2.0;
    let pixels = (0..GLOW_TEXTURE_PX * GLOW_TEXTURE_PX)
        .map(|i| {
            let offset = vec2(
                (i % GLOW_TEXTURE_PX) as f32 + 0.5 - half,
                (i / GLOW_TEXTURE_PX) as f32 + 0.5 - half,
            );
            let strength = falloff(offset.length() / half * GLOW_REACH);
            Color32::from_white_alpha((strength * 255.0).round() as u8)
        })
        .collect();
    let image = ColorImage::new([GLOW_TEXTURE_PX; 2], pixels);
    let texture = ctx.load_texture("backdrop-glow", image, TextureOptions::LINEAR);
    let texture_id = texture.id();
    ctx.data_mut(|d| d.insert_temp(id, texture));
    texture_id
}

/// The full texture.
const FULL_UV: Rect = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));

/// Paints the background over `rect`.
pub fn paint(ui: &Ui, rect: Rect, accent: Color32) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme::BG);
    let texture = glow_texture(ui.ctx());
    for glow in glows(rect, accent) {
        painter.image(texture, glow.square(), FULL_UV, glow.light);
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(memory_id(), Painted { rect, accent }));
}

/// The background color at `pos`, as painted in this frame.
pub fn color_at(ctx: &Context, pos: Pos2) -> Color32 {
    let Some(painted) = ctx.data(|d| d.get_temp::<Painted>(memory_id())) else {
        return theme::BG;
    };
    sample(painted.rect, painted.accent, pos)
}

/// The color at `pos` of a background over `rect` tinted by `accent`.
fn sample(rect: Rect, accent: Color32, pos: Pos2) -> Color32 {
    let mut rgb = [theme::BG.r(), theme::BG.g(), theme::BG.b()].map(f32::from);
    for glow in glows(rect, accent) {
        let strength = falloff(pos.distance(glow.center) / glow.radius);
        let light = [glow.light.r(), glow.light.g(), glow.light.b()];
        for (channel, light) in rgb.iter_mut().zip(light) {
            *channel += f32::from(light) * strength;
        }
    }
    let [r, g, b] = rgb.map(|c| c.round().min(255.0) as u8);
    Color32::from_rgb(r, g, b)
}

/// `rect`, with rounded corners, filled as the window background. But the main glow has the
/// complementary color of `accent`, and `accent` is in the opposite corner.
pub fn panel(ctx: &Context, rect: Rect, radius: f32, accent: Color32) -> Shape {
    let texture = glow_texture(ctx);
    let mut shapes = vec![Shape::from(RectShape::filled(rect, radius, theme::BG))];
    for glow in glows(rect, complementary(accent)) {
        // The part of the texture of the glow under the panel (outside its edges, the texture is
        // clear).
        let square = glow.square();
        let uv = Rect::from_min_max(
            ((rect.min - square.min) / square.size()).to_pos2(),
            ((rect.max - square.min) / square.size()).to_pos2(),
        );
        shapes.push(
            RectShape::filled(rect, radius, glow.light)
                .with_texture(texture, uv)
                .into(),
        );
    }
    Shape::Vec(shapes)
}

/// Draws `add_contents` in `margin` on a [`panel`] with rounded corners and a border, with the size
/// of the contents.
pub fn framed<R>(
    ui: &mut Ui,
    margin: Margin,
    accent: Color32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    // The background goes under the contents when their size is known.
    let background = ui.painter().add(Shape::Noop);
    let content = Frame::new().inner_margin(margin).show(ui, add_contents);
    let rect = content.response.rect;
    let painter = ui.painter();
    painter.set(
        background,
        panel(ui.ctx(), rect, theme::RADIUS_LARGE, accent),
    );
    painter.rect_stroke(
        rect,
        theme::RADIUS_LARGE,
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    content.inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_icon_uses_the_colors_of_the_idle_background() {
        let rgb = |c: Color32| [c.r(), c.g(), c.b()];
        assert_eq!(crate::icon::GREEN, rgb(theme::DEFAULT_ACCENT));
        assert_eq!(
            crate::icon::PURPLE,
            rgb(complementary(theme::DEFAULT_ACCENT))
        );
        assert_eq!(crate::icon::BACKGROUND, rgb(theme::BG));
    }

    #[test]
    fn a_glow_adds_what_its_color_laid_over_the_background_would_show() {
        let green = light(theme::DEFAULT_ACCENT, 0.2);
        assert_eq!(green.a(), 0, "it only adds light");
        // (30, 215, 96) at 20 % over (11, 11, 14).
        assert_eq!(theme::BG + green, Color32::from_rgb(15, 52, 30));
        // Red cannot make the green and blue of the base darker: they stay.
        let red = light(Color32::from_rgb(255, 0, 0), 0.2);
        assert_eq!([red.r(), red.g(), red.b()], [49, 0, 0]);
    }

    #[test]
    fn the_glow_falloff_only_rounds_the_corners_of_its_straight_segments() {
        let straight = |r: f64| {
            if r < GLOW_MID_RING {
                1.0 + (GLOW_MID_STRENGTH - 1.0) * r / GLOW_MID_RING
            } else {
                GLOW_MID_STRENGTH * (1.0 - r) / (1.0 - GLOW_MID_RING)
            }
        };
        // Farther than GLOW_SOFTNESS from the corners (0, 0.45 and 1).
        for r in [0.12_f32, 0.25, 0.34, 0.56, 0.7, 0.88] {
            let error = (f64::from(falloff(r)) - straight(f64::from(r))).abs();
            assert!(error < 1e-4, "at {r}: {error}");
        }
        assert!((0.95..1.0).contains(&falloff(0.0)), "{}", falloff(0.0));
        assert!(falloff(1.0) > 0.0);
        assert_eq!(falloff(GLOW_REACH), 0.0);
    }

    #[test]
    fn the_glow_falloff_has_no_corners_that_would_show_as_rings() {
        const STEP: f32 = 0.0005;
        let slope = |r: f32| (falloff(r + STEP) - falloff(r)) / STEP;
        for i in 0..2400_u16 {
            let r = f32::from(i) * STEP;
            assert!(slope(r) < 1e-3, "it brightens outwards at {r}");
            // A corner changes the slope by 0.5 or more in one step. The tightest bend, at the
            // center, changes it by 0.013 for each step.
            let bend = (slope(r + STEP) - slope(r)).abs();
            assert!(bend < 0.02, "corner at {r}: {bend}");
        }
    }
}
