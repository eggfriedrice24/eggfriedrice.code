use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use efr_scope::{Git, Home};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use tokio::io::AsyncWriteExt as _;

use super::{GitDirs, ProjectGit, Refusal};
use crate::runner::Runner;

/// Runs plain git in `cwd` with no user config, `stdin` as its input.
async fn git_with(cwd: &Path, home: &Path, args: &[&str], stdin: &[u8]) -> Vec<u8> {
    let mut command = efr_stdx::process::command("git", cwd);
    command
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin).await.unwrap();
    drop(pipe);
    let output = child.wait_with_output().await.unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}

async fn git(cwd: &Path, home: &Path, args: &[&str]) {
    git_with(cwd, home, args, b"").await;
}

struct Project {
    _base: tempfile::TempDir,
    home: PathBuf,
    dir: PathBuf,
    markers: PathBuf,
    runner: Runner,
}

/// A project with one commit, a stash entry and a config whose every code key writes
/// a marker.
async fn project() -> Project {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    let dir = base.path().join("project");
    let markers = base.path().join("markers");
    let hooks = base.path().join("hooks");
    for made in [&home, &dir, &markers, &hooks] {
        fs::create_dir_all(made).unwrap();
    }
    git(&dir, &home, &["init", "-q"]).await;
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &home, &["add", "a.txt"]).await;
    git(&dir, &home, &["commit", "-q", "-m", "one"]).await;
    fs::write(dir.join("a.txt"), "b\n").unwrap();
    git(&dir, &home, &["stash", "-q"]).await;
    for hook in ["post-index-change", "reference-transaction", "pre-auto-gc"] {
        let path = hooks.join(hook);
        fs::write(&path, format!("#!/bin/sh\ntouch '{}/hook-{hook}'\n", markers.display()))
            .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mark = |name: &str| format!("touch '{}/{name}'; true", markers.display());
    let settings = [
        ("core.fsmonitor", mark("fsmonitor")),
        ("filter.x.clean", format!("{}; cat", mark("clean"))),
        ("core.hooksPath", hooks.display().to_string()),
        ("core.alternateRefsCommand", mark("alternate-refs")),
        ("uploadpack.packObjectsHook", mark("pack-objects-hook")),
    ];
    for (key, value) in &settings {
        git(&dir, &home, &["config", key, value]).await;
    }
    fs::write(dir.join(".gitattributes"), "* filter=x\n").unwrap();
    let clock = TestClock::starting_at("2026-10-11T10:00:00Z".parse().unwrap());
    let runner = Runner::new(
        Git::new(clock.shared()).isolated(),
        Home::new(&home).unwrap(),
        clock.shared(),
        std::time::Duration::from_secs(30),
        None,
    );
    Project { _base: base, home, dir, markers, runner }
}

fn markers(project: &Project) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(&project.markers)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

fn dirs(project: &Project) -> GitDirs {
    let git_dir = project.dir.join(".git");
    GitDirs { git_dir: git_dir.clone(), common_dir: git_dir }
}

#[tokio::test]
async fn project_git_commands_never_run_repo_config() {
    let project = project().await;
    let opened = ProjectGit::open(&project.runner, &dirs(&project), None).unwrap();
    let refs = opened.refs().await.unwrap();
    assert_eq!(refs.head_ref.as_deref(), Some("refs/heads/main"));
    let head = refs.refs["refs/heads/main"].clone();
    assert!(refs.refs.contains_key("refs/stash"));
    assert!(!opened.staged().await.unwrap());
    assert_eq!(opened.stash_entries().await.unwrap(), 1);
    assert!(opened.has_object(&head).await.unwrap());
    assert!(opened.update_ref("refs/heads/copy", Some(&head), None).await.unwrap());
    assert!(!opened.update_ref("refs/heads/copy", Some(&head), None).await.unwrap(), "it exists");
    assert!(opened.update_ref("refs/heads/copy", None, Some(&head)).await.unwrap());
    let args = ["-c", "core.hooksPath=/dev/null", "pack-objects", "--revs", "--stdout", "-q"];
    let pack = git_with(&project.dir, &project.home, &args, head.as_bytes()).await;
    opened.index_pack(pack).await.unwrap();
    assert_eq!(
        markers(&project),
        Vec::<String>::new(),
        "no hook, fsmonitor, filter or command ran"
    );

    // NOTE: the keys act for plain git, so the test proves something.
    git(&project.dir, &project.home, &["status", "--porcelain"]).await;
    git(&project.dir, &project.home, &["add", "-A"]).await;
    assert_eq!(markers(&project), ["clean", "fsmonitor", "hook-post-index-change"]);
}

#[tokio::test]
async fn staged_content_is_seen() {
    let project = project().await;
    fs::write(project.dir.join("a.txt"), "staged\n").unwrap();
    let quiet = ["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"];
    let args = [&quiet[..], &["-c", "filter.x.clean=cat", "add", "a.txt"]].concat();
    git(&project.dir, &project.home, &args).await;
    let opened = ProjectGit::open(&project.runner, &dirs(&project), None).unwrap();
    assert!(opened.staged().await.unwrap());
}

#[tokio::test]
async fn alternates_a_moved_dir_and_a_stray_commondir_are_refused() {
    let project = project().await;
    let dirs = dirs(&project);
    let opened = ProjectGit::open(&project.runner, &dirs, None).unwrap();
    let pins = opened.pins();

    let commondir = dirs.git_dir.join("commondir");
    fs::write(&commondir, "../elsewhere\n").unwrap();
    assert_eq!(ProjectGit::open(&project.runner, &dirs, None).err(), Some(Refusal::CommonDir));
    fs::remove_file(&commondir).unwrap();

    let alternates = dirs.common_dir.join("objects/info/alternates");
    fs::create_dir_all(alternates.parent().unwrap()).unwrap();
    fs::write(&alternates, "/elsewhere/objects\n").unwrap();
    assert_eq!(ProjectGit::open(&project.runner, &dirs, None).err(), Some(Refusal::Alternates));
    fs::remove_file(&alternates).unwrap();

    let moved = project.dir.join("moved.git");
    fs::rename(&dirs.git_dir, &moved).unwrap();
    fs::create_dir(&dirs.git_dir).unwrap();
    assert_eq!(ProjectGit::open(&project.runner, &dirs, Some(pins)).err(), Some(Refusal::Changed));
    fs::remove_dir(&dirs.git_dir).unwrap();
    std::os::unix::fs::symlink(&moved, &dirs.git_dir).unwrap();
    assert_eq!(ProjectGit::open(&project.runner, &dirs, None).err(), Some(Refusal::NotADir));
}

#[test]
fn the_upload_pack_string_carries_the_hardening() {
    let command = ProjectGit::upload_pack();
    assert!(
        command.starts_with("git -c core.hooksPath=/dev/null -c core.fsmonitor=false"),
        "{command}"
    );
    assert!(command.contains("-c core.alternateRefsCommand= "), "{command}");
    assert!(command.ends_with(" upload-pack"), "{command}");
}
