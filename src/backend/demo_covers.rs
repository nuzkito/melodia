//! Covers of the demo catalog, drawn from the name of their album or playlist. The same name
//! always gives the same image. Each name gets one of several styles and palettes, so that the
//! covers look different from each other, as real covers do.

use std::f32::consts::{PI, TAU};

use image::{Rgba, RgbaImage};
use rand::rngs::StdRng;
use rand::seq::{IndexedRandom, SliceRandom};
use rand::{RngExt, SeedableRng};

use super::images::hsl_to_rgb;

/// An sRGB color with channels in 0..255, as `f32` for the mixes.
type Color = [f32; 3];

/// The sine waves (amplitude, frequency, phase) that make the top edge of a hill.
type Ridge = [(f32, f32, f32); 3];

/// A function that gives the color at (u, v), both in 0..1. The third value is the size of a
/// pixel in the same units, for the anti-aliased edges.
type Painter = Box<dyn Fn(f32, f32, f32) -> Color>;

/// The compositions of a cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Flat geometric shapes on a plain background.
    Shapes,
    /// Bands of color, straight or at an angle, sometimes with a shifted disc.
    Stripes,
    /// Tiles with quarter circles and halves, in the palette colors.
    Tiles,
    /// Concentric rings or rays from a point.
    Rings,
    /// A sky, a sun and layers of hills.
    Landscape,
    /// Soft blurred spots of light on a dark background.
    Glow,
    /// Bands with wavy edges.
    Waves,
    /// A plain background with a thin frame and a small shape.
    Minimal,
    /// A grid of dots whose size follows a gradient.
    Halftone,
}

const STYLES: [Style; 9] = [
    Style::Shapes,
    Style::Stripes,
    Style::Tiles,
    Style::Rings,
    Style::Landscape,
    Style::Glow,
    Style::Waves,
    Style::Minimal,
    Style::Halftone,
];

/// The style that `seed` gets.
#[cfg(test)]
pub fn style_of(seed: &str) -> Style {
    *STYLES.choose(&mut rng_for(seed)).expect("there are styles")
}

/// Draws the cover of `seed` with `size` × `size` pixels.
pub fn render_cover(seed: &str, size: u32) -> RgbaImage {
    let mut rng = rng_for(seed);
    let style = *STYLES.choose(&mut rng).expect("there are styles");
    let palette = Palette::random(&mut rng);
    let paint = painter(style, &palette, &mut rng);
    let grain = match style {
        Style::Glow => rng.random_range(10.0..18.0),
        _ if rng.random_bool(0.5) => rng.random_range(3.0..10.0),
        _ => 0.0,
    };
    let noise_seed: u32 = rng.random();
    let px = 1.0 / size as f32;
    RgbaImage::from_fn(size, size, |x, y| {
        let (u, v) = ((x as f32 + 0.5) * px, (y as f32 + 0.5) * px);
        let mut color = paint(u, v, px);
        if grain > 0.0 {
            let n = noise(x, y, noise_seed) * grain;
            color = color.map(|c| c + n);
        }
        Rgba([to_u8(color[0]), to_u8(color[1]), to_u8(color[2]), 255])
    })
}

fn rng_for(seed: &str) -> StdRng {
    let hash = seed.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    StdRng::seed_from_u64(hash)
}

/// The colors of a cover: a background and four colors that stand out on it.
struct Palette {
    background: Color,
    colors: [Color; 4],
}

