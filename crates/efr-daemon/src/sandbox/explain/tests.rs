//! The reasons of `sandbox.explain`.

use efr_protocol::ExitKind;
use efr_sandbox::{FloorKind, MaskKind, MountOrigin, WriteRootKind};

use super::why;

#[test]
fn every_origin_has_a_reason_and_the_write_exit_it_would_be() {
    let cases = [
        (
            Some(MountOrigin::Floor(FloorKind::ShellStartup)),
            "a shell startup file (floor)",
            Some(ExitKind::Persistence),
        ),
        (
            Some(MountOrigin::Mask(MaskKind::EngineSecret)),
            "a secret (hidden; no approval opens it)",
            Some(ExitKind::Secret),
        ),
        (
            Some(MountOrigin::WriteRoot(WriteRootKind::TurnProject)),
            "in the turn's project (write root)",
            None,
        ),
        (None, "outside every write root, like the rest of the system", Some(ExitKind::Write)),
        (Some(MountOrigin::Cache), "a tool cache (writes go to a private copy)", None),
        (
            Some(MountOrigin::Floor(FloorKind::Config)),
            "efr's config (floor; no approval writes it)",
            Some(ExitKind::Config),
        ),
        (Some(MountOrigin::Socket), "a socket or bus that an approval opens for one call", None),
        (Some(MountOrigin::Device), "a device that an approval opens for one call", None),
    ];
    for (origin, reason, exit) in cases {
        assert_eq!(why(origin), (reason, exit), "{origin:?}");
    }
}
