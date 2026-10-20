use super::{on_device, tty_device};

/// glibc's `makedev` of `/dev/pts/2` (major 136, minor 2).
const PTS_2: u64 = (136 << 8) | 2;

#[test]
fn send_by_hand_names_the_parent_as_shell() {
    let zsh = "4242 (zsh) S 1 4242 4242 34818 4242 4194560 0 0 0 0";
    assert!(on_device(zsh, PTS_2), "a shell on /dev/pts/2");
    assert!(!on_device(zsh, PTS_2 + 1), "a shell on another terminal");
    let cron = "77 (cron (daily)) S 1 77 77 0 -1 4194560";
    assert!(!on_device(cron, 0), "a parent with no terminal");
    assert!(!on_device("77 (cut", PTS_2));
}

#[test]
fn a_high_minor_number_packs_like_the_kernel() {
    let tty_nr = (136 << 8) | (300 & 0xff) | ((300 & 0xfff00) << 12);
    assert_eq!(tty_device(tty_nr), (136 << 8) | (300 & 0xff) | ((300 & 0xffff_ff00) << 12));
}
