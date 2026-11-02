//! The «Changelog» dialog, which opens from the «☰» menu. It lists the entries of `CHANGELOG.md`
//! (embedded in the app at build time), newest first. It uses a window similar to the other
//! dialogs ([`backdrop::framed`] over a modal).

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Color32, Context, Frame, Id, Key, Margin, Modifiers, Rect, ScrollArea, Sense, Ui, pos2, vec2,
};

use super::backdrop;
use super::theme;
use super::widgets::{a11y_label, pill_button, single_line};
use super::window_frame;

const WIDTH: f32 = 400.0;
const MAX_HEIGHT: f32 = 380.0;
const BUTTON_HEIGHT: f32 = 36.0;
const TITLE: &str = "Changelog";

const SOURCE: &str = include_str!("../../CHANGELOG.md");

/// A version of the changelog, with its release date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub version: String,
    pub date: String,
    pub blocks: Vec<Block>,
}

/// A part of an entry: a `- change` line, or a paragraph of free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Change(String),
    Paragraph(String),
}

impl Entry {
    /// «0.1.0 · 2026-10-24», or only the version if there is no date.
    fn heading(&self) -> String {
        if self.date.is_empty() {
            self.version.clone()
        } else {
            format!("{} · {}", self.version, self.date)
        }
    }
}

/// The entries in `markdown`, in their order. A `## version - date` line starts an entry. Below
/// it, each `- change` line adds a change, and other text adds a paragraph. As in Markdown, a line
/// directly below a block continues it. The function skips the lines before the first entry.
pub fn parse(markdown: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    // True if the previous line is part of the last block of the last entry.
    let mut open = false;
    for line in markdown.lines().map(str::trim) {
        if let Some(heading) = line.strip_prefix("## ") {
            let (version, date) = heading.split_once(" - ").unwrap_or((heading, ""));
            entries.push(Entry {
                version: version.trim().to_owned(),
                date: date.trim().to_owned(),
                blocks: Vec::new(),
            });
            open = false;
            continue;
        }
        let Some(entry) = entries.last_mut() else {
            continue;
        };
        if line.is_empty() {
            open = false;
        } else if let Some(change) = line.strip_prefix("- ") {
            entry.blocks.push(Block::Change(change.trim().to_owned()));
            open = true;
        } else if open
            && let Some(Block::Change(text) | Block::Paragraph(text)) = entry.blocks.last_mut()
        {
            text.push(' ');
            text.push_str(line);
        } else {
            entry.blocks.push(Block::Paragraph(line.to_owned()));
            open = true;
        }
    }
    entries
}

/// The state of the dialog (open or closed), and the entries that it shows.
pub struct Changelog {
    open: bool,
    entries: Vec<Entry>,
}

impl Default for Changelog {
    fn default() -> Self {
        Self {
            open: false,
            entries: parse(SOURCE),
        }
    }
}

impl Changelog {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    /// Esc and Enter close it. The UI calls it before the views, so that they do not see those
    /// keys while it is open.
    pub fn keyboard(&mut self, ctx: &Context) {
        if !self.open {
            return;
        }
        let pressed = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if pressed(Key::Escape) || (ctx.memory(|m| m.focused().is_none()) && pressed(Key::Enter)) {
            self.open = false;
        }
    }

