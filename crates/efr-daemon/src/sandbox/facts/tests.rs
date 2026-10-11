//! The facts of a shell line: targets, tracked files and programs.

use std::path::{Path, PathBuf};

use efr_permissions::{FactRequest, Source, TargetKind};
use efr_scope::{Git, Home};
use efr_test_support::TestClock;
use jiff::Timestamp;

use super::{FactInput, collect, resolve_program};

async fn git(dir: &Path, args: &[&str]) {
    let status = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test]
async fn targets_parents_tracked_counts_and_programs_are_collected() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let project = home.join("p");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::create_dir_all(project.join("build")).unwrap();
    std::fs::create_dir_all(project.join("target")).unwrap();
    std::fs::write(project.join("src/a.rs"), "").unwrap();
    std::fs::write(project.join("src/b.rs"), "").unwrap();
    std::fs::write(project.join("build/out"), "").unwrap();
    git(&project, &["init", "-q"]).await;
    git(&project, &["add", "src"]).await;
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("tool"), "").unwrap();
    // A file of a builtin's name: where the shell finds the program, its builtin runs.
    std::fs::write(bin.join("printf"), "").unwrap();
    let mut request = FactRequest::default();
    request.targets = vec![project.join("new/dir/file"), project.join("src")];
    request.tracked = vec![project.join("src"), project.join("build"), project.join("target")];
    request.programs = ["tool", "./src/a.rs", "missing", ":", "printf"].map(str::to_owned).to_vec();
    request.builtins = vec![":".to_owned(), "printf".to_owned()];
    let writes = [project.join("out.txt")];
    let rebuildable = vec!["target".to_owned()];
    let input = FactInput {
        request: &request,
        writes: &writes,
        command_dir: Some(&project),
        shell_path: &format!("relative:{}", bin.display()),
        rebuildable: &rebuildable,
        turn_start: Timestamp::UNIX_EPOCH,
        masks: &[],
    };
    let git_runner = Git::new(TestClock::new().shared()).isolated();
    let facts = collect(&input, &git_runner, &Home::new(&home).unwrap()).await;
    assert_eq!(
        facts.targets,
        [
            (project.join("new/dir/file"), None),
            (project.join("new/dir"), None),
            (project.join("new"), None),
            (project.clone(), Some(TargetKind::Dir)),
            (project.join("src"), Some(TargetKind::Dir)),
            (project.join("out.txt"), None),
        ]
    );
    assert_eq!(
        facts.tracked_counts,
        [(project.join("src"), 2), (project.join("build"), 0), (project.join("target"), 0)]
    );
    assert_eq!(
        facts.programs,
        [
            ("tool".to_owned(), Some(bin.join("tool")), true),
            ("./src/a.rs".to_owned(), Some(project.join("src/a.rs")), true),
            ("missing".to_owned(), None, false),
            (":".to_owned(), Some(PathBuf::from("builtin")), false),
            ("printf".to_owned(), Some(PathBuf::from("builtin")), false),
        ]
    );
}

#[test]
fn a_program_resolves_through_absolute_path_entries_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("x"), "").unwrap();
    assert_eq!(resolve_program("x", None, &root.display().to_string()), Some(root.join("x")));
    assert_eq!(resolve_program("x", None, "relative"), None);
    assert_eq!(resolve_program("./x", None, ""), None, "no start dir, no answer");
    assert_eq!(
        resolve_program(&root.join("x").display().to_string(), None, ""),
        Some(root.join("x"))
    );
    let _: PathBuf = root;
}

#[tokio::test]
async fn sources_of_persistence_forms_give_their_text_and_never_a_mask() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let units = home.join(".config/systemd/user");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::write(units.join("agent.service"), "[Install]\nWantedBy=default.target\n").unwrap();
    std::fs::write(home.join("evil"), "x").unwrap();
    std::os::unix::fs::symlink(home.join("evil"), home.join("link")).unwrap();
    std::fs::write(home.join("big"), "x".repeat(9000)).unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id"), "secret").unwrap();
    let mut request = FactRequest::default();
    request.sources = vec![
        Source::Unit("agent.service".to_owned()),
        Source::File(home.join("link")),
        Source::File(home.join("big")),
        Source::File(home.join(".ssh/id")),
        Source::Unit("absent.service".to_owned()),
    ];
    let masks = [home.join(".ssh")];
    let input = FactInput {
        request: &request,
        writes: &[],
        command_dir: None,
        shell_path: "",
        rebuildable: &[],
        turn_start: Timestamp::UNIX_EPOCH,
        masks: &masks,
    };
    let git_runner = Git::new(TestClock::new().shared()).isolated();
    let facts = collect(&input, &git_runner, &Home::new(&home).unwrap()).await;

    let found: Vec<(Option<PathBuf>, bool, Option<&str>)> = facts
        .sources
        .iter()
        .map(|fact| (fact.path.clone(), fact.changed, fact.text.as_deref()))
        .collect();
    let unit_text = "[Install]\nWantedBy=default.target\n";
    assert_eq!(
        found,
        [
            (Some(units.join("agent.service")), true, Some(unit_text)),
            (Some(home.join("evil")), true, Some("x")),
            (Some(home.join("big")), true, None),
            (None, true, None),
            (None, true, None),
        ]
    );
    // A file from before the turn did not change in it.
    let later = Timestamp::now().checked_add(jiff::SignedDuration::from_hours(1)).unwrap();
    let input = FactInput { turn_start: later, ..input };
    let facts = collect(&input, &git_runner, &Home::new(&home).unwrap()).await;
    assert!(!facts.sources[0].changed);
}
