use pretty_assertions::assert_eq;

use super::*;

const RAN: &[u8] =
    b"{ \"child-pid\": 1160575, \"mnt-namespace\": 4026533585 }\n{ \"exit-code\": 3 }\n";
const MOUNT_FAILED: &[u8] = b"{ \"child-pid\": 1160579, \"mnt-namespace\": 4026533589 }\n";

#[test]
fn status_stream_gives_pid_and_exit_code() {
    assert_eq!(parse_status(RAN), BwrapStatus { child_pid: Some(1_160_575), exit_code: Some(3) });
    assert_eq!(parse_status(MOUNT_FAILED).exit_code, None);
    assert_eq!(parse_status(b"not json\n{}\n"), BwrapStatus::default());
}

#[test]
fn exit_code_after_the_child_pid_is_a_command_ending() {
    assert_eq!(ending(parse_status(RAN), "", Some(3)), Ending::Ran { code: 3 });
}

#[test]
fn child_pid_without_exit_code_is_a_setup_failure() {
    let stderr = "bwrap: Can't find source path /nonexistent: No such file or directory\n";
    assert_eq!(
        ending(parse_status(MOUNT_FAILED), stderr, Some(1)),
        Ending::SetupFailed {
            reason: "bwrap: Can't find source path /nonexistent: No such file or directory"
                .to_owned(),
            inner: false,
        }
    );
}

#[test]
fn silent_bwrap_failure_names_its_exit() {
    let Ending::SetupFailed { reason, .. } = ending(BwrapStatus::default(), "", Some(1)) else {
        panic!("not a setup failure");
    };
    assert_eq!(reason, "bwrap exited with 1 before the command started");
}

#[test]
fn inner_report_wins_over_an_exit_code() {
    let stderr = format!("{SETUP_PREFIX}Landlock refused the rule set: no ABI 9\n");
    assert_eq!(
        ending(parse_status(RAN), &stderr, Some(125)),
        Ending::SetupFailed {
            reason: "Landlock refused the rule set: no ABI 9".to_owned(),
            inner: true
        }
    );
}

#[test]
fn only_a_busy_overlay_of_bwrap_is_tried_again() {
    let busy = Ending::SetupFailed {
        reason: "bwrap: Can't mount overlay on /home/u/.cargo: Device or resource busy".to_owned(),
        inner: false,
    };
    assert!(retry_overlay(&busy, 1, Duration::ZERO));
    assert!(!retry_overlay(&busy, OVERLAY_ATTEMPTS, Duration::ZERO));
    assert!(!retry_overlay(&busy, 1, OVERLAY_RETRY_WINDOW));
    let other = Ending::SetupFailed { reason: "bwrap: Can't stat fd".to_owned(), inner: false };
    assert!(!retry_overlay(&other, 1, Duration::ZERO));
    let inner = Ending::SetupFailed { reason: "overlay".to_owned(), inner: true };
    assert!(!retry_overlay(&inner, 1, Duration::ZERO));
    assert!(!retry_overlay(&Ending::Ran { code: 1 }, 1, Duration::ZERO));
}
