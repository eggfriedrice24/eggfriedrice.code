//! The ssh config reader and the read of `/proc/net/unix`.

use std::collections::HashSet;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::{control_paths, listened, listening, parse_config, star_match};

#[test]
fn parse_config_finds_every_control_path_and_include() {
    let text = "\
# ControlPath /commented
Host build
    ControlMaster auto
    ControlPath ~/.ssh/cm-%C
Host *
  controlpath=\"/tmp/%r@%h:%p\"
  ControlPath = none
Include ~/.ssh/config.d/* extra.conf
Match host x
\tControlPath\t${XDG_RUNTIME_DIR}/ssh-%C
";
    let (paths, includes) = parse_config(text);
    assert_eq!(paths, ["~/.ssh/cm-%C", "/tmp/%r@%h:%p", "none", "${XDG_RUNTIME_DIR}/ssh-%C"]);
    assert_eq!(includes, ["~/.ssh/config.d/*", "extra.conf"]);
}

#[test]
fn control_paths_read_the_user_config_and_its_includes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let ssh = home.join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(
        ssh.join("config"),
        "Include config.d/*.conf ~/other\nControlPath ~/.ssh/cm-%C\n",
    )
    .unwrap();
    std::fs::write(ssh.join("config.d/a.conf"), "ControlPath /tmp/a-%C\n").unwrap();
    std::fs::write(ssh.join("config.d/b.txt"), "ControlPath /tmp/not-read\n").unwrap();
    std::fs::write(home.join("other"), "Include other\nControlPath /tmp/other-%C\n").unwrap();
    let found = control_paths(home);
    // NOTE: `/etc/ssh/ssh_config` of the machine may add more; the user's come first,
    // each file before the files of its `Include` lines.
    assert_eq!(found[..3], ["~/.ssh/cm-%C", "/tmp/a-%C", "/tmp/other-%C"]);
    assert!(!found.iter().any(|path| path == "/tmp/not-read"), "{found:?}");
}

#[test]
fn star_match_matches_names() {
    assert!(star_match("*.conf", "a.conf"));
    assert!(star_match("a*b*c", "a-x-b-y-c"));
    assert!(!star_match("*.conf", "a.txt"));
    assert!(star_match("exact", "exact"));
    assert!(!star_match("exact", "exactly"));
}

#[test]
fn listening_keeps_listening_sockets_with_a_known_inode() {
    let table = "\
Num       RefCount Protocol Flags    Type St Inode Path
0000000000000000: 00000002 00000000 00010000 0001 01 4242 /tmp/cm
0000000000000000: 00000002 00000000 00010000 0001 01 4343 /tmp/other
0000000000000000: 00000003 00000000 00000000 0001 03 4444 /tmp/cm
0000000000000000: 00000002 00000000 00010000 0001 01 4545 @abstract
0000000000000000: 00000002 00000000 00010000 0001 01 4646
";
    let inodes: HashSet<u64> = [4242, 4444, 4545, 4646].into_iter().collect();
    assert_eq!(listening(table, &inodes), [PathBuf::from("/tmp/cm")]);
}

#[test]
fn listened_finds_no_ssh_listener_for_a_plain_socket() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("app.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    assert!(listened(std::slice::from_ref(&socket)).is_empty());
    assert!(listened(&[]).is_empty());
}
