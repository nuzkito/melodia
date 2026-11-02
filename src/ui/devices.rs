//! The Spotify Connect device picker: a button in the title bar, just left of the «☰» menu. It
//! opens a popover with the player of this app and the other devices.

use std::time::Duration;

use egui::{
    Align, Align2, Color32, CursorIcon, Id, Key, Margin, OpenUrl, Order, Painter, Pos2, Rect,
    Sense, Stroke, Ui, Vec2, pos2, vec2,
};

use super::anim::{self, lerp_color};
use super::backdrop;
use super::icons;
use super::theme;
use super::widgets::{custom_button, paint_galley, single_line, spaced_line, text_button};
use crate::backend::{Command, WebPlayerStatus};
use crate::model::{Device, PlaybackSnapshot};
use crate::webplayer::DEVICE_NAME;

/// The interval between reads of the device list while the popover is open (a device can appear
/// in the meantime). When it is closed, the app does not read the list, because nothing shows it.
/// If the popover opens again sooner, it shows the last list and does not ask again.
const REFRESH_EVERY: Duration = Duration::from_secs(10);

/// The maximum width of the name of the other device on the button. The button cuts longer names
/// with «…».
const DEVICE_NAME_MAX_WIDTH: f32 = 180.0;
/// The end of the name, from the left edge of the square of the icon (the icon is centered in it,
/// so this leaves a small gap before it).
const NAME_GAP: f32 = 8.0;
/// The space left of the name, in the button.
const NAME_PADDING: f32 = 12.0;

#[derive(Default)]
pub struct DevicesView {
    popover_open: bool,
    popover_size: Vec2,
    /// The time of the last request for the device list (egui time), if there was one.
    refreshed_at: Option<f64>,
}

pub struct DevicesData<'a> {
    pub playback: &'a PlaybackSnapshot,
    /// The different device where the music plays, while it is not in this app.
    pub elsewhere: Option<&'a Device>,
    pub devices: &'a [Device],
    pub web_player: &'a WebPlayerStatus,
    pub accent: Color32,
}

