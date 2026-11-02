//! Animation helpers: easing curves and time-based tweens kept in the temporary memory of egui.
//!
//! Durations are in seconds, as written. The `animation_time` style setting of egui only turns
//! them off. At zero (as `egui_kittest` and `MELODIA_NO_ANIMATIONS` set it), all transitions are
//! instant.

use std::f32::consts::PI;
use std::hash::Hash;

use egui::{Color32, Context, Id};

/// True if transitions animate. The `animation_time` of egui is zero in the tests and with
/// `MELODIA_NO_ANIMATIONS`, and then all transitions are instant.
pub fn enabled(ctx: &Context) -> bool {
    ctx.global_style().animation_time > 0.0
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_in_quad(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(2)
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Goes a little past the target before it stops: a small "pop".
pub fn ease_out_back(t: f32) -> f32 {
    const C1: f32 = 1.70158;
    const C3: f32 = C1 + 1.0;
    let t = t.clamp(0.0, 1.0);
    1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
}

/// 0 → 1 → 0 as `t` goes through `0..=1`.
pub fn bump(t: f32) -> f32 {
    (t.clamp(0.0, 1.0) * PI).sin()
}

pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

pub fn lerp_color(from: Color32, to: Color32, t: f32) -> Color32 {
    from.lerp_to_gamma(to, t.clamp(0.0, 1.0))
}

/// The entrance of list rows (playlists, search results, queue): the seconds between one row and
/// the next, and the seconds for each row to appear.
pub const ROW_APPEAR_DELAY: f32 = 0.018;
pub const ROW_APPEAR_DURATION: f32 = 0.18;

/// The progress of the item at `index` of a staggered animation.
pub fn stagger(elapsed: f32, index: usize, delay: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    ((elapsed - index as f32 * delay) / duration).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Tween {
    from: f32,
    to: f32,
    start: f64,
    duration: f32,
}

impl Tween {
    fn value(&self, now: f64, easing: fn(f32) -> f32) -> f32 {
        if self.duration <= 0.0 {
            return self.to;
        }
        let t = ((now - self.start) / f64::from(self.duration)) as f32;
        if t >= 1.0 {
            self.to
        } else {
            lerp(self.from, self.to, easing(t.max(0.0)))
        }
    }

    fn running(&self, now: f64) -> bool {
        now < self.start + f64::from(self.duration)
    }
}

/// Animates smoothly (ease-out) towards `target`. A new target during the animation continues from
/// the current value. The first call returns `target`.
pub fn tween(ctx: &Context, id: Id, target: f32, duration: f32) -> f32 {
    tween_with(ctx, id, target, duration, ease_out_cubic)
}

pub fn tween_bool(ctx: &Context, id: Id, on: bool, duration: f32) -> f32 {
    tween(ctx, id, if on { 1.0 } else { 0.0 }, duration)
}

pub fn tween_with(
    ctx: &Context,
    id: Id,
    target: f32,
    duration: f32,
    easing: fn(f32) -> f32,
) -> f32 {
    let now = ctx.input(|i| i.time);
    let stored = ctx.data(|d| d.get_temp::<Tween>(id));
    let tween = match stored {
        Some(tween) if (tween.to - target).abs() < 1e-3 => tween,
        Some(tween) => Tween {
            from: tween.value(now, easing),
            to: target,
            start: now,
            duration: if enabled(ctx) { duration } else { 0.0 },
        },
        None => Tween {
            from: target,
            to: target,
            start: now,
            duration: 0.0,
        },
    };
    if stored != Some(tween) {
        ctx.data_mut(|d| d.insert_temp(id, tween));
    }
    if tween.running(now) {
        ctx.request_repaint();
    }
    tween.value(now, easing)
}

/// Makes the tween `id` continue from `value`. The next [`tween`] call animates from there to its
/// target (a dropped row glides from the release point into its position).
pub fn set_tween_value(ctx: &Context, id: Id, value: f32) {
    let now = ctx.input(|i| i.time);
    let tween = Tween {
        from: value,
        to: value,
        start: now,
        duration: 0.0,
    };
    ctx.data_mut(|d| d.insert_temp(id, tween));
}

#[derive(Clone, Copy, Debug)]
struct Change {
    key: u64,
    at: f64,
}

/// The seconds after `key` last changed for `id` (infinite when animations are disabled). When
/// `animate_first` is true, the first key counts as a change.
///
/// Callers that animate with the result must request repaints themselves (see
/// [`keep_animating`]).
pub fn since_change(
    ctx: &Context,
    id: Id,
    key: impl Hash + std::fmt::Debug,
    animate_first: bool,
) -> f32 {
    let now = ctx.input(|i| i.time);
    let key = Id::unique(key).value();
    let stored = ctx.data(|d| d.get_temp::<Change>(id));
    let at = match stored {
        Some(change) if change.key == key => change.at,
        Some(_) => now,
        None if animate_first => now,
        None => f64::NEG_INFINITY,
    };
    if stored.is_none_or(|change| change.key != key) {
        ctx.data_mut(|d| d.insert_temp(id, Change { key, at }));
    }
    animated_elapsed(ctx, now - at)
}

/// Makes [`since_change`] see `key` for `id` as set long ago, so that it does not animate it (an
/// item that appears where a different item already was).
pub fn settle_change(ctx: &Context, id: Id, key: impl Hash + std::fmt::Debug) {
    let key = Id::unique(key).value();
    ctx.data_mut(|d| {
        d.insert_temp(
            id,
            Change {
                key,
                at: f64::NEG_INFINITY,
            },
        );
    });
}

#[derive(Clone, Copy, Debug)]
struct TriggeredAt(f64);

/// The seconds after the last time `trigger` was true for `id` (infinite if never, or when
/// animations are disabled).
pub fn since_trigger(ctx: &Context, id: Id, trigger: bool) -> f32 {
    let now = ctx.input(|i| i.time);
    if trigger {
        ctx.data_mut(|d| d.insert_temp(id, TriggeredAt(now)));
    }
    let start = ctx
        .data(|d| d.get_temp::<TriggeredAt>(id))
        .map_or(f64::NEG_INFINITY, |p| p.0);
    animated_elapsed(ctx, now - start)
}

/// The seconds after the UI time `start` (infinite when animations are disabled).
pub fn elapsed_since(ctx: &Context, start: f64) -> f32 {
    animated_elapsed(ctx, ctx.input(|i| i.time) - start)
}

/// `elapsed` for an animation: infinite (complete) when animations are disabled.
fn animated_elapsed(ctx: &Context, elapsed: f64) -> f32 {
    if !enabled(ctx) || !elapsed.is_finite() {
        return f32::INFINITY;
    }
    elapsed as f32
}

/// Schedules the next frame of a looping animation (loading indicators, equalizer…), if
/// animations are not disabled. Finite transitions request immediate repaints.
pub fn request_frame(ctx: &Context) {
    request_frame_after(ctx, std::time::Duration::from_millis(16));
}

pub fn request_frame_after(ctx: &Context, delay: std::time::Duration) {
    if enabled(ctx) {
        ctx.request_repaint_after(delay);
    }
}

/// The progress of a looping animation of `period` seconds, from 0 to 1. It also schedules its
/// next frame every `frame` seconds. Always 0 (no motion) when animations are disabled.
pub fn cycle(ctx: &Context, period: f32, frame: f32) -> f32 {
    if !enabled(ctx) {
        return 0.0;
    }
    request_frame_after(ctx, std::time::Duration::from_secs_f32(frame));
    (ctx.input(|i| i.time) / f64::from(period)).fract() as f32
}

/// Requests a repaint while an animation of `duration` seconds, which started `elapsed` seconds
/// ago, still runs.
pub fn keep_animating(ctx: &Context, elapsed: f32, duration: f32) {
    if elapsed < duration {
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easings_start_at_zero_and_end_at_one() {
        for easing in [ease_out_cubic, ease_in_out_cubic, ease_out_back] {
            assert!(easing(0.0).abs() < 1e-6);
            assert!((easing(1.0) - 1.0).abs() < 1e-6);
        }
        assert!(ease_out_back(0.7) > 1.0, "overshoots");
        assert!(bump(0.5) > 0.99 && bump(0.0).abs() < 1e-6);
    }

    #[test]
    fn stagger_delays_items() {
        assert_eq!(stagger(0.1, 0, 0.05, 0.2), 0.5);
        assert_eq!(stagger(0.1, 2, 0.05, 0.2), 0.0);
        assert_eq!(stagger(1.0, 2, 0.05, 0.2), 1.0);
    }

    #[test]
    fn tween_retargets_from_the_current_value() {
        let ctx = Context::default();
        let id = Id::unique("tween");
        let mut value = 0.0;
        let mut frame = |time: f64, target: f32| {
            let input = egui::RawInput {
                time: Some(time),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| value = tween(ui.ctx(), id, target, 1.0));
            output.textures_delta.clear(); // nothing renders them here
            value
        };
        assert_eq!(frame(0.0, 0.0), 0.0);
        frame(1.0, 1.0); // starts animating towards 1
        let mid = frame(1.5, 1.0);
        assert!(mid > 0.0 && mid < 1.0);
        // New target: continues from `mid`, and does not jump back to 0.
        let back = frame(1.5, 0.0);
        assert!((back - mid).abs() < 1e-4);
        assert_eq!(frame(100.0, 0.0), 0.0);
    }
}
