//! The blob dir: `<data dir>/attachments/<conversation-id>/<sha256>`, the bytes that
//! efrd sends to a model.
//!
//! A blob is exactly what a model gets: the prepared image or the PDF, with no
//! metadata. Its name is the SHA-256 of the original bytes, so the attachments of one
//! conversation with the same bytes share it. The dirs are 0700 and the files 0600;
//! the conversation's delete removes its dir.

use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use efr_protocol::ConversationId;

/// The dir of the blobs below the data dir.
pub(crate) const ATTACHMENTS_DIR: &str = "attachments";

/// The blob dir of the daemon.
#[derive(Debug, Clone)]
pub(crate) struct Blobs {
    root: PathBuf,
}

impl Blobs {
    /// The blobs below the data dir `data`.
    pub(crate) fn new(data: &Path) -> Self {
        Blobs { root: data.join(ATTACHMENTS_DIR) }
    }

    /// The dir of the blobs of `conversation_id`.
    pub(crate) fn dir(&self, conversation_id: ConversationId) -> PathBuf {
        self.root.join(conversation_id.to_string())
    }

    /// The blob of the original bytes with the SHA-256 `sha256`. Each part is an id or
    /// a hash that efrd made, never text from a client.
    fn path(&self, conversation_id: ConversationId, sha256: &str) -> PathBuf {
        self.dir(conversation_id).join(sha256)
    }

    /// Writes `bytes` as the blob `sha256` of `conversation_id`, unless it is there. A
    /// reader sees no blob or the whole one. It blocks.
    pub(crate) fn write(
        &self,
        conversation_id: ConversationId,
        sha256: &str,
        bytes: &[u8],
    ) -> io::Result<()> {
        let path = self.path(conversation_id, sha256);
        if path.is_file() {
            return Ok(());
        }
        DirBuilder::new().recursive(true).mode(0o700).create(self.dir(conversation_id))?;
        efr_stdx::fs::write_atomic(&path, bytes).map_err(io::Error::other)
    }

    /// Removes the blob `sha256` of `conversation_id`; a blob that is gone already is no
    /// error. It blocks.
    pub(crate) fn remove(&self, conversation_id: ConversationId, sha256: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(conversation_id, sha256)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// The bytes of the blob `sha256` of `conversation_id`. It blocks.
    pub(crate) fn read(
        &self,
        conversation_id: ConversationId,
        sha256: &str,
    ) -> io::Result<Vec<u8>> {
        std::fs::read(self.path(conversation_id, sha256))
    }
}