impl DevicesView {
    /// The device button, drawn as the window buttons in `rect`. It opens the device popover.
    /// While the music is on a different device, the button grows to the left to show the name of
    /// that device. Returns the rect of the button.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        data: &DevicesData<'_>,
        actions: &mut Vec<Command>,
    ) -> Rect {
        let ctx = ui.ctx().clone();
        let active = data
            .playback
            .device
            .as_ref()
            .or(data.elsewhere)
            .or_else(|| data.devices.iter().find(|d| d.is_active));
        let web_device = match data.web_player {
            WebPlayerStatus::Ready { device_id } => Some(device_id.as_str()),
            _ => None,
        };
        let here =
            active.is_some_and(|d| d.name == DEVICE_NAME || Some(d.id.as_str()) == web_device);
        let (label, kind) = match active {
            Some(device) if here => ("This app".to_owned(), device.kind.clone()),
            Some(device) => (device.name.clone(), device.kind.clone()),
            None => ("Choose a device".to_owned(), String::new()),
        };
        let open = anim::tween_bool(&ctx, Id::unique("devices-open"), self.popover_open, 0.2);
        // The name of the different device where the music is, left of the icon.
        let name = active.filter(|_| !here).map(|device| {
            single_line(
                ui.painter(),
                &device.name,
                theme::regular(theme::FONT_SMALL),
                theme::TEXT_DIM,
                DEVICE_NAME_MAX_WIDTH,
            )
        });
        let button = match &name {
            Some(name) => rect.with_min_x(rect.left() + NAME_GAP - name.size().x - NAME_PADDING),
            None => rect,
        };
        let response = custom_button(
            ui,
            button,
            Id::unique("device-button"),
            &format!("Device: {label}"),
            true,
            |painter, button, hover, press| {
                let highlight = hover.max(open);
                painter.rect_filled(
                    button,
                    0.0,
                    Color32::from_white_alpha((14.0 * highlight + 10.0 * press) as u8),
                );
                // Neutral, as the window buttons next to it, also while a device is selected.
                let color = lerp_color(theme::TEXT_DIM, theme::TEXT, highlight);
                icons::device(painter, rect.center(), 17.0, &kind, color);
                if let Some(name) = &name {
                    let pos = pos2(
                        rect.left() + NAME_GAP - name.size().x,
                        rect.center().y - name.size().y / 2.0,
                    );
                    painter.galley(pos, name.clone(), color);
                }
            },
        )
        .on_hover_text(label)
        // As the window buttons next to it, with the normal arrow.
        .on_hover_cursor(CursorIcon::Default);
        if response.clicked() {
            self.popover_open = !self.popover_open;
        }
        if self.popover_open {
            // Real time, not an animation: it changes also with animations off.
            let now = ctx.input(|i| i.time);
            let left = self
                .refreshed_at
                .map_or(0.0, |at| REFRESH_EVERY.as_secs_f64() - (now - at));
            if left <= 0.0 {
                actions.push(Command::RefreshDevices);
                self.refreshed_at = Some(now);
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(left));
            }
        }
        if open > 0.001 {
            self.devices_popover(&ctx, rect, open, data, actions);
        }
        button
    }

    fn devices_popover(
        &mut self,
        ctx: &egui::Context,
        button: Rect,
        open: f32,
        data: &DevicesData<'_>,
        actions: &mut Vec<Command>,
    ) {
        let accent = data.accent;
        // Below the button, as the «☰» menu.
        let area = egui::Area::new(Id::unique("devices-popover"))
            .order(Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(button.right_bottom() + vec2(0.0, 6.0 - (1.0 - open) * 8.0))
            .constrain(true)
            .show(ctx, |ui| {
                ui.set_opacity(open);
                // As the dialogs: the window background, with the complementary color as the main
                // color.
                egui::Frame::new()
                    .corner_radius(theme::RADIUS_LARGE)
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        backdrop::framed(ui, Margin::same(8), accent, |ui| {
                        ui.set_width(280.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let (title_rect, _) = ui.allocate_exact_size(vec2(280.0, 26.0), Sense::hover());
                        let title = spaced_line(ui.painter(), "DEVICES", theme::strong(theme::FONT_SMALL), theme::TEXT_FAINT, 1.2);
                        ui.painter().galley(pos2(title_rect.left() + 10.0, title_rect.center().y - title.size().y / 2.0), title, theme::TEXT_FAINT);

                        let web_device = match data.web_player {
                            WebPlayerStatus::Ready { device_id } => Some(device_id.as_str()),
                            _ => None,
                        };
                        let device = data.playback.device.as_ref();
                        let web_active = device.is_some_and(|d| {
                            Some(d.id.as_str()) == web_device || d.name == DEVICE_NAME
                        });
                        match web_player_row(ui, data.web_player, web_active, data.playback.is_playing, accent) {
                            WebRowAction::Start => actions.push(Command::StartWebPlayer),
                            WebRowAction::Reauthorize => {
                                actions.push(Command::ReauthorizeWebPlayer);
                                self.popover_open = false;
                            }
                            WebRowAction::Close => self.popover_open = false,
                            WebRowAction::None => {}
                        }
                        let (line, _) = ui.allocate_exact_size(vec2(280.0, 9.0), Sense::hover());
                        ui.painter().hline(line.x_range().shrink(8.0), line.center().y, Stroke::new(1.0, theme::BORDER));

                        // The device of the SDK is the row above.
                        let devices: Vec<&Device> = data
                            .devices
                            .iter()
                            .filter(|d| Some(d.id.as_str()) != web_device && d.name != DEVICE_NAME)
                            .collect();
                        if devices.is_empty() {
                            let text = "No other devices. Open Spotify on this computer, on your phone or in the browser.";
                            ui.add(egui::Label::new(egui::RichText::new(text).color(theme::TEXT_DIM).font(theme::regular(theme::FONT_BODY))).wrap());
                            ui.add_space(4.0);
                        }
                        for device in devices {
                            let detail = device.is_active.then_some("Active");
                            let kind = device.kind.clone();
                            let row = menu_row(ui, 280.0, Id::unique(("device", &device.id)), &device.name, detail, device.is_active, accent, move |painter, center, color| {
                                icons::device(painter, center, 18.0, &kind, color);
                            });
                            if row.clicked() {
                                actions.push(Command::TransferPlayback { device_id: device.id.clone() });
                                self.popover_open = false;
                            }
                        }

                        ui.add_space(4.0);
                        let (line, _) = ui.allocate_exact_size(vec2(280.0, 9.0), Sense::hover());
                        ui.painter().hline(line.x_range().shrink(8.0), line.center().y, Stroke::new(1.0, theme::BORDER));
                        if menu_row(ui, 280.0, Id::unique("open-spotify"), "Open Spotify", None, false, accent, |painter, center, color| icons::speaker(painter, center, 16.0, color)).clicked() {
                            if let Err(error) = open::that_detached("spotify:") {
                                log::warn!("Could not open the Spotify app: {error}");
                            }
                            self.popover_open = false;
                        }
                        if menu_row(ui, 280.0, Id::unique("open-web-player"), "Open the web player", None, false, accent, |painter, center, color| icons::device(painter, center, 18.0, "computer", color)).clicked() {
                            ui.ctx().open_url(OpenUrl::new_tab("https://open.spotify.com"));
                            self.popover_open = false;
                        }
                        });
                    });
            });

        // The popover hangs from a corner, so egui puts it with the size of the last frame. When
        // the content changes, draw again immediately.
        if area.response.rect.size() != self.popover_size {
            self.popover_size = area.response.rect.size();
            ctx.request_repaint();
        }

        if self.popover_open {
            let popover = area.response.rect;
            let clicked_outside = ctx.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !popover.contains(p) && !button.contains(p))
            });
            if clicked_outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.popover_open = false;
            }
        }
    }
}

