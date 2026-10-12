//! Key suppression — swallow selected keys so they never reach the foreground
//! application, while still reporting them to the tap receiver.
//!
//! The suppressed set is process-global and read from the platform event
//! callbacks on every key event, so membership checks must stay lock-free in
//! the common case: an [`AtomicUsize`] count gates a `RwLock`-guarded
//! [`HashSet`], and the set is empty (the default) almost always.
//!
//! Platform notes:
//!
//! - **macOS**: suppression requires the tap to be created with
//!   [`TapBuilder::filter_events`](crate::TapBuilder::filter_events), which
//!   switches the `CGEventTap` from listen-only to an active (filtering) tap
//!   and needs the *Accessibility* permission rather than just Input
//!   Monitoring. A listen-only tap reports suppressed keys as ordinary
//!   events; they simply still reach the foreground.
//! - **Windows**: the low-level keyboard hook can always swallow; the set
//!   takes effect regardless of builder flags.
//! - **Linux**: not implemented; the set is inert.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Key;

/// Number of currently suppressed keys. Zero (the default) lets the platform
/// callbacks skip the lock entirely.
static SUPPRESSED_COUNT: AtomicUsize = AtomicUsize::new(0);

fn suppressed_set() -> &'static RwLock<HashSet<Key>> {
    static SET: OnceLock<RwLock<HashSet<Key>>> = OnceLock::new();
    SET.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Replace the set of keys the tap swallows. Suppressed keys are still
/// delivered through the tap's channel — they just no longer reach the
/// foreground application.
///
/// Call with an empty slice to stop suppressing.
pub fn set_suppressed_keys(keys: &[Key]) {
    let set: HashSet<Key> = keys.iter().copied().collect();
    let count = set.len();
    match suppressed_set().write() {
        Ok(mut guard) => *guard = set,
        Err(poisoned) => {
            *poisoned.into_inner() = set;
        }
    }
    SUPPRESSED_COUNT.store(count, Ordering::Release);
}

/// The currently suppressed keys, in unspecified order.
pub fn suppressed_keys() -> Vec<Key> {
    if SUPPRESSED_COUNT.load(Ordering::Acquire) == 0 {
        return Vec::new();
    }
    match suppressed_set().read() {
        Ok(guard) => guard.iter().copied().collect(),
        Err(poisoned) => poisoned.into_inner().iter().copied().collect(),
    }
}

/// Whether `key` is currently suppressed. Called from the platform event
/// callbacks; the count fast-path avoids the lock when nothing is suppressed.
pub(crate) fn is_suppressed(key: Key) -> bool {
    if SUPPRESSED_COUNT.load(Ordering::Acquire) == 0 {
        return false;
    }
    match suppressed_set().read() {
        Ok(guard) => guard.contains(&key),
        Err(poisoned) => poisoned.into_inner().contains(&key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_by_default_and_never_suppresses() {
        assert!(suppressed_keys().is_empty());
        assert!(!is_suppressed(Key::Enter));
        assert!(!is_suppressed(Key::Escape));
    }

    #[test]
    fn set_suppressed_keys_replaces_the_set() {
        set_suppressed_keys(&[Key::Enter, Key::NumpadEnter]);
        assert!(is_suppressed(Key::Enter));
        assert!(is_suppressed(Key::NumpadEnter));
        assert!(!is_suppressed(Key::Escape));
        assert_eq!(suppressed_keys().len(), 2);

        set_suppressed_keys(&[Key::Escape]);
        assert!(!is_suppressed(Key::Enter));
        assert!(is_suppressed(Key::Escape));

        set_suppressed_keys(&[]);
        assert!(suppressed_keys().is_empty());
        assert!(!is_suppressed(Key::Escape));
    }
}
