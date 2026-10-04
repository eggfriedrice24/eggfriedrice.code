//! `pty.resize`: change a PTY's size.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{PtyId, Size};

/// The params of `pty.resize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyResize {
    /// The PTY to resize.
    pub pty_id: PtyId,
    /// The size the client wants.
    pub size: Size,
}

/// The result of `pty.resize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyResizeResult {
    /// The size the PTY has now. It differs from the request when the daemon clamps it
    /// or another client holds the size.
    pub size: Size,
}
