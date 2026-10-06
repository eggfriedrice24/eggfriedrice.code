use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use crate::SandboxError;
use crate::fs_view::{FileKind, resolve};
use crate::testing::FakeFs;

#[test]
fn a_path_without_links_resolves_to_itself() {
    let mut fs = FakeFs::new();
    fs.file("/home/u/.zshrc", "x");
    let resolved = resolve(&fs, Path::new("/home/u/.zshrc")).unwrap();
    assert_eq!(resolved.path, PathBuf::from("/home/u/.zshrc"));
    assert!(resolved.exists);
    assert_eq!(resolved.kind, Some(FileKind::File));
    assert!(resolved.links.is_empty());
}

#[test]
fn relative_and_absolute_links_are_followed_and_recorded() {
    let mut fs = FakeFs::new();
    fs.file("/home/u/dotfiles/zsh/.zshrc", "x")
        .link("/home/u/.zshrc", "dotfiles/zsh/.zshrc")
        .link("/home/u/df", "/home/u/dotfiles")
        .link("/home/u/up", "../u/df/zsh");
    let one = resolve(&fs, Path::new("/home/u/.zshrc")).unwrap();
    assert_eq!(one.path, PathBuf::from("/home/u/dotfiles/zsh/.zshrc"));
    assert_eq!(one.links, [PathBuf::from("/home/u/.zshrc")]);
    let two = resolve(&fs, Path::new("/home/u/up/.zshrc")).unwrap();
    assert_eq!(two.path, PathBuf::from("/home/u/dotfiles/zsh/.zshrc"));
    assert_eq!(two.links, [PathBuf::from("/home/u/up"), PathBuf::from("/home/u/df")]);
}

#[test]
fn a_missing_component_keeps_the_rest_as_written() {
    let mut fs = FakeFs::new();
    fs.dir("/home/u").link("/home/u/cfg", "/home/u/.config");
    let resolved = resolve(&fs, Path::new("/home/u/cfg/systemd/user")).unwrap();
    assert!(!resolved.exists);
    assert_eq!(resolved.path, PathBuf::from("/home/u/.config/systemd/user"));
}

#[test]
fn a_link_loop_is_an_error() {
    let mut fs = FakeFs::new();
    fs.link("/a", "/b").link("/b", "/a");
    assert!(matches!(resolve(&fs, Path::new("/a/x")), Err(SandboxError::LinkLoop { .. })));
}
