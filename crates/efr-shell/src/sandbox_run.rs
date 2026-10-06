//! What a run needs to go through the launcher of the `auto` sandbox.

use std::fmt;
use std::path::PathBuf;

use efr_protocol::CallId;

/// A call that runs through the launcher, `efr-sbx run`, instead of being typed into
/// the hidden shell: efrd wrote its spec into `dir` and keeps the nonce that ends the
/// run.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxRun {
    /// The call's directory, `$R/sbx/<conversation>/<call>`, which holds the spec.
    pub dir: PathBuf,
    /// The call.
    pub call: CallId,
    /// The secret that only the launcher's end mark carries, so a mark that sandboxed
    /// code prints cannot end the run.
    pub nonce: [u8; 16],
}

/// The nonce ends the run, so `Debug` never shows it.
impl fmt::Debug for SandboxRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxRun")
            .field("dir", &self.dir)
            .field("call", &self.call)
            .field("nonce", &"<redacted>")
            .finish()
    }
}
