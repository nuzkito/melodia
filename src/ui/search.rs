//! The search box, at the top of the middle column, and its results (artists, albums and songs)
//! under it. While the box has the focus, ↑/↓ and Enter select a result without the mouse.

use std::collections::HashMap;

use egui::{Color32, Id, Key, Modifiers, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

use super::album;
use super::anim::{self, ROW_APPEAR_DELAY, ROW_APPEAR_DURATION, ease_out_cubic, lerp_color};
use super::artist::{self, CardStyle, card_row};
use super::context_menu::{self, Choice, ContextMenu, Target};
use super::dnd::{self, DragItem, DragOrigin};
use super::icons;
use super::textures::{THUMB_PX, TextureCache};
use super::theme;
use super::track_row::{ROW_HEIGHT, RowStyle, track_row};
use super::widgets::{self, custom_button_with_ring, single_line};
use crate::backend::Command;
use crate::model::{AlbumHit, ArtistHit, PlaybackSnapshot, SearchPage, Track};

const SEARCH_DEBOUNCE: f64 = 0.35;
pub const SEARCH_FIELD_ID: &str = "search";

/// One result of a search.
enum Hit {
    Artist(ArtistHit),
    Album(AlbumHit),
    Track(Track),
}

#[derive(Default)]
pub struct SearchView {
    query: String,
    edited_at: Option<f64>,
    /// The query of the request in progress (first page).
    pending_query: Option<String>,
    /// The query of the results that show.
    results_query: String,
    results: Vec<Hit>,
    /// The number of songs in `results`: the offset of the next page.
    track_count: usize,
    /// The (offset, arrival time) of each loaded page, for the entrance animation.
    pages: Vec<(usize, f64)>,
    has_more: bool,
    loading_more: bool,
    error: Option<String>,
    /// The tracks that the user recently added to the queue (uri → time), for the ✓ feedback.
    added_at: HashMap<String, f64>,
    focus_requested: bool,
    /// The result selected with ↑/↓ from the search box (which keeps the focus).
    highlighted: Option<usize>,
    /// The highlight moved: scroll its row into view.
    scroll_to_highlighted: bool,
    /// The menu of a result, which opens with a right click.
    menu: ContextMenu,
}

pub struct SearchData<'a> {
    pub playback: &'a PlaybackSnapshot,
    pub accent: Color32,
}

impl SearchView {
    pub fn focus(&mut self) {
        self.focus_requested = true;
    }

    /// Empties the search box, which closes the results.
    pub fn clear(&mut self) {
        self.query.clear();
        self.edited_at = None;
        self.highlighted = None;
    }

    /// The text in the search box.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Puts `query` back in the search box (to go back to an earlier search) and searches for it
    /// immediately, if its results do not show already.
    pub fn restore(&mut self, query: String, actions: &mut Vec<Command>) {
        self.query = query;
        self.highlighted = None;
        let trimmed = self.query.trim();
        let shown =
            self.pending_query.is_none() && self.results_query == trimmed && self.error.is_none();
        if shown {
            self.edited_at = None;
        } else if !trimmed.is_empty() {
            self.start_search(actions);
        }
    }

