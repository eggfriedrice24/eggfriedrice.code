//! The shared tables: what they hold, and that the engine and the sandbox read the same
//! paths.

use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{
    PERSISTENCE_FLOORS, PROTECTED_NAMES, SANDBOX_MASKS, ZSH_STARTUP_FILES, is_env_file,
    persistence_floors, protected_names, sandbox_masks,
};
use crate::{Locations, PathClass, secret_paths};

#[test]
fn the_secret_table_is_the_one_the_classes_use() {
    let home = Path::new("/home/u");
    let secrets = secret_paths(home);
    assert!(secrets.contains(&PathBuf::from("/home/u/.ssh")));
    assert!(secrets.contains(&PathBuf::from("/home/u/.aws/credentials")));
    assert!(secrets.contains(&PathBuf::from("/etc/shadow")));
    let locations = Locations::new(home).unwrap();
    for secret in &secrets {
        assert_eq!(
            locations.classify(secret, Path::new("/x")),
            Some(PathClass::Secrets),
            "{secret:?}"
        );
    }
    // The config's and the daemon's secret roots come on top.
    let locations = locations.with_secret_root("/srv/vault").unwrap();
    assert_eq!(locations.secret_paths().len(), secrets.len() + 1);
    assert!(locations.secret_paths().contains(&PathBuf::from("/srv/vault")));
}

#[test]
fn the_sandbox_masks_hold_the_groups_of_the_spec_and_no_engine_secret() {
    let masks = sandbox_masks(Path::new("/home/u"));
    assert_eq!(masks.len(), SANDBOX_MASKS.len());
    for mask in [".aws", ".mozilla", ".zsh_history", ".bash_history", ".config/Signal"] {
        assert!(SANDBOX_MASKS.contains(&mask), "{mask}");
    }
    let locations = Locations::new("/home/u").unwrap();
    // A sandbox mask is no engine secret: reading it stays free outside auto.
    for mask in &masks {
        if mask.ends_with(".aws") {
            continue;
        }
        assert_ne!(locations.classify(mask, Path::new("/x")), Some(PathClass::Secrets), "{mask:?}");
    }
}

#[test]
fn the_protected_names_mark_directories_with_a_slash() {
    assert_eq!(protected_names(), PROTECTED_NAMES);
    for name in [".mcp.json", ".claude/", ".codex/", ".vscode/", ".envrc", ".efr/"] {
        assert!(PROTECTED_NAMES.contains(&name), "{name}");
    }
    assert!(PROTECTED_NAMES.iter().all(|name| !name.starts_with('/') && !name.contains("..")));
}

#[test]
fn the_persistence_floors_cover_startup_files_services_and_tool_config() {
    let floors = persistence_floors(Path::new("/home/u"));
    assert_eq!(floors.len(), PERSISTENCE_FLOORS.len());
    for floor in
        [".zshrc", ".bashrc", ".config/systemd", ".config/autostart", ".gitconfig", ".local/bin"]
    {
        assert!(floors.contains(&Path::new("/home/u").join(floor)), "{floor}");
    }
}

#[test]
fn every_zsh_startup_file_is_a_floor_in_the_home_directory_too() {
    for name in ZSH_STARTUP_FILES {
        assert!(PERSISTENCE_FLOORS.contains(name), "{name}");
    }
}

#[test]
fn env_files_are_masked_but_not_their_examples() {
    for name in [".env", ".env.local", ".env.production"] {
        assert!(is_env_file(name), "{name}");
    }
    for name in [".env.example", ".env.sample", ".env.template", ".envrc", "env", "x.env"] {
        assert!(!is_env_file(name), "{name}");
    }
}
