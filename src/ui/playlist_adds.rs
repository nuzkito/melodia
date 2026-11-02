//! The dialog that asks the user before the app adds tracks that a playlist already has. The UI
//! knows if the playlist has them if it read its tracks or added them from here. If not, the
//! backend looks ([`Command::FindInPlaylist`], [`Command::FindAlbumInPlaylist`]), and the add
//! waits for its answer.

use std::collections::{HashMap, HashSet};

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Color32, Context, Frame, Id, Key, Margin, Modifiers, Rect, Sense, TextFormat, Ui, pos2, vec2,
};

use super::anim;
use super::backdrop;
use super::theme;
use super::widgets::{a11y_label, pill_button, single_line};
use super::window_frame;
use crate::backend::Command;
use crate::model::{Playlist, PlaylistEdit, Track};
use crate::util::plural;

const WIDTH: f32 = 360.0;
const BUTTON_HEIGHT: f32 = 36.0;
const BUTTON_GAP: f32 = 8.0;
/// The seconds for the dialog to fade out when it closes.
const FADE: f32 = 0.15;

/// A track to add to a playlist, at `index` of its tracks (at the end with `None`), as in
/// [`crate::model::PlaylistEdit::Insert`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistAdd {
    pub playlist: Playlist,
    pub index: Option<usize>,
    pub track: Track,
}

/// The tracks of the album with the name `name`, to add at the end of a playlist that already has
/// the tracks in `repeated` (URIs).
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumAdd {
    pub playlist: Playlist,
    pub name: String,
    pub tracks: Vec<Track>,
    pub repeated: HashSet<String>,
}

impl AlbumAdd {
    /// The number of its tracks that the playlist already has.
    fn repeated_count(&self) -> usize {
        self.tracks
            .iter()
            .filter(|track| self.repeated.contains(&track.uri))
            .count()
    }
}

/// The subject of the dialog.
enum Asking {
    Track(PlaylistAdd),
    Album(AlbumAdd),
}

/// The tracks that the user chose to add, which the UI did not send yet.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirmed {
    /// One track, again.
    Track(PlaylistAdd),
    /// Tracks of an album, at the end of `playlist`.
    Tracks {
        playlist: Playlist,
        tracks: Vec<Track>,
    },
}

impl Confirmed {
    pub fn playlist_mut(&mut self) -> &mut Playlist {
        match self {
            Self::Track(add) => &mut add.playlist,
            Self::Tracks { playlist, .. } => playlist,
        }
    }

    /// The command that adds them.
    pub fn command(self) -> Command {
        match self {
            Self::Track(add) => PlaylistAdds::command(add),
            Self::Tracks { playlist, tracks } => Command::EditPlaylist {
                playlist,
                edit: PlaylistEdit::Append { tracks },
            },
        }
    }
}

/// The buttons of the dialog.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Cancel,
    /// The track, or all the tracks of the album.
    AddAll,
    /// Only the tracks of the album that the playlist does not have.
    SkipRepeated,
}

/// The state of the dialog of repeated tracks.
#[derive(Default)]
pub struct PlaylistAdds {
    /// The tracks added from here to each playlist (id → URIs) after its last read. Thus, when the
    /// user adds the same track two times quickly, the two adds ask the backend before the UI makes
    /// one.
    added: HashMap<String, HashSet<String>>,
    /// The add that the dialog asks about.
    asking: Option<Asking>,
    /// The adds that the user confirmed, which the UI did not send yet.
    confirmed: Vec<Confirmed>,
    /// What the dialog last showed, kept while it fades out after it closes.
    shown: Option<(String, Vec<Button>)>,
}

impl PlaylistAdds {
    pub fn is_asking(&self) -> bool {
        self.asking.is_some()
    }

    /// True if the user added `uri` to the playlist `id` from here after the last read of its
    /// tracks.
    pub fn was_added(&self, id: &str, uri: &str) -> bool {
        self.added.get(id).is_some_and(|uris| uris.contains(uri))
    }

    /// The UI adds `uri` to the playlist `id`.
    pub fn on_added(&mut self, id: &str, uri: &str) {
        self.added
            .entry(id.to_owned())
            .or_default()
            .insert(uri.to_owned());
    }

