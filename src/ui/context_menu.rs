//! The right-click menu of a song, an album, an artist or a playlist. Each view keeps its own
//! [`ContextMenu`] and changes the choice into commands ([`command`] has the commands that all
//! views share). «Like» and «Unlike» go directly to [`liked::toggle`]. «Share» copies the link to
//! the item in Spotify's web player.

use egui::{Context, Id, Key, Margin, Order, Pos2, vec2};

use super::anim;
use super::backdrop;
use super::devices::menu_row;
use super::icons;
use super::liked;
use super::theme;
use super::toasts;
use crate::backend::{Command, QueueAt, ToastKind};
use crate::model::{ArtistHit, Playlist, QueueEdit, Release, Track, web_link};

const MENU_WIDTH: f32 = 220.0;

/// The choice in the menu, for the item at `index` of the list of the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Plays it now: a song as the ▶ of its row does, and an album from its first song, with the
    /// other songs first in the queue.
    Play,
    /// Puts it (all its songs, for an album) first in the queue.
    PlayNext,
    /// Puts it (all its songs, for an album) at the end of the queue.
    PlayLast,
    /// Removes a song from the open playlist.
    Remove,
}

/// The item of the menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Track(Track),
    Album {
        id: String,
        name: String,
    },
    /// The menu has only «Share».
    Artist {
        id: String,
        name: String,
    },
    /// The menu has only «Share».
    Playlist {
        id: String,
        name: String,
    },
}

impl Target {
    pub fn album(release: &Release) -> Self {
        Self::Album {
            id: release.id.clone(),
            name: release.name.clone(),
        }
    }

    pub fn artist(hit: &ArtistHit) -> Self {
        Self::Artist {
            id: hit.id.clone(),
            name: hit.name.clone(),
        }
    }

    pub fn playlist(playlist: &Playlist) -> Self {
        Self::Playlist {
            id: playlist.id.clone(),
            name: playlist.name.clone(),
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Track(track) => &track.name,
            Self::Album { name, .. } | Self::Artist { name, .. } | Self::Playlist { name, .. } => {
                name
            }
        }
    }

    /// The menu has «Play», «Play next» and «Play last».
    fn playable(&self) -> bool {
        matches!(self, Self::Track(_) | Self::Album { .. })
    }

    /// The link that «Share» copies. `None` if the item has no page in Spotify (a local file).
    fn link(&self) -> Option<String> {
        let (kind, id) = match self {
            Self::Track(track) => return track.web_link(),
            Self::Album { id, .. } => ("album", id),
            Self::Artist { id, .. } => ("artist", id),
            Self::Playlist { id, .. } => ("playlist", id),
        };
        (!id.is_empty()).then(|| web_link(kind, id))
    }
}

/// The command for `choice` on `target` that is the same in all views: all choices for an album,
/// and «Play next» and «Play last» for a song. `None` for the other choices, which each view does
/// in its own way.
pub fn command(choice: Choice, target: Target) -> Option<Command> {
    match (target, choice) {
        (Target::Album { id, name }, choice) => {
            let at = match choice {
                Choice::Play => QueueAt::Now,
                Choice::PlayNext => QueueAt::Next(0),
                Choice::PlayLast => QueueAt::End,
                Choice::Remove => return None,
            };
            Some(Command::QueueAlbum { id, name, at })
        }
        (Target::Track(track), Choice::PlayNext) => {
            Some(Command::EditQueue(QueueEdit::Insert { index: 0, track }))
        }
        (Target::Track(track), Choice::PlayLast) => Some(Command::AddToQueue { track }),
        (Target::Track(_), Choice::Play | Choice::Remove)
        | (Target::Artist { .. } | Target::Playlist { .. }, _) => None,
    }
}

#[derive(Default)]
pub struct ContextMenu {
    menu: Option<Opened>,
    /// It shows and the user can use it. While it fades out, `menu` is still there.
    open: bool,
}

struct Opened {
    /// It is different for each opening. Thus the menus of two views can fade at the same time.
    id: Id,
    target: Target,
    index: usize,
    pos: Pos2,
    since: f64,
    removable: bool,
}

/// True if a context menu was open in this frame or in the last frame. Then Esc closes the menu,
/// not the panel under it. When the view of a menu stops to draw it, it stops to count
/// immediately.
pub fn is_open(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<u64>(open_id()))
        .is_some_and(|frame| ctx.cumulative_frame_nr() <= frame + 1)
}

fn open_id() -> Id {
    Id::unique("context-menu-open")
}

