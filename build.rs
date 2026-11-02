//! Embeds the icon and version information in the Windows executable.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
    // windres runs from another directory, so the path must be absolute.
    let icon = Path::new(&manifest_dir).join("assets").join("icon.ico");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(icon.to_str().expect("the project path is valid UTF-8"))
        .set("ProductName", "Melodia")
        .set(
            "FileDescription",
            "Melodia: a desktop player for Spotify on Windows",
        )
        .set("InternalName", "melodia")
        .set("OriginalFilename", "melodia.exe")
        .set_language(0x0409); // English (United States)
    resource
        .compile()
        .expect("failed to embed the Windows resources (icon, version)");
}