    /// The UI removes a copy of `uri` from the playlist `id`.
    pub fn on_removed(&mut self, id: &str, uri: &str) {
        if let Some(uris) = self.added.get_mut(id) {
            uris.remove(uri);
        }
    }

    /// The UI read the tracks of the playlist `id`: they tell what it has.
    pub fn on_read(&mut self, id: &str) {
        self.added.remove(id);
    }

    /// Opens the dialog for `add` (it replaces a different dialog).
    pub fn ask(&mut self, add: PlaylistAdd) {
        self.asking = Some(Asking::Track(add));
    }

    /// Opens the dialog for the album `add`. The playlist has its `repeated` tracks. It replaces a
    /// different dialog.
    pub fn ask_album(&mut self, add: AlbumAdd) {
        self.asking = Some(Asking::Album(add));
    }

    /// The adds that the user confirmed after the last call.
    pub fn take_confirmed(&mut self) -> Vec<Confirmed> {
        std::mem::take(&mut self.confirmed)
    }

    /// Esc cancels, and Enter (if no button has the focus) selects the main button. The UI calls
    /// it before the views, so that they do not see those keys while the dialog asks.
    pub fn keyboard(&mut self, ctx: &Context) {
        let Some(asking) = &self.asking else {
            return;
        };
        let pressed = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        if pressed(Key::Escape) {
            self.asking = None;
        } else if ctx.memory(|m| m.focused().is_none()) && pressed(Key::Enter) {
            let main = buttons(asking).last().map_or(Choice::Cancel, |b| b.choice);
            self.choose(main);
        }
    }

    fn choose(&mut self, choice: Choice) {
        let Some(asking) = self.asking.take() else {
            return;
        };
        let confirmed = match (asking, choice) {
            (_, Choice::Cancel) => None,
            (Asking::Track(add), _) => Some(Confirmed::Track(add)),
            (Asking::Album(add), Choice::AddAll) => Some(Confirmed::Tracks {
                playlist: add.playlist,
                tracks: add.tracks,
            }),
            (Asking::Album(add), Choice::SkipRepeated) => {
                let tracks: Vec<Track> = add
                    .tracks
                    .into_iter()
                    .filter(|track| !add.repeated.contains(&track.uri))
                    .collect();
                (!tracks.is_empty()).then_some(Confirmed::Tracks {
                    playlist: add.playlist,
                    tracks,
                })
            }
        };
        self.confirmed.extend(confirmed);
    }

    /// The dialog, above all other items, while it asks (and while it fades out after it closes).
    /// A click outside it cancels.
    pub fn show(&mut self, ctx: &Context, accent: Color32) {
        let asking = self.asking.is_some();
        let opacity = anim::tween_bool(ctx, Id::unique("playlist-add-fade"), asking, FADE);
        if let Some(asking) = &self.asking {
            self.shown = Some((message(asking), buttons(asking)));
        } else if opacity <= 0.0 {
            self.shown = None;
        }
        let Some((message, buttons)) = &self.shown else {
            return;
        };
        // Only the shadow: `backdrop::framed` draws the other parts. The frame paints the shadow
        // outside the opacity of the contents, so this code fades it.
        let mut shadow = ctx.global_style().visuals.popup_shadow;
        shadow.color = shadow.color.gamma_multiply(opacity);
        let outer = Frame::new()
            .corner_radius(theme::RADIUS_LARGE)
            .shadow(shadow);
        let modal = egui::Modal::new(Id::unique("playlist-add-dialog"))
            .backdrop_color(Color32::from_black_alpha((140.0 * opacity) as u8))
            .frame(outer)
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                // The window background, with the complementary color as the main color.
                backdrop::framed(ui, Margin::same(20), accent, |ui| {
                    contents(ui, message, buttons, accent)
                })
            });
        window_frame::drag_through_backdrop(ctx, &modal.backdrop_response);
        // While it fades out, the buttons do not respond.
        if !asking {
            return;
        }
        if let Some(choice) = modal.inner {
            self.choose(choice);
        } else if modal.should_close() {
            self.asking = None;
        }
    }

    /// The command that adds `add`.
    pub fn command(add: PlaylistAdd) -> Command {
        Command::EditPlaylist {
            playlist: add.playlist,
            edit: crate::model::PlaylistEdit::Insert {
                index: add.index,
                track: add.track,
            },
        }
    }
}