impl Palette {
    fn random(rng: &mut StdRng) -> Self {
        let hue: f32 = rng.random();
        let hsl = |h: f32, s: f32, l: f32| hsl_to_rgb(h, s, l).map(f32::from);
        let dark = rng.random_bool(0.5);
        let (background, mut colors) = match rng.random_range(0..5) {
            // Vivid, with complementary hues.
            0 => (
                if dark {
                    hsl(hue + 0.3, 0.35, 0.1)
                } else {
                    hsl(hue + 0.1, 0.4, 0.9)
                },
                [0.0, 0.5, 0.1, 0.6].map(|offset| hsl(hue + offset, 0.8, 0.52)),
            ),
            // Pastel.
            1 => (
                if dark {
                    hsl(hue + 0.55, 0.3, 0.22)
                } else {
                    hsl(hue, 0.35, 0.94)
                },
                [0.0, 0.18, 0.36, 0.7].map(|offset| hsl(hue + offset, 0.55, 0.76)),
            ),
            // One hue, from dark to light.
            2 => (
                if dark {
                    hsl(hue, 0.4, 0.08)
                } else {
                    hsl(hue, 0.25, 0.95)
                },
                [0.25, 0.42, 0.6, 0.78].map(|l| hsl(hue, 0.55, l)),
            ),
            // Black, white and grey, with one strong color.
            3 => (
                if dark {
                    [20.0, 20.0, 22.0]
                } else {
                    [237.0, 230.0, 216.0]
                },
                [
                    [245.0, 242.0, 235.0],
                    [128.0, 128.0, 128.0],
                    [28.0, 28.0, 30.0],
                    hsl(hue, 0.85, 0.52),
                ],
            ),
            // Earth colors and a cool contrast.
            _ => (
                if dark {
                    hsl(0.07, 0.35, 0.13)
                } else {
                    hsl(0.11, 0.4, 0.88)
                },
                [
                    hsl(0.03, 0.55, 0.45),
                    hsl(0.09, 0.6, 0.55),
                    hsl(0.13, 0.5, 0.68),
                    hsl(0.47 + hue * 0.1, 0.4, 0.38),
                ],
            ),
        };
        colors.shuffle(rng);
        Self { background, colors }
    }

    fn pick(&self, rng: &mut StdRng) -> Color {
        self.colors[rng.random_range(0..self.colors.len())]
    }

    /// The color of the palette (or the background) that is the most different from `color`.
    fn contrast(&self, color: Color) -> Color {
        std::iter::once(self.background)
            .chain(self.colors)
            .max_by(|a, b| distance(*a, color).total_cmp(&distance(*b, color)))
            .unwrap_or(self.background)
    }
}

fn painter(style: Style, palette: &Palette, rng: &mut StdRng) -> Painter {
    match style {
        Style::Shapes => shapes(palette, rng),
        Style::Stripes => stripes(palette, rng),
        Style::Tiles => tiles(palette, rng),
        Style::Rings => rings(palette, rng),
        Style::Landscape => landscape(palette, rng),
        Style::Glow => glow(palette, rng),
        Style::Waves => waves(palette, rng),
        Style::Minimal => minimal(palette, rng),
        Style::Halftone => halftone(palette, rng),
    }
}

/// A flat shape, as a signed distance (positive inside).
#[derive(Clone, Copy)]
enum Shape {
    Circle {
        c: [f32; 2],
        r: f32,
    },
    /// The half of a circle on one side of a line through its center.
    HalfCircle {
        c: [f32; 2],
        r: f32,
        angle: f32,
    },
    Rect {
        c: [f32; 2],
        half: [f32; 2],
        angle: f32,
    },
    Triangle {
        c: [f32; 2],
        r: f32,
        angle: f32,
    },
}

impl Shape {
    fn random(rng: &mut StdRng) -> Self {
        let c = [rng.random_range(0.15..0.85), rng.random_range(0.15..0.85)];
        let angle = if rng.random_bool(0.5) {
            f32::from(rng.random_range(0_u8..4)) * PI / 2.0
        } else {
            rng.random_range(0.0..TAU)
        };
        match rng.random_range(0..4) {
            0 => Self::Circle {
                c,
                r: rng.random_range(0.1..0.32),
            },
            1 => Self::HalfCircle {
                c,
                r: rng.random_range(0.15..0.4),
                angle,
            },
            2 => Self::Rect {
                c,
                half: [rng.random_range(0.05..0.35), rng.random_range(0.03..0.3)],
                angle,
            },
            _ => Self::Triangle {
                c,
                r: rng.random_range(0.12..0.35),
                angle,
            },
        }
    }

