use std::path::PathBuf;

use efr_protocol::{CacheMode, CheckOutcome};
use pretty_assertions::assert_eq;

use crate::probe::{
    BwrapFacts, ProbeFailure, ProbeReport, bwrap_missing_flags, check_bwrap, check_landlock,
    parse_bwrap_version,
};

const HELP: &str = "usage: bwrap\n    --args FD  x\n    --disable-userns  x\n    --bind-fd FD DEST\n    --ro-bind-fd FD DEST\n    --overlay RWSRC WORKDIR DEST\n    --tmp-overlay DEST\n    --json-status-fd FD\n";

fn bwrap() -> BwrapFacts {
    BwrapFacts {
        path: Some("/usr/bin/bwrap".into()),
        owner_uid: 0,
        mode: 0o755,
        writable_by_user: false,
        help: HELP.to_owned(),
        version: "bubblewrap 0.13.0\n".to_owned(),
    }
}

#[test]
fn landlock_needs_abi_nine_and_the_erratum_fix() {
    assert_eq!(check_landlock(Some(10), 0xf), Ok(()));
    assert_eq!(check_landlock(None, 0), Err(ProbeFailure::LandlockOff));
    assert_eq!(check_landlock(Some(8), 0xf), Err(ProbeFailure::AbiLow { found: 8 }));
    assert_eq!(check_landlock(Some(9), 0b011), Err(ProbeFailure::Erratum));
    assert_eq!(
        ProbeFailure::AbiLow { found: 6 }.reason(),
        "Landlock ABI 6 found; auto needs 9 (Linux 7.1)"
    );
}

#[test]
fn bwrap_must_be_roots_unprivileged_and_complete() {
    assert_eq!(check_bwrap(&bwrap()), Ok(()));
    assert_eq!(check_bwrap(&BwrapFacts { path: None, ..bwrap() }), Err(ProbeFailure::NoBwrap));
    assert_eq!(
        check_bwrap(&BwrapFacts { mode: 0o4755, ..bwrap() }),
        Err(ProbeFailure::SetuidBwrap)
    );
    assert!(matches!(
        check_bwrap(&BwrapFacts { owner_uid: 1000, ..bwrap() }),
        Err(ProbeFailure::BwrapNotTrusted { .. })
    ));
    let old = BwrapFacts {
        help: HELP.replace("--bind-fd", "--bind"),
        version: "bubblewrap 0.6.2".to_owned(),
        ..bwrap()
    };
    let failure = check_bwrap(&old).unwrap_err();
    assert_eq!(failure.reason(), "bubblewrap 0.6.2 found; it lacks --bind-fd");
    assert_eq!(bwrap_missing_flags(HELP), Vec::<&str>::new());
    assert_eq!(parse_bwrap_version("bubblewrap 0.13.0"), Some("0.13.0".to_owned()));
    assert_eq!(parse_bwrap_version("nothing"), None);
}

#[test]
fn every_failure_has_a_reason_a_fix_and_a_check() {
    let failures = [
        ProbeFailure::Disabled,
        ProbeFailure::Platform { platform: "linux riscv64".to_owned() },
        ProbeFailure::NoBwrap,
        ProbeFailure::OldBwrap { version: None, flag: "--args".to_owned() },
        ProbeFailure::SetuidBwrap,
        ProbeFailure::BwrapNotTrusted { path: "/opt/bwrap".into() },
        ProbeFailure::UsernsOff,
        ProbeFailure::AppArmor,
        ProbeFailure::LandlockOff,
        ProbeFailure::AbiLow { found: 4 },
        ProbeFailure::Erratum,
        ProbeFailure::InWriteRoot {
            what: "efr-sbx".to_owned(),
            path: "/home/u/p/efr/target/debug".into(),
        },
        ProbeFailure::LauncherMismatch,
        ProbeFailure::NoZsh,
        ProbeFailure::RelativePath { entry: "bin".to_owned() },
        ProbeFailure::SelfTest { detail: "a write outside".to_owned() },
    ];
    for failure in failures {
        let check = failure.check();
        assert_eq!(check.outcome, CheckOutcome::Fail);
        assert!(check.detail.as_ref().is_some_and(|text| text.len() > 10), "{failure:?}");
        assert!(check.fix.as_ref().is_some_and(|text| text.len() > 10), "{failure:?}");
        assert!(!failure.reason().contains(['\u{2014}', '\u{2013}']));
    }
    assert!(ProbeFailure::NoBwrap.fix().contains("pacman -S bubblewrap"));
}

#[test]
fn a_report_turns_into_the_status() {
    let ready = ProbeReport {
        landlock_abi: Some(10),
        errata: Some(0xf),
        bwrap: Some("/usr/bin/bwrap".into()),
        bwrap_version: Some("0.13.0".to_owned()),
        cache_mode: CacheMode::Overlay,
        ..ProbeReport::default()
    };
    let status = ready.status();
    assert!(status.available);
    assert_eq!(status.bwrap, Some(PathBuf::from("/usr/bin/bwrap")));
    let failed = ProbeReport { failure: Some(ProbeFailure::UsernsOff), ..ready };
    let status = failed.status();
    assert!(!status.available);
    assert_eq!(status.fix.as_deref(), Some("sysctl kernel.unprivileged_userns_clone=1"));
    assert_eq!(ProbeReport::from_json(&failed.to_json().unwrap()).unwrap(), failed);
}
