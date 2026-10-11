//! The reasons of `sandbox.explain`.

use efr_protocol::ExitKind;
use efr_sandbox::{FloorKind, MaskKind, MountOrigin, WriteRootKind};

use super::{REVIEWED_FLOOR, reason, why};

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

#[test]
fn explain_names_undo_for_floor_file() {
    let shell = Some(MountOrigin::Floor(FloorKind::ShellStartup));

    let (kept, _) = reason(shell, true);
    let (off, _) = reason(shell, false);

    assert!(
        kept.ends_with("efr copies the file first, so efr undo reverts it; else you decide"),
        "{kept}"
    );
    assert!(!off.contains("undo"), "without the home store, no undo: {off}");
}

#[test]
fn explain_names_reviewer_for_floor_file() {
    let shell = Some(MountOrigin::Floor(FloorKind::ShellStartup));
    let hypr = Some(MountOrigin::Floor(FloorKind::Autostart));

    let (with_backups, exit) = reason(shell, true);
    let (without, _) = reason(shell, false);

    assert_eq!(with_backups, format!("a shell startup file (floor); {REVIEWED_FLOOR}"));
    assert_eq!(exit, Some(ExitKind::Persistence));
    assert!(reason(hypr, true).0.contains("goes to the reviewer"));
    assert_eq!(without, "a shell startup file (floor)", "no backups, no reviewer");
    // The surface of a repository and sandbox.protect stay with the user, and efr's own
    // files are refused.
    for user in [FloorKind::GitConfig, FloorKind::GitHooks, FloorKind::User] {
        let (text, _) = reason(Some(MountOrigin::Floor(user)), true);
        assert!(!text.contains("reviewer"), "{user:?}: {text}");
    }
    assert_eq!(
        reason(Some(MountOrigin::Floor(FloorKind::EfrBinary)), true),
        ("an efr program (floor; no approval writes it)".to_owned(), Some(ExitKind::Config))
    );
}
