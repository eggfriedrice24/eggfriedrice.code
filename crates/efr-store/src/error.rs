//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_protocol::{ConversationId, PtyId, Seq};
use efr_stdx::StdxError;

use crate::outbox::OutboxId;
use crate::receipts::Receipt;

/// Every way a store operation can fail.
///
/// Variants carry the data a caller acts on (the path, the sequence number, the stored
/// receipt). The message names what failed in one sentence and leaves the text of the
/// cause to the `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// SQLite could not open the database.
    #[error("could not open the database {}", .path.display())]
    Open {
        /// The database file, or `:memory:`.
        path: PathBuf,
        /// The error from SQLite.
        #[source]
        source: rusqlite::Error,
    },

    /// A file that the store needs could not be created.
    #[error("could not create {}", .path.display())]
    CreateFile {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: StdxError,
    },

    /// A directory that the store needs could not be created.
    #[error("could not create the directory {}", .path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A connection setting could not be applied.
    #[error("could not set the {pragma} pragma")]
    Configure {
        /// The pragma, such as `journal_mode`.
        pragma: &'static str,
        /// The error from SQLite.
        #[source]
        source: rusqlite::Error,
    },

    /// SQLite kept the database out of write-ahead-log mode, which the single writer and
    /// the concurrent readers depend on.
    #[error("the database {} stayed in journal mode {mode:?} instead of wal", .path.display())]
    NotWal {
        /// The database file.
        path: PathBuf,
        /// The journal mode SQLite reported.
        mode: String,
    },

    /// The database stayed locked longer than the busy timeout.
    #[error("the database is busy")]
    Busy {
        /// The error from SQLite.
        #[source]
        source: rusqlite::Error,
    },

    /// A statement failed.
    #[error("a database statement failed")]
    Sqlite {
        /// The error from SQLite.
        #[source]
        source: rusqlite::Error,
    },

    /// A migration failed. Each migration runs in its own transaction, so the database
    /// stays at the last version that succeeded.
    #[error("could not migrate the database")]
    Migrate {
        /// The error from the migration runner.
        #[source]
        source: Box<rusqlite_migration::Error>,
    },

    /// The database was written by a newer efr whose migrations this build does not
    /// have. Migrations are forward-only, so this build cannot use it.
    #[error("the database has schema version {found}, newer than the {supported} this build knows")]
    SchemaTooNew {
        /// The database's `user_version`.
        found: u32,
        /// The newest version this build can migrate to.
        supported: u32,
    },

    /// The copy taken before a migration could not be written.
    #[error("could not back up the database to {}", .path.display())]
    Backup {
        /// The backup file.
        path: PathBuf,
        /// The error from SQLite.
        #[source]
        source: rusqlite::Error,
    },

    /// A backup file could not be cleared away or moved into place.
    #[error("could not put the backup file {} in place", .path.display())]
    BackupFile {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A path that SQLite must receive as text is not valid UTF-8.
    #[error("{} is not valid UTF-8", .path.display())]
    NonUtf8Path {
        /// The path.
        path: PathBuf,
    },

    /// A value could not be serialised to JSON for a column.
    #[error("could not serialise the {what} to JSON")]
    Encode {
        /// What was being serialised, such as `"event"`.
        what: &'static str,
        /// The error from the serialiser.
        #[source]
        source: serde_json::Error,
    },

    /// A stored event does not decode as an [`efr_protocol::Event`]. An unknown kind is
    /// not this error: it decodes as `Event::Unknown`.
    #[error("the stored event {seq} is malformed")]
    DecodeEvent {
        /// The event's sequence number.
        seq: Seq,
        /// The error from the parser.
        #[source]
        source: serde_json::Error,
    },

    /// A stored value does not decode as the type its column holds.
    #[error("the {table}.{column} column holds a malformed value")]
    DecodeColumn {
        /// The table.
        table: &'static str,
        /// The column.
        column: &'static str,
        /// What was wrong with the value.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// A command id already has a receipt. The batch that carried the new receipt
    /// wrote nothing; the caller answers with the stored outcome instead.
    #[error("the command {} already has a receipt", .receipt.command_id)]
    DuplicateCommand {
        /// The stored receipt.
        receipt: Box<Receipt>,
    },

    /// An event names a conversation that has no `conversation_created` event.
    #[error("the conversation {conversation_id} does not exist")]
    UnknownConversation {
        /// The conversation.
        conversation_id: ConversationId,
    },

    /// A `conversation_created` event names a conversation that already exists.
    #[error("the conversation {conversation_id} already exists")]
    ConversationExists {
        /// The conversation.
        conversation_id: ConversationId,
    },

    /// An event that belongs to a conversation was appended without one.
    #[error("the {kind} event needs a conversation id")]
    MissingConversation {
        /// The event's kind.
        kind: String,
    },

    /// No claimed, unfinished outbox item has this id.
    #[error("no claimed outbox item has the id {id}")]
    OutboxItemNotClaimed {
        /// The id.
        id: OutboxId,
    },

    /// The writer thread could not be started.
    #[error("could not start the store writer thread")]
    SpawnWriter {
        /// The error from the thread builder.
        #[source]
        source: StdxError,
    },

    /// The writer has stopped, so nothing can be written any more.
    #[error("the store writer has stopped")]
    WriterStopped,

    /// A task of the store panicked before it answered.
    #[error("the store's {task} task panicked")]
    TaskPanicked {
        /// The task, such as `"reader"`.
        task: &'static str,
    },

    /// A recording file could not be read, written or created.
    #[error("could not access the recording file {}", .path.display())]
    RecordingIo {
        /// The file or directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A recording file holds bytes that are not a chunk where a chunk must start.
    #[error("the recording file {} is corrupt at byte {offset}", .path.display())]
    CorruptRecording {
        /// The file.
        path: PathBuf,
        /// The file offset of the bad chunk header.
        offset: u64,
    },

    /// A recording writer lost its open segment when an earlier write panicked; the
    /// next `Recordings::start` for the PTY recovers it.
    #[error("the recording of {pty_id} has no open segment")]
    RecordingBroken {
        /// The PTY.
        pty_id: PtyId,
    },
}

impl From<rusqlite::Error> for StoreError {
    fn from(source: rusqlite::Error) -> Self {
        match source.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                StoreError::Busy { source }
            }
            _ => StoreError::Sqlite { source },
        }
    }
}

impl From<rusqlite_migration::Error> for StoreError {
    fn from(source: rusqlite_migration::Error) -> Self {
        StoreError::Migrate { source: Box::new(source) }
    }
}

#[cfg(test)]
mod tests;