    fn distance(self, p: [f32; 2]) -> f32 {
        match self {
            Self::Circle { c, r } => r - length(sub(p, c)),
            Self::HalfCircle { c, r, angle } => {
                let q = rotate(sub(p, c), angle);
                (r - length(q)).min(q[1])
            }
            Self::Rect { c, half, angle } => {
                let q = rotate(sub(p, c), angle);
                (half[0] - q[0].abs()).min(half[1] - q[1].abs())
            }
            Self::Triangle { c, r, angle } => {
                let q = rotate(sub(p, c), angle);
                // The distance to each of its three sides, at r / 2 from the center.
                (0_u8..3)
                    .map(|i| {
                        let a = f32::from(i) * TAU / 3.0;
                        r / 2.0 - (q[0] * a.sin() - q[1] * a.cos())
                    })
                    .fold(f32::INFINITY, f32::min)
            }
        }
    }
}

fn shapes(palette: &Palette, rng: &mut StdRng) -> Painter {
    let background = palette.background;
    let shapes: Vec<(Shape, Color)> = (0..rng.random_range(2..=5))
        .map(|_| (Shape::random(rng), palette.pick(rng)))
        .collect();
    Box::new(move |u, v, px| {
        shapes.iter().fold(background, |color, (shape, fill)| {
            mix(color, *fill, coverage(shape.distance([u, v]), px))
        })
    })
}

fn stripes(palette: &Palette, rng: &mut StdRng) -> Painter {
    let colors = palette.colors;
    let count = rng.random_range(3..=9) as f32;
    let angle = [0.0, PI / 2.0, PI / 4.0, -PI / 4.0][rng.random_range(0..4)];
    let disc = rng.random_bool(0.5).then(|| {
        (
            [rng.random_range(0.3..0.7), rng.random_range(0.3..0.7)],
            rng.random_range(0.2..0.35),
        )
    });
    let band = move |t: f32, px: f32| {
        let t = t * count;
        let i = t.floor();
        let f = t - i;
        let color = |i: f32| colors[(i.rem_euclid(colors.len() as f32)) as usize];
        let mut out = color(i);
        out = mix(out, color(i + 1.0), coverage((f - 1.0) / count, px));
        mix(out, color(i - 1.0), coverage(-f / count, px))
    };
    Box::new(move |u, v, px| {
        let t = rotate([u - 0.5, v - 0.5], angle)[1] + 0.5;
        let color = band(t, px);
        match disc {
            Some((c, r)) => {
                let inside = coverage(r - length(sub([u, v], c)), px);
                mix(color, band(t + 0.5 / count, px), inside)
            }
            None => color,
        }
    })
}

fn tiles(palette: &Palette, rng: &mut StdRng) -> Painter {
    let n = rng.random_range(2..=5);
    let cells: Vec<(Color, Color, u8, u8)> = (0..n * n)
        .map(|_| {
            let back = palette.pick(rng);
            let mut front = palette.pick(rng);
            if distance(front, back) < 40.0 {
                front = palette.contrast(back);
            }
            (back, front, rng.random_range(0..4), rng.random_range(0..4))
        })
        .collect();
    let n_f = n as f32;
    Box::new(move |u, v, px| {
        let (x, y) = (u * n_f, v * n_f);
        let (col, row) = (x.floor().min(n_f - 1.0), y.floor().min(n_f - 1.0));
        let (back, front, kind, corner) = cells[row as usize * n + col as usize];
        // The position in the tile, in 0..1, turned so that the corner of the shape is at (0, 0).
        let (mut fx, mut fy) = (x - col, y - row);
        for _ in 0..corner {
            (fx, fy) = (fy, 1.0 - fx);
        }
        let distance = match kind {
            0 => 1.0 - length([fx, fy]),
            1 => 1.0 - fx - fy,
            2 => 0.42 - length([fx - 0.5, fy - 0.5]),
            _ => 0.5 - fy,
        };
        mix(back, front, coverage(distance / n_f, px))
    })
}

