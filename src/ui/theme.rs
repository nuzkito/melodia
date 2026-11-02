//! Colors, fonts and the egui style of the app.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use egui::epaint::text::{HintingTarget, SmoothHinting};
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, vec2,
};

pub const BG: Color32 = Color32::from_rgb(11, 11, 14);
pub const SURFACE: Color32 = Color32::from_rgb(23, 23, 29);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(32, 32, 40);
pub const BORDER: Color32 = Color32::from_rgb(42, 42, 52);
pub const TEXT: Color32 = Color32::from_rgb(242, 242, 247);
pub const TEXT_DIM: Color32 = Color32::from_rgb(166, 166, 180);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(128, 128, 142);
pub const SPOTIFY_GREEN: Color32 = Color32::from_rgb(30, 215, 96);
pub const DEFAULT_ACCENT: Color32 = SPOTIFY_GREEN;
pub const DANGER: Color32 = Color32::from_rgb(255, 110, 110);
/// The color of a folder of playlists with no cover for a color.
pub const FOLDER_GRAY: Color32 = Color32::from_rgb(132, 132, 146);
/// The ring around the element with the keyboard focus.
pub const FOCUS_RING: Color32 = Color32::from_rgba_premultiplied(206, 206, 210, 217);

/// Durations, counters, badges and secondary lines.
pub const FONT_SMALL: f32 = 12.0;
/// Regular text.
pub const FONT_BODY: f32 = 14.0;
/// Titles, in semibold.
pub const FONT_TITLE: f32 = 16.0;
/// The login screen headline.
pub const FONT_HERO: f32 = 28.0;

/// Covers, tooltips and other small items. Spotify's guidelines ask for covers with 4 px round
/// corners on small and medium screens.
pub const RADIUS_SMALL: f32 = 4.0;
/// Rows, buttons, fields and menus.
pub const RADIUS_MEDIUM: f32 = 6.0;
/// Cards, popovers, toasts and windows.
pub const RADIUS_LARGE: f32 = 8.0;

const STRONG_FAMILY: &str = "strong";

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Semibold text for titles.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG_FAMILY.into()))
}

fn corner(radius: f32) -> CornerRadius {
    CornerRadius::same(radius as u8)
}

/// Installs fonts and style. It keeps the animation settings that are already there (for example,
/// from tests).
pub fn install(ctx: &Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_fonts(font_definitions());
    ctx.all_styles_mut(apply_style);
}

fn apply_style(style: &mut egui::Style) {
    style.text_styles = [
        (TextStyle::Small, regular(FONT_SMALL)),
        (TextStyle::Body, regular(FONT_BODY)),
        (TextStyle::Button, regular(FONT_BODY)),
        (TextStyle::Heading, strong(FONT_TITLE)),
        (TextStyle::Monospace, FontId::monospace(FONT_BODY)),
    ]
    .into();
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(12.0, 6.0);
    style.spacing.scroll = egui::style::ScrollStyle::floating();
    style.interaction.selectable_labels = false;

    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.override_text_color = None;
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = SURFACE;
    visuals.faint_bg_color = SURFACE;
    visuals.code_bg_color = SURFACE_HOVER;
    visuals.hyperlink_color = DEFAULT_ACCENT;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = corner(RADIUS_LARGE);
    visuals.menu_corner_radius = corner(RADIUS_MEDIUM);
    visuals.window_shadow = Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    visuals.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    visuals.selection.bg_fill = DEFAULT_ACCENT.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, TEXT);
    visuals.text_cursor.stroke = Stroke::new(1.5, TEXT);

    let widgets = &mut visuals.widgets;
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.corner_radius = corner(RADIUS_MEDIUM);
        widget.expansion = 0.0;
    }
    widgets.noninteractive.bg_fill = BG;
    widgets.noninteractive.weak_bg_fill = BG;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    widgets.inactive.bg_fill = SURFACE;
    widgets.inactive.weak_bg_fill = SURFACE;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.hovered.bg_fill = SURFACE_HOVER;
    widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.active.bg_fill = SURFACE_HOVER;
    widgets.active.weak_bg_fill = SURFACE_HOVER;
    widgets.active.bg_stroke = Stroke::new(1.0, TEXT_FAINT);
    widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
}

/// Tints the scroll bars with the current accent color. They are translucent, so they get the
/// color of the background behind them, and are not white (egui draws them with the text color by
/// default). Only the scroll bars read these widget fills: the app paints its own widgets.
pub fn tint_scroll_bars(style: &mut egui::Style, accent: Color32) {
    let scroll = &mut style.spacing.scroll;
    scroll.foreground_color = false;
    // No track behind the handle: it would be a gray band on a tinted background.
    scroll.active_background_opacity = 0.0;
    scroll.interact_background_opacity = 0.0;
    let handle = accent.lerp_to_gamma(TEXT, 0.3);
    let widgets = &mut style.visuals.widgets;
    // While the pointer is over the list, egui scales this by 0.6.
    widgets.inactive.bg_fill = handle.gamma_multiply(0.5);
    widgets.hovered.bg_fill = handle.gamma_multiply(0.45);
    widgets.active.bg_fill = handle.gamma_multiply(0.6);
}

