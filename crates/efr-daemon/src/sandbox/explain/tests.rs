//! The reasons of `sandbox.explain`.

use efr_protocol::{ExitKind, SandboxPathRole};
use efr_sandbox::{FloorKind, MaskKind, MountOrigin, WriteRootKind};

use super::why;

#[test]
fn every_role_has_a_reason_and_the_write_exit_it_would_be() {
    let cases = [
        (
            SandboxPathRole::Floor,
            Some(MountOrigin::Floor(FloorKind::ShellStartup)),
            "a shell startup file (floor)",
            Some(ExitKind::Persistence),
        ),
        (
            SandboxPathRole::Masked,
            Some(MountOrigin::Mask(MaskKind::EngineSecret)),
            "a secret (hidden; no approval opens it)",
            Some(ExitKind::Secret),
        ),
        (
            SandboxPathRole::WriteRoot,
            Some(MountOrigin::WriteRoot(WriteRootKind::TurnProject)),
            "in the turn's project (write root)",
            None,
        ),
        (
            SandboxPathRole::ReadOnly,
            None,
            "outside every write root, like the rest of the system",
            Some(ExitKind::Write),
        ),
        (
            SandboxPathRole::CacheOverlay,
            Some(MountOrigin::Cache),
            "a tool cache (writes go to a private copy)",
            None,
        ),
        (
            SandboxPathRole::Floor,
            Some(MountOrigin::Floor(FloorKind::Config)),
            "efr's config (floor; no approval writes it)",
            Some(ExitKind::Config),
        ),
    ];
    for (role, origin, reason, exit) in cases {
        assert_eq!(why(role, origin), (reason, exit), "{origin:?}");
    }
}
