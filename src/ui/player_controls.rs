//! Playback widgets of the card of the current track in the queue: the seek bar, the transport
//! buttons and the volume slider. Each widget keeps its own drag state.

use std::time::Instant;

use egui::{
    Align, Color32, CursorIcon, Id, Painter, Pos2, Rect, Role, Sense, Stroke, Ui, Vec2, WidgetInfo,
    pos2, vec2,
};

use super::anim::{self, ease_in_out_cubic, lerp, lerp_color};
use super::icons;
use super::theme;
use super::widgets::{custom_button, icon_button, paint_galley, single_line};
use crate::backend::Command;
use crate::model::PlaybackSnapshot;
use crate::util::format_duration;

/// After a change, the seek bar and the volume slider show the new value until Spotify reports
/// it, or for this number of seconds.
const SENT_VALUE_HOLD: f64 = 2.5;

#[derive(Default)]
pub struct SeekBar {
    /// The position (0..1) under the pointer during a drag.
    drag: Option<f32>,
    /// The last position sent, and the time. It shows until Spotify reports it.
    sent: Option<(f32, f64)>,
}

impl SeekBar {
    /// The bar along the top of `rect`, with the elapsed and total times under it. A click or a
    /// drag seeks.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        id: Id,
        playback: &PlaybackSnapshot,
        accent: Color32,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        let duration = playback.track.as_ref().map_or(0, |t| t.duration_ms);
        let enabled = duration > 0;
        let bar_y = rect.top() + 10.0;
        let hit = Rect::from_min_max(
            pos2(rect.left() - 6.0, bar_y - 10.0),
            pos2(rect.right() + 6.0, bar_y + 10.0),
        );
        // Not a Tab stop: the keyboard cannot seek yet.
        let sense = if enabled {
            Sense::CLICK | Sense::DRAG
        } else {
            Sense::hover()
        };
        let mut response = ui.interact(hit, id, sense);
        if enabled {
            response = response.on_hover_cursor(CursorIcon::PointingHand);
        }
        let fraction_at = |pos: Pos2| ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let pointer = response
            .interact_pointer_pos()
            .or(response.hover_pos())
            .map(fraction_at);

        if enabled && (response.dragged() || response.is_pointer_button_down_on()) {
            self.drag = pointer.or(self.drag);
        }
        if enabled && (response.drag_stopped() || response.clicked()) {
            if let Some(fraction) = pointer.or(self.drag) {
                actions.push(Command::Seek {
                    position_ms: (f64::from(fraction) * duration as f64) as u64,
                });
                self.sent = Some((fraction, time));
            }
            self.drag = None;
        } else if !response.dragged() && !response.is_pointer_button_down_on() {
            self.drag = None;
        }

        let actual = if enabled {
            playback.progress_at(Instant::now()) as f32 / duration as f32
        } else {
            0.0
        };
        if let Some((fraction, at)) = self.sent
            && (time - at > SENT_VALUE_HOLD || (actual - fraction).abs() < 0.01)
        {
            self.sent = None;
        }
        let fraction = self
            .drag
            .or(self.sent.map(|(f, _)| f))
            .unwrap_or(actual)
            .clamp(0.0, 1.0);

        let active = enabled && (response.hovered() || self.drag.is_some());
        let hover = anim::tween_bool(&ctx, id.with("hover"), active, 0.18);
        let thickness = lerp(4.0, 6.0, hover);
        let painter = ui.painter();
        let track_rect = Rect::from_min_max(
            pos2(rect.left(), bar_y - thickness / 2.0),
            pos2(rect.right(), bar_y + thickness / 2.0),
        );
        painter.rect_filled(track_rect, thickness / 2.0, Color32::from_white_alpha(28));
        let fill_right = lerp(rect.left(), rect.right(), fraction);
        let fill = Rect::from_min_max(
            track_rect.min,
            pos2(fill_right.max(rect.left() + thickness), track_rect.max.y),
        );
        if enabled {
            painter.rect_filled(
                fill,
                thickness / 2.0,
                lerp_color(theme::TEXT, accent, hover),
            );
        }
        let knob = 7.0 * hover;
        if knob > 0.5 {
            painter.circle_filled(
                pos2(fill_right, bar_y + 1.0),
                knob + 2.5,
                Color32::from_black_alpha(70),
            );
            painter.circle_filled(pos2(fill_right, bar_y), knob, theme::TEXT);
        }