enum WebRowAction {
    None,
    Start,
    Reauthorize,
    Close,
}

/// «Play in this app»: the embedded web player, with its state and, when necessary, an
/// explanation under it.
fn web_player_row(
    ui: &mut Ui,
    status: &WebPlayerStatus,
    active: bool,
    playing: bool,
    accent: Color32,
) -> WebRowAction {
    let ctx = ui.ctx().clone();
    let time = ctx.input(|i| i.time);
    let starting = *status == WebPlayerStatus::Starting;
    let (detail, detail_color) = match status {
        WebPlayerStatus::Off => (Some("Off"), theme::TEXT_FAINT),
        WebPlayerStatus::Unavailable(_) => (Some("Unavailable"), theme::TEXT_FAINT),
        WebPlayerStatus::NeedsReauth => (Some("Permission"), theme::TEXT_FAINT),
        WebPlayerStatus::Starting => (None, theme::TEXT_FAINT),
        WebPlayerStatus::Ready { .. } if active => (Some("Active"), accent),
        WebPlayerStatus::Ready { .. } => (Some("Ready"), theme::TEXT_FAINT),
        WebPlayerStatus::Failed(_) => (Some("Error"), theme::DANGER),
    };
    let (rect, _) = ui.allocate_exact_size(vec2(280.0, 40.0), Sense::hover());
    let label = "Play in this app";
    let response = custom_button(
        ui,
        rect,
        Id::unique("web-player-row"),
        label,
        !starting,
        |painter, rect, hover, _| {
            painter.rect_filled(
                rect,
                theme::RADIUS_MEDIUM,
                Color32::from_white_alpha((13.0 * hover) as u8),
            );
            let color = if active {
                accent
            } else {
                lerp_color(theme::TEXT_DIM, theme::TEXT, hover)
            };
            let bars =
                Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(15.0, 15.0));
            let moving = if active && playing { 1.0 } else { 0.0 };
            icons::app_bars(painter, bars, time, moving, color);
            let galley = single_line(
                painter,
                label,
                theme::regular(theme::FONT_BODY),
                color,
                rect.width() - 112.0,
            );
            painter.galley(
                pos2(rect.left() + 40.0, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
            match detail {
                Some(detail) => {
                    let galley = single_line(
                        painter,
                        detail,
                        theme::regular(theme::FONT_SMALL),
                        detail_color,
                        80.0,
                    );
                    paint_galley(
                        painter,
                        galley,
                        rect.right() - 10.0,
                        rect.center().y - 7.0,
                        Align::Max,
                        1.0,
                    );
                }
                None => icons::loading_dots(
                    painter,
                    pos2(rect.right() - 24.0, rect.center().y),
                    7.0,
                    time,
                    theme::TEXT_DIM,
                ),
            }
        },
    );
    if starting {
        anim::request_frame(&ctx);
    } else if active && playing {
        anim::request_frame_after(&ctx, Duration::from_millis(33));
    }

    let explanation = match status {
        WebPlayerStatus::Off => Some((
            "The music plays in Melodia, without opening Spotify.".to_owned(),
            theme::TEXT_FAINT,
        )),
        WebPlayerStatus::Starting => Some(("Connecting to Spotify…".to_owned(), theme::TEXT_FAINT)),
        WebPlayerStatus::Unavailable(reason) => Some((reason.clone(), theme::TEXT_FAINT)),
        WebPlayerStatus::NeedsReauth => Some((
            "Spotify needs a new permission to play here. Reconnect your account (the browser will open).".to_owned(),
            theme::TEXT_DIM,
        )),
        WebPlayerStatus::Failed(reason) => Some((format!("{reason} Click to try again."), theme::DANGER)),
        WebPlayerStatus::Ready { .. } => None,
    };
    let mut action = WebRowAction::None;
    if let Some((text, color)) = explanation {
        ui.horizontal(|ui| {
            ui.add_space(34.0);
            let text = egui::RichText::new(text)
                .color(color)
                .font(theme::regular(theme::FONT_SMALL));
            ui.add(egui::Label::new(text).wrap());
        });
        ui.add_space(4.0);
    }
    if *status == WebPlayerStatus::NeedsReauth {
        ui.horizontal(|ui| {
            ui.add_space(30.0);
            if text_button(
                ui,
                Id::unique("web-player-reauth"),
                "Reconnect",
                theme::strong(theme::FONT_SMALL),
            )
            .clicked()
            {
                action = WebRowAction::Reauthorize;
            }
        });
        ui.add_space(4.0);
    }

    if response.clicked() {
        action = match status {
            WebPlayerStatus::NeedsReauth => WebRowAction::Reauthorize,
            WebPlayerStatus::Ready { .. } if active => WebRowAction::Close,
            WebPlayerStatus::Starting => WebRowAction::None,
            _ => WebRowAction::Start,
        };
    }
    action
}

/// A row of a popover menu, with the width `width`, without an icon.
pub(super) fn text_row(
    ui: &mut Ui,
    width: f32,
    id: Id,
    label: &str,
    accent: Color32,
) -> egui::Response {
    row(
        ui,
        width,
        id,
        label,
        None,
        false,
        accent,
        12.0,
        |_, _, _| {},
    )
}

/// A row of a popover menu, with the width `width`.
#[expect(clippy::too_many_arguments)]
pub(super) fn menu_row(
    ui: &mut Ui,
    width: f32,
    id: Id,
    label: &str,
    detail: Option<&str>,
    highlighted: bool,
    accent: Color32,
    paint_icon: impl FnOnce(&Painter, Pos2, Color32),
) -> egui::Response {
    row(
        ui,
        width,
        id,
        label,
        detail,
        highlighted,
        accent,
        40.0,
        paint_icon,
    )
}

/// A row with its label at `text_left` from its left edge.
#[expect(clippy::too_many_arguments)]
fn row(
    ui: &mut Ui,
    width: f32,
    id: Id,
    label: &str,
    detail: Option<&str>,
    highlighted: bool,
    accent: Color32,
    text_left: f32,
    paint_icon: impl FnOnce(&Painter, Pos2, Color32),
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    custom_button(ui, rect, id, label, true, |painter, rect, hover, _| {
        painter.rect_filled(
            rect,
            theme::RADIUS_MEDIUM,
            Color32::from_white_alpha((13.0 * hover) as u8),
        );
        let color = if highlighted {
            accent
        } else {
            lerp_color(theme::TEXT_DIM, theme::TEXT, hover)
        };
        paint_icon(painter, pos2(rect.left() + 20.0, rect.center().y), color);
        let detail_width = if detail.is_some() { 56.0 } else { 0.0 };
        let galley = single_line(
            painter,
            label,
            theme::regular(theme::FONT_BODY),
            color,
            rect.width() - text_left - 12.0 - detail_width,
        );
        painter.galley(
            pos2(
                rect.left() + text_left,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            color,
        );
        if let Some(detail) = detail {
            let galley = single_line(
                painter,
                detail,
                theme::regular(theme::FONT_SMALL),
                theme::TEXT_FAINT,
                detail_width,
            );
            paint_galley(
                painter,
                galley,
                rect.right() - 10.0,
                rect.center().y - 7.0,
                Align::Max,
                1.0,
            );
        }
    })
}