    /// The results panel shows while there is something to search for.
    pub fn is_open(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn on_results(&mut self, page: SearchPage, time: f64) {
        let first_page = page.offset == 0;
        if first_page && self.pending_query.as_ref() == Some(&page.query) {
            self.pending_query = None;
            self.results_query.clone_from(&page.query);
            self.results.clear();
            self.track_count = 0;
            self.pages.clear();
            self.highlighted = None;
            // Its row can be gone.
            self.menu.close();
        } else if first_page
            || page.query != self.results_query
            || page.offset as usize != self.track_count
        {
            return; // stale response
        }
        self.pages.push((self.results.len(), time));
        self.has_more = page.has_more();
        self.track_count += page.tracks.len();
        // The first page mixes all three types, sorted by relevance. The next pages are songs, in
        // the order that Spotify returns them.
        let order = page.relevance_order();
        let mut hits: Vec<Option<Hit>> = page
            .artists
            .into_iter()
            .map(Hit::Artist)
            .chain(page.albums.into_iter().map(Hit::Album))
            .chain(page.tracks.into_iter().map(Hit::Track))
            .map(Some)
            .collect();
        let hits: Vec<Hit> = order.into_iter().filter_map(|i| hits[i].take()).collect();
        self.results.extend(hits);
        self.loading_more = false;
        self.error = None;
    }

    pub fn on_search_failed(&mut self, query: &str, offset: u32, message: String) {
        if offset == 0 && self.pending_query.as_deref() == Some(query) {
            self.pending_query = None;
            query.clone_into(&mut self.results_query);
            self.results.clear();
            self.track_count = 0;
            self.pages.clear();
            self.highlighted = None;
            self.has_more = false;
            self.error = Some(message);
        } else if query == self.results_query {
            self.loading_more = false;
        }
    }

    /// The search box. When the user types, it searches after a short pause.
    pub fn field(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        accent: Color32,
        actions: &mut Vec<Command>,
        time: f64,
    ) {
        self.added_at.retain(|_, at| time - *at < 2.0);
        let id = Id::unique(SEARCH_FIELD_ID);
        if std::mem::take(&mut self.focus_requested) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        if ui.memory(|m| m.has_focus(id)) {
            self.keyboard(ui, actions);
        }
        let response = widgets::text_field(
            ui,
            rect,
            id,
            "Search",
            &mut self.query,
            "What do you want to listen to?",
            widgets::FieldStyle::Search,
            accent,
        );
        if response.changed() {
            self.edited_at = Some(time);
            self.highlighted = None;
        }
        if response.lost_focus() {
            self.highlighted = None;
        }
        let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        // Esc closes a context menu first.
        let escaped = response.has_focus()
            && !context_menu::is_open(ui.ctx())
            && ui.input(|i| i.key_pressed(Key::Escape));

        let clear_t = anim::tween_bool(ui.ctx(), id.with("clear"), !self.query.is_empty(), 0.15);
        let mut cleared = escaped;
        if clear_t > 0.01 {
            let center = pos2(rect.right() - 22.0, rect.center().y);
            let clear = widgets::icon_button(
                ui,
                center,
                28.0,
                id.with("clear-button"),
                "Clear search",
                true,
                |painter, c, color| {
                    icons::close(
                        painter,
                        c,
                        12.0 * (0.6 + 0.4 * clear_t),
                        color.gamma_multiply(clear_t),
                    );
                },
            );
            cleared |= clear.clicked();
        }
        if cleared {
            self.query.clear();
            self.edited_at = None;
            if escaped {
                response.surrender_focus();
            }
        }

        let query = self.query.trim();
        if query.is_empty() {
            return;
        }
        let already_sent = self.pending_query.as_deref() == Some(query)
            || (self.pending_query.is_none()
                && self.results_query == query
                && self.error.is_none());
        if already_sent {
            self.edited_at = None;
            return;
        }
        match self.edited_at {
            _ if submitted => self.start_search(actions),
            Some(at) if time - at >= SEARCH_DEBOUNCE => self.start_search(actions),
            Some(at) => ui
                .ctx()
                .request_repaint_after_secs((SEARCH_DEBOUNCE - (time - at)) as f32),
            None => {}
        }
    }

    /// ↑/↓ and Enter while the search box has the focus. The box consumes them before the text
    /// field sees them: ↑/↓ would move its cursor, and Enter would remove its focus.
    fn keyboard(&mut self, ui: &Ui, actions: &mut Vec<Command>) {
        if !self.is_open() || self.results.is_empty() {
            return;
        }
        let pressed = |key| ui.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let last = self.results.len() - 1;
        if pressed(Key::ArrowDown) {
            let next = self.highlighted.map_or(0, |i| (i + 1).min(last));
            self.highlighted = Some(next);
            self.scroll_to_highlighted = true;
            // At the end, the next page loads, as with «Show more results».
            if next == last && self.has_more && !self.loading_more {
                self.load_more(actions);
            }
        }
        if pressed(Key::ArrowUp) {
            // Up from the first result goes back to the text.
            self.highlighted = self.highlighted.and_then(|i| i.checked_sub(1));
            self.scroll_to_highlighted = self.highlighted.is_some();
        }
        if let Some(index) = self.highlighted
            && pressed(Key::Enter)
        {
            self.choose(index, ui.ctx(), actions);
        }
    }

    /// The action of Enter on the result `index`: it plays a song, or opens an artist or an album.
    fn choose(&self, index: usize, ctx: &egui::Context, actions: &mut Vec<Command>) {
        match &self.results[index] {
            Hit::Track(_) => actions.extend(self.play(index)),
            Hit::Album(hit) => album::request(ctx, &hit.release.id, &hit.release.name),
            Hit::Artist(hit) => artist::request(ctx, &hit.id, &hit.name),
        }
    }

    fn play(&self, index: usize) -> Option<Command> {
        match &self.results[index] {
            Hit::Track(track) => Some(Command::PlayTrack {
                track: track.clone(),
            }),
            Hit::Album(_) | Hit::Artist(_) => None,
        }
    }

    fn load_more(&mut self, actions: &mut Vec<Command>) {
        self.loading_more = true;
        actions.push(Command::Search {
            query: self.results_query.clone(),
            offset: self.track_count as u32,
        });
    }

    fn start_search(&mut self, actions: &mut Vec<Command>) {
        let query = self.query.trim().to_owned();
        self.pending_query = Some(query.clone());
        self.edited_at = None;
        self.error = None;
        actions.push(Command::Search { query, offset: 0 });
    }

    /// The results in `rect`, as a list similar to the queue: a title row and the tracks under it,
    /// faded and slid in by `open` (0..1).
    pub fn results(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        open: f32,
        data: &SearchData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) {
        if !self.is_open() {
            self.menu.close();
        }
        if open <= 0.001 {
            return;
        }
        let rect = rect.translate(vec2(0.0, -8.0 * (1.0 - open)));
        let mut panel_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("search-results")
                .max_rect(rect.expand2(vec2(8.0, 0.0))),
        );
        panel_ui.multiply_opacity(open);
        // No title row: the results start immediately under the search box.
        let list = widgets::list_rows(rect, Rect::from_min_size(rect.min, Vec2::ZERO));
        let mut list_ui =
            panel_ui.new_child(UiBuilder::new().id_salt("results-list").max_rect(list));
        list_ui.set_clip_rect(list.intersect(panel_ui.clip_rect()));
        self.list(&mut list_ui, data, textures, actions);
        let time = ui.input(|i| i.time);
        if let Some((choice, _, target)) = self.menu.show(ui.ctx(), data.accent) {
            if let (Target::Track(track), Choice::PlayLast) = (&target, choice) {
                self.added_at.insert(track.uri.clone(), time);
            }
            match (target, choice) {
                (Target::Track(track), Choice::Play) => {
                    actions.push(Command::PlayTrack { track });
                }
                (target, choice) => actions.extend(context_menu::command(choice, target)),
            }
        }
    }

