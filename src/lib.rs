//! architect — architectural memory engine (library surface).
//!
//! The binary is one client of this library; `cargo test` is another (the
//! law regression suite in tests/laws.rs); an embedding application would be
//! a third. Everything here is deterministic and offline — AI tools consume
//! this through MCP, they are never a component of it.

pub mod laws;
pub mod mcp;
pub mod model;
pub mod query;
pub mod scan;
pub mod store;
pub mod watch;
