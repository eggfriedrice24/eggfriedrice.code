use super::*;

#[test]
fn which_skips_relative_entries() {
    assert_eq!(which("sh", "relative:/nonexistent"), None);
    assert!(which("sh", "relative:/usr/bin:/bin").is_some());
}

#[test]
fn a_namespace_error_names_user_namespaces() {
    let failure = setup_failure("bwrap: No permissions to create a new namespace");
    assert!(matches!(failure, ProbeFailure::UsernsOff | ProbeFailure::AppArmor), "{failure:?}");
    let failure = setup_failure("bwrap: setting up uid map: Permission denied");
    assert!(matches!(failure, ProbeFailure::UsernsOff | ProbeFailure::AppArmor), "{failure:?}");
}

#[test]
fn another_setup_error_is_a_self_test_failure() {
    let failure = setup_failure("bwrap: Can't mount overlay on /x: Device or resource busy");
    assert_eq!(
        failure,
        ProbeFailure::SelfTest {
            detail: "bwrap: Can't mount overlay on /x: Device or resource busy".to_owned()
        }
    );
}

#[test]
fn missing_bwrap_has_no_facts() {
    assert_eq!(bwrap_facts(None), BwrapFacts::default());
    assert_eq!(bwrap_facts(Some(Path::new("/nonexistent/bwrap"))), BwrapFacts::default());
}
