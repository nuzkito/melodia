//! Stable identities for the entries of the queue (and of the other lists with rows that glide
//! when their order changes, for example the tracks of a playlist).
//!
//! A track can be in the queue more than one time, and the backend's snapshots list only tracks.
//! If the code counted copies, repeated rows would swap keys and fly to the incorrect position.
//! Thus each snapshot keeps the keys of its longest common subsequence with the previous snapshot.
//! The UI first applies its own edits to the keys, so the copy that moves is the copy that the
//! user selected.

use std::sync::atomic::{AtomicU64, Ordering};

/// The keys forget a predicted edit if the queue does not change in this number of seconds (the
/// backend refused it).
const PENDING_TTL: f64 = 3.0;

/// Keys are never used again, also not by a new [`QueueKeys`] (after logout). A new row must not
/// get the animation state of an old row.
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

fn fresh_key() -> u64 {
    NEXT_KEY.fetch_add(1, Ordering::Relaxed)
}

/// An edit of the full queue (see [`QueueKeys`]), by index.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeyEdit {
    /// As `Vec::remove(from)`, then `Vec::insert(to)`.
    Move {
        from: usize,
        to: usize,
    },
    Insert {
        at: usize,
        uri: String,
        key: u64,
    },
    Remove {
        at: usize,
    },
}

impl KeyEdit {
    fn apply(&self, uris: &mut Vec<String>, keys: &mut Vec<u64>) {
        match self {
            Self::Move { from, to } if *from < uris.len() => {
                let uri = uris.remove(*from);
                let key = keys.remove(*from);
                let to = (*to).min(uris.len());
                uris.insert(to, uri);
                keys.insert(to, key);
            }
            Self::Insert { at, uri, key } => {
                let at = (*at).min(uris.len());
                uris.insert(at, uri.clone());
                keys.insert(at, *key);
            }
            Self::Remove { at } if *at < uris.len() => {
                uris.remove(*at);
                keys.remove(*at);
            }
            _ => {}
        }
    }
}

/// The key of each entry of the full queue.
#[derive(Debug, Default)]
pub struct QueueKeys {
    uris: Vec<String>,
    keys: Vec<u64>,
    /// An edit that the UI just sent, and the time limit for it. It applies to the keys at the
    /// next change of the queue.
    pending: Option<(KeyEdit, f64)>,
}

impl QueueKeys {
    /// The key of each entry of `uris`, the full queue as it is now (at `now`, in seconds).
    /// Entries that were there before keep their key.
    pub fn update<'a>(&mut self, uris: impl IntoIterator<Item = &'a str>, now: f64) -> &[u64] {
        let uris: Vec<&str> = uris.into_iter().collect();
        let pending = self.pending.take().filter(|(_, until)| now <= *until);
        if uris
            .iter()
            .copied()
            .eq(self.uris.iter().map(String::as_str))
        {
            // The edit is not there yet: continue to wait for it.
            self.pending = pending;
            return &self.keys;
        }
        if let Some((edit, _)) = pending {
            edit.apply(&mut self.uris, &mut self.keys);
        }
        self.keys = carry_over(&self.uris, &self.keys, &uris);
        self.uris = uris.into_iter().map(str::to_owned).collect();
        &self.keys
    }

    /// The entry at `from` moved to `to` (as `Vec::remove`, then `Vec::insert`). Returns its key.
    pub fn moved(&mut self, from: usize, to: usize, now: f64) -> Option<u64> {
        let key = self.keys.get(from).copied();
        self.expect(KeyEdit::Move { from, to }, now);
        key
    }

    /// The UI inserted `uri` at `at`. Returns the key of the new entry.
    pub fn inserted(&mut self, at: usize, uri: &str, now: f64) -> u64 {
        let key = fresh_key();
        self.expect(
            KeyEdit::Insert {
                at,
                uri: uri.to_owned(),
                key,
            },
            now,
        );
        key
    }

    /// The UI removed the entry at `at`.
    pub fn removed(&mut self, at: usize, now: f64) {
        self.expect(KeyEdit::Remove { at }, now);
    }

    fn expect(&mut self, edit: KeyEdit, now: f64) {
        self.pending = Some((edit, now + PENDING_TTL));
    }
}

