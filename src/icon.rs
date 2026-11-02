//! The application icon: `assets/icon.svg` (described in `docs/design.md`), embedded in
//! the executable and rendered with resvg. The window icon uses it. `examples/gen_icon.rs` also
//! uses it for `assets/icon.ico`.
//!
//! The login screen draws it in layers ([`render_layers`]) to turn the gradient of the note and
//! the border ([`ink_color`]). A layer is the full SVG with a style sheet that hides the other
//! parts (ids `circle`, `border`, `note`). This module calculates and dithers the glow of the
//! circle, not resvg, so that it shows no rings ([`render_circle`]).
//!
//! Coordinates are normalized to the icon: -0.5..0.5, with y down.

use std::sync::OnceLock;

use resvg::{tiny_skia, usvg};

const SVG: &str = include_str!("../assets/icon.svg");

/// `theme::BG`, the window background: the edge of the circle.
pub const BACKGROUND: [u8; 3] = [11, 11, 14];
/// `theme::DEFAULT_ACCENT`, the app's green: the end of the gradient.
pub const GREEN: [u8; 3] = [30, 215, 96];
/// The complementary color of [`GREEN`], as the window background calculates it: the start of
/// the gradient.
pub const PURPLE: [u8; 3] = [215, 30, 200];

/// Renders the icon as straight (non-premultiplied) RGBA pixels.
pub fn render_rgba(size: u32) -> Vec<u8> {
    let mut rgba = render_circle(size);
    let ink = render("#circle { display: none }", size);
    for (below, above) in rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(ink.as_chunks::<4>().0)
    {
        *below = over(*above, *below);
    }
    rgba
}

/// The circle alone, with its glow dithered. In 8 bits, the glow is dark, so it has only
/// approximately twelve shades from its middle to its edge. Each step would show as a ring. A mix
/// of the two nearest shades pixel by pixel, in the proportion of the exact color, hides the steps.
fn render_circle(size: u32) -> Vec<u8> {
    let (center, radius, stops) = &gradients().glow;
    let mut rgba = render("#border, #note { display: none }", size);
    let s = size as f32;
    for (i, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i as u32 % size, i as u32 / size);
        let p = [(x as f32 + 0.5) / s - 0.5, (y as f32 + 0.5) / s - 0.5];
        let t = (p[0] - center[0]).hypot(p[1] - center[1]) / radius;
        let color = color_along(stops, t);
        let noise = dither(x, y);
        for (c, exact) in pixel.iter_mut().zip(color) {
            *c = (exact + noise).round().clamp(0.0, 255.0) as u8;
        }
    }
    rgba
}

/// Noise for dithering at pixel (x, y), from -1 to 1 and usually near 0 (the sum of two uniform
/// values). It hides the steps and does not look grainy.
fn dither(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    let uniform = |bits: u32| (bits & 0xffff) as f32 / 65536.0;
    uniform(h) + uniform(h >> 16) - 1.0
}

/// The straight RGBA pixel `above` drawn over `below`.
fn over(above: [u8; 4], below: [u8; 4]) -> [u8; 4] {
    let (a, b) = (f32::from(above[3]) / 255.0, f32::from(below[3]) / 255.0);
    let alpha = a + b * (1.0 - a);
    if alpha <= 0.0 {
        return [0; 4];
    }
    let mut out = [0; 4];
    for i in 0..3 {
        let c = (f32::from(above[i]) * a + f32::from(below[i]) * b * (1.0 - a)) / alpha;
        out[i] = c.round().clamp(0.0, 255.0) as u8;
    }
    out[3] = (alpha * 255.0).round() as u8;
    out
}

/// The icon in layers, as straight RGBA pixels: the circle alone, and white masks of the note and
/// of the border. [`ink_color`] gives the masks their color over the circle.
pub struct IconLayers {
    pub circle: Vec<u8>,
    pub note: Vec<u8>,
    pub border: Vec<u8>,
}

pub fn render_layers(size: u32) -> IconLayers {
    IconLayers {
        circle: render_circle(size),
        note: render_note(size),
        border: render(
            "#circle, #note { display: none } #border { stroke: #fff }",
            size,
        ),
    }
}

/// The note alone, as a white mask (straight RGBA pixels) at its position in the icon. The
/// placeholder of a playlist with no cover tints it.
pub fn render_note(size: u32) -> Vec<u8> {
    render(
        "#circle, #border { display: none } #note { fill: #fff }",
        size,
    )
}

