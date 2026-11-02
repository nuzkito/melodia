//! Edits of a playlist, translated into Web API requests.
//!
//! The UI counts the tracks that it shows, but Spotify counts all entries of the playlist. This
//! includes entries that the app cannot show (gone from Spotify, unknown types…). A
//! [`PlaylistMirror`] keeps the entries as read, and follows each edit. Thus it can change UI
//! indices into Spotify positions.
//!
//! To move an entry is a reorder by position. To remove an entry is more difficult: the Web API
//! removes only by URI, which removes all copies of the track. When the playlist has other copies,
//! the mirror puts them back at their positions immediately after.

use crate::model::PlaylistEdit;

/// One request to the Web API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiStep {
    /// Moves the entry at `range_start` before the entry at `insert_before` (positions from
    /// before the move).
    Reorder {
        range_start: usize,
        insert_before: usize,
    },
    /// Removes all copies of `uri`.
    RemoveAll { uri: String },
    /// Adds `uri` at `position` (at the end with `None`).
    Add {
        uri: String,
        position: Option<usize>,
    },
    /// Adds `uris`, in order, at the end (at most [`ADD_BATCH`]).
    Append { uris: Vec<String> },
}

/// The number of tracks that the Web API adds to a playlist in one request.
pub const ADD_BATCH: usize = 100;

/// The reason why an edit is not possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    /// The track is not at the position where the UI saw it: the playlist changed.
    Stale,
    /// The Web API cannot add local files to a playlist (or put them back after the removal of a
    /// different copy).
    LocalFile,
}

/// The entries of a playlist as last read, kept up to date with the edits after that read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistMirror {
    /// The URI of each entry, in order. `None` for the entries that the app does not show.
    entries: Vec<Option<String>>,
    /// All the entries of the playlist are here (the list is not cut).
    complete: bool,
    /// The version of the playlist of these entries, if known.
    pub snapshot: Option<String>,
}

impl PlaylistMirror {
    pub fn new(entries: Vec<Option<String>>, complete: bool, snapshot: Option<String>) -> Self {
        Self {
            entries,
            complete,
            snapshot,
        }
    }

    pub fn entries_read(&self) -> usize {
        self.entries.len()
    }

    /// Adds the entries read after the first entries. After this, the list is complete.
    pub fn complete_with(&mut self, rest: impl IntoIterator<Item = Option<String>>) {
        self.entries.extend(rest);
        self.complete = true;
    }

    /// True if the playlist has `uri`. `None` if it can be in the entries that the backend did not
    /// read.
    pub fn has(&self, uri: &str) -> Option<bool> {
        let found = self
            .entries
            .iter()
            .any(|entry| entry.as_deref() == Some(uri));
        (found || self.complete).then_some(found)
    }