    /// The dialog, above all other items, while it is open. A click outside it closes it.
    pub fn show(&mut self, ctx: &Context, accent: Color32) {
        if !self.open {
            return;
        }
        // Only the shadow: `backdrop::framed` draws the other parts.
        let outer = Frame::new()
            .corner_radius(theme::RADIUS_LARGE)
            .shadow(ctx.global_style().visuals.popup_shadow);
        let entries = &self.entries;
        let modal = egui::Modal::new(Id::unique("changelog-dialog"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(outer)
            .show(ctx, |ui| {
                backdrop::framed(ui, Margin::same(20), accent, |ui| {
                    contents(ui, entries, accent)
                })
            });
        window_frame::drag_through_backdrop(ctx, &modal.backdrop_response);
        if modal.inner || modal.should_close() {
            self.open = false;
        }
    }
}

/// A paragraph of `text` wrapped to `width`, with an accessible label.
fn paragraph(ui: &mut Ui, id: Id, text: &str, width: f32, color: Color32) {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: theme::regular(theme::FONT_BODY),
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::wrap_at_width(width);
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
    a11y_label(ui, rect, id, text);
}

/// The title, the `entries` and the «Close» button. Returns true if the user clicked the button.
fn contents(ui: &mut Ui, entries: &[Entry], accent: Color32) -> bool {
    ui.set_width(WIDTH);
    let painter = ui.painter().clone();
    let title = single_line(
        &painter,
        TITLE,
        theme::strong(theme::FONT_TITLE),
        theme::TEXT,
        WIDTH,
    );
    let (title_rect, _) = ui.allocate_exact_size(title.size(), Sense::hover());
    painter.galley(title_rect.min, title, theme::TEXT);
    a11y_label(ui, title_rect, Id::unique("changelog-title"), TITLE);
    ui.add_space(12.0);

    ScrollArea::vertical()
        .max_height(MAX_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_width(WIDTH);
            for (i, entry) in entries.iter().enumerate() {
                if i > 0 {
                    ui.add_space(14.0);
                }
                let heading = entry.heading();
                let date = single_line(
                    &painter,
                    &heading,
                    theme::strong(theme::FONT_SMALL),
                    accent,
                    WIDTH,
                );
                let (rect, _) = ui.allocate_exact_size(date.size(), Sense::hover());
                painter.galley(rect.min, date, accent);
                a11y_label(ui, rect, Id::unique(("changelog-date", i)), &heading);
                for (j, block) in entry.blocks.iter().enumerate() {
                    let id = Id::unique(("changelog-block", i, j));
                    match block {
                        Block::Change(change) => {
                            ui.add_space(4.0);
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let (dot, _) =
                                    ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                                let center = dot.center() - vec2(3.0, 0.0);
                                painter.circle_filled(center, 2.0, theme::TEXT_DIM);
                                paragraph(ui, id, change, WIDTH - 16.0, theme::TEXT_DIM);
                            });
                        }
                        Block::Paragraph(text) => {
                            ui.add_space(if j > 0 { 8.0 } else { 4.0 });
                            paragraph(ui, id, text, WIDTH, theme::TEXT_DIM);
                        }
                    }
                }
            }
        });
    ui.add_space(20.0);

    let (row, _) = ui.allocate_exact_size(vec2(WIDTH, BUTTON_HEIGHT), Sense::hover());
    let rect = Rect::from_min_size(
        pos2(row.right() - 100.0, row.top()),
        vec2(100.0, BUTTON_HEIGHT),
    );
    pill_button(
        ui,
        rect,
        Id::unique("changelog-close"),
        "Close",
        accent,
        theme::BG,
        true,
    )
    .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entries_are_read_with_their_dates_and_changes() {
        let entries = parse(
            "# Changelog\n\nSome text\n\n## 0.2.0 - 2026-02-01\n\n- Second\n- Third\n\n## 0.1.0 - 2026-01-01\n- First\n",
        );
        assert_eq!(
            entries,
            [
                Entry {
                    version: "0.2.0".into(),
                    date: "2026-02-01".into(),
                    blocks: vec![
                        Block::Change("Second".into()),
                        Block::Change("Third".into())
                    ],
                },
                Entry {
                    version: "0.1.0".into(),
                    date: "2026-01-01".into(),
                    blocks: vec![Block::Change("First".into())],
                },
            ]
        );
    }

    #[test]
    fn free_text_in_an_entry_is_a_paragraph_and_joins_the_lines_below_it() {
        let entries = parse(
            "## 0.1.0 - 2026-01-01\n\nHello\nNuria!\n\nBye, Pablo\n- Play\n  Queen\nSearch\n",
        );
        assert_eq!(
            entries[0].blocks,
            [
                Block::Paragraph("Hello Nuria!".into()),
                Block::Paragraph("Bye, Pablo".into()),
                Block::Change("Play Queen Search".into()),
            ]
        );
    }

    #[test]
    fn changes_before_any_date_are_skipped() {
        assert!(parse("- Orphan\n").is_empty());
    }

    #[test]
    fn the_embedded_changelog_has_entries() {
        assert!(parse(SOURCE).iter().all(|e| !e.blocks.is_empty()));
        assert!(!parse(SOURCE).is_empty());
    }
}
