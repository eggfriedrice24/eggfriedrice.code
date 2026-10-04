//! `pty.attach`: watch a PTY's screen and output.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Base64Bytes, PtyId, ScreenSnapshot, Seq, Size};

/// The params of `pty.attach`, a streaming method. Sequence numbers in this stream are
/// byte offsets in the PTY's recording.
///
/// With `since_seq`, the stream starts with the output after that offset, if the
/// recording still holds it. Otherwise it starts with a screen snapshot. Live output
/// follows either way. Attaching is idempotent and never changes the PTY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyAttach {
    /// The PTY to watch.
    pub pty_id: PtyId,
    /// The recording offset the client has already rendered up to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<Seq>,
    /// How many rows of scrollback a snapshot includes; the daemon caps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback_rows: Option<u32>,
}

/// One item of a `pty.attach` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PtyAttachItem {
    /// The screen as it was after the recording's first `seq` bytes.
    Snapshot {
        /// The recording offset that the snapshot reflects.
        seq: Seq,
        /// The screen.
        snapshot: ScreenSnapshot,
    },
    /// Raw output, to feed to the client's own terminal emulator.
    Output {
        /// The recording offset of the first byte of `data`.
        seq: Seq,
        /// The bytes.
        data: Base64Bytes,
    },
    /// The PTY changed size; output after `seq` assumes the new size.
    Resized {
        /// The recording offset at which the new size applies.
        seq: Seq,
        /// The new size.
        size: Size,
    },
}