/// Renders the SVG with the style sheet `css` to `size`×`size` straight RGBA pixels.
fn render(css: &str, size: u32) -> Vec<u8> {
    let tree = parse(&with_style(css));
    let Some(mut pixmap) = tiny_skia::Pixmap::new(size, size) else {
        return Vec::new();
    };
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// The embedded SVG. A test makes sure that it is valid.
fn parse(svg: &str) -> usvg::Tree {
    usvg::Tree::from_str(svg, &usvg::Options::default()).expect("assets/icon.svg is valid SVG")
}

/// The SVG with a `<style>` element that holds `css` at the start.
fn with_style(css: &str) -> String {
    if css.is_empty() {
        return SVG.to_owned();
    }
    // Immediately after the opening tag of the root element.
    let start = SVG.find("<svg").map_or(0, |i| {
        SVG[i..].find('>').map_or(SVG.len(), |end| i + end + 1)
    });
    format!("{}<style>{css}</style>{}", &SVG[..start], &SVG[start..])
}

/// The gradients of the icon, as read from the SVG.
struct Gradients {
    /// The colors of the note and the border.
    ink: Stops,
    /// The start and end points of the gradients of the note and the border, in the coordinates
    /// of the icon.
    note: [[f32; 2]; 2],
    border: [[f32; 2]; 2],
    /// The glow of the circle: its center and radius, in the coordinates of the icon, and its
    /// colors from the center out.
    glow: ([f32; 2], f32, Stops),
}

/// The colors of a gradient and the position of each color along it (0..1), in order.
type Stops = Vec<(f32, [u8; 3])>;

fn gradients() -> &'static Gradients {
    static GRADIENTS: OnceLock<Gradients> = OnceLock::new();
    GRADIENTS.get_or_init(|| {
        let tree = parse(SVG);
        let size = tree.size().width();
        let paint = |id: &str| {
            let Some(usvg::Node::Path(path)) = tree.node_by_id(id) else {
                panic!("assets/icon.svg has no #{id}");
            };
            let paint = path
                .fill()
                .map(usvg::Fill::paint)
                .or_else(|| path.stroke().map(usvg::Stroke::paint));
            (path.abs_transform(), paint.cloned())
        };
        // From the coordinates of a gradient to the coordinates of the icon.
        let to_icon = |transform: tiny_skia::Transform, x: f32, y: f32| {
            let mut p = tiny_skia::Point::from_xy(x, y);
            transform.map_point(&mut p);
            [p.x / size - 0.5, p.y / size - 0.5]
        };
        let stops = |stops: &[usvg::Stop]| {
            stops
                .iter()
                .map(|s| {
                    let c = s.color();
                    (s.offset().get(), [c.red, c.green, c.blue])
                })
                .collect::<Stops>()
        };
        let linear = |id: &str| {
            let (transform, paint) = paint(id);
            let Some(usvg::Paint::LinearGradient(g)) = paint else {
                panic!("#{id} of assets/icon.svg is not painted with a linear gradient");
            };
            let transform = transform.pre_concat(g.transform());
            let line = [
                to_icon(transform, g.x1(), g.y1()),
                to_icon(transform, g.x2(), g.y2()),
            ];
            (line, stops(g.stops()))
        };
        let (note, ink) = linear("note");
        let (border, _) = linear("border");
        let glow = {
            let (transform, paint) = paint("circle");
            let Some(usvg::Paint::RadialGradient(g)) = paint else {
                panic!("#circle of assets/icon.svg is not painted with a radial gradient");
            };
            let transform = transform.pre_concat(g.transform());
            let center = to_icon(transform, g.cx(), g.cy());
            let edge = to_icon(transform, g.cx() + g.r().get(), g.cy());
            let radius = (edge[0] - center[0]).hypot(edge[1] - center[1]);
            (center, radius, stops(g.stops()))
        };
        Gradients {
            ink,
            note,
            border,
            glow,
        }
    })
}

/// The color at `t` (0..1) along a gradient, mixed in sRGB as SVG does, without rounding.
fn color_along(stops: &[(f32, [u8; 3])], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    let at = |i: usize| stops[i].1.map(f32::from);
    let Some(next) = stops.iter().position(|&(offset, _)| offset > t) else {
        return stops.len().checked_sub(1).map_or([0.0; 3], at);
    };
    let Some(previous) = next.checked_sub(1) else {
        return at(next);
    };
    let (from, to) = (at(previous), at(next));
    let k = (t - stops[previous].0) / (stops[next].0 - stops[previous].0);
    std::array::from_fn(|i| from[i] + (to[i] - from[i]) * k)
}

/// The color of the note and the border at `t` (0..1) along their gradient: from purple through
/// blue to green.
pub fn ink_color(t: f32) -> [u8; 3] {
    color_along(&gradients().ink, t).map(|c| c.round().clamp(0.0, 255.0) as u8)
}

