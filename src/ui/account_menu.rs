//! The «☰» menu next to the window buttons: the user, «Log out», the changelog and the version of
//! the app, and the download of a newer version. When a newer version arrives, the button also
//! animates and shows «Update» beside it ([`update_button`]). Also the «DEMO» badge of the demo
//! ([`demo_badge`]).

use std::f32::consts::TAU;

use egui::ecolor::HsvaGamma;
use egui::{
    Align2, Color32, Context, Id, Key, Margin, Mesh, Order, Painter, Pos2, Rect, Sense, Shape,
    Stroke, Ui, pos2, vec2,
};

use super::anim::{self, lerp_color};
use super::backdrop;
use super::changelog::Changelog;
use super::devices::text_row;
use super::theme;
use super::widgets::{a11y_label, custom_button, custom_button_with_ring, single_line};
use super::window_frame::{self, BUTTON_HEIGHT, BUTTON_WIDTH, CONTROLS_WIDTH};
use crate::backend::Command;
use crate::model::UserProfile;
use crate::update::AvailableUpdate;
use crate::util::initial;

const MENU_WIDTH: f32 = 240.0;
/// The time for the colors of the «☰» lines to go round one time while there is a new version.
const UPDATE_CYCLE: f32 = 5.0;
/// The pieces of each «☰» line, with a color at each end, that make the gradient.
const STEPS: usize = 12;

/// The space on each side of the «Update» pill, in its slot of the title bar.
const UPDATE_MARGIN: f32 = 6.0;

/// The text of the last line of the menu, with the version from `Cargo.toml`.
const VERSION_LABEL: &str = concat!("Melodia ", env!("CARGO_PKG_VERSION"));

#[derive(Default)]
pub struct AccountMenu {
    open: bool,
}

impl AccountMenu {
    /// The «☰» button, just left of the window buttons in the top right corner of `rect`, and its
    /// menu while it is open. Returns the position of the button.
    #[expect(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        profile: Option<&UserProfile>,
        update: Option<&AvailableUpdate>,
        accent: Color32,
        actions: &mut Vec<Command>,
        changelog: &mut Changelog,
    ) -> Rect {
        let ctx = ui.ctx().clone();
        let open = anim::tween_bool(&ctx, Id::unique("account-menu-open"), self.open, 0.2);
        let button = Rect::from_min_size(
            pos2(rect.right() - CONTROLS_WIDTH - BUTTON_WIDTH, rect.top()),
            vec2(BUTTON_WIDTH, BUTTON_HEIGHT),
        );
        let response = window_frame::window_button_lit(
            ui,
            button,
            "More options",
            false,
            open,
            |p, c, color| {
                if update.is_some() {
                    paint_update_lines(&ctx, p, c, accent);
                } else {
                    let stroke = Stroke::new(1.0, color);
                    for dy in [-4.0, 0.0, 4.0] {
                        p.line_segment([c + vec2(-6.0, dy), c + vec2(6.0, dy)], stroke);
                    }
                }
            },
        );
        if response.clicked() {
            self.open = !self.open;
        }
        if open > 0.001 {
            let menu = Menu {
                profile,
                update,
                accent,
            };
            self.popover(&ctx, button, open, &menu, actions, changelog);
        }
        button
    }

    fn popover(
        &mut self,
        ctx: &egui::Context,
        button: Rect,
        open: f32,
        menu: &Menu,
        actions: &mut Vec<Command>,
        changelog: &mut Changelog,
    ) {
        let accent = menu.accent;
        let area = egui::Area::new(Id::unique("account-menu"))
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
                            ui.set_width(MENU_WIDTH);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            if let Some(profile) = menu.profile {
                                profile_row(ui, profile, accent);
                                separator(ui);
                            }
                            let changes = text_row(
                                ui,
                                MENU_WIDTH,
                                Id::unique("changelog"),
                                "Changelog",
                                accent,
                            );
                            if changes.clicked() {
                                changelog.open();
                                self.open = false;
                            }
                            let logout =
                                text_row(ui, MENU_WIDTH, Id::unique("logout"), "Log out", accent);
                            if logout.clicked() {
                                actions.push(Command::Logout);
                                self.open = false;
                            }
                            separator(ui);
                            version_row(ui);
                            if let Some(update) = menu.update
                                && update_row(ui, update, accent)
                            {
                                self.open = false;
                            }
                        });
                    });
            });

        if self.open {
            let menu = area.response.rect;
            let clicked_outside = ctx.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !menu.contains(p) && !button.contains(p))
            });
            if clicked_outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.open = false;
            }
        }
    }
}