    /// The tracks that show, in order, with their positions.
    fn shown(&self) -> impl Iterator<Item = (usize, &str)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(position, uri)| uri.as_deref().map(|uri| (position, uri)))
    }

    fn shown_positions(&self) -> Vec<usize> {
        self.shown().map(|(position, _)| position).collect()
    }

    /// The index (in the tracks that show) of the copy of `uri` nearest to `hint`.
    fn locate(&self, uri: &str, hint: usize) -> Option<usize> {
        self.shown()
            .enumerate()
            .filter(|(_, (_, shown))| *shown == uri)
            .map(|(index, _)| index)
            .min_by_key(|index| index.abs_diff(hint))
    }

    /// The playlist position of a track inserted at `index` of the tracks that show (`None`: at
    /// the end of the playlist).
    fn insert_position(&self, positions: &[usize], index: usize) -> Option<usize> {
        match positions.get(index) {
            Some(position) => Some(*position),
            None if self.complete => None,
            // After the last track that shows (other entries can follow, not read).
            None => Some(positions.last().map_or(0, |last| last + 1)),
        }
    }

    /// Applies `edit` and returns the requests that make it in Spotify (none if nothing changes).
    pub fn plan(&mut self, edit: &PlaylistEdit) -> Result<Vec<ApiStep>, PlanError> {
        let positions = self.shown_positions();
        match edit {
            PlaylistEdit::Move { from, to, uri } => {
                let from = self.locate(uri, *from).ok_or(PlanError::Stale)?;
                let to = (*to).min(positions.len() - 1);
                if from == to {
                    return Ok(Vec::new());
                }
                let range_start = positions[from];
                // Before the track that goes immediately after it, counted before the move. At the
                // end, immediately after the last track.
                let after = if to < from { to } else { to + 1 };
                let insert_before = positions
                    .get(after)
                    .copied()
                    .unwrap_or_else(|| positions[positions.len() - 1] + 1);
                let entry = self.entries.remove(range_start);
                let at = if insert_before > range_start {
                    insert_before - 1
                } else {
                    insert_before
                };
                self.entries.insert(at, entry);
                Ok(vec![ApiStep::Reorder {
                    range_start,
                    insert_before,
                }])
            }
            PlaylistEdit::Insert { index, track } => {
                if track.uri.starts_with("spotify:local:") {
                    return Err(PlanError::LocalFile);
                }
                let position = match index {
                    Some(index) => self.insert_position(&positions, *index),
                    None => None,
                };
                match position {
                    Some(position) => self.entries.insert(position, Some(track.uri.clone())),
                    None if self.complete => self.entries.push(Some(track.uri.clone())),
                    // After the entries read: they do not go to the end now.
                    None => {}
                }
                Ok(vec![ApiStep::Add {
                    uri: track.uri.clone(),
                    position,
                }])
            }
            PlaylistEdit::Remove { index, uri } => {
                let index = self.locate(uri, *index).ok_or(PlanError::Stale)?;
                let removed = positions[index];
                self.entries.remove(removed);
                // The other copies, at their positions after the removal.
                let copies: Vec<usize> = self
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.as_deref() == Some(uri))
                    .map(|(position, _)| position)
                    .collect();
                if !copies.is_empty() && uri.starts_with("spotify:local:") {
                    self.entries.insert(removed, Some(uri.clone()));
                    return Err(PlanError::LocalFile);
                }
                let mut steps = vec![ApiStep::RemoveAll { uri: uri.clone() }];
                // In order: each copy goes back after all the entries before it.
                steps.extend(copies.into_iter().map(|position| ApiStep::Add {
                    uri: uri.clone(),
                    position: Some(position),
                }));
                Ok(steps)
            }
            PlaylistEdit::Append { tracks } => {
                let uris: Vec<String> = tracks.iter().map(|track| track.uri.clone()).collect();
                if uris.iter().any(|uri| uri.starts_with("spotify:local:")) {
                    return Err(PlanError::LocalFile);
                }
                // After the entries read, if they do not go to the end.
                if self.complete {
                    self.entries.extend(uris.iter().cloned().map(Some));
                }
                Ok(uris
                    .chunks(ADD_BATCH)
                    .map(|uris| ApiStep::Append {
                        uris: uris.to_vec(),
                    })
                    .collect())
            }
        }
    }

    /// True if the removal of `uri` needs all entries (other copies of it can be in the entries
    /// that the backend did not read yet).
    pub fn needs_all_for(&self, edit: &PlaylistEdit) -> bool {
        matches!(edit, PlaylistEdit::Remove { .. }) && !self.complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    fn uri(id: char) -> String {
        format!("spotify:track:{id}")
    }

    /// One letter for each entry. `_` is an entry that does not show.
    fn mirror(ids: &str) -> PlaylistMirror {
        let entries = ids.chars().map(|id| (id != '_').then(|| uri(id))).collect();
        PlaylistMirror::new(entries, true, None)
    }

    fn ids(mirror: &PlaylistMirror) -> String {
        mirror
            .entries
            .iter()
            .map(|entry| {
                entry
                    .as_deref()
                    .map_or('_', |uri| uri.chars().last().unwrap())
            })
            .collect()
    }

    #[test]
    fn a_track_is_known_to_be_missing_only_if_every_entry_was_read() {
        assert_eq!(mirror("ab_c").has(&uri('c')), Some(true));
        assert_eq!(mirror("ab_c").has(&uri('d')), Some(false));
        let partial = PlaylistMirror::new(vec![Some(uri('a'))], false, None);
        assert_eq!(partial.has(&uri('a')), Some(true));
        assert_eq!(partial.has(&uri('b')), None);
    }

    fn moved(from: usize, to: usize, id: char) -> PlaylistEdit {
        PlaylistEdit::Move {
            from,
            to,
            uri: uri(id),
        }
    }

    fn removed(index: usize, id: char) -> PlaylistEdit {
        PlaylistEdit::Remove {
            index,
            uri: uri(id),
        }
    }

    fn track(id: char) -> Track {
        Track {
            uri: uri(id),
            name: id.to_string(),
            artists: vec![],
            album: String::new(),
            duration_ms: 1000,
            cover_small: None,
            cover_large: None,
            explicit: false,
            album_id: None,
            artist_ids: vec![],
        }
    }

    fn inserted(index: Option<usize>, id: char) -> PlaylistEdit {
        PlaylistEdit::Insert {
            index,
            track: track(id),
        }
    }

    fn reorder(range_start: usize, insert_before: usize) -> Vec<ApiStep> {
        vec![ApiStep::Reorder {
            range_start,
            insert_before,
        }]
    }

    #[test]
    fn moves_count_every_entry() {
        let mut list = mirror("a_bc");
        // "a" after "b": before "c", which is the fourth entry.
        assert_eq!(list.plan(&moved(0, 1, 'a')), Ok(reorder(0, 3)));
        assert_eq!(ids(&list), "_bac");
        // "c" first: before "b".
        assert_eq!(list.plan(&moved(2, 0, 'c')), Ok(reorder(3, 1)));
        assert_eq!(ids(&list), "_cba");
        // "c" last: after "a", the end.
        assert_eq!(list.plan(&moved(0, 2, 'c')), Ok(reorder(1, 4)));
        assert_eq!(ids(&list), "_bac");
        assert_eq!(list.plan(&moved(1, 1, 'a')), Ok(vec![]));
        assert_eq!(list.plan(&moved(0, 1, 'z')), Err(PlanError::Stale));
    }

    #[test]
    fn the_moved_copy_is_the_one_picked() {
        let mut list = mirror("abca");
        assert_eq!(list.plan(&moved(3, 1, 'a')), Ok(reorder(3, 1)));
        assert_eq!(ids(&list), "aabc");
        assert_eq!(list.plan(&moved(0, 3, 'a')), Ok(reorder(0, 4)));
        assert_eq!(ids(&list), "abca");
    }

    #[test]
    fn a_single_copy_is_removed_by_uri() {
        let mut list = mirror("ab_c");
        assert_eq!(
            list.plan(&removed(1, 'b')),
            Ok(vec![ApiStep::RemoveAll { uri: uri('b') }])
        );
        assert_eq!(ids(&list), "a_c");
    }

    #[test]
    fn removing_a_repeated_track_puts_the_other_copies_back() {
        let mut list = mirror("abaca");
        // The second "a" goes. The first and the last come back.
        assert_eq!(
            list.plan(&removed(2, 'a')),
            Ok(vec![
                ApiStep::RemoveAll { uri: uri('a') },
                ApiStep::Add {
                    uri: uri('a'),
                    position: Some(0),
                },
                ApiStep::Add {
                    uri: uri('a'),
                    position: Some(3),
                },
            ])
        );
        assert_eq!(ids(&list), "abca");
    }

    #[test]
    fn local_files_cannot_be_put_back() {
        let local = "spotify:local:x".to_owned();
        let mut list = PlaylistMirror::new(
            vec![Some(local.clone()), Some(uri('b')), Some(local.clone())],
            true,
            None,
        );
        let edit = PlaylistEdit::Remove {
            index: 0,
            uri: local,
        };
        assert_eq!(list.plan(&edit), Err(PlanError::LocalFile));
        assert_eq!(list.entries_read(), 3, "left as it was");
    }

    #[test]
    fn insertions_go_at_their_place_or_at_the_end() {
        let mut list = mirror("a_b");
        assert_eq!(
            list.plan(&inserted(Some(1), 'c')),
            Ok(vec![ApiStep::Add {
                uri: uri('c'),
                position: Some(2),
            }])
        );
        assert_eq!(ids(&list), "a_cb");
        assert_eq!(
            list.plan(&inserted(Some(9), 'd')),
            Ok(vec![ApiStep::Add {
                uri: uri('d'),
                position: None,
            }])
        );
        assert_eq!(list.plan(&inserted(None, 'e')).unwrap().len(), 1);
        assert_eq!(ids(&list), "a_cbde");
    }

    #[test]
    fn a_cut_playlist_inserts_after_the_last_one_read() {
        let mut list = PlaylistMirror::new(vec![Some(uri('a')), Some(uri('b'))], false, None);
        assert_eq!(
            list.plan(&inserted(Some(2), 'c')),
            Ok(vec![ApiStep::Add {
                uri: uri('c'),
                position: Some(2),
            }])
        );
        assert_eq!(
            list.plan(&inserted(None, 'd')).unwrap(),
            vec![ApiStep::Add {
                uri: uri('d'),
                position: None,
            }]
        );
        assert_eq!(ids(&list), "abc", "the end was not read");
        assert!(list.needs_all_for(&removed(0, 'a')));
        assert!(!list.needs_all_for(&moved(0, 1, 'a')));
        list.complete_with(vec![Some(uri('a'))]);
        assert!(!list.needs_all_for(&removed(0, 'a')));
    }

    #[test]
    fn appended_tracks_go_at_the_end_in_batches() {
        let mut list = mirror("a_b");
        let edit = PlaylistEdit::Append {
            tracks: "cad".chars().map(track).collect(),
        };
        assert_eq!(
            list.plan(&edit),
            Ok(vec![ApiStep::Append {
                uris: vec![uri('c'), uri('a'), uri('d')],
            }])
        );
        assert_eq!(ids(&list), "a_bcad");

        let many = PlaylistEdit::Append {
            tracks: (0..ADD_BATCH + 1).map(|_| track('e')).collect(),
        };
        let steps = list.plan(&many).unwrap();
        assert_eq!(steps.len(), 2, "{ADD_BATCH} at most per request");
        assert_eq!(
            steps[1],
            ApiStep::Append {
                uris: vec![uri('e')]
            }
        );
        assert_eq!(list.entries_read(), 6 + ADD_BATCH + 1);
    }

    #[test]
    fn local_files_cannot_be_appended() {
        let mut list = mirror("a");
        let mut local = track('x');
        local.uri = "spotify:local:x".into();
        let edit = PlaylistEdit::Append {
            tracks: vec![track('b'), local],
        };
        assert_eq!(list.plan(&edit), Err(PlanError::LocalFile));
        assert_eq!(ids(&list), "a", "nothing added");
    }
}
