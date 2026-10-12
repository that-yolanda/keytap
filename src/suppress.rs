//! Key suppression — swallow selected keys so they never reach the foreground
//! application, while still reporting them to the tap receiver.
//!
//! Two mechanisms feed the same decision, combined with a logical OR:
//!
//! - A static set of keys ([`set_suppressed_keys`]) — for keys that are
//!   suppressed whenever they are registered, regardless of context.
//! - An optional predicate ([`set_suppress_handler`]) — for keys whose
//!   suppression depends on application state the caller tracks. The handler
//!   runs synchronously inside the platform event callback on every key
//!   event, so it must be fast and must not block. A panicking handler is
//!   treated as "pass the key through".
//!
//! Both are process-global and read from the platform event callbacks on
//! every key event, so checks must stay lock-free in the common case: an
//! [`AtomicUsize`] count gates the set's `RwLock` and an [`AtomicBool`] gates
//! the handler slot, and both are unset (the default) almost always.
//!
//! Platform notes:
//!
//! - **macOS**: suppression requires the tap to be created with
//!   [`TapBuilder::filter_events`](crate::TapBuilder::filter_events), which
//!   switches the `CGEventTap` from listen-only to an active (filtering) tap
//!   and needs the *Accessibility* permission rather than just Input
//!   Monitoring. A listen-only tap reports suppressed keys as ordinary
//!   events; they simply still reach the foreground.
//! - **Windows**: the low-level keyboard hook can always swallow; both
//!   mechanisms take effect regardless of builder flags.
//! - **Linux**: not implemented; suppression is inert.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::Key;

/// Number of currently suppressed keys. Zero (the default) lets the platform
/// callbacks skip the lock entirely.
static SUPPRESSED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Whether a suppress handler is registered. `false` (the default) lets the
/// platform callbacks skip the handler slot entirely.
static HANDLER_SET: AtomicBool = AtomicBool::new(false);

fn suppressed_set() -> &'static RwLock<HashSet<Key>> {
    static SET: OnceLock<RwLock<HashSet<Key>>> = OnceLock::new();
    SET.get_or_init(|| RwLock::new(HashSet::new()))
}

fn handler_slot() -> &'static RwLock<Option<SuppressHandler>> {
    static SLOT: OnceLock<RwLock<Option<SuppressHandler>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// A predicate consulted on every key event while the tap is a filtering one.
/// Returning `true` swallows the event's key: the tap's channel still
/// receives it, but the foreground application does not.
///
/// The handler runs synchronously inside the platform event callback — it
/// must be fast, must not block on locks held elsewhere for long, and must
/// stay side-effect free. Panics are caught and treated as `false`.
pub type SuppressHandler = Arc<dyn Fn(Key) -> bool + Send + Sync>;

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

/// Register (or, with `None`, remove) the predicate consulted for keys that
/// are not in the static suppressed set. Suppression decisions thus combine
/// as: `set.contains(key) || handler(key)`.
///
/// The handler outlives the call; re-registering replaces the previous one.
pub fn set_suppress_handler(handler: Option<SuppressHandler>) {
    let set = handler.is_some();
    match handler_slot().write() {
        Ok(mut guard) => *guard = handler,
        Err(poisoned) => {
            *poisoned.into_inner() = handler;
        }
    }
    HANDLER_SET.store(set, Ordering::Release);
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
/// callbacks; the fast-path flags avoid all locking when neither mechanism is
/// active. The handler (if any) is invoked outside the slot lock, its clone
/// held instead, so it may itself call suppression APIs.
pub(crate) fn is_suppressed(key: Key) -> bool {
    if SUPPRESSED_COUNT.load(Ordering::Acquire) == 0 && !HANDLER_SET.load(Ordering::Acquire) {
        return false;
    }
    let in_set = SUPPRESSED_COUNT.load(Ordering::Acquire) > 0
        && match suppressed_set().read() {
            Ok(guard) => guard.contains(&key),
            Err(poisoned) => poisoned.into_inner().contains(&key),
        };
    if in_set {
        return true;
    }
    if !HANDLER_SET.load(Ordering::Acquire) {
        return false;
    }
    let handler = match handler_slot().read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    match handler {
        // The panic arm logs before falling back to "pass through", so the
        // `unwrap_or_default` shortcut clippy suggests would drop the log.
        #[allow(clippy::manual_unwrap_or_default)]
        Some(handler) => match catch_unwind(AssertUnwindSafe(|| handler(key))) {
            Ok(suppressed) => suppressed,
            Err(_) => {
                crate::log::error!("keytap: suppress handler panicked — passing key through");
                false
            }
        },
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The set and handler are process-global; serialize the tests that
    /// mutate them so parallel test threads don't see each other's state.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn handler_for(keys: &'static [Key]) -> SuppressHandler {
        Arc::new(move |key: Key| keys.contains(&key))
    }

    #[test]
    fn empty_by_default_and_never_suppresses() {
        let _lock = TEST_LOCK.lock().unwrap();
        set_suppress_handler(None);
        set_suppressed_keys(&[]);
        assert!(suppressed_keys().is_empty());
        assert!(!is_suppressed(Key::Enter));
        assert!(!is_suppressed(Key::Escape));
    }

    #[test]
    fn set_suppressed_keys_replaces_the_set() {
        let _lock = TEST_LOCK.lock().unwrap();
        set_suppress_handler(None);
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

    #[test]
    fn handler_suppresses_matching_keys() {
        let _lock = TEST_LOCK.lock().unwrap();
        set_suppressed_keys(&[]);
        set_suppress_handler(Some(handler_for(&[Key::F, Key::G])));
        assert!(is_suppressed(Key::F));
        assert!(is_suppressed(Key::G));
        assert!(!is_suppressed(Key::H));

        // Re-registering replaces the previous handler.
        set_suppress_handler(Some(handler_for(&[Key::H])));
        assert!(!is_suppressed(Key::F));
        assert!(is_suppressed(Key::H));

        set_suppress_handler(None);
        assert!(!is_suppressed(Key::H));
    }

    #[test]
    fn set_and_handler_combine_with_or() {
        let _lock = TEST_LOCK.lock().unwrap();
        set_suppressed_keys(&[Key::Enter]);
        set_suppress_handler(Some(handler_for(&[Key::F])));
        // Either mechanism suppresses on its own.
        assert!(is_suppressed(Key::Enter));
        assert!(is_suppressed(Key::F));
        assert!(!is_suppressed(Key::G));
        set_suppressed_keys(&[]);
        set_suppress_handler(None);
    }

    #[test]
    fn panicking_handler_passes_key_through() {
        let _lock = TEST_LOCK.lock().unwrap();
        set_suppressed_keys(&[]);
        set_suppress_handler(Some(Arc::new(|_key: Key| panic!("boom"))));
        assert!(!is_suppressed(Key::F));
        set_suppress_handler(None);
    }
}
