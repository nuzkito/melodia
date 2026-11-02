//! First-run screen: connects the Spotify account (and asks for the Client ID of the user's
//! Spotify app when there is none in the configuration). As Spotify's design guidelines ask, its
//! logo shows at the bottom, in place of the word in «Playing music requires Spotify Premium.»
//! (not in the demo).

use std::f32::consts::TAU;

use egui::epaint::{Mesh, Vertex};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, Id, OpenUrl, Rect, Shape, TextFormat, TextureHandle, Ui, Vec2, pos2, vec2,
};

use super::anim::{self, ease_out_cubic};
use super::icons;
use super::spotify_logo;
use super::theme;
use super::widgets::{self, a11y_label, paint_galley, pill_button, single_line, text_button};
use crate::backend::{AuthStatus, Command};
use crate::config::{SPOTIFY_DASHBOARD_URL, is_valid_client_id};
use crate::icon;

const COLUMN_WIDTH: f32 = 420.0;
/// The seconds for the gradient of the logo to make a full turn.
const LOGO_TURN_PERIOD: f32 = 5.0;
/// A mesh colors the note and border of the logo. It has this number of squares on each side, and
/// its corners get the color of the gradient at that point.
const LOGO_MESH_STEPS: usize = 24;
/// The width of Spotify's logo in place of the word in «… requires Spotify Premium.». It is the
/// smallest width that its guidelines allow, so that it stays near the size of the text.
const SPOTIFY_LOGO_WIDTH: f32 = spotify_logo::MIN_WIDTH;
/// The pixels for its render: three for each point, sharp on all screens.
pub const SPOTIFY_LOGO_PX: u32 = 3 * SPOTIFY_LOGO_WIDTH as u32;

#[derive(Default)]
pub struct LoginView {
    client_id: String,
    editing_client_id: bool,
    /// The user pushed "Connect", and the backend did not answer yet.
    connecting: bool,
    opened_url: Option<String>,
    /// The time when the gradient of the logo started to turn, and the time of the last draw of
    /// the screen. Each time the screen appears, it starts again from the colors of the app icon.
    turn_start: f64,
    last_shown: Option<f64>,
}