fn rings(palette: &Palette, rng: &mut StdRng) -> Painter {
    let colors = palette.colors;
    let used = rng.random_range(2..=4);
    let c = [rng.random_range(-0.2..1.2), rng.random_range(-0.2..1.2)];
    let rays = rng.random_bool(0.35);
    let width = rng.random_range(0.04..0.12);
    let ray_count = f32::from(rng.random_range(8_u8..24));
    Box::new(move |u, v, px| {
        let p = sub([u, v], c);
        let r = length(p);
        // The position across the bands, and the size of one band in pixels units.
        let (t, scale) = if rays {
            (
                (p[1].atan2(p[0]) / TAU + 0.5) * ray_count,
                TAU * r.max(px) / ray_count,
            )
        } else {
            (r / width, width)
        };
        let i = t.floor();
        let f = t - i;
        let color = |i: f32| colors[(i.rem_euclid(used as f32)) as usize];
        let edge = (0.5 - (f - 0.5).abs()) * scale;
        let next = if f > 0.5 { i + 1.0 } else { i - 1.0 };
        mix(color(next), color(i), coverage(edge, px))
    })
}

fn landscape(palette: &Palette, rng: &mut StdRng) -> Painter {
    let sky_top = palette.pick(rng);
    let sky_bottom = mix(palette.pick(rng), [255.0, 245.0, 225.0], 0.35);
    let sun = mix(palette.contrast(sky_bottom), [255.0, 250.0, 235.0], 0.4);
    let sun_c = [rng.random_range(0.25..0.75), rng.random_range(0.2..0.5)];
    let sun_r = rng.random_range(0.07..0.18);
    let dark = mix(palette.background, [10.0, 10.0, 16.0], 0.6);
    let layers = rng.random_range(2..=4);
    let hills: Vec<(f32, Ridge, Color)> = (0..layers)
        .map(|i| {
            let depth = (i + 1) as f32 / layers as f32;
            let waves = [0; 3].map(|_| {
                (
                    rng.random_range(0.01..0.06),
                    rng.random_range(2.0..12.0),
                    rng.random_range(0.0..TAU),
                )
            });
            let base = 0.5 + 0.38 * depth;
            let color = mix(mix(sky_bottom, palette.pick(rng), 0.5), dark, depth * 0.85);
            (base, waves, color)
        })
        .collect();
    Box::new(move |u, v, px| {
        let mut color = mix(sky_top, sky_bottom, v.powf(0.8));
        color = mix(color, sun, coverage(sun_r - length(sub([u, v], sun_c)), px));
        for (base, waves, fill) in &hills {
            let height = waves.iter().fold(*base, |h, (amp, freq, phase)| {
                h + amp * (u * freq + phase).sin()
            });
            color = mix(color, *fill, coverage(v - height, px));
        }
        color
    })
}

fn glow(palette: &Palette, rng: &mut StdRng) -> Painter {
    let background = mix(palette.background, [8.0, 8.0, 12.0], 0.7);
    let spots: Vec<([f32; 2], f32, Color)> = (0..rng.random_range(3..=5))
        .map(|_| {
            (
                [rng.random_range(0.0..1.0), rng.random_range(0.0..1.0)],
                rng.random_range(0.2..0.45),
                palette.pick(rng),
            )
        })
        .collect();
    Box::new(move |u, v, _| {
        spots.iter().fold(background, |color, (c, r, tint)| {
            let d = length(sub([u, v], *c)) / r;
            mix(color, *tint, 0.9 * (-d * d).exp())
        })
    })
}

fn waves(palette: &Palette, rng: &mut StdRng) -> Painter {
    let count = rng.random_range(4..=8);
    // Two bands next to each other never have the same color.
    let mut colors = vec![palette.pick(rng)];
    while colors.len() <= count {
        let color = palette.pick(rng);
        if colors.last() != Some(&color) {
            colors.push(color);
        }
    }
    let amp = rng.random_range(0.02..0.07);
    let freq = rng.random_range(1.0..4.0) * TAU;
    let phase = rng.random_range(0.0..TAU);
    let shift = rng.random_range(0.2..1.2);
    Box::new(move |u, v, px| {
        let mut color = colors[0];
        for (k, fill) in colors.iter().enumerate().skip(1) {
            let k = k as f32;
            let edge = k / (count + 1) as f32 + amp * (u * freq + phase + k * shift).sin();
            color = mix(color, *fill, coverage(v - edge, px));
        }
        color
    })
}