impl ContextMenu {
    /// Opens the menu of `target`, the item at `index` of the list, at `pos`. `removable`: a song
    /// in a playlist of the user.
    pub fn open(
        &mut self,
        ctx: &Context,
        target: Target,
        index: usize,
        pos: Pos2,
        removable: bool,
    ) {
        let since = ctx.input(|i| i.time);
        let id = Id::unique("context-menu").with((since.to_bits(), index));
        anim::set_tween_value(ctx, id.with("shown"), 0.0);
        self.menu = Some(Opened {
            id,
            target,
            index,
            pos,
            since,
            removable,
        });
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// The menu is open for the item at `index`: its row stays lit.
    pub fn is_open_for(&self, index: usize) -> bool {
        self.open && self.menu.as_ref().is_some_and(|menu| menu.index == index)
    }

    /// Draws the menu, if it is open, and returns the choice, with the index and the item of the
    /// menu. A click outside or Esc closes it.
    pub fn show(
        &mut self,
        ctx: &Context,
        accent: egui::Color32,
    ) -> Option<(Choice, usize, Target)> {
        let menu = self.menu.as_ref()?;
        let id = menu.id;
        let shown = anim::tween_bool(ctx, id.with("shown"), self.open, 0.16);
        if !self.open && shown <= 0.001 {
            self.menu = None;
            return None;
        }
        let interactive = self.open;
        let track = match &menu.target {
            Target::Track(track) => Some(track),
            Target::Album { .. } | Target::Artist { .. } | Target::Playlist { .. } => None,
        };
        let playable = menu.target.playable();
        let link = menu.target.link();
        let can_like = track.is_some_and(liked::can_like);
        let liked = can_like && track.is_some_and(|t| liked::state(ctx, &t.uri) == Some(true));
        let mut choice = None;
        let mut toggle_like = false;
        let mut share = false;
        let area = egui::Area::new(id)
            .order(Order::Foreground)
            .fixed_pos(menu.pos + vec2(0.0, 4.0 - (1.0 - shown) * 8.0))
            .constrain(true)
            .show(ctx, |ui| {
                ui.set_opacity(shown);
                if !interactive {
                    ui.disable();
                }
                egui::Frame::new()
                    .corner_radius(theme::RADIUS_LARGE)
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        backdrop::framed(ui, Margin::same(8), accent, |ui| {
                            ui.set_width(MENU_WIDTH);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let row = |ui: &mut egui::Ui, key: &str, label: &str| {
                                menu_row(
                                    ui,
                                    MENU_WIDTH,
                                    id.with(key),
                                    label,
                                    None,
                                    false,
                                    accent,
                                    |_, _, _| {},
                                )
                                .clicked()
                            };
                            if playable {
                                let play = menu_row(
                                    ui,
                                    MENU_WIDTH,
                                    id.with("play"),
                                    "Play",
                                    None,
                                    false,
                                    accent,
                                    |painter, center, color| {
                                        icons::play_pause(painter, center, 16.0, 1.0, color);
                                    },
                                );
                                if play.clicked() {
                                    choice = Some(Choice::Play);
                                }
                                if row(ui, "next", "Play next") {
                                    choice = Some(Choice::PlayNext);
                                }
                                if row(ui, "last", "Play last") {
                                    choice = Some(Choice::PlayLast);
                                }
                            }
                            if can_like {
                                let label = if liked { "Unlike" } else { "Like" };
                                // A broken heart removes it. A full heart saves it.
                                let like = menu_row(
                                    ui,
                                    MENU_WIDTH,
                                    id.with("like"),
                                    label,
                                    None,
                                    false,
                                    accent,
                                    |painter, center, color| {
                                        let split = if liked { 1.0 } else { 0.0 };
                                        icons::heart_split(
                                            painter, center, 16.0, 0.0, color, split,
                                        );
                                    },
                                );
                                toggle_like = like.clicked();
                            }
                            if link.is_some() {
                                share = menu_row(
                                    ui,
                                    MENU_WIDTH,
                                    id.with("share"),
                                    "Share",
                                    None,
                                    false,
                                    accent,
                                    |painter, center, color| {
                                        icons::share(painter, center, 16.0, color);
                                    },
                                )
                                .clicked();
                            }
                            if menu.removable && row(ui, "remove", "Remove from playlist") {
                                choice = Some(Choice::Remove);
                            }
                        });
                    });
            });
        if !interactive {
            return None;
        }
        let frame = ctx.cumulative_frame_nr();
        ctx.data_mut(|d| d.insert_temp(open_id(), frame));
        if toggle_like && let Some(track) = track {
            liked::toggle(ctx, track, !liked);
        }
        if share && let Some(link) = link {
            ctx.copy_text(link);
            let message = format!("Copied the link to “{}”", menu.target.name());
            toasts::request(ctx, ToastKind::Success, message);
        }
        if choice.is_some() || toggle_like || share {
            let chosen = choice.map(|choice| (choice, menu.index, menu.target.clone()));
            self.open = false;
            return chosen;
        }
        // The click that opened it does not close it.
        let rect = area.response.rect;
        let since = menu.since;
        let clicked_outside = ctx.input(|i| {
            i.time > since
                && i.pointer.any_click()
                && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
        });
        if clicked_outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.open = false;
        }
        None
    }
}