impl LoginView {
    /// Responds to a new auth status from the backend.
    pub fn on_status(&mut self, status: &AuthStatus, ctx: &egui::Context) {
        self.connecting = false;
        match status {
            AuthStatus::WaitingForBrowser { auth_url, .. } => {
                if self.opened_url.as_ref() != Some(auth_url) {
                    self.opened_url = Some(auth_url.clone());
                    ctx.open_url(OpenUrl::new_tab(auth_url));
                }
            }
            AuthStatus::LoggedOut { client_id, .. } => {
                if self.client_id.is_empty()
                    && let Some(client_id) = client_id
                {
                    self.client_id.clone_from(client_id);
                }
                self.editing_client_id |= client_id.is_none();
            }
            AuthStatus::LoggedIn { .. } => {
                self.editing_client_id = false;
                self.opened_url = None;
            }
            AuthStatus::Starting => {}
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        status: &AuthStatus,
        logo: Option<&LogoTextures>,
        demo: bool,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        let width = COLUMN_WIDTH.min(rect.width() - 48.0);
        let content_height = if self.editing_client_id { 560.0 } else { 420.0 };
        let top = rect.top() + ((rect.height() - content_height) / 2.0).max(24.0);
        let cx = rect.center().x;
        let painter = ui.painter().clone();

        // The entrance animation of the full column.
        let elapsed = anim::since_change(&ctx, Id::unique("login-enter"), (), true);
        anim::keep_animating(&ctx, elapsed, 0.6);
        let enter = |delay: f32| ease_out_cubic((elapsed - delay) / 0.45);

        // Logo: the app icon, with its gradient that turns.
        let logo_size = 84.0;
        let t = enter(0.0);
        let logo_rect = Rect::from_center_size(
            pos2(cx, top + logo_size / 2.0 + (1.0 - t) * 10.0),
            Vec2::splat(logo_size * (0.9 + 0.1 * t)),
        );
        if self.last_shown.is_none_or(|last| time - last > 0.5) {
            self.turn_start = time;
        }
        self.last_shown = Some(time);
        // The gradient turns (see «App icon» in `docs/design.md`). It is infinite with
        // animations off (tests), and then it stays as the icon.
        let elapsed = anim::elapsed_since(&ctx, self.turn_start);
        let angle = if elapsed.is_finite() {
            (elapsed / LOGO_TURN_PERIOD).fract() * TAU
        } else {
            0.0
        };
        if let Some(logo) = logo {
            paint_logo(ui, logo_rect, logo, angle, t);
        }
        anim::request_frame_after(&ctx, std::time::Duration::from_millis(33));

        let mut y = logo_rect.bottom() + 26.0;
        let t = enter(0.06);
        let title = single_line(
            &painter,
            "Melodia",
            theme::strong(theme::FONT_HERO),
            theme::TEXT,
            width,
        );
        let title_rect = paint_galley(&painter, title, cx, y + (1.0 - t) * 8.0, Align::Center, t);
        a11y_label(ui, title_rect, Id::unique("login-title"), "Melodia");
        y = title_rect.bottom() + 6.0;
        let subtitle = if demo {
            "Demo mode: sample data, no Spotify account needed."
        } else {
            "Your Spotify music, without distractions."
        };
        let galley = single_line(
            &painter,
            subtitle,
            theme::regular(theme::FONT_BODY),
            theme::TEXT_DIM,
            width,
        );
        y = paint_galley(
            &painter,
            galley,
            cx,
            y + (1.0 - enter(0.1)) * 8.0,
            Align::Center,
            enter(0.1),
        )
        .bottom()
            + 34.0;

        let column = Rect::from_min_size(pos2(cx - width / 2.0, y), vec2(width, rect.bottom() - y));
        let alpha = enter(0.16);
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(column)
                .layout(egui::Layout::top_down(Align::Center)),
        );
        ui.multiply_opacity(alpha);

        match status {
            AuthStatus::WaitingForBrowser {
                auth_url,
                redirect_uri,
            } => {
                Self::waiting_for_browser(&mut ui, auth_url, redirect_uri, time, actions);
            }
            AuthStatus::LoggedOut {
                error,
                redirect_uri,
                client_id,
            } => {
                if let Some(error) = error {
                    wrapped_text(
                        &mut ui,
                        error,
                        theme::regular(theme::FONT_BODY),
                        theme::DANGER,
                        width,
                    );
                    ui.add_space(16.0);
                }
                if self.editing_client_id || client_id.is_none() {
                    self.client_id_form(&mut ui, redirect_uri, width);
                }
                self.connect_button(&mut ui, time, demo, actions);
                if !self.editing_client_id && !demo {
                    ui.add_space(10.0);
                    if text_button(
                        &mut ui,
                        Id::unique("change-client-id"),
                        "Change Client ID",
                        theme::regular(theme::FONT_BODY),
                    )
                    .clicked()
                    {
                        self.editing_client_id = true;
                    }
                }
            }
            AuthStatus::Starting | AuthStatus::LoggedIn { .. } => {}
        }

        let footer_top = rect.bottom() - 34.0;
        if demo {
            // Demo mode is only for development. It has no Spotify content, so it has no Spotify
            // logo.
            let galley = single_line(
                &painter,
                "The demo's songs, albums and artists are made up.",
                theme::regular(theme::FONT_SMALL),
                theme::TEXT_FAINT,
                width,
            );
            paint_galley(&painter, galley, cx, footer_top, Align::Center, alpha);
        } else {
            // «Playing music requires Spotify Premium.» with Spotify's logo in place of the word.
            // This is also the attribution that its guidelines ask for. The words keep the clear
            // space of the logo (half the height of the icon) on each side.
            let font = theme::regular(theme::FONT_SMALL);
            let before = single_line(
                &painter,
                "Playing music requires",
                font.clone(),
                theme::TEXT_FAINT,
                width,
            );
            let after = single_line(&painter, "Premium.", font, theme::TEXT_FAINT, width);
            let logo_size = vec2(
                SPOTIFY_LOGO_WIDTH,
                SPOTIFY_LOGO_WIDTH / spotify_logo::ASPECT,
            );
            let clear_space = logo_size.y * spotify_logo::ICON_HEIGHT / 2.0;
            let line_width =
                before.size().x + clear_space + logo_size.x + clear_space + after.size().x;
            let left = cx - line_width / 2.0;
            let text_middle = footer_top + before.size().y / 2.0;
            let before_rect = paint_galley(&painter, before, left, footer_top, Align::Min, alpha);
            let spotify_rect = Rect::from_min_size(
                pos2(
                    before_rect.right() + clear_space,
                    text_middle - logo_size.y / 2.0,
                ),
                logo_size,
            );
            if let Some(logo) = logo {
                egui::Image::from_texture(egui::load::SizedTexture::from_handle(&logo.spotify))
                    .tint(Color32::WHITE.gamma_multiply(alpha))
                    .paint_at(&ui, spotify_rect);
            }
            a11y_label(&ui, spotify_rect, Id::unique("spotify-logo"), "Spotify");
            paint_galley(
                &painter,
                after,
                spotify_rect.right() + clear_space,
                footer_top,
                Align::Min,
                alpha,
            );
        }
    }

