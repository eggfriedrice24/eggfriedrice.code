use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{MAX_LINE, of};

/// A process `pid` in the fake `/proc` at `proc`, with `children` and `cmdline`.
fn process(proc: &Path, pid: u32, children: &[u32], cmdline: &[u8]) {
    let task = proc.join(format!("{pid}/task/{pid}"));
    fs::create_dir_all(&task).expect("task directory");
    let list: Vec<String> = children.iter().map(u32::to_string).collect();
    fs::write(task.join("children"), format!("{} ", list.join(" "))).expect("children");
    fs::write(proc.join(format!("{pid}/cmdline")), cmdline).expect("cmdline");
}

#[test]
fn the_jobs_are_the_command_lines_of_the_shells_children() {
    let proc = tempfile::tempdir().expect("a fake /proc");
    let long = format!("python3\0-c\0{}\0", "a".repeat(300));
    process(proc.path(), 10, &[11, 12, 13], b"zsh\0");
    process(proc.path(), 11, &[], b"sleep\x00600\x00");
    process(proc.path(), 12, &[], long.as_bytes());
    // 13 ended between the two reads.

    let jobs = of(proc.path(), 10).expect("the shell's children");

    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0], "sleep 600");
    assert_eq!(jobs[1].chars().count(), MAX_LINE + 3);
    assert!(jobs[1].starts_with("python3 -c aaa") && jobs[1].ends_with("..."));
}

#[test]
fn a_shell_without_children_has_no_jobs_and_a_gone_one_cannot_tell() {
    let proc = tempfile::tempdir().expect("a fake /proc");
    process(proc.path(), 10, &[], b"zsh\0");

    assert_eq!(of(proc.path(), 10), Some(Vec::new()));
    assert_eq!(of(proc.path(), 99), None);
}
