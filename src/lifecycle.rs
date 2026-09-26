//! Process-lifecycle primitives shared by long-running Archietect commands.
//!
//! The signal handler only flips an atomic flag.  It never performs I/O or
//! takes a lock; the server loops observe the flag and drain/exit normally.

use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Install the small, async-signal-safe SIGINT/SIGTERM handler used by
/// long-running commands. Safe to call more than once.
pub fn install_shutdown_handlers() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as libc::sighandler_t);
    }
}

/// Return whether the process has been asked to stop.
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Acquire)
}

/// Reset state for an embedded caller or a test. Binary callers should not
/// need this because each invocation is a fresh process.
pub fn reset_shutdown_for_test() {
    SHUTDOWN_REQUESTED.store(false, Ordering::Release);
}

#[cfg(unix)]
extern "C" fn handle_signal(_: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_flag_starts_clear_and_can_be_reset() {
        reset_shutdown_for_test();
        assert!(!shutdown_requested());
    }
}