/// The keys of `new`. The entries of the longest common subsequence of `old` and `new` keep their
/// keys, and the other entries get new keys. When two choices are equal, the latest copies keep
/// their keys (the oldest previous tracks leave).
fn carry_over(old: &[String], keys: &[u64], new: &[&str]) -> Vec<u64> {
    let (n, m) = (old.len(), new.len());
    // lengths[i * (m + 1) + j]: the length of the longest common subsequence of old[..i] and
    // new[..j].
    let width = m + 1;
    let mut lengths = vec![0u16; (n + 1) * width];
    for i in 1..=n {
        for j in 1..=m {
            lengths[i * width + j] = if old[i - 1] == new[j - 1] {
                lengths[(i - 1) * width + j - 1] + 1
            } else {
                lengths[(i - 1) * width + j].max(lengths[i * width + j - 1])
            };
        }
    }
    let mut result = vec![0; m];
    let (mut i, mut j) = (n, m);
    while j > 0 {
        if i > 0 && old[i - 1] == new[j - 1] {
            result[j - 1] = keys[i - 1];
            i -= 1;
            j -= 1;
        } else if i > 0 && lengths[(i - 1) * width + j] >= lengths[i * width + j - 1] {
            i -= 1;
        } else {
            result[j - 1] = fresh_key();
            j -= 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One-letter URIs.
    fn uris(ids: &str) -> Vec<String> {
        ids.chars().map(String::from).collect()
    }

    fn update(keys: &mut QueueKeys, ids: &str, now: f64) -> Vec<u64> {
        let uris = uris(ids);
        keys.update(uris.iter().map(String::as_str), now).to_vec()
    }

    fn all_different(keys: &[u64]) -> bool {
        let mut sorted = keys.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.len() == keys.len()
    }

    #[test]
    fn copies_of_a_track_get_keys_of_their_own() {
        let mut keys = QueueKeys::default();
        let first = update(&mut keys, "abab", 0.0);
        assert!(all_different(&first));
        // Nothing changed: the same keys.
        assert_eq!(update(&mut keys, "abab", 1.0), first);
    }

    #[test]
    fn entries_keep_their_key_when_the_music_moves_on() {
        // The full queue keeps its order when the track changes. Only the oldest previous track
        // leaves when the history is full.
        let mut keys = QueueKeys::default();
        let before = update(&mut keys, "axabca", 0.0);
        let after = update(&mut keys, "xabca", 1.0);
        assert_eq!(after, before[1..]);
        // A new track at the end (the UI read Spotify's queue again).
        let more = update(&mut keys, "xabcab", 2.0);
        assert_eq!(more[..5], after[..]);
        assert!(!after.contains(&more[5]));
    }

    #[test]
    fn a_moved_copy_keeps_its_key() {
        let mut keys = QueueKeys::default();
        let before = update(&mut keys, "abca", 0.0);
        // The last "a" goes first: that copy moves, not the first copy (which only makes space).
        assert_eq!(keys.moved(3, 0, 0.5), Some(before[3]));
        let after = update(&mut keys, "aabc", 1.0);
        assert_eq!(after, vec![before[3], before[0], before[1], before[2]]);
    }

    #[test]
    fn a_removed_copy_is_the_one_that_leaves() {
        let mut keys = QueueKeys::default();
        let before = update(&mut keys, "zaa", 0.0);
        keys.removed(1, 0.5);
        assert_eq!(update(&mut keys, "za", 1.0), vec![before[0], before[2]]);
    }

    #[test]
    fn an_inserted_copy_gets_the_key_given_when_dropped() {
        let mut keys = QueueKeys::default();
        let before = update(&mut keys, "ab", 0.0);
        let key = keys.inserted(1, "a", 0.5);
        // The edit can need a few frames to arrive.
        assert_eq!(update(&mut keys, "ab", 0.6), before);
        let after = update(&mut keys, "aab", 1.0);
        assert_eq!(after, vec![before[0], key, before[1]]);
    }

    #[test]
    fn an_edit_that_never_arrives_is_forgotten() {
        let mut keys = QueueKeys::default();
        let before = update(&mut keys, "abca", 0.0);
        keys.moved(3, 0, 0.5);
        // Much later, the history loses its oldest track: the old move does not apply now.
        let after = update(&mut keys, "bca", 10.0);
        assert_eq!(after, before[1..]);
    }

    #[test]
    fn a_new_queue_gets_new_keys() {
        let mut keys = QueueKeys::default();
        let before = update(&mut keys, "aa", 0.0);
        let after = update(&mut keys, "bb", 1.0);
        assert!(all_different(&[before, after].concat()));
        assert!(update(&mut keys, "", 2.0).is_empty());
    }
}
