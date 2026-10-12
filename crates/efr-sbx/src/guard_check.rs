//! `efr-sbx guard-check --call-dir <job call dir> --state-dir $SBX/jobs/<call>`: the
//! surface guard of a contained job while it runs (efr's auto spec, section 3.18).
//!
//! A job runs for hours with no turn around it, so its writes reach the git surface of
//! a repository that it made long before the job ends. efrd runs this check every few
//! minutes, before a contained call whose guard roots overlap the job's, and at a
//! restart for a job that left no `result.json`. It runs as the user, outside any
//! sandbox, like the launcher's own end guard.
//!
//! It reads the record of the job's start from the state dir
//! (`surface-before.json`, on disk, so it survives a reboot), takes the job's guard
//! lock, records the surface again, compares, moves every entry that breaks a rule to
//! the job's quarantine, and writes `guard.json` with the changes. The job's spec in
//! the call dir must name the same state dir; a call dir that is gone (after a reboot)
//! leaves the record alone to say where things are. Exit status: 0 with no change, 3
//! with a change, 1 on an error.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use efr_protocol::SurfaceChange;
use efr_sandbox::GUARD_FILE;
use serde::{Deserialize, Serialize};

use crate::call_dir::{CallDir, PrivateDir};
use crate::error::{CallDirProblem, SbxError};
use crate::guard::{self, Guard};

/// The exit status when the check found a change.
const BROKEN: u8 = 3;

/// `guard.json`: what one check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuardReport {
    /// Each change that breaks a rule of the guard.
    pub(crate) changes: Vec<SurfaceChange>,
    /// True when an entry moved to the quarantine.
    pub(crate) quarantined: bool,
}

/// Runs `efr-sbx guard-check`.
pub(crate) fn main(call_dir: &Path, state_dir: &Path) -> ExitCode {
    match check(call_dir, state_dir) {
        Ok(report) if report.changes.is_empty() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(BROKEN),
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "efr-sbx guard-check: {}", error.chain());
            ExitCode::from(1)
        }
    }
}

fn check(call_dir: &Path, state_dir: &Path) -> Result<GuardReport, SbxError> {
    let state = PrivateDir::open(state_dir)?;
    match CallDir::open(call_dir) {
        Ok(call) => {
            let names = call.spec.job.is_some()
                && call.spec.runtime.job_state_dir(call.spec.call) == state_dir;
            if !names {
                return Err(SbxError::CallDir {
                    path: call_dir.to_path_buf(),
                    problem: CallDirProblem::Ids,
                });
            }
        }
        // NOTE: $R is a tmpfs: after a reboot the call dir is gone, and the record on
        // disk says all that the check needs.
        Err(SbxError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let Some(_lock) = guard::lock(state_dir)? else {
        return Err(SbxError::os(
            "take the job's guard lock",
            std::io::ErrorKind::WouldBlock.into(),
        ));
    };
    let (mut guard, places) = Guard::load(&state)?;
    let mut changes = guard.after(&places.start);
    if guard::quarantine(&mut changes, &places.quarantine).is_err() {
        for change in &mut changes {
            change.quarantined = false;
        }
    }
    let report =
        GuardReport { quarantined: changes.iter().any(|change| change.quarantined), changes };
    let bytes = serde_json::to_vec_pretty(&report)
        .map_err(|error| SbxError::os("write the guard report", std::io::Error::other(error)))?;
    state.write_atomic(GUARD_FILE, &bytes)?;
    Ok(report)
}
