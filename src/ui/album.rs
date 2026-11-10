//! An album in the middle column, which opens with a click on its name under a track (see
//! [`link`]): its name, year and tracks.

use std::collections::HashMap;

use egui::{Color32, Context, FontId, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use super::anim::{self, ROW_APPEAR_DELAY, ROW_APPEAR_DURATION, ease_out_cubic};
use super::context_menu::{self, Choice, ContextMenu, Target};
use super::detail::{self, DetailPanel, Opened, Placeholders};
use super::dnd::{self, DragItem, DragOrigin};
use super::textures::{THUMB_PX, TextureCache};
use super::theme;
use super::track_row::{ROW_HEIGHT, RowStyle, track_row};
use super::widgets::{self, LIST_TITLE_HEIGHT};
use crate::backend::Command;
use crate::model::{Album, PlaybackSnapshot, Track};
use crate::util::format_long_duration;

/// The album that a [`link`] put here to open, until the app takes it with [`take_request`].
fn request_key() -> Id {
    Id::unique("album-request")
}

/// The text of the panel while it has no tracks to list.
const PLACEHOLDERS: Placeholders = Placeholders {
    error_id: "album-error",
    failed: "The album can't be shown",
    empty_id: "album-empty",
    empty_title: "No songs",
    empty_subtitle: "Spotify has no songs of this album to play.",
};

/// The detail of the title row: "1975 · 43 min". Each part shows only if it is known.
fn detail_line(album: &Album) -> String {
    let duration = (!album.tracks.is_empty())
        .then(|| format_long_duration(album.tracks.iter().map(|track| track.duration_ms).sum()));
    album
        .year
        .map(|year| year.to_string())
        .into_iter()
        .chain(duration)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The name of the album in the "artist · album" line of a track (`subtitle`, drawn with `font`)
/// opens it in the middle column. It is underlined on hover, as a link. The card of the current
/// track and all track rows use it.
pub fn link(ui: &mut Ui, id: Id, track: &Track, subtitle: Rect, font: FontId) {
    let Some(album_id) = track.album_id.as_deref() else {
        return;
    };
    if track.album.is_empty() || !subtitle.is_positive() {
        return;
    }
    // The album comes after "artist · " (see `Track::subtitle`).
    let prefix = widgets::single_line(
        ui.painter(),
        &format!("{} · ", track.artist_line()),
        font,
        theme::TEXT_DIM,
        f32::INFINITY,
    );
    let left = subtitle.left() + prefix.size().x;
    // When the cut leaves almost nothing, there is no name to click.
    if subtitle.right() - left < 16.0 {
        return;
    }
    let rect = Rect::from_min_max(pos2(left, subtitle.top()), subtitle.max);
    let label = format!("Open album “{}”", track.album);
    if widgets::text_link(ui, rect, id.with("album"), &label).clicked() {
        request(ui.ctx(), album_id, &track.album);
    }
}

/// Asks the app to open the album `id` (with the name `name`), as a click on a [`link`] does.
pub fn request(ctx: &Context, id: &str, name: &str) {
    detail::request(ctx, request_key(), id, name);
}

/// The album whose [`link`] the user clicked, if there is one: its ID and name.
pub fn take_request(ctx: &Context) -> Option<(String, String)> {
    detail::take_request(ctx, request_key())
}

#[derive(Default)]
pub struct AlbumView {
    panel: DetailPanel<Album>,
    /// The tracks that the user recently added to the queue (uri → time), for the ✓ feedback.
    added_at: HashMap<String, f64>,
    /// The menu of a track, which opens with a right click.
    menu: ContextMenu,
}

pub struct AlbumData<'a> {
    pub playback: &'a PlaybackSnapshot,
    pub accent: Color32,
}

impl AlbumView {
    pub fn is_open(&self) -> bool {
        self.panel.is_open()
    }

    /// The ID and the name of the album that shows, if there is one.
    pub fn open_id(&self) -> Option<(&str, &str)> {
        self.panel.open_id()
    }

    /// Shows the album `id` (with the name `name`), and loads it again.
    pub fn open(&mut self, id: String, name: String, time: f64, actions: &mut Vec<Command>) {
        actions.push(Command::LoadAlbum { id: id.clone() });
        self.menu.close();
        self.panel.open(id, name, time);
    }

    pub fn close(&mut self) {
        self.panel.close();
    }

    pub fn on_album(&mut self, album: Album, time: f64) {
        self.panel.on_loaded(album, time);
    }

    pub fn on_failed(&mut self, id: &str, message: String) {
        self.panel.on_failed(id, message);
    }

    /// The panel over `rect`, faded and slid in by `open_t` (0..1).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        open_t: f32,
        data: &AlbumData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        self.added_at.retain(|_, at| time - *at < 2.0);
        let Some((open, interactive)) = self.panel.shown(&ctx, open_t) else {
            self.menu.close();
            return;
        };
        let rect = rect.translate(vec2(0.0, -8.0 * (1.0 - open_t)));
        let mut panel_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("album")
                .max_rect(rect.expand2(vec2(8.0, 0.0))),
        );
        panel_ui.multiply_opacity(open_t);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), LIST_TITLE_HEIGHT));
        let detail = open.item.as_ref().map(detail_line).unwrap_or_default();
        widgets::list_title(
            &panel_ui,
            header,
            Id::unique("album-title"),
            &open.name,
            &format!("Songs in the album “{}”", open.name),
            &detail,
            header.right(),
        );
        let list = widgets::list_rows(rect, header);
        let mut list_ui = panel_ui.new_child(UiBuilder::new().id_salt("album-list").max_rect(list));
        list_ui.set_clip_rect(list.intersect(panel_ui.clip_rect()));
        if !interactive {
            list_ui.disable();
            self.menu.close();
        }
        track_list(
            &mut list_ui,
            open,
            &mut self.added_at,
            &mut self.menu,
            data,
            textures,
            actions,
        );
        if let Some((choice, _, Target::Track(track))) = self.menu.show(&ctx, data.accent) {
            match choice {
                // As its ▶: only this track plays.
                Choice::Play => actions.push(Command::PlayTrack { track }),
                choice => {
                    if choice == Choice::PlayLast {
                        self.added_at.insert(track.uri.clone(), time);
                    }
                    actions.extend(context_menu::command(choice, Target::Track(track)));
                }
            }
        }
    }
}