/// A thin horizontal line between groups of rows.
fn separator(ui: &mut Ui) {
    let (line, _) = ui.allocate_exact_size(vec2(MENU_WIDTH, 9.0), Sense::hover());
    ui.painter().hline(
        line.x_range().shrink(8.0),
        line.center().y,
        Stroke::new(1.0, theme::BORDER),
    );
}

/// The items of the menu other than the actions.
struct Menu<'a> {
    profile: Option<&'a UserProfile>,
    update: Option<&'a AvailableUpdate>,
    accent: Color32,
}

/// The name and version of the app, small and faint, at the bottom of the menu.
fn version_row(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(MENU_WIDTH, 24.0), Sense::hover());
    let painter = ui.painter();
    let text = single_line(
        painter,
        VERSION_LABEL,
        theme::regular(theme::FONT_SMALL),
        theme::TEXT_FAINT,
        rect.width() - 24.0,
    );
    let text_rect = Rect::from_min_size(
        pos2(rect.left() + 12.0, rect.center().y - text.size().y / 2.0),
        text.size(),
    );
    painter.galley(text_rect.min, text, theme::TEXT_FAINT);
    a11y_label(ui, text_rect, Id::unique("app-version"), VERSION_LABEL);
}

/// «Download Melodia X.Y.Z», immediately under the version and with its size, in the accent color.
/// It opens the download page of the new version. Returns true if the user clicked it.
fn update_row(ui: &mut Ui, update: &AvailableUpdate, accent: Color32) -> bool {
    let label = format!("Download Melodia {}", update.version);
    let (rect, _) = ui.allocate_exact_size(vec2(MENU_WIDTH, 24.0), Sense::hover());
    let id = Id::unique("update-available");
    let clicked = custom_button(ui, rect, id, &label, true, |painter, rect, hover, _| {
        painter.rect_filled(
            rect,
            theme::RADIUS_MEDIUM,
            Color32::from_white_alpha((13.0 * hover) as u8),
        );
        let color = lerp_color(accent, theme::TEXT, hover * 0.4);
        let text = single_line(
            painter,
            &label,
            theme::regular(theme::FONT_SMALL),
            color,
            rect.width() - 24.0,
        );
        let top = rect.center().y - text.size().y / 2.0;
        painter.galley(pos2(rect.left() + 12.0, top), text, color);
    })
    .clicked();
    if clicked {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&update.url));
    }
    clicked
}

/// «Update», a small accent pill just left of `button` (the «☰» button) while there is a new
/// version. It opens its download page. Returns its slot, with the height of the window buttons,
/// so that the device picker goes left of it.
pub(super) fn update_button(
    ui: &mut Ui,
    button: Rect,
    update: &AvailableUpdate,
    accent: Color32,
) -> Rect {
    let font = theme::strong(theme::FONT_SMALL);
    let text = single_line(ui.painter(), "Update", font.clone(), accent, 120.0);
    let size = (text.size() + vec2(20.0, 8.0)).round();
    let slot = Rect::from_min_max(
        pos2(button.left() - size.x - 2.0 * UPDATE_MARGIN, button.top()),
        button.left_bottom(),
    );
    let pill = Rect::from_center_size(slot.center(), size);
    let response = custom_button_with_ring(
        ui,
        pill,
        Id::unique("update-button"),
        "Update",
        true,
        size.y / 2.0,
        |painter, rect, hover, press| {
            let fill = accent.gamma_multiply(0.16 + 0.12 * hover + 0.06 * press);
            painter.rect_filled(rect, rect.height() / 2.0, fill);
            let color = lerp_color(accent, theme::TEXT, hover * 0.4);
            let text = single_line(painter, "Update", font, color, 120.0);
            painter.galley((rect.center() - text.size() / 2.0).round(), text, color);
        },
    )
    .on_hover_text(format!("Download Melodia {}", update.version));
    if response.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&update.url));
    }
    slot
}