/// The position of the point `p` along the gradient of the note (0..1). With `angle` 0, it is as
/// in the SVG (from the top of the stem to the bottom of the head). With a different `angle`, the
/// gradient turns that much clockwise around its middle. At π, it is reversed, green at the top.
pub fn note_gradient_t(p: [f32; 2], angle: f32) -> f32 {
    gradient_t(gradients().note, p, angle)
}

/// The position of the point `p` along the gradient of the border (0..1): across the full circle,
/// turned by `angle` as the gradient of the note.
pub fn border_gradient_t(p: [f32; 2], angle: f32) -> f32 {
    gradient_t(gradients().border, p, angle)
}

fn gradient_t([a, b]: [[f32; 2]; 2], p: [f32; 2], angle: f32) -> f32 {
    let length = (b[0] - a[0]).hypot(b[1] - a[1]);
    let middle = [f32::midpoint(a[0], b[0]), f32::midpoint(a[1], b[1])];
    let (sin, cos) = angle.sin_cos();
    let d = [(b[0] - a[0]) / length, (b[1] - a[1]) / length];
    let d = [d[0] * cos - d[1] * sin, d[0] * sin + d[1] * cos];
    let along = (p[0] - middle[0]) * d[0] + (p[1] - middle[1]) * d[1];
    (0.5 + along / length).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{PI, TAU};

    use super::*;

    const SIZE: u32 = 128;
    /// The middle of the width of the border.
    const BORDER: f32 = 0.476;
    /// Near the top of the stem, and in the bottom left of the head: the ends of the gradient.
    const STEM_TOP: [f32; 2] = [0.035, -0.315];
    const HEAD: [f32; 2] = [-0.15, 0.28];

    fn color_at(rgba: &[u8], p: [f32; 2]) -> [u8; 4] {
        let to_px = |c: f32| (((c + 0.5) * SIZE as f32) as u32).min(SIZE - 1);
        let i = ((to_px(p[1]) * SIZE + to_px(p[0])) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    /// The complementary purple: much more red and blue than green.
    fn is_purple(c: [u8; 4]) -> bool {
        c[0] > c[1] + 100 && c[2] > c[1] + 100
    }

    /// The app's green: much more green than blue.
    fn is_green(c: [u8; 4]) -> bool {
        c[1] > 150 && c[1] > c[2] + 60
    }

    #[test]
    fn the_svg_uses_the_app_colors() {
        let ink = &gradients().ink;
        assert_eq!(ink.first(), Some(&(0.0, PURPLE)));
        assert_eq!(ink.last(), Some(&(1.0, GREEN)));
        // The glow of the circle fades into the window background.
        let [r, g, b] = BACKGROUND;
        let background = format!(r##"<stop offset="1" stop-color="#{r:02x}{g:02x}{b:02x}"/>"##);
        assert!(SVG.contains(&background), "{background}");
    }

    #[test]
    fn the_circle_fills_the_image_and_the_corners_are_transparent() {
        let rgba = render_rgba(SIZE);
        assert_eq!(rgba.len(), (SIZE * SIZE * 4) as usize);
        for corner in [[-0.5, -0.5], [0.499, -0.5], [-0.5, 0.499], [0.499, 0.499]] {
            assert_eq!(color_at(&rgba, corner)[3], 0, "corner {corner:?}");
        }
        // The middle of each side is opaque: the circle touches it.
        for side in [[0.0, -0.5], [0.0, 0.499], [-0.5, 0.0], [0.499, 0.0]] {
            let c = color_at(&rgba, side);
            assert!(c[3] > 200, "side {side:?}: {c:?}");
        }
    }

    #[test]
    fn the_border_continues_the_note_gradient() {
        let rgba = render_rgba(SIZE);
        let at = |angle: f32| color_at(&rgba, [BORDER * angle.cos(), BORDER * angle.sin()]);
        // Purple at the top and green at the bottom, as the note.
        let (top, bottom) = (at(-1.4), at(1.8));
        assert!(is_purple(top), "purple: {top:?}");
        assert!(is_green(bottom), "green: {bottom:?}");
        let inside = color_at(&rgba, [0.4, 0.0]);
        assert!(
            inside[..3].iter().all(|&c| c < 40),
            "dark inside it: {inside:?}"
        );
    }

    #[test]
    fn the_circle_is_dark_with_a_green_glow() {
        let rgba = render_rgba(SIZE);
        let lit = color_at(&rgba, [-0.12, -0.14]);
        let edge = color_at(&rgba, [0.3, 0.32]);
        assert!(
            lit[1] > lit[0] + 25 && lit[1] > lit[2] + 15,
            "a green glow: {lit:?}"
        );
        assert!(
            edge[..3].iter().all(|&c| c < 30),
            "the background at the edge: {edge:?}"
        );
    }

    #[test]
    fn the_glow_is_dithered_so_it_shows_no_rings() {
        // An area where the exact color is between two 8-bit shades. Its pixels mix the two
        // shades, and their average gives the exact color.
        let size = 256;
        let rgba = render_circle(size);
        let (center, radius, stops) = &gradients().glow;
        let (x0, y0, side) = (150, 120, 8);
        let (mut sum, mut shades) = (0.0, std::collections::BTreeSet::new());
        let mut exact = 0.0;
        for y in y0..y0 + side {
            for x in x0..x0 + side {
                let green = rgba[((y * size + x) * 4 + 1) as usize];
                sum += f32::from(green);
                shades.insert(green);
                let p = [
                    (x as f32 + 0.5) / size as f32 - 0.5,
                    (y as f32 + 0.5) / size as f32 - 0.5,
                ];
                let t = (p[0] - center[0]).hypot(p[1] - center[1]) / radius;
                exact += color_along(stops, t)[1];
            }
        }
        let n = (side * side) as f32;
        assert!(shades.len() > 1, "a single shade: {shades:?}");
        assert!(
            (sum / n - exact / n).abs() < 0.3,
            "{} vs {}",
            sum / n,
            exact / n
        );
    }

    #[test]
    fn the_note_goes_from_purple_at_the_top_to_green_at_the_bottom() {
        let rgba = render_rgba(SIZE);
        let (top, head) = (color_at(&rgba, STEM_TOP), color_at(&rgba, HEAD));
        assert!(is_purple(top), "purple: {top:?}");
        assert!(is_green(head), "green: {head:?}");
    }

    #[test]
    fn the_note_is_centered() {
        let size = 256;
        let rgba = render_note(size);
        let (mut min, mut max) = ([size; 2], [0; 2]);
        for (i, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] > 127 {
                let p = [i as u32 % size, i as u32 / size];
                min = [min[0].min(p[0]), min[1].min(p[1])];
                max = [max[0].max(p[0] + 1), max[1].max(p[1] + 1)];
            }
        }
        for k in 0..2 {
            let center = (min[k] + max[k]) as f32 / 2.0 / size as f32 - 0.5;
            assert!(center.abs() < 0.01, "off center: {min:?} {max:?}");
        }
    }

    #[test]
    fn the_gradient_goes_through_blue_and_turns_round() {
        assert_eq!(ink_color(0.0), PURPLE);
        assert_eq!(ink_color(0.4), [40, 120, 235]);
        assert_eq!(ink_color(1.0), GREEN);
        // More green than purple: after the middle, it is nearer to green.
        let middle = ink_color(0.5);
        assert!(middle[1] > middle[0], "{middle:?}");
        let turned = |p, angle| note_gradient_t(p, angle);
        assert!(turned(STEM_TOP, 0.0) < 0.1 && turned(HEAD, 0.0) > 0.85);
        // Half a turn reverses it. A full turn keeps it as it was.
        assert!(turned(STEM_TOP, PI) > 0.9 && turned(HEAD, PI) < 0.15);
        assert!((turned(STEM_TOP, TAU) - turned(STEM_TOP, 0.0)).abs() < 1e-4);
        // The border turns with the note.
        let edge = [-0.2, 0.4];
        assert!(border_gradient_t(edge, 0.0) > 0.9 && border_gradient_t(edge, PI) < 0.1);
    }

    #[test]
    fn the_layers_put_together_make_the_icon() {
        let full = render_rgba(SIZE);
        let layers = render_layers(SIZE);
        let note = [0.017, 0.0];
        let border = [0.0, BORDER];
        let inside = [0.2, 0.0];
        assert_eq!(color_at(&layers.note, note), [255; 4]);
        assert_eq!(color_at(&layers.note, inside)[3], 0);
        assert_eq!(color_at(&layers.border, border), [255; 4]);
        assert_eq!(color_at(&layers.border, inside)[3], 0);
        assert_eq!(color_at(&layers.circle, inside), color_at(&full, inside));
        // The circle layer does not have the note or the border.
        for p in [note, border] {
            let c = color_at(&layers.circle, p);
            assert!(c[..3].iter().all(|&v| v < 60), "{p:?}: {c:?}");
        }
        // The note of the full icon has the color of the gradient at that point (sampled at the
        // center of the pixel, so a few units different).
        let (shown, ink) = (color_at(&full, note), ink_color(note_gradient_t(note, 0.0)));
        let close = (0..3).all(|i| shown[i].abs_diff(ink[i]) <= 6);
        assert!(close && shown[3] == 255, "{shown:?} vs {ink:?}");
    }
}
