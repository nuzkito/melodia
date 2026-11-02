//! «Liked Songs»: the heart of each track row.
//!
//! The app keeps its data in [`LikedSongs`]. It shares the data with the rows through the memory
//! of egui in each frame ([`LikedSongs::publish`]), so no view must pass it on. The rows read it
//! ([`state`]). They put there the tracks that they need data about and the hearts that the user
//! clicked ([`toggle`]). The app collects them at the end of the frame ([`LikedSongs::collect`]).
//! The card of the current track has the same heart ([`heart_button`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use egui::{Color32, Context, Id, Painter, Pos2, Ui};

use super::anim::{self, lerp_color};
use super::dnd;
use super::icons;
use super::theme;
use super::widgets::icon_button_hover;
use crate::backend::Command;
use crate::model::Track;
use crate::spotify::LIBRARY_BATCH;

/// The shortest time between two [`Command::CheckLiked`]. During a scroll, the UI asks about the
/// new rows together.
const CHECK_EVERY: f64 = 0.2;

/// The seconds for a heart to shatter, when the track leaves «Liked Songs».
const SHATTER_TIME: f32 = 0.55;
/// The time in that period when the empty heart starts to come back, and the duration.
const REAPPEAR_AT: f32 = 0.3;
const REAPPEAR_TIME: f32 = 0.25;

/// What the rows read in a frame.
#[derive(Clone, Default)]
struct Shared {
    known: Arc<HashMap<String, bool>>,
}

/// What the rows leave for the app in a frame.
#[derive(Clone, Default)]
struct Requests {
    wanted: Vec<String>,
    toggled: Vec<(Track, bool)>,
}

fn shared_id() -> Id {
    Id::unique("liked-songs")
}

fn requests_id() -> Id {
    Id::unique("liked-songs-requests")
}

/// Only songs go to «Liked Songs», not local files.
pub fn can_like(track: &Track) -> bool {
    track.uri.starts_with("spotify:track:")
}

/// True if the track `uri` is in «Liked Songs», if known. At the end of the frame, the app asks
/// about unknown tracks.
pub fn state(ctx: &Context, uri: &str) -> Option<bool> {
    let shared = ctx.data(|d| d.get_temp::<Shared>(shared_id()))?;
    let liked = shared.known.get(uri).copied();
    if liked.is_none() {
        ctx.data_mut(|d| {
            d.get_temp_mut_or_default::<Requests>(requests_id())
                .wanted
                .push(uri.to_owned());
        });
    }
    liked
}

/// The user clicked the heart of `track`: it goes to «Liked Songs» (`liked`) or leaves it.
pub fn toggle(ctx: &Context, track: &Track, liked: bool) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Requests>(requests_id())
            .toggled
            .push((track.clone(), liked));
    });
}

/// The ♥ of `track`, with its center on `center`. It is filled with the accent color while the
/// track is in «Liked Songs», with a small pop when it goes in. A click adds or removes it. When
/// the pointer is on a liked heart, the heart cracks open (the click removes it). The click
/// shatters it into pieces of its color, and an empty heart pops back. While the track is not
/// liked, the heart shows only as much as `shown` (the hover of a row; 1 on the card).
pub fn heart_button(
    ui: &mut Ui,
    center: Pos2,
    id: Id,
    track: &Track,
    shown: f32,
    alpha: f32,
    accent: Color32,
) {
    let ctx = ui.ctx().clone();
    let liked = state(&ctx, &track.uri) == Some(true);
    let fill = anim::tween_bool(&ctx, id.with("liked"), liked, 0.22);
    // The seconds after the heart last went in or out. Only "out" is important.
    let broken = anim::since_change(&ctx, id.with("broken"), liked, false);
    let shattering = !liked && broken < SHATTER_TIME;
    anim::keep_animating(&ctx, broken, SHATTER_TIME);
    let visible = if shattering { 1.0 } else { shown.max(fill) } * alpha;
    let pop = 1.0 + 0.25 * (fill * std::f32::consts::PI).sin();
    // The opening of the crack, and the color of the heart, before the click.
    let split_id = id.with("split");
    let paint = |p: &Painter, c: Pos2, color: Color32, hover: f32| {
        if shattering {
            // The pieces fly from their position in the crack, in the color of the heart. The
            // new, empty heart appears while they go.
            let (split, before) = ctx
                .data(|d| d.get_temp::<(f32, Color32)>(split_id))
                .unwrap_or((0.0, accent));
            icons::heart_shards(
                p,
                c,
                17.0,
                before.gamma_multiply(alpha),
                split,
                broken / SHATTER_TIME,
            );
            let back = ((broken - REAPPEAR_AT) / REAPPEAR_TIME).clamp(0.0, 1.0);
            if back > 0.0 {
                let size = 17.0 * anim::lerp(0.5, 1.0, anim::ease_out_back(back));
                icons::heart(p, c, size, 0.0, color.gamma_multiply(back * alpha));
            }
            return;
        }
        let color = lerp_color(color, accent, fill);
        let split = hover * fill;
        if liked {
            ctx.data_mut(|d| d.insert_temp(split_id, (split, color)));
        }
        icons::heart_split(p, c, 17.0 * pop, fill, color.gamma_multiply(visible), split);
    };
    // Nothing responds during a drag, but a liked song keeps its heart.
    if dnd::is_dragging(&ctx) {
        if visible > 0.0 {
            paint(ui.painter(), center, theme::TEXT_DIM, 0.0);
        }
        return;
    }
    let label = if liked {
        format!("Remove {} from Liked Songs", track.name)
    } else {
        format!("Save {} to Liked Songs", track.name)
    };
    let response = icon_button_hover(ui, center, 30.0, id.with("like"), &label, true, paint);
    if response.clicked() {
        toggle(&ctx, track, !liked);
    }
}

