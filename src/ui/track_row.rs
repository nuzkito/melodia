//! A track in a list (queue, search results, a playlist, an album): cover, title, "artist ·
//! album" with links, and the duration. On hover, the ▶ / ♥ / ＋ / ✕ buttons replace the duration
//! (see «Track row» in `docs/design.md`).

use egui::{
    Color32, CursorIcon, Id, Key, PointerButton, Pos2, Rect, Role, Sense, Ui, UiBuilder, Vec2,
    WidgetInfo, pos2, vec2,
};

use super::album;
use super::anim::{self, ease_out_cubic, lerp_color};
use super::artist;
use super::dnd;
use super::icons;
use super::liked;
use super::textures::Cover;
use super::theme;
use super::widgets::{self, focus_ring, icon_button, paint_cover, single_line};
use crate::model::Track;
use crate::util::format_duration;

pub use super::widgets::{ROW_COVER, ROW_COVER_INSET, ROW_HEIGHT};
/// The distance between the centers of two buttons (▶, ♥, ＋, ✕) next to each other.
const BUTTON_GAP: f32 = 34.0;
/// The distance from the center of the ♥ to the end of the text: half the heart and a gap.
const HEART_ROOM: f32 = 18.0;
/// The equalizer left of the title of the current track.
const EQUALIZER_SIZE: Vec2 = vec2(12.0, 11.0);
/// The distance that the title of the current track moves right to make space for it.
const EQUALIZER_ROOM: f32 = 20.0;

#[derive(Clone, Copy)]
pub struct RowStyle {
    pub alpha: f32,
    /// The track that plays now: accent tint and an equalizer before the title.
    pub current: bool,
    /// The motion (0..1) of that equalizer.
    pub playing: f32,
    /// The ▶ button pauses (the current track while it plays).
    pub pause_button: bool,
    /// The user can drag it (into the queue, or in the queue).
    pub draggable: bool,
    /// Shows the ＋ (add to queue) button.
    pub can_add: bool,
    /// Shows the ✕ button on hover, which removes the track from this list («the queue»).
    pub remove_from: Option<&'static str>,
    /// The app already sent it to Spotify: a padlock shows that it cannot move.
    pub locked: bool,
    /// The seconds after the user added this track to the queue, for the ✓.
    pub added: Option<f32>,
    /// The user selected it with the keyboard from a different area (the search box). It looks
    /// like a row with the keyboard focus.
    pub highlighted: bool,
    /// Its menu (right click) is open: it stays lit, as on hover.
    pub menu_open: bool,
    pub accent: Color32,
    pub time: f64,
}

#[derive(Default)]
pub struct RowActions {
    /// The user clicked the ▶ button, double-clicked the row, or pushed Enter while the row has
    /// the keyboard focus.
    pub play: bool,
    pub add: bool,
    pub remove: bool,
    /// The user started to drag the row, from this point.
    pub drag_from: Option<Pos2>,
    /// The user right-clicked here: its menu opens at this point (see `context_menu`). Views
    /// without a menu ignore it.
    pub menu_at: Option<Pos2>,
}