        let elapsed = format_duration((f64::from(fraction) * duration as f64) as u64);
        let total = format_duration(duration);
        let times_y = bar_y + 12.0;
        let font = theme::regular(theme::FONT_SMALL);
        let elapsed_galley = single_line(
            painter,
            &elapsed,
            font.clone(),
            theme::TEXT_FAINT,
            rect.width() / 2.0,
        );
        paint_galley(
            painter,
            elapsed_galley,
            rect.left(),
            times_y,
            Align::Min,
            1.0,
        );
        let total_galley =
            single_line(painter, &total, font, theme::TEXT_FAINT, rect.width() / 2.0);
        paint_galley(
            painter,
            total_galley,
            rect.right(),
            times_y,
            Align::Max,
            1.0,
        );

        // A preview bubble with the time under the pointer.
        let bubble = anim::tween_bool(
            &ctx,
            id.with("bubble"),
            enabled && response.hovered() && self.drag.is_none(),
            0.15,
        );
        if bubble > 0.01
            && let Some(pos) = response.hover_pos()
        {
            let text = format_duration((f64::from(fraction_at(pos)) * duration as f64) as u64);
            let galley = single_line(
                painter,
                &text,
                theme::regular(theme::FONT_SMALL),
                theme::TEXT,
                80.0,
            );
            let size = galley.size() + vec2(14.0, 8.0);
            let center = pos2(
                pos.x
                    .clamp(rect.left() + size.x / 2.0, rect.right() - size.x / 2.0),
                bar_y - 20.0 - (1.0 - bubble) * 4.0,
            );
            let bubble_rect = Rect::from_center_size(center, size);
            let mut painter = painter.clone();
            painter.multiply_opacity(bubble);
            painter.rect_filled(bubble_rect, theme::RADIUS_SMALL, theme::SURFACE_HOVER);
            painter.galley(
                bubble_rect.center() - galley.size() / 2.0,
                galley,
                theme::TEXT,
            );
        }

        response.widget_info(|| {
            WidgetInfo::labeled(
                Role::Slider,
                enabled,
                format!("Position {elapsed} of {total}"),
            )
        });
    }
}

/// Speaker button (mute) and volume slider.
#[derive(Default)]
pub struct VolumeSlider {
    /// The volume (0..1) under the pointer during a drag.
    drag: Option<f32>,
    /// The target volume (0..1) of the wheel, not rounded, so that small steps of a smooth scroll
    /// add up.
    wheel: Option<f32>,
    /// The last volume sent, and the time. It shows until Spotify reports it.
    sent: Option<(u8, f64)>,
    /// The volume to go back to when the sound comes on again.
    before_mute: Option<u8>,
}

/// The wheel scroll points from silence to full volume. A notch of the wheel (40 points) changes
/// the volume by 5 %.
const WHEEL_SPAN: f32 = 800.0;

/// The vertical points scrolled with the wheel in this frame while the user holds Ctrl (⌘ on
/// macOS). Positive is up.
fn ctrl_wheel(ui: &Ui) -> f32 {
    let line = ui.ctx().options(|o| o.input_options.line_scroll_speed);
    ui.input(|i| {
        let page = i.viewport_rect().height();
        i.events
            .iter()
            .filter_map(|event| match event {
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if modifiers.command => Some(match unit {
                    egui::MouseWheelUnit::Point => delta.y,
                    egui::MouseWheelUnit::Line => delta.y * line,
                    egui::MouseWheelUnit::Page => delta.y * page,
                }),
                _ => None,
            })
            .sum()
    })
}

