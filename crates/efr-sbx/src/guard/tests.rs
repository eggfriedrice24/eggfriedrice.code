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
fn git_lists_a_content_from_stdin_as_it_lists_the_file() {
    let Some(lister) = Some(GitLister::find("/usr/bin:/bin", &[])).filter(|l| l.git.is_some())
    else {
        return;
    };
    let temp = temp_dir();
    let config = temp.path().join("config");
    // Larger than a pipe: git prints while it reads, and nothing may block.
    let mut content = String::from("[core]\n\thooksPath = ./h\n[include]\n\tpath = other\n");
    for n in 0..4000 {
        content.push_str(&format!("[branch \"b{n}\"]\n\tremote = origin\n"));
    }
    fs::write(&config, &content).unwrap();
    fs::write(temp.path().join("other"), "[core]\n\tpager = evil\n").unwrap();
    let listing = lister.list_content(content.as_bytes()).unwrap();
    assert_eq!(listing, lister.list(&config).unwrap());
    assert_eq!(efr_sandbox::code_keys(&listing), vec!["core.hookspath", "include.path"]);
    assert!(lister.list_content(b"[[broken").is_err());
    // The identity names this git; there is none without a git.
    assert!(lister.identity().starts_with(&lister.git.as_ref().unwrap().display().to_string()));
    assert_eq!(GitLister { git: None }.identity(), "");
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

#[test]
fn a_copy_across_file_systems_streams_within_its_budget_and_still_removes_the_entry() {
    let temp = temp_dir();
    let hooks = temp.path().join("repo/.git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("a-small"), b"1234").unwrap();
    fs::write(hooks.join("b-large"), vec![b'x'; 64 * 1024]).unwrap();
    std::os::unix::fs::symlink("/nonexistent", hooks.join("c-link")).unwrap();
    let quarantined = temp.path().join("q");
    fs::create_dir(&quarantined).unwrap();
    let to = quarantined.join("0-hooks");
    let mut budget = 1000;
    let mut truncated = Vec::new();
    move_by_copy(&hooks, &to, &mut budget, &mut truncated).unwrap();
    // Every part left the sandbox's reach, the large file cut to the budget.
    assert!(!hooks.exists());
    assert_eq!(fs::read(to.join("a-small")).unwrap(), b"1234");
    assert_eq!(fs::read(to.join("b-large")).unwrap().len(), 996);
    assert_eq!(fs::read_link(to.join("c-link")).unwrap(), Path::new("/nonexistent"));
    assert_eq!(fs::metadata(to.join("b-large")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(truncated, [hooks.join("b-large")]);
    assert_eq!(budget, 0);
    // With the budget spent, the next file keeps nothing, and still moves.
    let config = temp.path().join("repo/.git/config");
    fs::write(&config, b"[core]\n").unwrap();
    let mut truncated = Vec::new();
    move_by_copy(&config, &quarantined.join("1-config"), &mut budget, &mut truncated).unwrap();
    assert!(!config.exists());
    assert_eq!(fs::read(quarantined.join("1-config")).unwrap(), b"");
    assert_eq!(truncated, [config]);
}

#[test]
fn a_call_from_the_home_dir_scans_only_the_guard_roots() {
    let temp = temp_dir();
    let home = temp.path().canonicalize().unwrap();
    let project = home.join("p/proj");
    let cache = home.join(".cache");
    for dir in [project.join(".git"), home.join("other/.git"), cache.join("tool/.git")] {
        fs::create_dir_all(dir).unwrap();
    }
    for n in 0..200 {
        fs::write(home.join(format!("file{n}")), b"").unwrap();
    }
    let guard = Guard {
        pins: Vec::new(),
        roots: vec![project.clone()],
        chain_roots: vec![project.clone(), cache.clone()],
        protected: Vec::new(),
        lister: GitLister { git: None },
        listings: ConfigListings::default(),
        kept: Vec::new(),
        before: SurfaceManifest::default(),
        started: now(),
    };
    // The home dir is never a write root, so a call that starts there adds nothing to
    // the scan: the cost of the guard does not grow with the home dir.
    assert!(guard.chain(&home).is_empty());
    let found: Vec<PathBuf> = guard.targets(&home).into_iter().map(|t| t.git_dir).collect();
    assert_eq!(found, vec![project.join(".git")]);
    // A start dir in a cache adds its own chain, never a scan of the cache.
    assert_eq!(guard.chain(&cache.join("tool")), vec![cache.join("tool"), cache.clone()]);
}
