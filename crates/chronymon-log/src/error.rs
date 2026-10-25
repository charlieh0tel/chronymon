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

    /// Reading a file to be written into the database failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A statement failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// The result of a database operation.
pub type Result<T> = std::result::Result<T, Error>;
