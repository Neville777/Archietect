//! archietect — architectural memory engine (library surface).
//!
//! The binary is one client of this library; `cargo test` is another (the
//! law regression suite in tests/laws.rs); an embedding application would be
//! a third. Everything here is deterministic and offline — AI tools consume
//! this through MCP, they are never a component of it.

pub mod invariants;
pub mod laws;
pub mod mcp;
pub mod model;
pub mod proposal;
pub mod query;
pub mod root;
pub mod rest;
pub mod scan;
pub mod scoring;
pub mod store;
pub mod structural;
pub mod watch;

/// The mtime of the currently-running binary's file on disk, at the moment
/// this is called. A long-running process (the MCP server, the REST server,
/// the watch daemon) captures this ONCE at startup; comparing that snapshot
/// against a fresh call later tells it whether the file at its own exe path
/// has been rebuilt since — i.e. whether it is now serving stale, in-memory
/// code while a newer binary sits on disk unused. Found the hard way: this
/// exact scenario, undetected, produced silently wrong answers across five
/// concurrent sessions during one afternoon of rapid rebuilds.
pub fn exe_mtime() -> Option<std::time::SystemTime> {
    std::env::current_exe().ok()?.metadata().ok()?.modified().ok()
}