/// The tracks of the album, or a placeholder while they load (or if they cannot load).
fn track_list(
    ui: &mut Ui,
    open: &Opened<Album>,
    added_at: &mut HashMap<String, f64>,
    menu: &mut ContextMenu,
    data: &AlbumData<'_>,
    textures: &mut TextureCache,
    actions: &mut Vec<Command>,
) {
    let ctx = ui.ctx().clone();
    let time = ctx.input(|i| i.time);
    let rect = ui.max_rect();
    let Some(album) = open.loaded(ui, rect, time, &PLACEHOLDERS, |album| {
        !album.tracks.is_empty()
    }) else {
        return;
    };
    let tracks = &album.tracks;

    let current_uri = data.playback.track_uri();
    let playing = anim::tween_bool(
        &ctx,
        Id::unique("album-playing"),
        data.playback.is_playing,
        0.4,
    );
    let elapsed = anim::elapsed_since(&ctx, open.arrived_at);
    anim::keep_animating(&ctx, elapsed, ROW_APPEAR_DELAY * 10.0 + ROW_APPEAR_DURATION);
    let mut play = None;
    let mut add = None;
    let scrolled = egui::ScrollArea::vertical()
        .id_salt(("album-scroll", open.id.as_str()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            let (area, _) = ui.allocate_exact_size(
                vec2(width, tracks.len() as f32 * ROW_HEIGHT + 8.0),
                Sense::hover(),
            );
            for (i, track) in tracks.iter().enumerate() {
                let appear = ease_out_cubic(anim::stagger(
                    elapsed,
                    i,
                    ROW_APPEAR_DELAY,
                    ROW_APPEAR_DURATION,
                ));
                let rect = Rect::from_min_size(
                    pos2(
                        area.left(),
                        area.top() + i as f32 * ROW_HEIGHT + (1.0 - appear) * 14.0,
                    ),
                    vec2(width, ROW_HEIGHT),
                );
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let cover = track
                    .cover_small
                    .as_deref()
                    .and_then(|url| textures.get(url, THUMB_PX, time));
                let added = added_at
                    .get(&track.uri)
                    .map(|at| anim::elapsed_since(&ctx, *at));
                let row = track_row(
                    ui,
                    rect,
                    Id::unique(("album-row", i)),
                    track,
                    cover.as_ref(),
                    RowStyle {
                        alpha: appear,
                        current: current_uri == Some(track.uri.as_str()),
                        playing,
                        pause_button: false,
                        draggable: true,
                        can_add: true,
                        remove_from: None,
                        locked: false,
                        added,
                        highlighted: false,
                        menu_open: menu.is_open_for(i),
                        accent: data.accent,
                        time,
                    },
                );
                if row.play {
                    play = Some(track.clone());
                }
                if row.add {
                    add = Some(track.clone());
                }
                if let Some(pos) = row.menu_at {
                    menu.open(&ctx, Target::Track(track.clone()), i, pos, false);
                }
                if let Some(pointer) = row.drag_from {
                    dnd::start(
                        &ctx,
                        DragItem::Track(track.clone()),
                        DragOrigin::Album,
                        rect,
                        pointer,
                    );
                }
            }
        });
    widgets::scroll_hints(ui, &scrolled, (0.0, 0.0));
    if let Some(track) = play {
        actions.push(Command::PlayTrack { track });
    }
    if let Some(track) = add {
        added_at.insert(track.uri.clone(), time);
        actions.push(Command::AddToQueue { track });
    }
}
