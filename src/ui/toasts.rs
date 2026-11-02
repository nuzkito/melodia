//! Short notifications that slide up from the bottom and fade away.

use std::time::Duration;

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Color32, Context, Id, LayerId, Order, Rect, Shadow, Stroke, StrokeKind, TextFormat, pos2, vec2,
};

use super::anim::{self, ease_out_back, ease_out_cubic};
use super::theme;
use crate::backend::ToastKind;

const LIFETIME: f64 = 3.4;
const FADE: f32 = 0.28;
const MAX_VISIBLE: usize = 4;

struct ShownToast {
    kind: ToastKind,
    message: String,
    created_at: f64,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<ShownToast>,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, message: String, time: f64) {
        // A repeated message starts again, and does not add one more toast.
        self.items.retain(|item| item.message != message);
        self.items.push(ShownToast {
            kind,
            message,
            created_at: time,
        });
        if self.items.len() > MAX_VISIBLE {
            self.items.remove(0);
        }
    }

    pub fn messages(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|item| item.message.as_str())
    }

    pub fn show(&mut self, ctx: &Context, time: f64, accent: Color32) {
        self.items.retain(|item| time - item.created_at < LIFETIME);
        if self.items.is_empty() {
            return;
        }
        let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("toasts")));
        let screen = ctx.content_rect();
        // The lifetime is real time. Only the fades go off with animations.
        let fade_out = if anim::enabled(ctx) { FADE } else { 0.0 };
        let remaining = |item: &ShownToast| (LIFETIME - (time - item.created_at)) as f32;
        let mut bottom = screen.bottom() - 26.0;
        for item in self.items.iter().rev() {
            let age = anim::elapsed_since(ctx, item.created_at);
            let appear = (age / FADE).clamp(0.0, 1.0);
            let leaving = if fade_out > 0.0 {
                ease_out_cubic(remaining(item) / fade_out)
            } else {
                1.0
            };
            let alpha = ease_out_cubic(appear) * leaving;
            let lift = (1.0 - ease_out_back(appear)) * 18.0;

            let mut job = LayoutJob::single_section(
                item.message.clone(),
                TextFormat {
                    font_id: theme::regular(theme::FONT_BODY),
                    color: theme::TEXT,
                    ..Default::default()
                },
            );
            job.wrap = TextWrapping::wrap_at_width(440.0_f32.min(screen.width() - 120.0));
            let galley = painter.layout_job(job);
            let size = vec2(galley.size().x + 52.0, galley.size().y + 22.0);
            let rect = Rect::from_min_size(
                pos2(screen.center().x - size.x / 2.0, bottom - size.y + lift),
                size,
            );

            let mut painter = painter.clone();
            painter.multiply_opacity(alpha);
            painter.add(
                Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(150),
                }
                .as_shape(rect, 14),
            );
            painter.rect(
                rect,
                theme::RADIUS_LARGE,
                theme::SURFACE_HOVER,
                Stroke::new(1.0, theme::BORDER),
                StrokeKind::Inside,
            );
            let dot = match item.kind {
                ToastKind::Info => accent,
                ToastKind::Success => theme::SPOTIFY_GREEN,
                ToastKind::Error => theme::DANGER,
            };
            painter.circle_filled(pos2(rect.left() + 20.0, rect.center().y), 4.0, dot);
            painter.galley(
                pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0),
                galley,
                theme::TEXT,
            );

            bottom = rect.top() - 10.0 - lift;
        }
        let animating = self.items.iter().any(|item| {
            anim::elapsed_since(ctx, item.created_at) < FADE || remaining(item) < fade_out
        });
        if animating {
            ctx.request_repaint();
        } else if let Some(next_change) = self
            .items
            .iter()
            .map(|item| remaining(item) - fade_out)
            .reduce(f32::min)
        {
            // Wake up to fade out (or remove) the oldest toast.
            ctx.request_repaint_after(Duration::from_secs_f32(next_change.max(0.0)));
        }
    }
}