/// The «☰» lines while there is a new version (see «Menus and popovers» in
/// `docs/design.md`). They do not move when animations are off.
fn paint_update_lines(ctx: &Context, painter: &Painter, center: Pos2, accent: Color32) {
    // 20 frames a second is sufficient for colors that move this slowly.
    let phase = anim::cycle(ctx, UPDATE_CYCLE, 0.05);
    let from = HsvaGamma::from(accent);
    let to = HsvaGamma::from(backdrop::complementary(accent));
    let color_at = |pos: Pos2| {
        // The position of `pos` towards the bottom left (0 at the top right corner of the icon, 1
        // at the bottom left corner). The gradient goes along that diagonal, so its bands have a
        // 45° tilt.
        let along = ((pos.y - center.y) - (pos.x - center.x) + 12.0) / 24.0;
        // Accent → complementary → accent along two times the diagonal. Thus only half of it shows
        // at a time, and the loop has no seam. When the code subtracts the phase, the colors move
        // towards the bottom left.
        let wave = 0.5 - 0.5 * (TAU * (along / 2.0 - phase)).cos();
        Color32::from(HsvaGamma {
            h: (from.h + 0.5 * wave).fract(),
            s: anim::lerp(from.s, to.s, wave),
            v: anim::lerp(from.v, to.v, wave),
            a: 1.0,
        })
    };
    // A mesh with a color for each vertex. The painter does not draw line segments as short as
    // each piece.
    let mut mesh = Mesh::default();
    for dy in [-4.0, 0.0, 4.0] {
        let y = center.y + dy;
        for step in 0..=STEPS {
            let left = center.x - 6.0 + 12.0 * step as f32 / STEPS as f32;
            let (top, bottom) = (pos2(left, y - 0.75), pos2(left, y + 0.75));
            let next = mesh.vertices.len() as u32;
            mesh.colored_vertex(top, color_at(top));
            mesh.colored_vertex(bottom, color_at(bottom));
            if step > 0 {
                mesh.add_triangle(next - 2, next - 1, next);
                mesh.add_triangle(next - 1, next, next + 1);
            }
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// The avatar (the initial on a circle in the accent color) and the name.
fn profile_row(ui: &mut Ui, profile: &UserProfile, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(MENU_WIDTH, 48.0), Sense::hover());
    let painter = ui.painter();
    let avatar = pos2(rect.left() + 22.0, rect.center().y);
    painter.circle_filled(avatar, 15.0, accent.gamma_multiply(0.22));
    let letter = single_line(
        painter,
        &initial(&profile.display_name),
        theme::strong(theme::FONT_BODY),
        accent,
        30.0,
    );
    painter.galley(avatar - letter.size() / 2.0, letter, accent);
    let name = single_line(
        painter,
        &profile.display_name,
        theme::strong(theme::FONT_BODY),
        theme::TEXT,
        rect.width() - 56.0,
    );
    let name_rect = Rect::from_min_size(
        pos2(avatar.x + 25.0, rect.center().y - name.size().y / 2.0),
        name.size(),
    );
    painter.galley(name_rect.min, name, theme::TEXT);
    a11y_label(
        ui,
        name_rect,
        Id::unique("profile-name"),
        &profile.display_name,
    );
}

/// «DEMO», just left of `button`.
pub(super) fn demo_badge(ui: &Ui, button: Rect, accent: Color32) {
    let painter = ui.painter();
    let badge = single_line(
        painter,
        "DEMO",
        theme::strong(theme::FONT_SMALL),
        accent,
        60.0,
    );
    let size = badge.size() + vec2(14.0, 6.0);
    let badge_rect = Rect::from_min_size(
        pos2(
            button.left() - 6.0 - size.x,
            button.center().y - size.y / 2.0,
        ),
        size,
    );
    painter.rect_filled(
        badge_rect,
        theme::RADIUS_MEDIUM,
        accent.gamma_multiply(0.16),
    );
    painter.galley(badge_rect.center() - badge.size() / 2.0, badge, accent);
    a11y_label(ui, badge_rect, Id::unique("demo-badge"), "DEMO");
}