impl VolumeSlider {
    /// Narrower than this, only the speaker shows. There is no space for a usable slider, and
    /// Ctrl + wheel still changes the volume.
    pub const MIN_WIDTH: f32 = 96.0;

    /// Speaker button (mute) and volume slider. A drag, a click or Ctrl + wheel over it changes
    /// it. The new value shows until Spotify reports it. In a `rect` narrower than
    /// [`Self::MIN_WIDTH`], only the speaker shows, centered.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        id: Id,
        playback: &PlaybackSnapshot,
        accent: Color32,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        let device = playback.device.as_ref();
        let enabled = device.is_some_and(|d| d.supports_volume);
        let reported = device.and_then(|d| d.volume_percent);
        let compact = rect.width() < Self::MIN_WIDTH;

        let track = Rect::from_min_max(
            pos2(rect.left() + 36.0, rect.center().y),
            pos2(rect.right() - 8.0, rect.center().y),
        );
        let response = (!compact).then(|| {
            let hit = Rect::from_min_max(
                pos2(track.left() - 6.0, track.top() - 10.0),
                pos2(track.right() + 6.0, track.bottom() + 10.0),
            );
            // Not a Tab stop: Ctrl+↑/↓ change the volume.
            let sense = if enabled {
                Sense::CLICK | Sense::DRAG
            } else {
                Sense::hover()
            };
            let response = ui.interact(hit, id, sense);
            if enabled {
                response.on_hover_cursor(CursorIcon::PointingHand)
            } else {
                response.on_hover_text("This device doesn't allow changing the volume from here")
            }
        });
        let fraction_at = |pos: Pos2| ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
        let pressed = response
            .as_ref()
            .is_some_and(|r| r.dragged() || r.is_pointer_button_down_on());

        if let Some((sent, at)) = self.sent
            && self.drag.is_none()
            && (time - at > SENT_VALUE_HOLD || reported == Some(sent))
        {
            self.sent = None;
        }
        let shown = |view: &Self| {
            view.drag
                .or(view.sent.map(|(p, _)| f32::from(p) / 100.0))
                .or(reported.map(|p| f32::from(p) / 100.0))
                .unwrap_or(0.0)
        };

        let mut wanted = None;
        if enabled
            && let Some(response) = &response
            && let Some(pos) = response.interact_pointer_pos()
        {
            let fraction = fraction_at(pos);
            if pressed {
                self.drag = Some(fraction);
            }
            if pressed || response.clicked() || response.drag_stopped() {
                wanted = Some(fraction);
            }
        }
        if !pressed {
            self.drag = None;
        }

        // Ctrl + wheel over the control increases or decreases the volume. The plain wheel
        // continues to scroll the list under it, so the volume never changes by accident. egui
        // changes Ctrl + wheel into zoom (which this app does not use). Thus the code reads the
        // raw wheel events, not the scroll delta.
        let over = enabled && ui.is_enabled() && ui.rect_contains_pointer(rect);
        let scrolled = if over { ctrl_wheel(ui) } else { 0.0 };
        if scrolled != 0.0 && self.drag.is_none() {
            let from = self.wheel.unwrap_or_else(|| shown(self));
            let to = (from + scrolled / WHEEL_SPAN).clamp(0.0, 1.0);
            self.wheel = Some(to);
            wanted = Some(to);
        } else if !over {
            self.wheel = None;
        }

        let value = shown(self);
        let level = anim::tween(&ctx, id.with("level"), value, 0.12);
        let muted = value < 0.005;
        let icon_center = if compact {
            rect.center()
        } else {
            pos2(rect.left() + 14.0, rect.center().y)
        };
        let icon = icon_button(
            ui,
            icon_center,
            30.0,
            id.with("mute"),
            if muted { "Unmute" } else { "Mute" },
            enabled,
            |painter, center, color| icons::volume(painter, center, 18.0, level, color),
        );
        if icon.clicked() {
            wanted = Some(if muted {
                f32::from(self.before_mute.take().unwrap_or(50)) / 100.0
            } else {
                self.before_mute = Some((value * 100.0).round() as u8);
                0.0
            });
            self.wheel = None;
        }

