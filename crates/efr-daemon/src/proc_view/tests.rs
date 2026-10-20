use pretty_assertions::assert_eq;

use super::{ProcView as _, Stat, SystemProcView, makedev, parse_stat, tty_nr_device};

#[test]
fn zombie_shell_counts_as_dead() {
    let zombie = "4242 (zsh) Z 1 4242 4242 0 -1 4227332 0 0 0 0 0 0 0 0 20 0 1 0";
    let stat = parse_stat(zombie).unwrap();
    assert_eq!(stat.state, 'Z');
    let running = "4242 (zsh) S 1 4242 4242 34818 4242 4194560 0 0 0 0 0 0 0 0 20 0 1 0";
    assert_eq!(parse_stat(running).unwrap(), Stat { state: 'S', tty_nr: 34818 });
}

#[test]
fn a_command_name_with_spaces_and_parentheses_is_skipped_whole() {
    let odd = "77 (my (odd) cmd) R 1 77 77 34819 77 0";
    assert_eq!(parse_stat(odd).unwrap(), Stat { state: 'R', tty_nr: 34819 });
    assert_eq!(parse_stat("77 (cut"), None);
    assert_eq!(parse_stat("77 (x) S 1 2"), None);
}

#[test]
fn tty_nr_matches_the_pts_device() {
    // /dev/pts/2 is major 136, minor 2: tty_nr 34818 (136 << 8 | 2).
    assert_eq!(tty_nr_device(34818), makedev(136, 2));
    // /dev/pts/300: the minor number's high part sits in bits 20 and up.
    let tty_nr = (136 << 8) | (300 & 0xff) | ((300 & 0xfff00) << 12);
    assert_eq!(tty_nr_device(tty_nr), makedev(136, 300));
    assert_ne!(tty_nr_device(34818), makedev(136, 3));
}

#[test]
fn this_process_is_alive_and_a_pid_far_above_the_limit_is_not() {
    let view = SystemProcView;
    assert!(view.alive(std::process::id()));
    assert!(!view.alive(4_000_000_000));
    assert_eq!(view.tty_of(4_000_000_000), None);
    assert_eq!(view.device_of("/dev/pts/efr-test-none"), None);
}