/// The text of the dialog.
fn message(asking: &Asking) -> String {
    match asking {
        Asking::Track(add) => format!(
            "“{}” is already in “{}”. Add it again?",
            add.track.name, add.playlist.name
        ),
        Asking::Album(add) => {
            let repeated = add.repeated_count();
            let total = add.tracks.len();
            if repeated == total {
                format!(
                    "All the songs of “{}” are already in “{}”. Add them again?",
                    add.name, add.playlist.name
                )
            } else {
                format!(
                    "{repeated} of the {} of “{}” {} already in “{}”.",
                    plural(total, "song", "songs"),
                    add.name,
                    if repeated == 1 { "is" } else { "are" },
                    add.playlist.name
                )
            }
        }
    }
}

/// A button of the dialog.
#[derive(Clone)]
struct Button {
    choice: Choice,
    id: &'static str,
    label: &'static str,
    width: f32,
}

/// The buttons of the dialog, from left to right. The last button is the main button (Enter).
fn buttons(asking: &Asking) -> Vec<Button> {
    let cancel = Button {
        choice: Choice::Cancel,
        id: "playlist-add-cancel",
        label: "Don't add",
        width: 100.0,
    };
    let again = Button {
        choice: Choice::AddAll,
        id: "playlist-add-again",
        label: "Add again",
        width: 120.0,
    };
    match asking {
        Asking::Album(add) if add.repeated_count() < add.tracks.len() => vec![
            cancel,
            Button {
                choice: Choice::AddAll,
                id: "playlist-add-all",
                label: "Add all",
                width: 96.0,
            },
            // Users seldom want repeated tracks: this is the main choice.
            Button {
                choice: Choice::SkipRepeated,
                id: "playlist-add-skip",
                label: "Skip repeated",
                width: 140.0,
            },
        ],
        _ => vec![cancel, again],
    }
}

/// The title, `message` and the `buttons` of the dialog. Returns the button that the user
/// clicked.
fn contents(ui: &mut Ui, message: &str, buttons: &[Button], accent: Color32) -> Option<Choice> {
    const TITLE: &str = "Already in this playlist";
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
    a11y_label(ui, title_rect, Id::unique("playlist-add-title"), TITLE);
    ui.add_space(6.0);

    let mut job = LayoutJob::single_section(
        message.to_owned(),
        TextFormat {
            font_id: theme::regular(theme::FONT_BODY),
            color: theme::TEXT_DIM,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::wrap_at_width(WIDTH);
    let galley = painter.layout_job(job);
    let (text_rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    painter.galley(text_rect.min, galley, theme::TEXT_DIM);
    a11y_label(ui, text_rect, Id::unique("playlist-add-message"), message);
    ui.add_space(20.0);

    let (row, _) = ui.allocate_exact_size(vec2(WIDTH, BUTTON_HEIGHT), Sense::hover());
    // Aligned on the right, in order.
    let mut left = row.right() + BUTTON_GAP
        - buttons
            .iter()
            .map(|button| button.width + BUTTON_GAP)
            .sum::<f32>();
    let mut clicked = None;
    for (i, button) in buttons.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(left, row.top()), vec2(button.width, BUTTON_HEIGHT));
        left = rect.right() + BUTTON_GAP;
        let main = i + 1 == buttons.len();
        // The other buttons are translucent, so that the tinted background shows through.
        let (fill, text) = if main {
            (accent, theme::BG)
        } else {
            (Color32::from_white_alpha(18), theme::TEXT)
        };
        let response = pill_button(
            ui,
            rect,
            Id::unique(button.id),
            button.label,
            fill,
            text,
            true,
        );
        if response.clicked() {
            clicked = Some(button.choice);
        }
    }
    clicked
}