        if let Some(fraction) = wanted {
            let percent = (fraction * 100.0).round() as u8;
            if self.sent.map(|(p, _)| p) != Some(percent) {
                actions.push(Command::SetVolume { percent });
            }
            self.sent = Some((percent, time));
        }

        let value = shown(self);
        match response {
            Some(response) => self.paint_slider(ui, &response, track, id, enabled, accent, value),
            None => {
                // Without the slider, the percentage floats over the speaker while the pointer is
                // on it.
                let bubble = anim::tween_bool(&ctx, id.with("bubble"), over, 0.15);
                paint_percent(ui.painter(), icon_center - vec2(0.0, 30.0), value, bubble);
            }
        }
    }

    /// The bar of the slider, its knob and, during a drag, the percentage.
    #[expect(clippy::too_many_arguments)]
    fn paint_slider(
        &self,
        ui: &Ui,
        response: &egui::Response,
        track: Rect,
        id: Id,
        enabled: bool,
        accent: Color32,
        value: f32,
    ) {
        let ctx = ui.ctx();
        let active = enabled && (response.hovered() || self.drag.is_some());
        let hover = anim::tween_bool(ctx, id.with("hover"), active, 0.18);
        let thickness = lerp(4.0, 6.0, hover);
        let mut painter = ui.painter().clone();
        if !enabled {
            painter.multiply_opacity(0.35);
        }
        let bar = Rect::from_min_max(
            pos2(track.left(), track.center().y - thickness / 2.0),
            pos2(track.right(), track.center().y + thickness / 2.0),
        );
        painter.rect_filled(bar, thickness / 2.0, Color32::from_white_alpha(28));
        let fill_right = lerp(track.left(), track.right(), value);
        if value > 0.0 {
            let fill = Rect::from_min_max(
                bar.min,
                pos2(fill_right.max(bar.left() + thickness), bar.max.y),
            );
            painter.rect_filled(
                fill,
                thickness / 2.0,
                lerp_color(theme::TEXT, accent, hover),
            );
        }
        let knob = 6.0 * hover;
        if knob > 0.5 {
            let center = pos2(fill_right, track.center().y);
            painter.circle_filled(
                center + vec2(0.0, 1.0),
                knob + 2.0,
                Color32::from_black_alpha(70),
            );
            painter.circle_filled(center, knob, theme::TEXT);
        }
        // The percentage floats over the knob during a drag.
        let bubble = anim::tween_bool(ctx, id.with("bubble"), self.drag.is_some(), 0.15);
        paint_percent(
            &painter,
            pos2(fill_right, track.center().y - 20.0),
            value,
            bubble,
        );
        let percent = (value * 100.0).round();
        response.widget_info(|| WidgetInfo::slider(enabled, f64::from(percent), "Volume"));
    }
}

/// The volume (0..1) as a percentage in a small bubble with its center on `center`. It moves up
/// into position as `alpha` goes from 0 to 1.
fn paint_percent(painter: &Painter, center: Pos2, value: f32, alpha: f32) {
    if alpha <= 0.01 {
        return;
    }
    let text = format!("{} %", (value * 100.0).round());
    let galley = single_line(
        painter,
        &text,
        theme::regular(theme::FONT_SMALL),
        theme::TEXT,
        60.0,
    );
    let size = galley.size() + vec2(12.0, 6.0);
    let bubble_rect = Rect::from_center_size(center - vec2(0.0, (1.0 - alpha) * 4.0), size);
    let mut painter = painter.clone();
    painter.multiply_opacity(alpha);
    painter.rect_filled(bubble_rect, theme::RADIUS_SMALL, theme::SURFACE_HOVER);
    painter.galley(
        bubble_rect.center() - galley.size() / 2.0,
        galley,
        theme::TEXT,
    );
}