/// The data that the UI has about «Liked Songs».
#[derive(Default)]
pub struct LikedSongs {
    /// URI → in «Liked Songs». The rows share it (the app copies it when it changes).
    known: Arc<HashMap<String, bool>>,
    /// The tracks that the user changed here. Spotify's answers to earlier checks do not replace
    /// them.
    changed: HashSet<String>,
    /// The tracks that the app asked about, or will ask about soon.
    asked: HashSet<String>,
    /// The tracks to ask about in the next [`Command::CheckLiked`].
    wanted: Vec<String>,
    last_check: Option<f64>,
    /// The session does not have the permission. A click connects the account again, and the
    /// heart fills only when Spotify confirms it.
    needs_reauth: bool,
}

impl LikedSongs {
    /// Forgets all data (a different account, or new permissions).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Answers to [`Command::CheckLiked`].
    pub fn on_states(&mut self, states: Vec<(String, bool)>) {
        let known = Arc::make_mut(&mut self.known);
        for (uri, liked) in states {
            if !self.changed.contains(&uri) {
                known.insert(uri, liked);
            }
        }
    }

    /// The value that Spotify has for `uri`, whatever the UI showed.
    pub fn on_changed(&mut self, uri: String, liked: bool) {
        self.changed.insert(uri.clone());
        Arc::make_mut(&mut self.known).insert(uri, liked);
    }

    pub fn on_needs_reauth(&mut self) {
        self.needs_reauth = true;
    }

    /// The user clicked a heart. It shows the change immediately, but not when the user must first
    /// connect the account again.
    pub fn on_toggle(&mut self, uri: &str, liked: bool) {
        if !self.needs_reauth {
            self.on_changed(uri.to_owned(), liked);
        }
    }

    /// Shares the known states with the rows of this frame.
    pub fn publish(&self, ctx: &Context) {
        let shared = Shared {
            known: Arc::clone(&self.known),
        };
        ctx.data_mut(|d| d.insert_temp(shared_id(), shared));
    }

    /// The commands that the rows asked for in this frame: the hearts that the user clicked, and a
    /// check of the tracks that are not known yet (one at most for each [`CHECK_EVERY`]).
    pub fn collect(&mut self, ctx: &Context, time: f64) -> Vec<Command> {
        let requests = ctx
            .data_mut(|d| d.remove_temp::<Requests>(requests_id()))
            .unwrap_or_default();
        for uri in requests.wanted {
            if self.asked.insert(uri.clone()) {
                self.wanted.push(uri);
            }
        }
        let mut commands: Vec<Command> = requests
            .toggled
            .into_iter()
            .map(|(track, liked)| Command::SetLiked { track, liked })
            .collect();
        if !self.wanted.is_empty() {
            let wait = self
                .last_check
                .map_or(0.0, |last| CHECK_EVERY - (time - last));
            if wait <= 0.0 || self.wanted.len() >= LIBRARY_BATCH {
                self.last_check = Some(time);
                commands.push(Command::CheckLiked {
                    uris: std::mem::take(&mut self.wanted),
                });
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(wait));
            }
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earlier_checks_do_not_undo_a_change_made_here() {
        let mut liked = LikedSongs::default();
        liked.on_toggle("spotify:track:a", true);
        liked.on_states(vec![
            ("spotify:track:a".into(), false),
            ("spotify:track:b".into(), true),
        ]);
        assert_eq!(liked.known.get("spotify:track:a"), Some(&true));
        assert_eq!(liked.known.get("spotify:track:b"), Some(&true));
    }

    #[test]
    fn without_the_permission_the_heart_waits_for_spotify() {
        let mut liked = LikedSongs::default();
        liked.on_needs_reauth();
        liked.on_toggle("spotify:track:a", true);
        assert_eq!(liked.known.get("spotify:track:a"), None);
        liked.on_changed("spotify:track:a".into(), true);
        assert_eq!(liked.known.get("spotify:track:a"), Some(&true));
    }

    #[test]
    fn only_songs_can_be_liked() {
        let track = |uri: &str| Track {
            uri: uri.into(),
            name: "Under Pressure".into(),
            artists: vec!["Queen".into()],
            artist_ids: vec![None],
            album: String::new(),
            album_id: None,
            duration_ms: 0,
            cover_small: None,
            cover_large: None,
            explicit: false,
        };
        assert!(can_like(&track("spotify:track:1")));
        assert!(!can_like(&track("spotify:local:Queen::Under+Pressure:248")));
    }
}
