use std::fs::File;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::{
    Limits, Lines, MARKER_FILE, MAX_LINE_BYTES, Matcher, OutputFile, last_lines, line_text,
    make_dir, read, spawn_reader,
};

/// A `$SCRATCH` with the dir of the job outputs that efrd makes.
fn scratch() -> tempfile::TempDir {
    let scratch = tempfile::tempdir().unwrap();
    make_dir(scratch.path()).unwrap();
    scratch
}

fn small() -> Limits {
    Limits { head: 10, rest: 30, keep: 12 }
}

#[test]
fn job_output_head_and_tail_kept() {
    let scratch = scratch();
    let mut file = OutputFile::create(scratch.path(), "j1.out", small()).unwrap();
    let input: Vec<u8> = (0..200_u8).map(|n| b'a' + n % 26).collect();
    for chunk in input.chunks(7) {
        file.write(chunk).unwrap();
    }
    let kept = std::fs::read(scratch.path().join("jobs/j1.out")).unwrap();
    assert_eq!(&kept[..10], &input[..10], "the head stays whole");
    let text = String::from_utf8_lossy(&kept);
    let marker = text.find("\n[").unwrap();
    let after = text[marker..].split_once("bytes omitted]\n").unwrap().1;
    assert!(input.ends_with(after.as_bytes()), "the file ends with the output's end");
    assert!(after.len() >= 12 && after.len() <= 12 + 30, "{}", after.len());
    let omitted: usize = text[marker + 2..].split_whitespace().next().unwrap().parse().unwrap();
    assert_eq!(10 + omitted + after.len(), input.len(), "the marker counts what is left out");
}

#[test]
fn output_rename_keeps_a_whole_file() {
    let scratch = scratch();
    let mut file = OutputFile::create(scratch.path(), "j2.out", small()).unwrap();
    file.write(&[b'x'; 35]).unwrap();
    // NOTE: a reader that opened the file before the cut keeps its whole old file.
    let mut before = File::open(scratch.path().join("jobs/j2.out")).unwrap();
    file.write(&[b'y'; 10]).unwrap();
    let mut old = String::new();
    std::io::Read::read_to_string(&mut before, &mut old).unwrap();
    assert_eq!(old, format!("{}{}", "x".repeat(35), "y".repeat(10)), "the old file stays whole");
    let names: Vec<String> = std::fs::read_dir(scratch.path().join("jobs"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|name| name.ends_with(".tmp")), "{names:?}");
    let new = std::fs::read_to_string(scratch.path().join("jobs/j2.out")).unwrap();
    assert!(new.starts_with("xxxxxxxxxx\n[") && new.ends_with("yyyyyyyyyy"), "{new}");
}

#[test]
fn job_output_link_swap_refused() {
    let scratch = scratch();
    let elsewhere = scratch.path().join("elsewhere");
    std::fs::write(&elsewhere, "keep me").unwrap();
    std::os::unix::fs::symlink(&elsewhere, scratch.path().join("jobs/j3.out")).unwrap();
    assert!(OutputFile::create(scratch.path(), "j3.out", small()).is_err());
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "keep me");
    // A link in place of the whole dir is refused too.
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir(other.path().join("real")).unwrap();
    std::os::unix::fs::symlink(other.path().join("real"), other.path().join("jobs")).unwrap();
    assert!(OutputFile::create(other.path(), "j1.out", small()).is_err());
    assert_eq!(std::fs::read_dir(other.path().join("real")).unwrap().count(), 0);
}

#[tokio::test]
async fn output_write_error_keeps_draining() {
    let scratch = scratch();
    let mut file = OutputFile::create(scratch.path(), "j4.out", small()).unwrap();
    // NOTE: a descriptor that refuses writes stands in for a full disk.
    file.file = File::open(scratch.path().join("jobs/j4.out")).unwrap();
    let (reader, mut writer) = std::io::pipe().unwrap();
    let (send, _matches) = mpsc::unbounded_channel();
    let output = spawn_reader("j4", reader, Some(file), None, send).unwrap();
    for n in 0..1000 {
        writeln!(writer, "line {n}").unwrap();
    }
    drop(writer);
    output.ended().await;
    assert!(output.truncated());
    assert_eq!(
        output.bytes(),
        (0..1000).map(|n: u32| format!("line {n}\n").len() as u64).sum::<u64>()
    );
    assert!(output.tail().ends_with("line 998\nline 999"), "{}", output.tail());
}