/// The seconds for the play/pause button to spring after a toggle.
const TOGGLE_DURATION: f32 = 0.55;

/// Previous, play/pause and next around `center`. `scale` 1 is the full size. The ids come from
/// `id`.
pub fn transport(
    ui: &mut Ui,
    center: Pos2,
    scale: f32,
    id: Id,
    playback: &PlaybackSnapshot,
    actions: &mut Vec<Command>,
) {
    let id_salt = id;
    let ctx = ui.ctx().clone();
    let has_track = playback.track.is_some();
    let playing = playback.is_playing;

    for (forward, label, command) in [
        (false, "Previous", Command::Previous),
        (true, "Next", Command::Next),
    ] {
        let id = id_salt.with(label);
        let nudge = anim::since_trigger(&ctx, id.with("nudge"), false);
        anim::keep_animating(&ctx, nudge, 0.32);
        let dir = if forward { 1.0 } else { -1.0 };
        let button_center = center + vec2(dir * 80.0 * scale, 0.0);
        let response = icon_button(
            ui,
            button_center,
            46.0 * scale,
            id,
            label,
            has_track,
            |painter, c, color| {
                let offset = dir * 5.0 * anim::bump(nudge / 0.32);
                icons::skip(painter, c + vec2(offset, 0.0), 22.0 * scale, forward, color);
            },
        );
        if response.clicked() {
            anim::since_trigger(&ctx, id.with("nudge"), true);
            actions.push(command);
        }
    }

    let id = id_salt.with("play-pause");
    let morph = anim::tween_with(
        &ctx,
        id.with("morph"),
        if playing { 0.0 } else { 1.0 },
        0.28,
        ease_in_out_cubic,
    );
    // Each click squashes the button and lets it spring back (a damped wobble). A ring spreads out
    // of it, and the glyph pops.
    let toggled = anim::since_trigger(&ctx, id.with("toggled"), false);
    anim::keep_animating(&ctx, toggled, TOGGLE_DURATION);
    let progress = (toggled / TOGGLE_DURATION).clamp(0.0, 1.0);
    let spring = if progress < 1.0 {
        -(1.0 - progress).powi(2) * (progress * 3.5 * std::f32::consts::TAU).sin() * 0.2
    } else {
        0.0
    };
    let glyph_pop = 1.0 + 0.3 * anim::bump(progress * 2.0).min(1.0) * (1.0 - progress);
    let rect = Rect::from_center_size(center, Vec2::splat(68.0 * scale));
    let label = if playing { "Pause" } else { "Play" };
    let response = custom_button(
        ui,
        rect,
        id,
        label,
        has_track,
        |painter: &Painter, rect, hover, press| {
            let base = 31.0 * scale;
            let radius = base * (1.0 + 0.05 * hover - 0.07 * press + spring);
            if progress < 1.0 {
                let ring = anim::ease_out_cubic(progress);
                painter.circle_stroke(
                    rect.center(),
                    base * (1.0 + 0.7 * ring),
                    Stroke::new(
                        3.0 * scale * (1.0 - ring),
                        theme::TEXT.gamma_multiply(0.6 * (1.0 - ring)),
                    ),
                );
            }
            painter.circle_filled(
                rect.center() + vec2(0.0, 4.0 * scale),
                radius + 2.0,
                Color32::from_black_alpha(60),
            );
            painter.circle_filled(rect.center(), radius, theme::TEXT);
            icons::play_pause(
                painter,
                rect.center(),
                radius * 0.8 * glyph_pop,
                morph,
                theme::BG,
            );
        },
    );
    if response.clicked() {
        anim::since_trigger(&ctx, id.with("toggled"), true);
        actions.push(if playing {
            Command::Pause
        } else {
            Command::Resume
        });
    }
}
