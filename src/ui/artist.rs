//! An artist in the middle column, which opens with a click on their name under a track (see
//! [`links`]). It shows their albums and singles, the newest first. They open as albums, or the
//! user can drag them onto the queue.

use egui::{
    Color32, Context, CursorIcon, FontId, Id, Pos2, Rect, Role, Sense, Ui, UiBuilder, WidgetInfo,
    pos2, vec2,
};

use super::album;
use super::anim::{self, ROW_APPEAR_DELAY, ROW_APPEAR_DURATION, ease_out_cubic};
use super::context_menu::{self, ContextMenu, Target};
use super::detail::{self, DetailPanel, Opened, Placeholders};
use super::dnd::{self, DragItem, DragOrigin};
use super::textures::{Cover, THUMB_PX, TextureCache};
use super::theme;
use super::widgets::{self, LIST_TITLE_HEIGHT};
use crate::backend::Command;
use crate::model::{Artist, ReleaseKind, Track};
use crate::util::plural;

/// The height of the row of each release (cover, name, type, year and songs).
const RELEASE_HEIGHT: f32 = widgets::ROW_HEIGHT;
const RELEASE_COVER: f32 = widgets::ROW_COVER;

/// The artist that [`links`] put here to open, until the app takes it with [`take_request`].
fn request_key() -> Id {
    Id::unique("artist-request")
}

/// The text of the panel while it has no releases to list.
const PLACEHOLDERS: Placeholders = Placeholders {
    error_id: "artist-error",
    failed: "The artist can't be shown",
    empty_id: "artist-empty",
    empty_title: "No albums",
    empty_subtitle: "Spotify has no albums or singles of this artist.",
};

/// Each artist name in the "artist, artist · album" line of a track (`subtitle`, drawn with
/// `font`) opens that artist in the middle column. It is underlined on hover, as a link. The card
/// of the current track and all track rows use it.
pub fn links(ui: &mut Ui, id: Id, track: &Track, subtitle: Rect, font: FontId) {
    if !subtitle.is_positive() {
        return;
    }
    let painter = ui.painter().clone();
    let width = |text: &str| {
        widgets::single_line(&painter, text, font.clone(), theme::TEXT_DIM, f32::INFINITY)
            .size()
            .x
    };
    // The names have ", " between them (see `Track::artist_line`).
    let mut left = subtitle.left();
    for (i, name) in track.artists.iter().enumerate() {
        if i > 0 {
            left += width(", ");
        }
        let name_width = width(name);
        let right = (left + name_width).min(subtitle.right());
        // When the cut leaves almost nothing, there is no name to click.
        if let Some(artist_id) = track.artist_id(i)
            && right - left >= 16.0
        {
            let rect =
                Rect::from_min_max(pos2(left, subtitle.top()), pos2(right, subtitle.bottom()));
            let label = format!("Open artist “{name}”");
            if widgets::text_link(ui, rect, id.with(("artist", i)), &label).clicked() {
                detail::request(ui.ctx(), request_key(), artist_id, name);
            }
        }
        left += name_width;
        if left >= subtitle.right() {
            break;
        }
    }
}

/// Asks the app to open the artist `id` (with the name `name`), as a click on a name in [`links`]
/// does.
pub fn request(ctx: &Context, id: &str, name: &str) {
    detail::request(ctx, request_key(), id, name);
}

/// The artist whose name the user clicked (see [`links`]), if there is one: their ID and name.
pub fn take_request(ctx: &Context) -> Option<(String, String)> {
    detail::take_request(ctx, request_key())
}

#[derive(Default)]
pub struct ArtistView {
    panel: DetailPanel<Artist>,
    /// The menu of a release, which opens with a right click.
    menu: ContextMenu,
}

impl ArtistView {
    pub fn is_open(&self) -> bool {
        self.panel.is_open()
    }

    /// The ID and the name of the artist that shows, if there is one.
    pub fn open_id(&self) -> Option<(&str, &str)> {
        self.panel.open_id()
    }

    /// Shows the artist `id` (with the name `name`), and loads them again.
    pub fn open(&mut self, id: String, name: String, time: f64, actions: &mut Vec<Command>) {
        actions.push(Command::LoadArtist { id: id.clone() });
        self.menu.close();
        self.panel.open(id, name, time);
    }

    pub fn close(&mut self) {
        self.panel.close();
    }

