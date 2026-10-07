//! `efr-sbx bridge --listen-fd N --socket P`: the seam of phase 2 (the spec's sections
//! 2.1 and 8.2).
//!
//! In phase 2 the inner stage starts the bridge before it applies its own Landlock
//! domain. The bridge is not dumpable, gets a Landlock domain of its own that allows
//! `RESOLVE_UNIX` on the call's proxy socket only, and copies TCP from
//! `127.0.0.1:3128` inside the network namespace to that socket. Phase 1 has no
//! network at all, so the subcommand exists and refuses, and the launcher refuses a spec
//! with `NetworkPlan::Proxy`.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use crate::error::SbxError;

/// Refuses: the bridge comes with the proxy.
pub(crate) fn main(_listen_fd: i32, _socket: &Path) -> ExitCode {
    let error = SbxError::LaterPhase { what: "the network bridge" };
    let _ = writeln!(std::io::stderr(), "efr-sbx: {error}");
    ExitCode::from(2)
}