    fn client_id_form(&mut self, ui: &mut Ui, redirect_uri: &str, width: f32) {
        let label = single_line(
            ui.painter(),
            "Your Spotify app's Client ID",
            theme::regular(theme::FONT_BODY),
            theme::TEXT_DIM,
            width,
        );
        let (label_rect, _) =
            ui.allocate_exact_size(vec2(width, label.size().y), egui::Sense::hover());
        ui.painter()
            .galley(label_rect.left_top(), label, theme::TEXT_DIM);
        ui.add_space(6.0);

        let (field_rect, _) = ui.allocate_exact_size(vec2(width, 46.0), egui::Sense::hover());
        let id = Id::unique("client-id");
        widgets::text_field(
            ui,
            field_rect,
            id,
            "Client ID",
            &mut self.client_id,
            "32 characters, e.g. 1a2b3c…",
            widgets::FieldStyle::Boxed,
            theme::SPOTIFY_GREEN,
        );
        let trimmed = self.client_id.trim();
        if !trimmed.is_empty() && !is_valid_client_id(trimmed) {
            ui.add_space(4.0);
            wrapped_text(
                ui,
                "The Client ID is 32 hexadecimal characters.",
                theme::regular(theme::FONT_SMALL),
                theme::DANGER,
                width,
            );
        }
        ui.add_space(14.0);

        // Help: where to get the Client ID, and which Redirect URI to register.
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.x = 4.0;
            let help = egui::RichText::new("Create an app at")
                .color(theme::TEXT_FAINT)
                .font(theme::regular(theme::FONT_BODY));
            ui.label(help);
            ui.hyperlink_to(
                egui::RichText::new("developer.spotify.com").font(theme::regular(theme::FONT_BODY)),
                SPOTIFY_DASHBOARD_URL,
            );
            ui.label(
                egui::RichText::new("(Web API) and add this Redirect URI:")
                    .color(theme::TEXT_FAINT)
                    .font(theme::regular(theme::FONT_BODY)),
            );
        });
        ui.add_space(8.0);
        let (uri_rect, _) = ui.allocate_exact_size(vec2(width, 38.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(uri_rect, theme::RADIUS_MEDIUM, Color32::from_white_alpha(8));
        let galley = single_line(
            painter,
            redirect_uri,
            egui::FontId::monospace(theme::FONT_BODY),
            theme::TEXT,
            width - 110.0,
        );
        painter.galley(
            pos2(
                uri_rect.left() + 12.0,
                uri_rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            theme::TEXT,
        );
        a11y_label(
            ui,
            uri_rect.shrink2(vec2(60.0, 0.0)),
            Id::unique("redirect-uri"),
            redirect_uri,
        );
        let copied = anim::since_trigger(ui.ctx(), Id::unique("uri-copied"), false);
        let button_rect = Rect::from_min_max(
            pos2(uri_rect.right() - 92.0, uri_rect.top() + 5.0),
            pos2(uri_rect.right() - 5.0, uri_rect.bottom() - 5.0),
        );
        let label = if copied < 1.6 { "Copied!" } else { "Copy" };
        let response = widgets::custom_button(
            ui,
            button_rect,
            Id::unique("copy-uri"),
            "Copy Redirect URI",
            true,
            |painter, rect, hover, _| {
                painter.rect_filled(
                    rect,
                    theme::RADIUS_SMALL,
                    Color32::from_white_alpha((12.0 + 14.0 * hover) as u8),
                );
                let galley = single_line(
                    painter,
                    label,
                    theme::regular(theme::FONT_SMALL),
                    theme::TEXT,
                    rect.width(),
                );
                painter.galley(rect.center() - galley.size() / 2.0, galley, theme::TEXT);
            },
        );
        if response.clicked() {
            ui.ctx().copy_text(redirect_uri.to_owned());
            anim::since_trigger(ui.ctx(), Id::unique("uri-copied"), true);
        }
        if copied < 1.6 {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        ui.add_space(26.0);
    }

    fn connect_button(&mut self, ui: &mut Ui, time: f64, demo: bool, actions: &mut Vec<Command>) {
        let (rect, _) = ui.allocate_exact_size(vec2(270.0, 50.0), egui::Sense::hover());
        if self.connecting {
            icons::loading_dots(ui.painter(), rect.center(), 12.0, time, theme::TEXT_DIM);
            anim::request_frame(ui.ctx());
            return;
        }
        let client_id = self.client_id.trim().to_owned();
        let needs_id = self.editing_client_id;
        let enabled = !needs_id || is_valid_client_id(&client_id);
        let label = if demo {
            "Enter the demo"
        } else {
            "Connect to Spotify"
        };
        let response = pill_button(
            ui,
            rect,
            Id::unique("connect"),
            label,
            theme::SPOTIFY_GREEN,
            Color32::from_rgb(8, 8, 10),
            enabled,
        );
        if response.clicked() {
            self.connecting = true;
            actions.push(Command::Login {
                client_id: needs_id.then_some(client_id),
            });
        }
    }

    fn waiting_for_browser(
        ui: &mut Ui,
        auth_url: &str,
        redirect_uri: &str,
        time: f64,
        actions: &mut Vec<Command>,
    ) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 26.0), egui::Sense::hover());
        icons::loading_dots(
            ui.painter(),
            rect.center(),
            11.0,
            time,
            theme::SPOTIFY_GREEN,
        );
        anim::request_frame(ui.ctx());
        ui.add_space(10.0);
        wrapped_text(
            ui,
            "Waiting for authorization in the browser…",
            theme::strong(theme::FONT_TITLE),
            theme::TEXT,
            width,
        );
        ui.add_space(6.0);
        wrapped_text(
            ui,
            "Accept the permissions in the tab that just opened, then come back here.",
            theme::regular(theme::FONT_BODY),
            theme::TEXT_DIM,
            width,
        );
        ui.add_space(18.0);

        ui.horizontal(|ui| {
            // Center the three actions.
            ui.add_space((width - 330.0).max(0.0) / 2.0);
            if text_button(
                ui,
                Id::unique("reopen"),
                "Open again",
                theme::regular(theme::FONT_BODY),
            )
            .clicked()
            {
                ui.ctx().open_url(OpenUrl::new_tab(auth_url));
            }
            separator_dot(ui);
            if text_button(
                ui,
                Id::unique("copy-link"),
                "Copy link",
                theme::regular(theme::FONT_BODY),
            )
            .clicked()
            {
                ui.ctx().copy_text(auth_url.to_owned());
            }
            separator_dot(ui);
            if text_button(
                ui,
                Id::unique("cancel-login"),
                "Cancel",
                theme::regular(theme::FONT_BODY),
            )
            .clicked()
            {
                actions.push(Command::CancelLogin);
            }
        });
        ui.add_space(26.0);
        let hint = format!(
            "Does the browser show “INVALID_CLIENT: Invalid redirect URI”? Add {redirect_uri} to your Spotify app's settings and try again."
        );
        wrapped_text(
            ui,
            &hint,
            theme::regular(theme::FONT_SMALL),
            theme::TEXT_FAINT,
            width,
        );
    }
}