    pub fn on_artist(&mut self, artist: Artist, time: f64) {
        self.panel.on_loaded(artist, time);
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
        accent: Color32,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let Some((open, interactive)) = self.panel.shown(&ctx, open_t) else {
            self.menu.close();
            return;
        };
        let rect = rect.translate(vec2(0.0, -8.0 * (1.0 - open_t)));
        let mut panel_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("artist")
                .max_rect(rect.expand2(vec2(8.0, 0.0))),
        );
        panel_ui.multiply_opacity(open_t);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), LIST_TITLE_HEIGHT));
        let detail = open.item.as_ref().map(count_line).unwrap_or_default();
        widgets::list_title(
            &panel_ui,
            header,
            Id::unique("artist-title"),
            &open.name,
            &format!("Albums and singles of “{}”", open.name),
            &detail,
            header.right(),
        );
        let list = widgets::list_rows(rect, header);
        let mut list_ui =
            panel_ui.new_child(UiBuilder::new().id_salt("artist-list").max_rect(list));
        list_ui.set_clip_rect(list.intersect(panel_ui.clip_rect()));
        if !interactive {
            list_ui.disable();
            self.menu.close();
        }
        release_list(&mut list_ui, open, &mut self.menu, textures);
        if let Some((choice, _, target)) = self.menu.show(&ctx, accent) {
            actions.extend(context_menu::command(choice, target));
        }
    }
}

/// "3 albums · 2 singles", for the title row.
fn count_line(artist: &Artist) -> String {
    let count = |kind| artist.releases.iter().filter(|r| r.kind == kind).count();
    let (albums, singles) = (count(ReleaseKind::Album), count(ReleaseKind::Single));
    match (albums, singles) {
        (0, 0) => String::new(),
        (albums, 0) => plural(albums, "album", "albums"),
        (0, singles) => plural(singles, "single", "singles"),
        (albums, singles) => format!(
            "{} · {}",
            plural(albums, "album", "albums"),
            plural(singles, "single", "singles")
        ),
    }
}

/// The releases of the artist, or a placeholder while they load (or if they cannot load).
fn release_list(
    ui: &mut Ui,
    open: &Opened<Artist>,
    menu: &mut ContextMenu,
    textures: &mut TextureCache,
) {
    let ctx = ui.ctx().clone();
    let time = ctx.input(|i| i.time);
    let rect = ui.max_rect();
    let Some(artist) = open.loaded(ui, rect, time, &PLACEHOLDERS, |artist| {
        !artist.releases.is_empty()
    }) else {
        return;
    };
    let releases = &artist.releases;

    let elapsed = anim::elapsed_since(&ctx, open.arrived_at);
    anim::keep_animating(&ctx, elapsed, ROW_APPEAR_DELAY * 10.0 + ROW_APPEAR_DURATION);
    let scrolled = egui::ScrollArea::vertical()
        .id_salt(("artist-scroll", open.id.as_str()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            let (area, _) = ui.allocate_exact_size(
                vec2(width, releases.len() as f32 * RELEASE_HEIGHT + 8.0),
                Sense::hover(),
            );
            for (i, release) in releases.iter().enumerate() {
                let appear = ease_out_cubic(anim::stagger(
                    elapsed,
                    i,
                    ROW_APPEAR_DELAY,
                    ROW_APPEAR_DURATION,
                ));
                let rect = Rect::from_min_size(
                    pos2(
                        area.left(),
                        area.top() + i as f32 * RELEASE_HEIGHT + (1.0 - appear) * 14.0,
                    ),
                    vec2(width, RELEASE_HEIGHT),
                );
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let cover = release
                    .cover
                    .as_deref()
                    .and_then(|url| textures.get(url, THUMB_PX, time));
                let row = card_row(
                    ui,
                    rect,
                    Id::unique(("artist-release", release.id.as_str())),
                    &release.name,
                    &release.summary(),
                    cover.as_ref(),
                    CardStyle {
                        alpha: appear,
                        highlighted: false,
                        draggable: true,
                        menu_open: menu.is_open_for(i),
                    },
                );
                if row.clicked {
                    album::request(&ctx, &release.id, &release.name);
                }
                if let Some(pos) = row.menu_at {
                    menu.open(&ctx, Target::album(release), i, pos, false);
                }
                if let Some(pointer) = row.drag_from {
                    dnd::start(
                        &ctx,
                        DragItem::Release(release.clone()),
                        DragOrigin::Artist,
                        rect,
                        pointer,
                    );
                }
            }
        });
    widgets::scroll_hints(ui, &scrolled, (0.0, 0.0));
}

