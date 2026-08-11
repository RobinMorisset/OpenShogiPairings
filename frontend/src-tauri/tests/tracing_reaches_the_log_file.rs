//! The embedded server reports trouble through `tracing`; the desktop app's only
//! error channel is `tauri-plugin-log`, which listens on the `log` facade. The
//! bridge between the two is `tracing`'s `log` feature, which forwards events
//! *only while no `tracing` subscriber is installed* — exactly this app's setup.
//!
//! That is a guarantee made by a Cargo feature, invisible in the code that
//! depends on it: turn it off (or install a subscriber here) and every
//! "could not write" the server emits vanishes, with nothing failing to build.
//! So it is asserted rather than assumed.

use std::sync::atomic::{AtomicUsize, Ordering};

static WARNINGS: AtomicUsize = AtomicUsize::new(0);

struct CountingLogger;

impl log::Log for CountingLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if record.level() == log::Level::Warn {
            WARNINGS.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {}
}

#[test]
fn a_tracing_warning_arrives_as_a_log_record() {
    log::set_boxed_logger(Box::new(CountingLogger)).expect("no logger installed yet");
    log::set_max_level(log::LevelFilter::Trace);

    tracing::warn!("persist: could not write /nowhere/tournament.json: disk full");

    assert_eq!(
        WARNINGS.load(Ordering::Relaxed),
        1,
        "a `tracing` warning did not reach the `log` facade — the desktop app \
         would log nothing when a save fails. Check that `tracing/log` is still \
         enabled (see this crate's Cargo.toml) and that nothing installed a \
         `tracing` subscriber."
    );
}