fn separator_dot(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 20.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 1.5, theme::TEXT_FAINT);
}

/// Centered, wrapped text as an accessible label.
fn wrapped_text(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32, width: f32) {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::wrap_at_width(width);
    job.halign = Align::Center;
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y), egui::Sense::hover());
    ui.painter()
        .galley(pos2(rect.center().x, rect.top()), galley, color);
    a11y_label(ui, rect, ui.scope_id().with(text), text);
}

/// The layers of the app icon, so that the login screen can color the note and the border itself.
pub struct LogoTextures {
    pub circle: TextureHandle,
    pub note: TextureHandle,
    pub border: TextureHandle,
    /// Spotify's full logo, white (see `spotify_logo`).
    pub spotify: TextureHandle,
}

/// The app icon with its gradient turned `angle` radians: the circle as it is, and the white masks
/// of the note and the border, tinted by a mesh with the gradient's color at its corners
/// ([`icon::ink_color`]).
fn paint_logo(ui: &Ui, rect: Rect, logo: &LogoTextures, angle: f32, alpha: f32) {
    egui::Image::from_texture(egui::load::SizedTexture::from_handle(&logo.circle))
        .tint(Color32::WHITE.gamma_multiply(alpha))
        .paint_at(ui, rect);
    for (texture, is_note) in [(&logo.note, true), (&logo.border, false)] {
        let mut mesh = Mesh::with_texture(texture.id());
        let n = LOGO_MESH_STEPS;
        for j in 0..=n {
            for i in 0..=n {
                let uv = pos2(i as f32 / n as f32, j as f32 / n as f32);
                let p = [uv.x - 0.5, uv.y - 0.5];
                let t = if is_note {
                    icon::note_gradient_t(p, angle)
                } else {
                    icon::border_gradient_t(p, angle)
                };
                let [r, g, b] = icon::ink_color(t);
                mesh.vertices.push(Vertex {
                    pos: rect.lerp_inside(uv.to_vec2()),
                    uv,
                    color: Color32::from_rgb(r, g, b).gamma_multiply(alpha),
                });
                if i < n && j < n {
                    let corner = (j * (n + 1) + i) as u32;
                    let below = corner + n as u32 + 1;
                    mesh.add_triangle(corner, corner + 1, below);
                    mesh.add_triangle(corner + 1, below + 1, below);
                }
            }
        }
        ui.painter().add(Shape::mesh(mesh));
    }
}
