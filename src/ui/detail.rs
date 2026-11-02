//! The common parts of the album and artist panels. Each panel shows one item of the middle
//! column, which opens from a link under a track and loads from the backend. A [`DetailPanel`]
//! keeps the open item, the item that fades out and the items opened before. When the user opens
//! one of them again, it shows immediately while it loads again.

use std::collections::HashMap;

use egui::{Color32, Context, Id, Key, Modifiers, Painter, Pos2, Rect, Ui};

use super::context_menu;
use super::dnd;
use super::icons;
use super::widgets;
use crate::model::{Album, Artist};

/// An item that a [`DetailPanel`] shows, loaded by its ID.
pub trait Detail: Clone + PartialEq {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
}

impl Detail for Album {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Detail for Artist {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// The item that a panel shows, loaded or not yet loaded.
pub struct Opened<T> {
    pub id: String,
    /// Its name at the position where the user clicked it, shown until it loads.
    pub name: String,
    pub item: Option<T>,
    pub error: Option<String>,
    /// The arrival time, for the entrance animation.
    pub arrived_at: f64,
}

/// The text of a panel while it has nothing to list (see [`Opened::loaded`]).
pub struct Placeholders {
    pub error_id: &'static str,
    /// The title when the item cannot show. The error is its subtitle.
    pub failed: &'static str,
    pub empty_id: &'static str,
    pub empty_title: &'static str,
    pub empty_subtitle: &'static str,
}

impl<T> Opened<T> {
    /// The item, if it loaded with something to list (`has_rows`). Until then, or if it cannot
    /// load, `rect` tells this: an error, placeholder rows while it loads, or that it is empty.
    pub fn loaded(
        &self,
        ui: &Ui,
        rect: Rect,
        time: f64,
        placeholders: &Placeholders,
        has_rows: impl Fn(&T) -> bool,
    ) -> Option<&T> {
        let music_note = |painter: &Painter, center: Pos2, color: Color32| {
            icons::music_note(painter, center, 24.0, color);
        };
        match (&self.item, &self.error) {
            (_, Some(error)) => {
                widgets::empty_state(
                    ui,
                    rect,
                    Id::unique(placeholders.error_id),
                    placeholders.failed,
                    error,
                    1.0,
                    music_note,
                );
                None
            }
            (None, None) => {
                widgets::skeleton_rows(ui, rect, time, 8);
                None
            }
            (Some(item), None) if !has_rows(item) => {
                widgets::empty_state(
                    ui,
                    rect,
                    Id::unique(placeholders.empty_id),
                    placeholders.empty_title,
                    placeholders.empty_subtitle,
                    1.0,
                    music_note,
                );
                None
            }
            (Some(item), None) => Some(item),
        }
    }
}

pub struct DetailPanel<T> {
    open: Option<Opened<T>>,
    /// The item that just closed, shown while the panel fades out.
    closing: Option<Opened<T>>,
    /// The items opened before (id → item).
    cache: HashMap<String, T>,
}

// If derived, it would need `T: Default`.
impl<T> Default for DetailPanel<T> {
    fn default() -> Self {
        Self {
            open: None,
            closing: None,
            cache: HashMap::new(),
        }
    }
}

impl<T: Detail> DetailPanel<T> {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The ID and the name of the item that shows, if there is one.
    pub fn open_id(&self) -> Option<(&str, &str)> {
        self.open
            .as_ref()
            .map(|open| (open.id.as_str(), open.name.as_str()))
    }

    /// Shows the item `id` (with the name `name`), immediately if it opened before.
    pub fn open(&mut self, id: String, name: String, time: f64) {
        let item = self.cache.get(&id).cloned();
        self.closing = None;
        self.open = Some(Opened {
            name: item.as_ref().map_or(name, |item| item.name().to_owned()),
            id,
            item,
            error: None,
            arrived_at: time,
        });
    }

    pub fn close(&mut self) {
        if let Some(open) = self.open.take() {
            self.closing = Some(open);
        }
    }

    /// `item` arrived from the backend.
    pub fn on_loaded(&mut self, item: T, time: f64) {
        if let Some(open) = self.open.as_mut().filter(|open| open.id == item.id()) {
            // It already shows (from the cache): no new entrance animation.
            if open.item.as_ref() != Some(&item) {
                if open.item.is_none() {
                    open.arrived_at = time;
                }
                item.name().clone_into(&mut open.name);
                open.item = Some(item.clone());
            }
            open.error = None;
        }
        self.cache.insert(item.id().to_owned(), item);
    }

    pub fn on_failed(&mut self, id: &str, message: String) {
        // The data that showed from the cache is still good.
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.id == id && open.item.is_none())
        {
            open.error = Some(message);
        }
    }

    /// What the panel shows while it is `open_t` (0..1) open: the item, and true if the user can
    /// use it (it does not fade out). Esc closes it.
    pub fn shown(&mut self, ctx: &Context, open_t: f32) -> Option<(&Opened<T>, bool)> {
        if self.open.is_some() && escape_closes(ctx) {
            self.close();
        }
        if open_t <= 0.001 {
            self.closing = None;
            return None;
        }
        // The panel fades out: it shows, but the user cannot use it.
        let interactive = self.open.is_some();
        let open = self.open.as_ref().or(self.closing.as_ref())?;
        Some((open, interactive))
    }
}

/// Esc closes the panel of the middle column, if the user types nothing, drags nothing and no
/// context menu is open. The panel consumes the key.
pub fn escape_closes(ctx: &Context) -> bool {
    !ctx.text_edit_focused()
        && !dnd::is_dragging(ctx)
        && !context_menu::is_open(ctx)
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
}

/// An item that a link asked to open, until the app takes it.
#[derive(Clone, Default)]
struct Request {
    id: String,
    name: String,
}

/// Asks the app to open the item `id` (with the name `name`) in the panel with requests under
/// `key`.
pub fn request(ctx: &Context, key: Id, id: &str, name: &str) {
    let request = Request {
        id: id.to_owned(),
        name: name.to_owned(),
    };
    ctx.data_mut(|d| d.insert_temp(key, request));
}

/// The item requested with [`request`] under `key`, if there is one: its ID and name.
pub fn take_request(ctx: &Context, key: Id) -> Option<(String, String)> {
    ctx.data_mut(|d| d.remove_temp::<Request>(key))
        .map(|Request { id, name }| (id, name))
}
