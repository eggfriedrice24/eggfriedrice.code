//! The only SQLite owner in efr.
//!
//! The store keeps the append-only event log, the projections derived from it
//! (conversations, turns, approvals, shells), command receipts, the outbox, and the
//! index and segment files of PTY recordings. One writer actor owns the only
//! read-write connection and commits every write; read-only connections serve reads
//! through `spawn_blocking`.
//!
//! Allowed dependencies: `efr-protocol` (the events and ids it stores) and `efr-stdx`
//! (the clock, the named writer thread, private file creation). What does not belong
//! here: deciding what happens next (the conversation actor and the daemon do that),
//! the wire mapping of errors, and any other crate opening SQLite.
//!
//! `db` is a public module because its paths are the replacements that `clippy.toml`
//! names for `rusqlite::Connection::open` and `open_in_memory`.

// NOTE: missing_docs is set here and not in Cargo.toml, because Cargo rejects a
// `[lints]` table that both inherits the workspace lints and adds its own.
#![warn(missing_docs)]

pub mod db;
mod error;

pub use error::StoreError;
