//! Spotify's full logo, white and without changes, rendered with resvg for the attribution on the
//! login screen (see `docs/design.md` and
//! <https://developer.spotify.com/documentation/design>).

use egui::ColorImage;
use resvg::{tiny_skia, usvg};

const SVG: &str = include_str!("../../assets/spotify-full-logo-white.svg");

/// The width of the logo divided by its height (its `viewBox`).
pub const ASPECT: f32 = 823.46 / 225.25;
/// The height of the icon divided by the height of the full logo. The clear space around it is
/// half the height of the icon.
pub const ICON_HEIGHT: f32 = 219.0 / 225.25;
/// The smallest width that the guidelines allow on screens.
pub const MIN_WIDTH: f32 = 70.0;

/// The logo rendered with a width of `width` pixels (the height follows [`ASPECT`]).
pub fn render(width: u32) -> ColorImage {
    let height = (width as f32 / ASPECT).round() as u32;
    let tree = usvg::Tree::from_str(SVG, &usvg::Options::default())
        .expect("assets/spotify-full-logo-white.svg is valid SVG");
    let Some(mut pixmap) = tiny_skia::Pixmap::new(width, height) else {
        return ColorImage::filled([0, 0], egui::Color32::TRANSPARENT);
    };
    let scale = width as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // The pixels of tiny-skia are premultiplied, as egui wants them.
    let rgba: Vec<u8> = pixmap
        .pixels()
        .iter()
        .flat_map(|p| [p.red(), p.green(), p.blue(), p.alpha()])
        .collect();
    ColorImage::from_rgba_premultiplied([width as usize, height as usize], &rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_renders_white_with_the_proportions_of_its_view_box() {
        let image = render(300);
        assert_eq!(image.size, [300, 82]);
        let opaque: Vec<_> = image.pixels.iter().filter(|p| p.a() == 255).collect();
        assert!(opaque.len() > 300 * 82 / 10, "the logo has ink");
        assert!(
            opaque
                .iter()
                .all(|p| p.r() == 255 && p.g() == 255 && p.b() == 255),
            "the logo is white"
        );
    }
}
