//! chronymon's database: what the daemon writes and the web view reads.
//!
//! SQLite in WAL mode: one writer, the daemon, and readers opening the
//! same file while it writes.

pub mod chrony_log;
pub mod error;
pub mod reader;
pub mod schema;
mod text;
pub mod timestamp;
pub mod writer;
