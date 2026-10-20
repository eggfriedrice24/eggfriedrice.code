use efr_config::DiffGroups;
use efr_protocol::{ChangeKind, FileChange};
use pretty_assertions::assert_eq;

use super::{GroupCount, breakdown, group_of, only_code};

fn file(path: &str, added: u32, removed: u32) -> FileChange {
    FileChange {
        path: path.to_owned(),
        kind: ChangeKind::Modified,
        from: None,
        added,
        removed,
        binary: false,
    }
}

fn evals() -> DiffGroups {
    DiffGroups::new(vec![
        ("evals".to_owned(), vec!["evals/".to_owned(), "*.eval.toml".to_owned()]),
        ("code".to_owned(), vec!["tools/fixtures/".to_owned()]),
    ])
}

#[test]
fn the_built_in_groups_follow_the_path_and_the_first_match_wins() {
    let table = [
        // generated
        ("Cargo.lock", "generated"),
        ("web/package-lock.json", "generated"),
        ("yarn.lock", "generated"),
        ("pnpm-lock.yaml", "generated"),
        ("poetry.lock", "generated"),
        ("go.sum", "generated"),
        ("src/snapshots/a__b.snap", "generated"),
        ("tests/fixtures/case.json", "generated"),
        ("crates/x/tests/it/snapshots/y.snap", "generated"),
        ("fixtures/a.rs", "generated"),
        // tests
        ("tests/it/main.rs", "tests"),
        ("crates/x/test/a.py", "tests"),
        ("web/__tests__/a.tsx", "tests"),
        ("spec/models/user_spec.rb", "tests"),
        ("src/format/tests.rs", "tests"),
        ("pkg/server_test.go", "tests"),
        ("src/a.test.ts", "tests"),
        ("lib/a_spec.rb", "tests"),
        ("src/a.spec.ts", "tests"),
        ("tests/README.md", "tests"),
        // docs
        ("README.md", "docs"),
        ("crates/x/README.md", "docs"),
        ("docs/config.schema.json", "docs"),
        ("CHANGELOG.md", "docs"),
        // code
        ("src/main.rs", "code"),
        ("src/testing.rs", "code"),
        ("src/latest.rs", "code"),
        ("_test.go", "code"),
        ("Cargo.toml", "code"),
        ("src/lock.rs", "code"),
        ("$SCRATCH/out.txt", "code"),
        ("~/.zshrc", "code"),
        ("/etc/hosts", "code"),
        ("contest/a.rs", "code"),
        ("a.mdx", "code"),
    ];
    let none = DiffGroups::default();
    for (path, group) in table {
        assert_eq!(group_of(path, &none), group, "{path}");
    }
}

#[test]
fn the_groups_of_the_file_match_before_the_built_in_ones() {
    let groups = evals();

    assert_eq!(group_of("evals/a/README.md", &groups), "evals");
    assert_eq!(group_of("tests/smoke.eval.toml", &groups), "evals");
    assert_eq!(group_of("tools/fixtures/a.json", &groups), "code");
    assert_eq!(group_of("tests/fixtures/a.json", &groups), "generated");
    assert_eq!(group_of("tests/a.rs", &groups), "tests");
}

#[test]
fn the_breakdown_counts_each_group_and_puts_the_largest_first() {
    let files = [
        file("src/a.rs", 10, 2),
        file("tests/a.rs", 70, 7),
        file("evals/a.toml", 500, 3),
        file("README.md", 1, 0),
        file("src/b.rs", 5, 0),
        file("docs/b.md", 0, 1),
    ];

    let counts = breakdown(&files, &evals());

    let count = |name: &str, files, added, removed| GroupCount {
        name: name.to_owned(),
        files,
        added,
        removed,
    };
    assert_eq!(
        counts,
        [
            count("evals", 1, 500, 3),
            count("tests", 1, 70, 7),
            count("code", 2, 15, 2),
            count("docs", 2, 1, 1),
        ]
    );
    assert!(!only_code(&counts));
    assert!(only_code(&breakdown(&[file("src/a.rs", 1, 0)], &evals())));
    assert!(only_code(&[]));
}