    fn list(
        &mut self,
        ui: &mut Ui,
        data: &SearchData<'_>,
        textures: &mut TextureCache,
        actions: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let time = ctx.input(|i| i.time);
        let loading = self.pending_query.is_some() || self.edited_at.is_some();

        if self.results.is_empty() {
            let rect = ui.max_rect();
            if loading {
                widgets::skeleton_rows(ui, rect, time, 8);
            } else if let Some(error) = &self.error {
                widgets::empty_state(
                    ui,
                    rect,
                    Id::unique("results-error"),
                    "Search failed",
                    error,
                    1.0,
                    |p, c, color| icons::close(p, c, 20.0, color),
                );
            } else {
                let subtitle = format!("Nothing found for “{}”.", self.results_query);
                widgets::empty_state(
                    ui,
                    rect,
                    Id::unique("results-empty"),
                    "No results",
                    &subtitle,
                    1.0,
                    |p, c, color| icons::search(p, c, 24.0, color),
                );
            }
            return;
        }

        let current_uri = data.playback.track_uri().map(str::to_owned);
        let playing = anim::tween_bool(
            &ctx,
            Id::unique("results-playing"),
            data.playback.is_playing,
            0.4,
        );
        let scrolled = egui::ScrollArea::vertical()
            .id_salt(("results-scroll", self.results_query.as_str()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let footer = if self.has_more || self.loading_more {
                    72.0
                } else {
                    8.0
                };
                let (area, _) = ui.allocate_exact_size(
                    vec2(width, self.results.len() as f32 * ROW_HEIGHT + footer),
                    Sense::hover(),
                );
                let mut play = None;
                let mut menu_at = None;
                // Dim old results while a new search is in progress.
                let dim = anim::tween_bool(&ctx, Id::unique("results-dim"), loading, 0.2);
                for (i, hit) in self.results.iter().enumerate() {
                    let (page_start, arrived) = self
                        .pages
                        .iter()
                        .rev()
                        .find(|(start, _)| *start <= i)
                        .copied()
                        .unwrap_or((0, f64::NEG_INFINITY));
                    let elapsed = anim::elapsed_since(&ctx, arrived);
                    let appear = ease_out_cubic(anim::stagger(
                        elapsed,
                        i - page_start,
                        ROW_APPEAR_DELAY,
                        ROW_APPEAR_DURATION,
                    ));
                    anim::keep_animating(
                        &ctx,
                        elapsed,
                        ROW_APPEAR_DELAY * 10.0 + ROW_APPEAR_DURATION,
                    );
                    let rect = Rect::from_min_size(
                        pos2(
                            area.left(),
                            area.top() + i as f32 * ROW_HEIGHT + (1.0 - appear) * 14.0,
                        ),
                        vec2(width, ROW_HEIGHT),
                    );
                    let highlighted = self.highlighted == Some(i);
                    if highlighted && std::mem::take(&mut self.scroll_to_highlighted) {
                        ui.scroll_to_rect(rect, None);
                    }
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let alpha = appear * (1.0 - 0.5 * dim);
                    match hit {
                        Hit::Track(track) => {
                            let cover = track
                                .cover_small
                                .as_deref()
                                .and_then(|url| textures.get(url, THUMB_PX, time));
                            let added = self
                                .added_at
                                .get(&track.uri)
                                .map(|at| anim::elapsed_since(&ctx, *at));
                            let row = track_row(
                                ui,
                                rect,
                                Id::unique(("result-row", i)),
                                track,
                                cover.as_ref(),
                                RowStyle {
                                    alpha,
                                    current: current_uri.as_deref() == Some(track.uri.as_str()),
                                    playing,
                                    pause_button: false,
                                    draggable: true,
                                    can_add: true,
                                    remove_from: None,
                                    locked: false,
                                    added,
                                    highlighted,
                                    menu_open: self.menu.is_open_for(i),
                                    accent: data.accent,
                                    time,
                                },
                            );
                            if row.play {
                                play = Some(i);
                            }
                            if let Some(pos) = row.menu_at {
                                menu_at = Some((i, pos));
                            }
                            if row.add {
                                actions.push(Command::AddToQueue {
                                    track: track.clone(),
                                });
                                self.added_at.insert(track.uri.clone(), time);
                            }
                            if let Some(pointer) = row.drag_from {
                                dnd::start(
                                    &ctx,
                                    DragItem::Track(track.clone()),
                                    DragOrigin::Search,
                                    rect,
                                    pointer,
                                );
                            }
                        }
                        Hit::Album(hit) => {
                            let cover = hit
                                .release
                                .cover
                                .as_deref()
                                .and_then(|url| textures.get(url, THUMB_PX, time));
                            let row = card_row(
                                ui,
                                rect,
                                Id::unique(("result-row", i)),
                                &hit.release.name,
                                &hit.subtitle(),
                                cover.as_ref(),
                                CardStyle {
                                    alpha,
                                    highlighted,
                                    draggable: true,
                                    menu_open: self.menu.is_open_for(i),
                                },
                            );
                            if row.clicked {
                                album::request(&ctx, &hit.release.id, &hit.release.name);
                            }
                            if let Some(pos) = row.menu_at {
                                menu_at = Some((i, pos));
                            }
                            if let Some(pointer) = row.drag_from {
                                dnd::start(
                                    &ctx,
                                    DragItem::Release(hit.release.clone()),
                                    DragOrigin::Search,
                                    rect,
                                    pointer,
                                );
                            }
                        }
                        Hit::Artist(hit) => {
                            let cover = hit
                                .cover
                                .as_deref()
                                .and_then(|url| textures.get(url, THUMB_PX, time));
                            let row = card_row(
                                ui,
                                rect,
                                Id::unique(("result-row", i)),
                                &hit.name,
                                "Artist",
                                cover.as_ref(),
                                CardStyle {
                                    alpha,
                                    highlighted,
                                    draggable: false,
                                    menu_open: self.menu.is_open_for(i),
                                },
                            );
                            if row.clicked {
                                artist::request(&ctx, &hit.id, &hit.name);
                            }
                            if let Some(pos) = row.menu_at {
                                menu_at = Some((i, pos));
                            }
                        }
                    }
                }
                if let Some(index) = play {
                    actions.extend(self.play(index));
                }
                if let Some((index, pos)) = menu_at {
                    let target = match &self.results[index] {
                        Hit::Track(track) => Target::Track(track.clone()),
                        Hit::Album(hit) => Target::album(&hit.release),
                        Hit::Artist(hit) => Target::artist(hit),
                    };
                    self.menu.open(&ctx, target, index, pos, false);
                }

                let footer_rect = Rect::from_min_size(
                    pos2(area.left(), area.bottom() - footer),
                    vec2(width, footer),
                );
                if self.loading_more {
                    icons::loading_dots(
                        ui.painter(),
                        footer_rect.center(),
                        10.0,
                        time,
                        theme::TEXT_DIM,
                    );
                    anim::request_frame(&ctx);
                } else if self.has_more {
                    let button = Rect::from_center_size(footer_rect.center(), vec2(200.0, 38.0));
                    let response = custom_button_with_ring(
                        ui,
                        button,
                        Id::unique("load-more"),
                        "Show more results",
                        true,
                        button.height() / 2.0,
                        |painter, r, hover, press| {
                            let r = r.shrink(press * 2.0);
                            painter.rect_filled(
                                r,
                                r.height() / 2.0,
                                Color32::from_white_alpha((10.0 + 14.0 * hover) as u8),
                            );
                            let color = lerp_color(theme::TEXT_DIM, theme::TEXT, hover);
                            let galley = single_line(
                                painter,
                                "Show more results",
                                theme::regular(theme::FONT_BODY),
                                color,
                                r.width(),
                            );
                            painter.galley(r.center() - galley.size() / 2.0, galley, color);
                        },
                    );
                    if response.clicked() {
                        self.load_more(actions);
                    }
                }
            });
        widgets::scroll_hints(ui, &scrolled, (0.0, 0.0));
    }
}
