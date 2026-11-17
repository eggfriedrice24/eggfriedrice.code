use std::os::fd::OwnedFd;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use pretty_assertions::assert_eq;
use rustix::fs::{Mode, OFlags};

use super::{ensure, path, send};
use crate::notices::{append, append_and_wake};

/// Opens the FIFO at `path` as the plugin does: read and write, so it never sees an
/// end, and without blocking.
fn reader(path: &Path) -> OwnedFd {
    rustix::fs::open(path, OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())
        .unwrap()
}

/// What waits in the FIFO of `fd` now.
fn drained(fd: &OwnedFd) -> String {
    let mut text = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        match rustix::io::read(fd, &mut buf) {
            Ok(0) | Err(rustix::io::Errno::AGAIN) => break,
            Ok(read) => text.extend_from_slice(&buf[..read]),
            Err(errno) => panic!("{errno}"),
        }
    }
    String::from_utf8(text).unwrap()
}

#[test]
fn ensure_makes_a_private_fifo_once_and_keeps_what_is_there() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");

    let fifo = ensure(&dir, "/dev/pts/3").unwrap().unwrap();

    assert_eq!(fifo, dir.join("pts-3.wake"));
    let meta = std::fs::metadata(&fifo).unwrap();
    assert!(meta.file_type().is_fifo());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(ensure(&dir, "/dev/pts/3").unwrap(), Some(fifo.clone()));
    assert_eq!(std::fs::metadata(&fifo).unwrap().ino(), meta.ino(), "an existing FIFO stays");

    std::fs::write(dir.join("pts-4.wake"), "a file").unwrap();
    assert_eq!(ensure(&dir, "/dev/pts/4").unwrap(), Some(dir.join("pts-4.wake")));
    assert_eq!(std::fs::read_to_string(dir.join("pts-4.wake")).unwrap(), "a file");
}

#[test]
fn a_tty_without_a_safe_name_gets_no_fifo() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");

    assert_eq!(ensure(&dir, "..").unwrap(), None);
    assert!(!dir.exists());
}

#[test]
fn a_line_reaches_a_reader_and_is_skipped_without_one() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");
    let fifo = path(&dir, "pts-3");

    assert!(!send(&fifo, Some("app: a question waits")).unwrap(), "no FIFO");
    ensure(&dir, "/dev/pts/3").unwrap();
    assert!(!send(&fifo, Some("app: a question waits")).unwrap(), "no reader");

    let fd = reader(&fifo);
    assert!(send(&fifo, Some("app: a question waits")).unwrap());
    assert!(send(&fifo, None).unwrap());
    assert_eq!(drained(&fd), "app: a question waits\n\n");
}

#[test]
fn a_full_pipe_skips_the_line_and_never_blocks() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");
    let fifo = ensure(&dir, "/dev/pts/3").unwrap().unwrap();
    let fd = reader(&fifo);
    let chunk = [b'x'; 4096];
    while rustix::io::write(&fd, &chunk).is_ok() {}

    assert!(!send(&fifo, Some("app: a question waits")).unwrap());
    assert!(!send(&fifo, None).unwrap());
    assert!(!drained(&fd).contains('\n'), "no line went in");
}

#[test]
fn a_path_that_is_no_fifo_gets_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");
    std::fs::create_dir(&dir).unwrap();
    let file = path(&dir, "pts-3");
    std::fs::write(&file, "kept").unwrap();

    assert!(!send(&file, Some("app: a question waits")).unwrap(), "a regular file");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");

    let fifo = ensure(&dir, "/dev/pts/4").unwrap().unwrap();
    let fd = reader(&fifo);
    let link = path(&dir, "pts-5");
    std::os::unix::fs::symlink(&fifo, &link).unwrap();
    assert!(!send(&link, Some("app: a question waits")).unwrap(), "a symbolic link");
    assert_eq!(drained(&fd), "");
}

#[test]
fn a_notice_line_wakes_the_prompt_after_it_is_written() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");
    let fifo = ensure(&dir, "/dev/pts/3").unwrap().unwrap();
    let fd = reader(&fifo);

    append(&dir, "/dev/pts/3", "efr: job j3 ended after 42s (exit 0)").unwrap();
    append_and_wake(&dir, "/dev/pts/3", "efr: a question waits", Some("app: a question waits"))
        .unwrap();

    assert_eq!(drained(&fd), "\napp: a question waits\n");
    assert_eq!(
        std::fs::read_to_string(dir.join("pts-3")).unwrap(),
        "efr: job j3 ended after 42s (exit 0)\nefr: a question waits\n"
    );
    // A terminal whose plugin reads no FIFO still gets its line.
    append_and_wake(&dir, "/dev/pts/4", "efr: other", Some("app: a question waits")).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("pts-4")).unwrap(), "efr: other\n");
}
