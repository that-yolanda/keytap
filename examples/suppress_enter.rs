//! Manual verification that key suppression actually swallows keys.
//!
//! Run: cargo run --example suppress_enter
//!
//! Then focus any text field (TextEdit, a browser input, …) and press Enter:
//! the field should NOT gain a newline, while this process still prints the
//! key. That proves the filtering tap works on this machine. Press Ctrl-C to
//! exit.
//!
//! If the field DOES newline, the active-tap mechanism itself is not working
//! on this machine (permissions or OS behavior) — independent of any app
//! integrating keytap.

use keytap::{EventKind, Key, Tap};

fn main() {
    keytap::set_suppressed_keys(&[Key::Enter, Key::NumpadEnter]);
    let tap = match Tap::builder().filter_events().build() {
        Ok(tap) => tap,
        Err(e) => {
            eprintln!(
                "filter tap failed: {:?} — grant Accessibility to this terminal and retry",
                e
            );
            return;
        }
    };
    eprintln!("filter tap running; Enter/Return is suppressed.");
    eprintln!("focus a text field and press Enter — no newline should appear.");
    eprintln!("Ctrl-C to quit.");
    for event in tap.iter() {
        match event.kind {
            EventKind::KeyDown(k) => eprintln!("down  {:?}", k),
            EventKind::KeyRepeat(k) => eprintln!("rep   {:?}", k),
            EventKind::KeyUp(k) => eprintln!("up    {:?}", k),
        }
    }
}