fn minimal(palette: &Palette, rng: &mut StdRng) -> Painter {
    let background = palette.background;
    let ink = palette.contrast(background);
    let accent = palette.pick(rng);
    let margin = rng.random_range(0.06..0.12);
    let line = rng.random_range(0.004..0.012);
    let framed = rng.random_bool(0.6);
    let c = [rng.random_range(0.3..0.7), rng.random_range(0.3..0.6)];
    let bars_y = if rng.random_bool(0.5) { 0.8 } else { 0.18 };
    let shape = match rng.random_range(0..3) {
        0 => Shape::Circle {
            c,
            r: rng.random_range(0.06..0.16),
        },
        1 => Shape::Rect {
            c,
            half: [0.1, 0.1],
            angle: [0.0, PI / 4.0][rng.random_range(0..2)],
        },
        _ => Shape::Rect {
            c,
            half: [rng.random_range(0.15..0.3), line * 1.5],
            angle: 0.0,
        },
    };
    Box::new(move |u, v, px| {
        let mut color = background;
        if framed {
            let outer = (0.5 - margin) - (u - 0.5).abs().max((v - 0.5).abs());
            let frame = coverage(outer, px) * coverage(-(outer - line), px);
            color = mix(color, ink, frame);
        }
        color = mix(color, accent, coverage(shape.distance([u, v]), px));
        // Two short bars on the left, as the title and the artist on a cover.
        for (y, w) in [(bars_y, 0.28), (bars_y + 0.04, 0.18)] {
            let bar = Shape::Rect {
                c: [margin + 0.06 + w / 2.0, y],
                half: [w / 2.0, 0.008],
                angle: 0.0,
            };
            color = mix(color, ink, 0.85 * coverage(bar.distance([u, v]), px));
        }
        color
    })
}

fn halftone(palette: &Palette, rng: &mut StdRng) -> Painter {
    let background = palette.background;
    let ink = palette.contrast(background);
    let second = palette.pick(rng);
    let n = f32::from(rng.random_range(9_u8..20));
    let angle = [0.0, PI / 4.0][rng.random_range(0..2)];
    let focus = [rng.random_range(0.2..0.8), rng.random_range(0.2..0.8)];
    let radial = rng.random_bool(0.6);
    Box::new(move |u, v, px| {
        let p = rotate([u - 0.5, v - 0.5], angle);
        let cell = [(p[0] * n).floor(), (p[1] * n).floor()];
        let center = [(cell[0] + 0.5) / n, (cell[1] + 0.5) / n];
        // The amount of ink at the center of the cell: it gives the size of the dot.
        let at = rotate(center, -angle);
        let at = [at[0] + 0.5, at[1] + 0.5];
        let amount = if radial {
            1.0 - length(sub(at, focus)) * 1.4
        } else {
            at[1]
        }
        .clamp(0.0, 1.0);
        let r = 0.5 / n * amount.sqrt() * 1.1;
        let dot = coverage(r - length(sub(p, center)), px);
        mix(mix(background, second, 0.15 * (1.0 - v)), ink, dot)
    })
}

/// The anti-aliased coverage of a shape, from its signed distance (positive inside).
fn coverage(signed_distance: f32, pixel: f32) -> f32 {
    (signed_distance / pixel + 0.5).clamp(0.0, 1.0)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn distance(a: Color, b: Color) -> f32 {
    length([a[0] - b[0], a[1] - b[1]]).hypot(a[2] - b[2])
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn length(p: [f32; 2]) -> f32 {
    p[0].hypot(p[1])
}

/// `p` turned by `angle` (in radians) around the origin.
fn rotate(p: [f32; 2], angle: f32) -> [f32; 2] {
    let (sin, cos) = angle.sin_cos();
    [p[0] * cos + p[1] * sin, p[1] * cos - p[0] * sin]
}

/// A value in -1..1 for each pixel, as film grain.
fn noise(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed;
    h = (h ^ (h >> 13)).wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 32_767.5 - 1.0
}

fn to_u8(channel: f32) -> u8 {
    channel.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_deterministic_and_colorful() {
        let a = render_cover("tides", 64);
        assert_eq!(a, render_cover("tides", 64));
        assert_ne!(a, render_cover("roots", 64));
        let first = a.get_pixel(0, 0);
        assert!(a.pixels().any(|p| p != first));
    }
}
