//! Makes `assets/icon.ico` again from the vector icon, `assets/icon.svg`, rendered by
//! `src/icon.rs` (run it in the container):
//!
//! ```sh
//! docker compose run --rm dev cargo run --example gen_icon
//! ```

use std::fs::File;
use std::io::BufWriter;

use image::ExtendedColorType;
use image::codecs::ico::{IcoEncoder, IcoFrame};

const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("assets")?;

    let frames = ICO_SIZES
        .iter()
        .map(|&size| {
            let rgba = melodia::icon::render_rgba(size);
            IcoFrame::as_png(&rgba, size, size, ExtendedColorType::Rgba8)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ico = BufWriter::new(File::create("assets/icon.ico")?);
    IcoEncoder::new(ico).encode_images(&frames)?;

    println!("Wrote assets/icon.ico ({ICO_SIZES:?})");
    Ok(())
}
