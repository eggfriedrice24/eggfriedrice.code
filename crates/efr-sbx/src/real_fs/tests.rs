use std::os::unix::fs::symlink;

use super::*;

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("efr-sbx-fs").tempdir().unwrap()
}

#[test]
fn lstat_tells_links_from_their_targets() {
    let dir = temp();
    let file = dir.path().join("file");
    fs::write(&file, b"x").unwrap();
    symlink(&file, dir.path().join("link")).unwrap();
    assert_eq!(RealFs.lstat(&file).unwrap(), FileKind::File);
    assert_eq!(RealFs.lstat(&dir.path().join("link")).unwrap(), FileKind::Symlink);
    assert_eq!(RealFs.lstat(dir.path()).unwrap(), FileKind::Dir);
    assert_eq!(
        RealFs.lstat(&dir.path().join("missing")).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn open_no_symlinks_refuses_a_link_on_the_way() {
    let dir = temp();
    fs::create_dir(dir.path().join("real")).unwrap();
    fs::write(dir.path().join("real/file"), b"x").unwrap();
    symlink(dir.path().join("real"), dir.path().join("via")).unwrap();
    assert!(RealFs.open_no_symlinks(&dir.path().join("real/file")).is_ok());
    let error = RealFs.open_no_symlinks(&dir.path().join("via/file")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(rustix::io::Errno::LOOP.raw_os_error()));
}

#[test]
fn read_file_keeps_its_limit_and_refuses_a_fifo() {
    let dir = temp();
    let file = dir.path().join("file");
    fs::write(&file, b"12345").unwrap();
    assert_eq!(RealFs.read_file(&file, 5).unwrap(), b"12345");
    assert_eq!(RealFs.read_file(&file, 4).unwrap_err().kind(), io::ErrorKind::FileTooLarge);
    let fifo = dir.path().join("fifo");
    rustix::fs::mknodat(CWD, &fifo, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0).unwrap();
    assert!(RealFs.read_file(&fifo, 10).is_err());
}

#[test]
fn read_dir_lists_names_without_dots() {
    let dir = temp();
    fs::write(dir.path().join("a"), b"").unwrap();
    fs::create_dir(dir.path().join("b")).unwrap();
    let mut names = RealFs.read_dir(dir.path()).unwrap();
    names.sort();
    assert_eq!(names, vec![OsString::from("a"), OsString::from("b")]);
}

#[test]
fn changed_since_sees_a_new_file() {
    let dir = temp();
    let file = dir.path().join("file");
    fs::write(&file, b"x").unwrap();
    assert!(RealFs::changed_since(&file, (0, 0)));
    assert!(!RealFs::changed_since(&file, (i64::MAX, 0)));
    assert!(RealFs::changed_since(&dir.path().join("missing"), (i64::MAX, 0)));
}

#[test]
fn a_listing_gives_each_entry_its_kind_like_lstat() {
    let dir = temp();
    fs::write(dir.path().join("file"), b"x").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    symlink(dir.path().join("sub"), dir.path().join("link")).unwrap();
    let mut entries = RealFs.read_dir_kinds(dir.path()).unwrap();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, kind) in &entries {
        // A file system without d_type gives no kind; then lstat decides.
        if let Some(kind) = kind {
            assert_eq!(*kind, RealFs.lstat(&dir.path().join(name)).unwrap(), "{name:?}");
        }
    }
    let names: Vec<&OsString> = entries.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["file", "link", "sub"]);
}
