use std::os::unix::fs::PermissionsExt;

use crate::testing::temp_dir;

use super::*;

#[test]
fn git_from_a_writable_dir_never_runs() {
    let temp = temp_dir();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("git"), b"#!/bin/sh\n").unwrap();
    fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:relative:/nonexistent", bin.display());
    assert_eq!(GitLister::find(&path, &[temp.path().to_path_buf()]).git, None);
    assert_eq!(GitLister::find(&path, &[]).git, Some(bin.join("git")));
}

#[test]
fn git_lists_a_config_without_its_includes() {
    let Some(lister) = Some(GitLister::find("/usr/bin:/bin", &[])).filter(|l| l.git.is_some())
    else {
        return;
    };
    let temp = temp_dir();
    let config = temp.path().join("config");
    fs::write(&config, "[core]\n\tfsmonitor = ./x\n[include]\n\tpath = other\n").unwrap();
    fs::write(temp.path().join("other"), "[core]\n\tpager = evil\n").unwrap();
    let listing = lister.list(&config).unwrap();
    assert_eq!(efr_sandbox::code_keys(&listing), vec!["core.fsmonitor", "include.path"]);
}

#[test]
fn quarantine_moves_entries_and_writes_the_index() {
    let temp = temp_dir();
    let repo = temp.path().join("repo/.git");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("commondir"), b"/elsewhere\n").unwrap();
    let mut changes = vec![
        SurfaceChange {
            path: repo.join("commondir"),
            rule: "commondir_in_main_git_dir".to_owned(),
            key: None,
            quarantined: true,
        },
        SurfaceChange {
            path: temp.path().join("repo/.envrc"),
            rule: "protected_name_created".to_owned(),
            key: None,
            quarantined: false,
        },
    ];
    let dir = temp.path().join("quarantine/call");
    quarantine(&mut changes, &dir).unwrap();
    assert!(!repo.join("commondir").exists());
    assert_eq!(fs::read(dir.join("0-commondir")).unwrap(), b"/elsewhere\n");
    let index = fs::read_to_string(dir.join(QUARANTINE_INDEX)).unwrap();
    assert!(index.contains("0-commondir"), "{index}");
    assert!(changes[0].quarantined);
    assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn an_entry_that_cannot_move_is_reported_as_not_quarantined() {
    let temp = temp_dir();
    let mut changes = vec![SurfaceChange {
        path: temp.path().join("missing/config"),
        rule: "config_code_key".to_owned(),
        key: Some("core.fsmonitor".to_owned()),
        quarantined: true,
    }];
    quarantine(&mut changes, &temp.path().join("q")).unwrap();
    assert!(!changes[0].quarantined);
}