pub fn track_row(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    track: &Track,
    cover: Option<&Cover>,
    style: RowStyle,
) -> RowActions {
    let ctx = ui.ctx().clone();
    let RowStyle { accent, time, .. } = style;
    let dragging = dnd::is_dragging(&ctx);
    let sense = if style.draggable {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let mut response = ui.interact(rect, id, sense);
    if style.draggable && !dragging {
        response = response.on_hover_cursor(CursorIcon::Grab);
    }
    if style.locked {
        response =
            response.on_hover_text("Already set up in Spotify: it plays next and can't be moved.");
    }
    let subtitle = track.subtitle();
    // Tab goes to the row itself. With its buttons, a highlighted row looks the same.
    let focused = style.highlighted || response.has_focus();
    response.widget_info(|| {
        let mut info =
            WidgetInfo::labeled(Role::Label, true, format!("{} — {}", track.name, subtitle));
        info.selected = style.highlighted.then_some(true);
        info
    });
    // `contains_pointer` (not `hovered`), so that the row stays highlighted while the pointer is
    // over its own buttons. Nothing responds during a drag. The keyboard focus shows the same
    // buttons as the pointer.
    let hover = anim::tween_bool(
        &ctx,
        id.with("hover"),
        !dragging && (response.contains_pointer() || focused || style.menu_open),
        0.18,
    );
    let focus = anim::tween_bool(&ctx, id.with("focus"), focused, 0.12);

    let mut painter = ui.painter().clone();
    painter.multiply_opacity(style.alpha);
    let background = rect.shrink2(vec2(2.0, 3.0));
    if style.current {
        painter.rect_filled(
            background,
            theme::RADIUS_MEDIUM,
            accent.gamma_multiply(0.12),
        );
    }
    if hover > 0.0 {
        painter.rect_filled(
            background,
            theme::RADIUS_MEDIUM,
            Color32::from_white_alpha((10.0 * hover) as u8),
        );
    }
    focus_ring(&painter, background, theme::RADIUS_MEDIUM, focus);

    let thumb = Rect::from_min_size(
        pos2(
            rect.left() + ROW_COVER_INSET,
            rect.center().y - ROW_COVER / 2.0,
        ),
        vec2(ROW_COVER, ROW_COVER),
    );
    // Spotify's guidelines tell that nothing must cover the artwork. Thus the play button goes
    // with the other buttons on the right, and the equalizer of the current track goes left of its
    // title.
    let mut thumb_ui = ui.new_child(UiBuilder::new().max_rect(thumb));
    thumb_ui.multiply_opacity(style.alpha);
    paint_cover(&thumb_ui, thumb, cover, theme::RADIUS_SMALL, 1.0);
    let alpha = style.alpha;
    if style.current && style.playing > 0.0 {
        anim::request_frame_after(&ctx, std::time::Duration::from_millis(33));
    }

    // On hover, the ▶ / ＋ / ✕ appear in place of the duration, which fades out. With ＋ and ✕,
    // the ✕ goes last. The ♥ goes left of them, and never over the duration, because liked tracks
    // always show it. The ▶ goes first. Slots count from the right edge.
    let both = style.can_add && style.remove_from.is_some();
    let can_like = liked::can_like(track);
    let edit_slots = f32::from(u8::from(style.can_add) + u8::from(style.remove_from.is_some()));
    let heart_slot = edit_slots.max(1.0);
    let play_slot = if can_like {
        heart_slot + 1.0
    } else {
        edit_slots
    };
    let slots = play_slot + 1.0;
    let buttons_width = 40.0 + (slots - 1.0) * BUTTON_GAP;
    let duration_width: f32 = 46.0;
    let trailing = duration_width.max(buttons_width + 6.0);
    let slot_x = |slot: f32| rect.right() - 24.0 - slot * BUTTON_GAP;
    // The ✓ of a new track keeps the buttons visible for some time.
    let check = style.added.map_or(0.0, |t| {
        if t < 1.4 {
            ease_out_cubic(t / 0.25)
        } else {
            1.0 - ease_out_cubic((t - 1.4) / 0.25)
        }
    });
    let buttons_visible = hover.max(check);
    let text_left = thumb.right() + widgets::ROW_TEXT_GAP;
    // Without hover, only the ♥ (of liked tracks) and the duration show, so the text goes to the
    // ♥. On hover, the text makes space for the other buttons. The padlock of a locked row always
    // shows, left of the ▶.
    let text_right = if hover > 0.0 || buttons_visible > 0.0 || style.locked {
        rect.right() - 12.0 - trailing
    } else if can_like {
        slot_x(heart_slot) - HEART_ROOM
    } else {
        rect.right() - 12.0 - duration_width
    };
    let text_width = (text_right - text_left).max(40.0);
    let title_color = if style.current {
        lerp_color(theme::TEXT, accent, 0.85)
    } else {
        theme::TEXT
    };
    // The equalizer of the current track goes before its title.
    let title_left = if style.current {
        text_left + EQUALIZER_ROOM
    } else {
        text_left
    };
    let title = single_line(
        &painter,
        &track.name,
        theme::regular(theme::FONT_BODY),
        title_color,
        (text_width - (title_left - text_left) - widgets::explicit_room(track.explicit)).max(20.0),
    );
    let sub = single_line(
        &painter,
        &subtitle,
        theme::regular(theme::FONT_SMALL),
        theme::TEXT_DIM,
        text_width,
    );
    let block = title.size().y + 3.0 + sub.size().y;
    let top = rect.center().y - block / 2.0;
    let title_rect = Rect::from_min_size(pos2(title_left, top), title.size());
    let sub_rect = Rect::from_min_size(pos2(text_left, title_rect.bottom() + 3.0), sub.size());
    if style.current {
        let bars = Rect::from_min_size(
            pos2(text_left, title_rect.center().y - EQUALIZER_SIZE.y / 2.0),
            EQUALIZER_SIZE,
        );
        icons::equalizer(&painter, bars, time, style.playing, accent);
    }
    widgets::full_text_tooltip(ui, title_rect, id.with("title"), &title, &track.name);
    painter.galley(title_rect.min, title, title_color);
    if track.explicit {
        widgets::explicit_badge(
            ui,
            &painter,
            title_rect.right() + widgets::EXPLICIT_BADGE_GAP,
            title_rect.center().y,
            id,
        );
    }
    painter.galley(sub_rect.min, sub.clone(), theme::TEXT_DIM);
    // Not during a drag: nothing responds then.
    if !dragging {
        artist::links(ui, id, track, sub_rect, theme::regular(theme::FONT_SMALL));
        album::link(ui, id, track, sub_rect, theme::regular(theme::FONT_SMALL));
        // After the links, so that it is over them and also shows there.
        widgets::full_text_tooltip(ui, sub_rect, id.with("subtitle"), &sub, &subtitle);
    }

    let duration = single_line(
        &painter,
        &format_duration(track.duration_ms),
        theme::regular(theme::FONT_SMALL),
        theme::TEXT_FAINT,
        duration_width,
    );
    let duration_right = rect.right() - 16.0;
    let duration_left = duration_right - duration.size().x;
    if buttons_visible < 1.0 {
        let mut faded = painter.clone();
        faded.multiply_opacity(1.0 - buttons_visible);
        faded.galley(
            pos2(duration_left, rect.center().y - duration.size().y / 2.0),
            duration,
            theme::TEXT_FAINT,
        );
    }
    if style.locked {
        // Left of the ▶ when it does not go over the duration.
        let x = if play_slot > 0.0 {
            slot_x(play_slot) - 26.0
        } else {
            duration_left - 14.0
        };
        icons::lock(&painter, pos2(x, rect.center().y), 12.0, theme::TEXT_FAINT);
    }

    let mut actions = RowActions {
        play: response.double_clicked()
            || (response.has_focus() && ctx.input(|i| i.key_pressed(Key::Enter))),
        add: false,
        remove: false,
        drag_from: None,
        menu_at: None,
    };
    // At any point on the row, also its links and buttons (they take the click themselves).
    let right_click = ctx.input(|i| i.pointer.button_clicked(PointerButton::Secondary));
    if !dragging && (response.secondary_clicked() || right_click && response.contains_pointer()) {
        actions.menu_at = ctx.input(|i| i.pointer.interact_pos());
    }
    if response.drag_started() {
        // The point where the button went down. Thus the row stays under the pointer at the point
        // where the user took it.
        actions.drag_from = ctx
            .input(|i| i.pointer.press_origin())
            .or(response.interact_pointer_pos());
    }
    let label = if style.pause_button {
        format!("Pause {}", track.name)
    } else {
        format!("Play {}", track.name)
    };
    let play_t = if style.pause_button { 0.0 } else { 1.0 };
    let play = icon_button(
        ui,
        pos2(slot_x(play_slot), rect.center().y),
        30.0,
        id.with("play"),
        &label,
        true,
        |p, c, color| icons::play_pause(p, c, 15.0, play_t, color.gamma_multiply(hover * alpha)),
    );
    actions.play |= play.clicked();
    if style.can_add {
        if style.added.is_some_and(|t| t < 1.7) {
            ctx.request_repaint();
        }
        let visible = buttons_visible;
        let x = slot_x(if both { 1.0 } else { 0.0 });
        let add = icon_button(
            ui,
            pos2(x, rect.center().y),
            30.0,
            id.with("add"),
            &format!("Add {} to the queue", track.name),
            true,
            |p, c, color| {
                let color = lerp_color(color, accent, check);
                icons::plus_to_check(p, c, 15.0, check, color.gamma_multiply(visible * alpha));
            },
        );
        actions.add = add.clicked();
    }
    if let Some(from) = style.remove_from
        && !dragging
    {
        let remove = icon_button(
            ui,
            pos2(slot_x(0.0), rect.center().y),
            30.0,
            id.with("remove"),
            &format!("Remove {} from {from}", track.name),
            true,
            |p, c, color| icons::close(p, c, 18.0, color.gamma_multiply(hover * alpha)),
        );
        actions.remove = remove.clicked();
    }
    if can_like {
        let center = pos2(slot_x(heart_slot), rect.center().y);
        liked::heart_button(ui, center, id, track, hover, style.alpha, accent);
    }
    actions
}