/// A font file on the disk (`index` selects a face in `.ttc` collections).
struct SystemFont {
    path: PathBuf,
    index: u32,
}

/// The fonts that the UI prefers on each platform: Segoe UI on Windows, Inter or DejaVu on Linux.
/// CJK and symbol fonts are fallbacks, so that titles in all scripts show. The fonts in egui stay
/// as the last fallback.
struct FontCandidates {
    regular: Vec<SystemFont>,
    strong: Vec<SystemFont>,
    fallbacks: Vec<SystemFont>,
}

fn font_file(path: impl Into<PathBuf>) -> SystemFont {
    SystemFont {
        path: path.into(),
        index: 0,
    }
}

fn font_candidates() -> FontCandidates {
    if cfg!(windows) {
        let windir =
            std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let fonts = windir.join("Fonts");
        FontCandidates {
            regular: vec![font_file(fonts.join("segoeui.ttf"))],
            strong: vec![
                font_file(fonts.join("seguisb.ttf")),
                font_file(fonts.join("segoeuib.ttf")),
            ],
            fallbacks: vec![
                font_file(fonts.join("seguisym.ttf")),
                font_file(fonts.join("msyh.ttc")),
                font_file(fonts.join("malgun.ttf")),
            ],
        }
    } else {
        let shared = Path::new("/usr/share/fonts");
        FontCandidates {
            regular: vec![
                font_file(shared.join("opentype/inter/Inter-Regular.otf")),
                font_file(shared.join("truetype/inter/Inter-Regular.ttf")),
                font_file(shared.join("truetype/dejavu/DejaVuSans.ttf")),
            ],
            strong: vec![
                font_file(shared.join("opentype/inter/Inter-SemiBold.otf")),
                font_file(shared.join("truetype/inter/Inter-SemiBold.ttf")),
                font_file(shared.join("truetype/dejavu/DejaVuSans-Bold.ttf")),
            ],
            fallbacks: vec![font_file(
                shared.join("opentype/noto/NotoSansCJK-Regular.ttc"),
            )],
        }
    }
}

fn load_first(fonts: &[SystemFont]) -> Option<FontData> {
    fonts.iter().find_map(load_font)
}

/// With the egui hinting defaults, fonts with TrueType instructions (Segoe UI) have soft stems. Full
/// grid fit makes them sharp. The advances do not change, so the layout stays the same.
fn load_font(font: &SystemFont) -> Option<FontData> {
    let mut data = match map_font(&font.path) {
        Some(bytes) => FontData::from_static(bytes),
        None => FontData::from_owned(std::fs::read(&font.path).ok()?),
    };
    data.index = font.index;
    data.tweak.hinting_target = HintingTarget::Smooth(SmoothHinting {
        light: false,
        symmetric_rendering: false,
        preserve_linear_metrics: false,
    });
    Some(data)
}

/// Maps a font file into memory, and does not read it. The CJK fallbacks are more than 30 MB. A
/// mapping loads only the pages of the glyphs that the UI draws. The system can remove these pages
/// again, because the file holds their data. The function maps each file one time for each process
/// and never unmaps it (egui wants `'static` data, and each app that the UI tests create installs
/// the fonts again).
fn map_font(path: &Path) -> Option<&'static [u8]> {
    static MAPPED: LazyLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = LazyLock::new(Mutex::default);
    let mut mapped = crate::util::lock(&MAPPED);
    if let Some(bytes) = mapped.get(path) {
        return Some(bytes);
    }
    let file = File::open(path).ok()?;
    // SAFETY: the mapping is safe only while nobody changes the file. These are system fonts
    // (`C:\Windows\Fonts`, `/usr/share/fonts`) that usual programs cannot write. Windows refuses
    // to truncate or delete a mapped file. If the bytes change, the font parser checks each read,
    // so the worst result is incorrect glyphs.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let bytes: &'static [u8] = Box::leak(Box::new(map));
    mapped.insert(path.to_owned(), bytes);
    Some(bytes)
}

fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let candidates = font_candidates();
    let defaults = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let mut proportional = Vec::new();
    if let Some(data) = load_first(&candidates.regular) {
        fonts.font_data.insert("ui-regular".into(), Arc::new(data));
        proportional.push("ui-regular".to_owned());
    }
    let mut strong = Vec::new();
    if let Some(data) = load_first(&candidates.strong) {
        fonts.font_data.insert("ui-strong".into(), Arc::new(data));
        strong.push("ui-strong".to_owned());
    }
    proportional.extend(defaults);
    for (i, data) in candidates
        .fallbacks
        .iter()
        .filter_map(load_font)
        .enumerate()
    {
        let name = format!("ui-fallback-{i}");
        fonts.font_data.insert(name.clone(), Arc::new(data));
        proportional.push(name);
    }
    strong.extend(proportional.iter().cloned());

    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts
        .families
        .insert(FontFamily::Name(STRONG_FAMILY.into()), strong);
    fonts
}
