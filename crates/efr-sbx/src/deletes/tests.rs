use std::ffi::OsString;
use std::path::Path;

use efr_sandbox::{LayerEntry, LayerView};
use pretty_assertions::assert_eq;
use rustix::fs::{CWD, FileType, Mode, OFlags};

use crate::deletes::UpperView;

fn view(root: &Path) -> UpperView {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    UpperView::new(rustix::fs::open(root, flags, Mode::empty()).unwrap())
}

fn sorted(mut entries: Vec<(OsString, LayerEntry)>) -> Vec<(OsString, LayerEntry)> {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

#[test]
fn an_upper_dir_lists_dirs_whiteouts_opaque_dirs_and_the_rest() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("dir/inner")).unwrap();
    std::fs::write(root.join("file"), "x").unwrap();
    std::os::unix::fs::symlink("file", root.join("link")).unwrap();
    // NOTE: a whiteout (a character device 0/0) and the overlay's user xattr need no
    // privilege on the kernels that efr supports; where the file system refuses one,
    // the entry is left out of the check.
    let whiteout = rustix::fs::mknodat(
        CWD,
        root.join("gone"),
        FileType::CharacterDevice,
        Mode::from_raw_mode(0o600),
        0,
    )
    .is_ok();
    std::fs::create_dir(root.join("remade")).unwrap();
    let opaque = rustix::fs::setxattr(
        root.join("remade").as_path(),
        "user.overlay.opaque",
        b"y",
        rustix::fs::XattrFlags::empty(),
    )
    .is_ok();
    let upper = view(root);

    let entries = sorted(upper.entries(Path::new("")).unwrap());

    let mut expected = vec![
        (OsString::from("dir"), LayerEntry::Dir { opaque: false }),
        (OsString::from("file"), LayerEntry::Other),
        (OsString::from("link"), LayerEntry::Other),
        (OsString::from("remade"), LayerEntry::Dir { opaque }),
    ];
    if whiteout {
        expected.insert(2, (OsString::from("gone"), LayerEntry::Whiteout));
    }
    assert_eq!(entries, expected);
    assert_eq!(
        upper.entries(Path::new("dir")).unwrap(),
        [(OsString::from("inner"), LayerEntry::Dir { opaque: false })]
    );
}

#[test]
fn an_upper_dir_never_follows_a_link_or_leaves_its_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(outside.path().join("secret")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("away")).unwrap();
    let upper = view(root.path());

    assert!(upper.entries(Path::new("away")).is_err());
    assert!(upper.entries(Path::new("../")).is_err());
}
