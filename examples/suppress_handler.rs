//! Manual verification of the suppress-handler predicate API.
//!
//! Run with `cargo run --example suppress_handler` (macOS needs the
//! Accessibility permission). Then:
//!
//! - Press `F` — it is swallowed (never reaches the focused app) while the
//!   simulated "recording" is active.
//! - Press `Space` — always swallowed via the static set.
//! - Press any other key — passes through normally.
//! - Toggle the simulated recording state with `R`.
//!
//! Every key still shows up in the tap's event stream below; only the
//! foreground delivery changes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use keytap::{Key, SuppressHandler, Tap, set_suppress_handler, set_suppressed_keys};

static RECORDING: AtomicBool = AtomicBool::new(true);

fn main() {
    // Static set: Space is suppressed unconditionally.
    set_suppressed_keys(&[Key::Space]);

    // Predicate: F is suppressed only while "recording" is active.
    let handler: SuppressHandler =
        Arc::new(|key: Key| key == Key::F && RECORDING.load(Ordering::Acquire));
    set_suppress_handler(Some(handler));

    let tap = Tap::builder()
        .filter_events()
        .build()
        .expect("tap build failed");
    println!("filtering tap active — F while recording / Space always; R toggles recording");
    for event in tap.iter() {
        match event.kind {
            keytap::EventKind::KeyDown(Key::R) => {
                let now = !RECORDING.load(Ordering::Acquire);
                RECORDING.store(now, Ordering::Release);
                println!("[toggle] recording = {now}");
            }
            keytap::EventKind::KeyDown(k) => println!("[down ] {k:?}"),
            keytap::EventKind::KeyUp(k) => println!("[up   ] {k:?}"),
            keytap::EventKind::KeyRepeat(k) => println!("[rep  ] {k:?}"),
        }
    }
}
