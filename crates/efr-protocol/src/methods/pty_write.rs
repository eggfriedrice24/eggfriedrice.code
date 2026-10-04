//! `pty.write`: type into a PTY.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Base64Bytes, PtyId};

/// The params of `pty.write`: input bytes for the PTY, as if typed. Only input goes
/// this way; answers to terminal queries come from the daemon's own screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyWrite {
    /// The PTY to write to.
    pub pty_id: PtyId,
    /// The bytes.
    pub data: Base64Bytes,
}

/// The result of `pty.write`: the bytes were queued for the PTY.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyWriteResult {}
