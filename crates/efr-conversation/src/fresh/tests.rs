use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{AGENTS_MAX_BYTES, FreshFacts, chain, read_agents};

#[test]
fn the_block_names_the_directories_the_jobs_the_status_and_each_agents_file() {
    let facts = FreshFacts {
        cwd: PathBuf::from("/home/u/p/app/src"),
        shell_cwd: Some(PathBuf::from("/home/u/p/app")),
        jobs: Some(vec!["sleep 600".to_owned()]),
        root: Some(PathBuf::from("/home/u/p/app")),
        git_status: Some("## main...origin/main\n M src/lib.rs\n".to_owned()),
        agents: vec![
            (PathBuf::from("/home/u/p/app/AGENTS.md"), "Run just check.\n".to_owned()),
            (PathBuf::from("/home/u/p/app/src/AGENTS.md"), "No panics here.".to_owned()),
        ],
    };

    insta::assert_snapshot!(facts.render());
}

#[test]
fn a_block_without_a_shell_a_repository_or_files_says_only_that() {
    let facts =
        FreshFacts { cwd: PathBuf::from("/etc"), jobs: Some(Vec::new()), ..FreshFacts::default() };

    assert_eq!(
        facts.render(),
        "<fresh-context>\nefr read this from disk when it compacted the conversation. It is \
         newer than the summary after it.\n\n## Directories\n- The user's directory: /etc\n- \
         No hidden shell runs.\n\n## Running jobs of the hidden shell\nNone.\n</fresh-context>"
    );
}

#[test]
fn the_chain_goes_from_the_root_down_to_the_directory() {
    let root = Path::new("/home/u/p/app");

    assert_eq!(
        chain(Some(root), Path::new("/home/u/p/app/crates/core")),
        [
            PathBuf::from("/home/u/p/app"),
            PathBuf::from("/home/u/p/app/crates"),
            PathBuf::from("/home/u/p/app/crates/core"),
        ]
    );
    assert_eq!(chain(Some(root), root), [root.to_path_buf()]);
    assert_eq!(chain(Some(root), Path::new("/etc")), [PathBuf::from("/etc")]);
    assert_eq!(chain(None, Path::new("/etc")), [PathBuf::from("/etc")]);
}

#[test]
fn agents_files_are_read_where_they_exist_and_a_large_one_is_cut() {
    let dirs = efr_test_support::TestDirs::new().expect("temporary directories");
    let root = dirs.create_dir("home/app").expect("root");
    let inner = dirs.create_dir("home/app/inner").expect("inner directory");
    std::fs::write(root.join("AGENTS.md"), "x".repeat(AGENTS_MAX_BYTES + 10)).expect("write");
    std::fs::create_dir(inner.join("AGENTS.md")).expect("a directory with the name");

    let agents = read_agents(&[root.clone(), inner]);

    assert_eq!(agents.len(), 1, "a directory named AGENTS.md is no file");
    let (path, text) = &agents[0];
    assert_eq!(path, &root.join("AGENTS.md"));
    assert!(text.starts_with(&"x".repeat(AGENTS_MAX_BYTES)));
    assert!(text.ends_with("\n[efr cut this file at 32 KiB]"), "{}", &text[AGENTS_MAX_BYTES..]);
}