#[test]
fn watch_line_cut_at_4_kib() {
    let mut lines = Lines::default();
    let mut seen: Vec<usize> = Vec::new();
    let long = vec![b'a'; MAX_LINE_BYTES + 900];
    lines.feed(&long, &mut |line| seen.push(line.len()));
    lines.feed(b"\nshort\nlast", &mut |line| seen.push(line.len()));
    lines.finish(&mut |line| seen.push(line.len()));
    assert_eq!(seen, [MAX_LINE_BYTES, 5, 4]);
    // NOTE: a cut never splits a character.
    let mut text = "a".repeat(MAX_LINE_BYTES - 1).into_bytes();
    text.extend("é".as_bytes());
    let cut = line_text(&text);
    assert_eq!(cut.len(), MAX_LINE_BYTES - 1);
}

#[test]
fn watch_pattern_over_200_chars_refused() {
    let refused = Matcher::new(&"a".repeat(201)).unwrap_err();
    assert!(refused.contains("more than 200 characters"), "{refused}");
    assert!(Matcher::new(&"a".repeat(200)).is_ok());
    let invalid = Matcher::new("(unclosed").unwrap_err();
    assert!(invalid.contains("not a valid regular expression"), "{invalid}");
    assert!(Matcher::new("error|panic").unwrap().is_match("a panic here"));
}

#[test]
fn contained_call_before_first_job_cannot_plant_job_output() {
    // NOTE: efrd makes the dir before every contained call, with its marker, so the
    // plan of the call (P2) finds it and keeps it read-only for the call.
    let scratch = tempfile::tempdir().unwrap();
    make_dir(scratch.path()).unwrap();
    let jobs = scratch.path().join("jobs");
    let meta = std::fs::symlink_metadata(&jobs).unwrap();
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    assert!(jobs.join(MARKER_FILE).is_file());
    std::fs::write(jobs.join("j1.out"), "kept").unwrap();
    make_dir(scratch.path()).unwrap();
    assert_eq!(std::fs::read_to_string(jobs.join("j1.out")).unwrap(), "kept", "made once");
    // A scratch that does not exist yet holds nothing to make.
    make_dir(&scratch.path().join("not-yet")).unwrap();
}

#[test]
fn foreign_jobs_dir_moved_aside() {
    let scratch = tempfile::tempdir().unwrap();
    let jobs = scratch.path().join("jobs");
    std::fs::create_dir(&jobs).unwrap();
    std::fs::write(jobs.join("j1.out"), "planted").unwrap();
    make_dir(scratch.path()).unwrap();
    assert!(jobs.join(MARKER_FILE).is_file());
    assert!(!jobs.join("j1.out").exists());
    let aside = scratch.path().join("jobs.moved-1");
    assert_eq!(std::fs::read_to_string(aside.join("j1.out")).unwrap(), "planted");
    // A link named jobs goes aside too, and its target is untouched.
    let other = tempfile::tempdir().unwrap();
    let target = other.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, other.path().join("jobs")).unwrap();
    make_dir(other.path()).unwrap();
    assert!(other.path().join("jobs.moved-1").is_symlink());
    assert!(!target.join(MARKER_FILE).exists());
    assert!(other.path().join("jobs").join(MARKER_FILE).is_file());
}

#[test]
fn last_lines_take_the_end() {
    assert_eq!(last_lines(b"a\nb\nc\n", 2), "b\nc");
    assert_eq!(last_lines(b"a\nb", 5), "a\nb");
    assert_eq!(last_lines(b"", 5), "");
    assert_eq!(last_lines(b"\n", 5), "");
}

#[test]
fn read_shows_an_offset_or_the_last_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("j1.out");
    let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&path, &text).unwrap();
    assert_eq!(read(&path, None, 2, 1024).unwrap(), "line 99\nline 100");
    assert_eq!(read(&path, Some(7), 0, 14).unwrap(), "line 2\nline 3\n");
    assert_eq!(read(&path, Some(100_000), 0, 14).unwrap(), "");
    assert!(read(Path::new("/nonexistent/j9.out"), None, 1, 10).is_err());
}
