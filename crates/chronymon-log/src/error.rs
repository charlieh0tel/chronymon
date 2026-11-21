//! Typed errors for the database.

use std::path::PathBuf;

/// Anything that can go wrong with the database.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database could not be opened or created.
    #[error("{path}: {source}")]
    Open {
        /// The database's path.
        path: PathBuf,
        /// What failed.
        source: rusqlite::Error,
    },

    /// The daemon kept checkpointing into the database while it was
    /// read without its WAL.
    #[error("{path}: changed while read, {attempts} times")]
    Changing {
        /// The database's path.
        path: PathBuf,
        /// How many times it was read.
        attempts: usize,
    },

    /// A newer chronymon wrote this database.
    #[error("database is schema {found}, newer than this program's {supported}")]
    NewerSchema {
        /// The schema the database records.
        found: i64,
        /// The newest this program knows.
        supported: i64,
    },

    /// A timestamp outside what 64 bits of nanoseconds hold.
    #[error("timestamp out of range")]
    Timestamp,

    /// An inode or a length beyond what SQLite's integers hold.
    #[error("{0} out of range")]
    Range(u64),

    /// A stored inode or length that is negative.
    #[error("stored value {0} is negative")]
    Negative(i64),

    /// Reading a file failed: one to tell whether the database changed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A write, outside a batch, or a batch began with a transaction
    /// still open: one a write's failed rollback left, or a batch.
    #[error("a transaction is still open")]
    Unfinished,

    /// A batch's writes were rolled back, all of them, and are lost: by
    /// SQLite after an error such as a full disk, or for a failed commit.
    #[error("uncommitted writes lost: {0}")]
    BatchLost(String),

    /// A statement failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// The result of a database operation.
pub type Result<T> = std::result::Result<T, Error>;
