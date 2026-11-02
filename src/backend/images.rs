//! Download and decoding of covers, and extraction of the accent color.

use image::RgbaImage;
use image::imageops::FilterType;

use super::protocol::DecodedImage;

/// The accent of images that are mostly gray.
const NEUTRAL_ACCENT: [u8; 3] = [205, 205, 214];

/// The reason why a cover could not load.
#[derive(Debug, thiserror::Error)]
pub enum FetchImageError {
    #[error("download failed: {0}")]
    Download(#[from] reqwest::Error),
    #[error("decoding failed: {0}")]
    Decode(#[from] image::ImageError),
    #[error("the decoding task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Downloads and decodes an image, scaled down to fit in `max_px`.
pub async fn fetch_image(
    http: &reqwest::Client,
    url: &str,
    max_px: u32,
) -> Result<DecodedImage, FetchImageError> {
    let response = http.get(url).send().await?.error_for_status()?;
    let bytes = response.bytes().await?;
    let image = tokio::task::spawn_blocking(move || decode_image(&bytes, max_px)).await??;
    Ok(image)
}

pub fn decode_image(bytes: &[u8], max_px: u32) -> Result<DecodedImage, image::ImageError> {
    let image = image::load_from_memory(bytes)?;
    let image = if image.width() > max_px || image.height() > max_px {
        image.resize(max_px, max_px, FilterType::Triangle)
    } else {
        image
    };
    Ok(from_rgba(image.to_rgba8()))
}

/// Packs a decoded image with its accent color. The accent comes from a 32 px version. Thus a
/// thumbnail gives the same color as the full cover, and the UI does not need to load a large
/// cover only for the color.
pub fn from_rgba(image: RgbaImage) -> DecodedImage {
    let accent = accent_color(&image);
    DecodedImage {
        width: image.width(),
        height: image.height(),
        accent,
        rgba: image.into_raw(),
    }
}

/// Selects the dominant vivid hue of an image, and returns it with a lightness that is easy to see
/// on a dark background. Images that are mostly gray get a neutral light gray.
pub fn accent_color(image: &RgbaImage) -> Option<[u8; 3]> {
    const BINS: usize = 24;
    let small = image::imageops::thumbnail(image, 32, 32);
    let mut bins = [(0.0f32, [0.0f32; 3]); BINS];
    let mut opaque_pixels = 0.0f32;

    for pixel in small.pixels() {
        if pixel[3] < 128 {
            continue;
        }
        opaque_pixels += 1.0;
        let rgb = [pixel[0], pixel[1], pixel[2]];
        let (hue, saturation, lightness) = rgb_to_hsl(rgb);
        // Prefer saturated pixels with medium lightness.
        let weight = saturation * saturation * (1.0 - (lightness - 0.5).abs() * 2.0).max(0.0);
        if weight <= 0.0 {
            continue;
        }
        let bin = ((hue * BINS as f32) as usize).min(BINS - 1);
        bins[bin].0 += weight;
        for (sum, value) in bins[bin].1.iter_mut().zip(rgb) {
            *sum += f32::from(value) * weight;
        }
    }
    if opaque_pixels == 0.0 {
        return None;
    }

    let (weight, sum) = bins
        .iter()
        .copied()
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap_or_default();
    if weight < opaque_pixels * 0.02 {
        return Some(NEUTRAL_ACCENT);
    }
    let average = sum.map(|channel| (channel / weight).round().clamp(0.0, 255.0) as u8);
    let (hue, saturation, lightness) = rgb_to_hsl(average);
    Some(hsl_to_rgb(
        hue,
        saturation.clamp(0.45, 0.9),
        lightness.clamp(0.58, 0.72),
    ))
}

/// RGB to (hue, saturation, lightness), all in `0..=1`.
pub fn rgb_to_hsl([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (hue / 6.0, saturation.clamp(0.0, 1.0), lightness)
}

pub fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> [u8; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue.rem_euclid(1.0) * 6.0;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    [r, g, b].map(|c| ((c + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_roundtrip() {
        for rgb in [[255, 0, 0], [12, 200, 90], [30, 60, 240], [128, 128, 128]] {
            let (h, s, l) = rgb_to_hsl(rgb);
            let back = hsl_to_rgb(h, s, l);
            for i in 0..3 {
                assert!(back[i].abs_diff(rgb[i]) <= 1, "{rgb:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn accent_follows_the_dominant_hue() {
        // Mostly blue with a small red corner.
        let image = RgbaImage::from_fn(64, 64, |x, y| {
            if x < 8 && y < 8 {
                image::Rgba([220, 20, 20, 255])
            } else {
                image::Rgba([20, 60, 210, 255])
            }
        });
        let [r, g, b] = accent_color(&image).unwrap();
        assert!(b > r && b > g, "expected a blue accent, got {r},{g},{b}");
        let (_, _, lightness) = rgb_to_hsl([r, g, b]);
        assert!((0.57..=0.73).contains(&lightness));
    }

    #[test]
    fn gray_images_get_a_neutral_accent() {
        let image = RgbaImage::from_pixel(16, 16, image::Rgba([90, 90, 90, 255]));
        assert_eq!(accent_color(&image), Some(NEUTRAL_ACCENT));
    }

    #[test]
    fn decodes_and_downscales_png() {
        let image = RgbaImage::from_pixel(400, 200, image::Rgba([10, 200, 120, 255]));
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let decoded = decode_image(&png, 100).unwrap();
        assert_eq!((decoded.width, decoded.height), (100, 50));
        assert_eq!(decoded.rgba.len(), 100 * 50 * 4);
        assert!(decoded.accent.is_some());

        let large = decode_image(&png, 600).unwrap();
        assert_eq!((large.width, large.height), (400, 200));
        assert_eq!(large.accent, decoded.accent);
    }

    #[test]
    fn invalid_data_is_an_error() {
        assert!(decode_image(b"not an image", 64).is_err());
    }
}
