//! `admin.sandbox_check`: runs the sandbox probe now, for `efr sandbox check`. The new
//! status counts from now on, so a fixed sysctl needs no restart.

use std::sync::Arc;

use efr_protocol::{AdminSandboxCheck, AdminSandboxCheckResult, CheckOutcome, SandboxCheck};
use efr_transport::Responder;

use crate::DaemonError;
use crate::state::State;

pub(crate) async fn handle(
    state: &State,
    _params: AdminSandboxCheck,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let settings = Arc::clone(&state.settings.borrow());
    let report = state.sandbox.probe(&settings).await;
    let status = state.sandbox.current();
    let mut checks = report.checks.clone();
    // NOTE: a status that a test set has no checks behind it; its reason stands as one.
    if checks.is_empty() {
        checks.push(SandboxCheck {
            name: "probe".to_owned(),
            outcome: if status.available { CheckOutcome::Ok } else { CheckOutcome::Fail },
            detail: status.reason.clone(),
            fix: status.fix.clone(),
        });
    }
    let result = AdminSandboxCheckResult {
        status,
        checks,
        launch_us: report.launch_us,
        snapshot_launch_us: report.snapshot_launch_us,
    };
    responder.item(&result).await?;
    Ok(())
}
