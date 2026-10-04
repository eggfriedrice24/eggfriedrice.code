use std::error::Error as _;
use std::io;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::ToolError;

#[test]
fn a_symlink_error_names_the_real_path() {
    let error = ToolError::ThroughSymlink {
        path: PathBuf::from("/home/u/.zshrc"),
        real: PathBuf::from("/home/u/dotfiles/zshrc"),
    };
    assert_eq!(
        error.to_string(),
        "/home/u/.zshrc goes through a symbolic link; the real path is /home/u/dotfiles/zshrc"
    );
}

#[test]
fn a_journal_error_keeps_its_source() {
    let error = ToolError::journal("/etc/hosts", io::Error::other("disk full"));
    assert_eq!(error.to_string(), "could not record the original of /etc/hosts before writing it");
    assert_eq!(error.source().unwrap().to_string(), "disk full");
}
