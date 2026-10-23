use std::fs;
use std::os::unix::fs::symlink;

use pretty_assertions::assert_eq;

use super::{
    LeftOut, LeftOutReason, MAX_IGNORED_BYTES, Unreadable, by_size, keep_others, parse_listing,
    parse_split, small_ignored, unreadable,
};

mod listing;

fn left(path: &str, reason: LeftOutReason) -> LeftOut {
    LeftOut { path: path.to_owned(), reason }
}

#[test]
fn unreadable_file_errors_are_counted_with_the_first_path() {
    let stderr = b"warning: could not open directory 'work/': Permission denied\n\
        error: open(\"research/a/secret\"): Permission denied\n\
        error: unable to index file 'research/a/secret'\n\
        error: open(\"b \\\"q\\\").txt\"): Permission denied\n\
        error: open(\"c.txt\"): Permission denied\n\
        error: open(\"cut-off";
    assert_eq!(
        unreadable(stderr),
        Unreadable {
            count: 3,
            first: Some("research/a/secret".to_owned()),
            paths: ["research/a/secret", "b \\\"q\\\").txt", "c.txt"].map(str::to_owned).into(),
        }
    );
    assert_eq!(unreadable(b"fatal: adding files failed\n"), Unreadable::default());
    assert_eq!(unreadable(b""), Unreadable::default());
}

#[test]
fn a_listing_splits_tracked_changes_from_new_files() {
    let out = b"C src/a.rs\0R gone.txt\0C gone.txt\0? new.txt\0? nested/\0";
    let listing = parse_listing(out);
    assert_eq!(
        listing.changed.into_iter().collect::<Vec<_>>(),
        ["gone.txt".to_owned(), "src/a.rs".to_owned()]
    );
    assert_eq!(listing.others, ["new.txt", "nested/"]);
}

#[test]
fn a_split_listing_reads_the_changes_and_the_new_files_of_two_runs() {
    let listing = parse_split(b"src/a.rs\0gone.txt\0", b"new.txt\0nested/\0bad\xff\0");
    assert_eq!(
        listing.changed.into_iter().collect::<Vec<_>>(),
        ["gone.txt".to_owned(), "src/a.rs".to_owned()]
    );
    assert_eq!(listing.others, ["new.txt", "nested/"]);
    assert_eq!(listing.not_utf8, ["bad\u{fffd}"]);
    assert_eq!(parse_split(b"", b""), parse_listing(b""));
}

#[test]
fn new_files_above_the_limit_and_nested_repositories_are_left_out() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("small.txt"), "ok\n").unwrap();
    fs::write(root.path().join("large.bin"), vec![0_u8; 2048]).unwrap();
    symlink("/etc/passwd", root.path().join("link")).unwrap();
    let others = vec![
        "small.txt".to_owned(),
        "large.bin".to_owned(),
        "link".to_owned(),
        "nested/".to_owned(),
        "vanished.txt".to_owned(),
    ];
    let (kept, record) = keep_others(root.path(), others, 1024);
    assert_eq!(kept, ["small.txt", "link", "vanished.txt"]);
    assert_eq!(
        record.left_out,
        [left("large.bin", LeftOutReason::TooLarge), left("nested", LeftOutReason::NestedRepo)]
    );
}

#[test]
fn small_ignored_files_outside_build_dirs_are_taken() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    fs::write(dir.join(".env"), "KEY=1\n").unwrap();
    fs::write(dir.join("big.log"), vec![b'x'; MAX_IGNORED_BYTES as usize + 1]).unwrap();
    fs::create_dir_all(dir.join("target/debug")).unwrap();
    fs::write(dir.join("target/debug/out"), "x").unwrap();
    fs::create_dir_all(dir.join("secret/node_modules")).unwrap();
    fs::write(dir.join("secret/key"), "k").unwrap();
    fs::write(dir.join("secret/node_modules/pkg.js"), "x").unwrap();
    fs::create_dir_all(dir.join("secret/repo/.git")).unwrap();
    fs::write(dir.join("secret/repo/file"), "x").unwrap();
    let entries: Vec<String> = [".env", "big.log", "target/", "secret/"].map(str::to_owned).into();
    let (kept, record) = small_ignored(dir, &entries);
    assert_eq!(kept, [".env", "secret/key"]);
    assert_eq!(
        record.left_out,
        [
            left("big.log", LeftOutReason::IgnoredTooLarge),
            left("target", LeftOutReason::SkippedDir),
            left("secret/node_modules", LeftOutReason::SkippedDir),
            left("secret/repo", LeftOutReason::NestedRepo),
        ]
    );
}

#[test]
fn changed_files_sort_by_their_size_now() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    fs::write(dir.join("small"), "x").unwrap();
    fs::write(dir.join("mid"), vec![0_u8; MAX_IGNORED_BYTES as usize + 1]).unwrap();
    fs::write(dir.join("large"), vec![0_u8; 3 * 1024 * 1024]).unwrap();
    symlink("/etc/passwd", dir.join("link")).unwrap();
    let changed = ["small", "mid", "large", "link", "gone"].map(str::to_owned).to_vec();
    let sized = by_size(dir, changed, 2 * 1024 * 1024);
    assert_eq!(sized.small, ["small", "link", "gone"]);
    assert_eq!(sized.mid, ["mid"]);
    assert_eq!(sized.large, ["large"]);
}

#[test]
fn ignored_files_that_git_lists_in_a_nested_repository_are_left_out() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    fs::write(dir.join("notes.md"), "n").unwrap();
    fs::create_dir_all(dir.join("vendor/lib/.git")).unwrap();
    fs::create_dir_all(dir.join("vendor/lib/src")).unwrap();
    fs::write(dir.join("vendor/lib/src/a.txt"), "a").unwrap();
    fs::create_dir_all(dir.join("vendor/wt/sub")).unwrap();
    fs::write(dir.join("vendor/wt/.git"), "gitdir: /elsewhere\n").unwrap();
    fs::write(dir.join("vendor/wt/sub/f"), "f").unwrap();
    fs::create_dir_all(dir.join("vendor/repo/.git")).unwrap();
    fs::write(dir.join("vendor/repo/x"), "x").unwrap();
    fs::write(dir.join("vendor/key"), "k").unwrap();
    let entries: Vec<String> =
        ["notes.md", "vendor/lib/src/a.txt", "vendor/wt/sub/f", "vendor/repo/", "vendor/key"]
            .map(str::to_owned)
            .into();
    assert_eq!(small_ignored(dir, &entries).0, ["notes.md", "vendor/key"]);
}
