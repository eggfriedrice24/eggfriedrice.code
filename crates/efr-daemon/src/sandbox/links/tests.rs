//! The dotfile link scan: link targets in a write root become floors.

use super::{dotfile_links, link_targets};

#[test]
fn links_of_home_config_and_local_bin_into_a_root_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let dotfiles = home.join("dotfiles");
    for dir in ["zsh", "nvim", "bin", ".config", ".local/bin"] {
        let path = if dir.starts_with('.') { home.join(dir) } else { dotfiles.join(dir) };
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(dotfiles.join("zsh/.zshrc"), "").unwrap();
    std::fs::write(dotfiles.join("bin/tool"), "").unwrap();
    std::fs::write(home.join("elsewhere"), "").unwrap();
    let link = |target: &std::path::Path, at: &str| {
        std::os::unix::fs::symlink(target, home.join(at)).unwrap();
    };
    link(&dotfiles.join("zsh/.zshrc"), ".zshrc");
    link(&dotfiles.join("nvim"), ".config/nvim");
    link(&dotfiles.join("bin/tool"), ".local/bin/tool");
    link(&home.join("elsewhere"), ".other");
    // A name without a dot in the home dir is not scanned.
    link(&dotfiles.join("zsh/.zshrc"), "plain");
    let mut found = link_targets(&home, std::slice::from_ref(&dotfiles));
    found.sort();
    let mut expected =
        vec![dotfiles.join("zsh/.zshrc"), dotfiles.join("nvim"), dotfiles.join("bin/tool")];
    expected.sort();
    assert_eq!(found, expected);
    assert!(link_targets(&home, &[]).is_empty());
}

#[test]
fn each_dotfile_link_comes_with_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let dotfiles = home.join("dotfiles");
    std::fs::create_dir_all(dotfiles.join("hypr")).unwrap();
    std::fs::create_dir_all(home.join(".config")).unwrap();
    std::fs::write(dotfiles.join("zshrc"), "").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("hypr"), home.join(".config/hypr")).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("zshrc"), home.join(".zshrc")).unwrap();
    // Two links to one target: each link names it.
    std::os::unix::fs::symlink(dotfiles.join("zshrc"), home.join(".zshrc.local")).unwrap();
    let mut found = dotfile_links(&home, std::slice::from_ref(&dotfiles));
    found.sort();
    let mut expected = vec![
        (home.join(".config/hypr"), dotfiles.join("hypr")),
        (home.join(".zshrc"), dotfiles.join("zshrc")),
        (home.join(".zshrc.local"), dotfiles.join("zshrc")),
    ];
    expected.sort();
    assert_eq!(found, expected);
    assert!(dotfile_links(&home, &[]).is_empty());
}
