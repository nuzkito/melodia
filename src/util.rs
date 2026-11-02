//! Small helpers: formatting, accent-insensitive search and locking.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

/// Locks `mutex` also if a thread panicked while it held it. The data that it protects here
/// (settings, caches) stays usable, and one panic must not stop the full app.
pub fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Logs the time since the app started, to find what makes the start slow. The first call (at the
/// start of `main`) is the zero.
pub fn startup_mark(what: &str) {
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed();
    log::info!("[startup] +{} ms: {what}", elapsed.as_millis());
}

/// Formats a duration as `m:ss` (or `h:mm:ss` after one hour).
pub fn format_duration(ms: u64) -> String {
    let total_secs = ms / 1000;
    let (hours, minutes, seconds) = (total_secs / 3600, (total_secs / 60) % 60, total_secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// `count` and the item that it counts: "1 song", "12 songs".
pub fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Changes to lowercase and removes the most common Latin diacritics. Thus a search for "corazon"
/// matches "Corazón". The demo backend uses it.
pub fn normalize_for_search(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// The similarity of two texts, from 0.0 (nothing in common) to 1.0 (equal): one minus the
/// Levenshtein distance divided by the length of the longer text.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    1.0 - previous[b.len()] as f64 / longest as f64
}

/// Returns the first character of a name, in uppercase (for avatars).
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(9_999), "0:09");
        assert_eq!(format_duration(61_000), "1:01");
        assert_eq!(format_duration(225_000), "3:45");
        assert_eq!(format_duration(3_723_000), "1:02:03");
    }

    #[test]
    fn counts_in_singular_or_plural() {
        assert_eq!(plural(0, "song", "songs"), "0 songs");
        assert_eq!(plural(1, "song", "songs"), "1 song");
        assert_eq!(plural(12, "album", "albums"), "12 albums");
    }

    #[test]
    fn normalizes_accents_and_case() {
        assert_eq!(normalize_for_search("Corazón ÑANDÚ"), "corazon nandu");
        assert_eq!(normalize_for_search("Ça va"), "ca va");
    }

    #[test]
    fn similarity_goes_from_equal_to_unrelated() {
        assert!((similarity("queen", "queen") - 1.0).abs() < f64::EPSILON);
        assert!(similarity("tesserac", "tessel") > similarity("tesserac", "jungle"));
        assert!(similarity("abc", "xyz") < f64::EPSILON);
    }

    #[test]
    fn initial_skips_symbols() {
        assert_eq!(initial("nuzkito"), "N");
        assert_eq!(initial("  @ana"), "A");
        assert_eq!(initial(""), "?");
    }

    #[test]
    fn a_poisoned_lock_still_gives_its_data() {
        let mutex = Mutex::new(1);
        let _ = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = mutex.lock().unwrap();
                    panic!("poisons the lock");
                })
                .join()
        });
        assert!(mutex.is_poisoned());
        *lock(&mutex) += 1;
        assert_eq!(*lock(&mutex), 2);
    }
}
