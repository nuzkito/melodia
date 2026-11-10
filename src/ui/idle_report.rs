//! The idle report of the smoke test (`MELODIA_IDLE_REPORT=1`, demo only). After each F18 key, the
//! app writes `idle <n>` to stdout when it is idle: it handled the key, it has no repaint soon,
//! and the demo has no work. `n` counts the F18 keys. The app never sees F18. This is the only I/O
//! of the interface. See `docs/testing-and-dev.md`.

use std::sync::Arc;
use std::time::Duration;

use egui::{Context, Event, FullOutput, Key, RawInput, ViewportId};

use crate::backend::WorkCounter;

/// A repaint sooner than this counts as work that waits: the search sends its request after
/// 0.35 s, and the hearts ask Spotify after 0.2 s. The timers of the progress bar (1 s) and of the
/// device list (10 s) do not wait for work.
const BUSY_REPAINT: Duration = Duration::from_millis(500);

pub struct IdleReport {
    work: Arc<WorkCounter>,
    /// The F18 keys received.
    markers: u64,
    /// The last marker that the report wrote.
    reported: u64,
    /// The frame that received the last marker.
    marker_frame: Option<u64>,
    /// The finished work and if the backend was idle, at the start of the pass.
    work_at_start: (u64, bool),
}

impl IdleReport {
    pub fn new(work: Arc<WorkCounter>) -> Self {
        Self {
            work,
            markers: 0,
            reported: 0,
            marker_frame: None,
            work_at_start: (0, false),
        }
    }
}

impl egui::Plugin for IdleReport {
    fn debug_name(&self) -> &'static str {
        "idle_report"
    }

    fn setup(&mut self, ctx: &Context) {
        // The blink repaints the frame. It also makes the screenshots random.
        ctx.all_styles_mut(|style| style.visuals.text_cursor.blink = false);
    }

    fn input_hook(&mut self, ctx: &Context, input: &mut RawInput) {
        let mut presses = 0;
        input.events.retain(|event| match event {
            Event::Key {
                key: Key::F18,
                pressed,
                ..
            } => {
                presses += u64::from(*pressed);
                false
            }
            _ => true,
        });
        if presses > 0 {
            self.markers += presses;
            self.marker_frame = Some(ctx.cumulative_frame_nr());
        }
        self.work_at_start = self.work.snapshot();
    }

    fn output_hook(&mut self, ctx: &Context, output: &mut FullOutput) {
        let Some(viewport) = output.viewport_output.get_mut(&ViewportId::ROOT) else {
            return;
        };
        // A frame can have more than one pass. The report waits for a later frame: then the frame
        // of the marker is on the screen.
        if self.marker_frame == Some(ctx.cumulative_frame_nr()) {
            viewport.repaint_delay = Duration::ZERO;
            return;
        }
        // Work that ended during the pass can have events that the pass did not read.
        let (finished, idle) = self.work_at_start;
        let backend_idle = idle && self.work.snapshot() == (finished, true);
        if self.reported < self.markers && backend_idle && viewport.repaint_delay >= BUSY_REPAINT {
            self.reported = self.markers;
            println!("idle {}", self.markers);
        }
    }
}