pub struct CardActions {
    /// The user clicked it (or pushed Enter while it has the focus): open it.
    pub clicked: bool,
    /// The user started to drag it, from this point.
    pub drag_from: Option<Pos2>,
    /// The user right-clicked here: its menu opens at this point (see `context_menu`). Views
    /// without a menu ignore it.
    pub menu_at: Option<Pos2>,
}

#[derive(Clone, Copy)]
pub struct CardStyle {
    /// Fades the card in.
    pub alpha: f32,
    /// The user selected it with the keyboard from a different area (the search box). It looks
    /// like a row with the keyboard focus.
    pub highlighted: bool,
    pub draggable: bool,
    /// Its menu (right click) is open: it stays lit, as on hover.
    pub menu_open: bool,
}

/// An album, single or artist as a card similar to the track cards: its cover, its `title` and a
/// `subtitle` under it ("Album · 1975 · 12 songs"). It opens with a click. If `draggable`, the
/// user can drag it onto the queue.
pub fn card_row(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    title: &str,
    subtitle: &str,
    cover: Option<&Cover>,
    style: CardStyle,
) -> CardActions {
    let CardStyle {
        alpha,
        highlighted,
        draggable,
        menu_open,
    } = style;
    let ctx = ui.ctx().clone();
    let dragging = dnd::is_dragging(&ctx);
    let sense = if draggable {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let mut response = ui.interact(rect, id, sense);
    if !dragging {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    }
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::Label, true, format!("{title} — {subtitle}"));
        info.selected = highlighted.then_some(true);
        info
    });
    // Tab goes to the row and Enter opens it (egui changes Enter into a click).
    let focused = highlighted || response.has_focus();
    let hover = anim::tween_bool(
        &ctx,
        id.with("hover"),
        !dragging && (response.contains_pointer() || focused || menu_open),
        0.18,
    );
    let focus = anim::tween_bool(&ctx, id.with("focus"), focused, 0.12);

    let mut painter = ui.painter().clone();
    painter.multiply_opacity(alpha);
    let background = rect.shrink2(vec2(2.0, 3.0));
    if hover > 0.0 {
        painter.rect_filled(
            background,
            theme::RADIUS_MEDIUM,
            Color32::from_white_alpha((10.0 * hover) as u8),
        );
    }
    widgets::focus_ring(&painter, background, theme::RADIUS_MEDIUM, focus);

    let thumb = Rect::from_min_size(
        pos2(
            rect.left() + widgets::ROW_COVER_INSET,
            rect.center().y - RELEASE_COVER / 2.0,
        ),
        vec2(RELEASE_COVER, RELEASE_COVER),
    );
    widgets::paint_cover(ui, thumb, cover, theme::RADIUS_SMALL, alpha);
    let text_left = thumb.right() + widgets::ROW_TEXT_GAP;
    let text_width = (rect.right() - 12.0 - text_left).max(40.0);
    let name = widgets::single_line(
        &painter,
        title,
        theme::regular(theme::FONT_BODY),
        theme::TEXT,
        text_width,
    );
    let sub = widgets::single_line(
        &painter,
        subtitle,
        theme::regular(theme::FONT_SMALL),
        theme::TEXT_DIM,
        text_width,
    );
    let block = name.size().y + 3.0 + sub.size().y;
    let top = rect.center().y - block / 2.0;
    let name_rect = Rect::from_min_size(pos2(text_left, top), name.size());
    let sub_rect = Rect::from_min_size(pos2(text_left, name_rect.bottom() + 3.0), sub.size());
    widgets::full_text_tooltip(ui, name_rect, id.with("title"), &name, title);
    widgets::full_text_tooltip(ui, sub_rect, id.with("subtitle"), &sub, subtitle);
    painter.galley(name_rect.min, name, theme::TEXT);
    painter.galley(sub_rect.min, sub, theme::TEXT_DIM);

    // The point where the button went down. Thus the row stays under the pointer at the point
    // where the user took it.
    let drag_from = response
        .drag_started()
        .then(|| {
            ctx.input(|i| i.pointer.press_origin())
                .or(response.interact_pointer_pos())
        })
        .flatten();
    let menu_at = (!dragging && response.secondary_clicked())
        .then(|| ctx.input(|i| i.pointer.interact_pos()))
        .flatten();
    CardActions {
        clicked: response.clicked(),
        drag_from,
        menu_at,
    }
}
