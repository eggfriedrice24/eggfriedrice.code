//! The line rule of ssh control sockets and the match of a `ControlPath` pattern.

use rstest::rstest;

use super::{makes_or_uses, matches_pattern};

fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
}

#[rstest]
#[case::control_path_flag("ssh -S /tmp/cm host true", true)]
#[case::control_path_inline("ssh -S/tmp/cm host", true)]
#[case::scratch("ssh -S $SCRATCH/cm host true", true)]
#[case::master("ssh -M -S /tmp/cm -fN host", true)]
#[case::master_in_a_cluster("ssh -fNM host", true)]
#[case::control_command("ssh -O exit host", true)]
#[case::option_separate("ssh -o ControlPath=~/.ssh/cm host", true)]
#[case::option_inline("ssh -oControlPath=/tmp/cm host", true)]
#[case::option_with_space("ssh -o ControlMaster auto host", true)]
#[case::option_case("ssh -o controlmaster=yes host", true)]
#[case::persist("ssh -o ControlPersist=10m host", true)]
#[case::after_the_destination("ssh host -S /tmp/cm", true)]
#[case::autossh("autossh -M 0 -S /tmp/cm host", true)]
#[case::path_none("ssh -S none host", false)]
#[case::option_none("ssh -o ControlPath=none host", false)]
#[case::master_no("ssh -o ControlMaster=no host", false)]
#[case::autossh_monitor_port("autossh -M 20000 host", false)]
#[case::plain("ssh host true", false)]
#[case::remote_command("ssh host ssh -S /tmp/cm other", false)]
#[case::other_option("ssh -o StrictHostKeyChecking=no host", false)]
#[case::port_letter_is_a_value("ssh -p 2222 host", false)]
#[case::scp("scp -o ControlPath=/tmp/cm a host:b", true)]
#[case::scp_plain("scp a host:b", false)]
#[case::sftp("sftp -oControlMaster=auto host", true)]
#[case::sshfs_list("sshfs -o reconnect,ControlPath=/tmp/cm host:/ m", true)]
#[case::sshfs_plain("sshfs -o reconnect host:/ m", false)]
#[case::rsync("rsync -a -e ssh_-S_/tmp/cm src host:dst", true)]
#[case::rsync_long("rsync -a --rsh=ssh_-o_ControlPath=/tmp/cm src host:dst", true)]
#[case::rsync_plain("rsync -a -e ssh src host:dst", false)]
#[case::other_program("echo -S /tmp/cm", false)]
fn line_rule(#[case] line: &str, #[case] expected: bool) {
    // NOTE: an `_` stands for a space inside one word of `rsync -e`.
    let words: Vec<String> = words(line).into_iter().map(|word| word.replace('_', " ")).collect();
    assert_eq!(makes_or_uses(&words), expected, "{line:?}");
}

#[rstest]
#[case::exact("/tmp/cm", "/tmp/cm", true)]
#[case::exact_other("/tmp/cm", "/tmp/cm2", false)]
#[case::hash("/home/u/.cache/ssh/%C", "/home/u/.cache/ssh/0a1b2c", true)]
#[case::hash_other_dir("/home/u/.cache/ssh/%C", "/home/u/.cache/other/0a1b2c", false)]
#[case::tokens("/tmp/%r@%h:%p", "/tmp/u@example.org:22", true)]
#[case::tokens_need_the_at("/tmp/%r@%h:%p", "/tmp/u-example.org:22", false)]
#[case::variable("${XDG_RUNTIME_DIR}/ssh-%C", "/run/user/1000/ssh-0a1b", true)]
#[case::percent("/tmp/100%%-%h", "/tmp/100%-host", true)]
#[case::none("none", "none", false)]
#[case::prefix_only("/tmp/cm-%C", "/srv/tmp/cm-x", false)]
fn control_path_pattern(#[case] pattern: &str, #[case] path: &str, #[case] expected: bool) {
    assert_eq!(matches_pattern(pattern, path), expected, "{pattern:?} {path:?}");
}
