use super::{check_glob, check_name, matches};

#[test]
fn a_glob_matches_as_its_table_says() {
    let table: &[(&str, &str, bool)] = &[
        // No slash: the name of a file in any directory.
        ("*.snap", "src/snapshots/a.snap", true),
        ("*.snap", "a.snap", true),
        ("*.snap", "a.snap.new", false),
        ("*.eval.toml", "evals/x/y.eval.toml", true),
        ("Cargo.lock", "Cargo.lock", true),
        ("Cargo.lock", "crates/a/Cargo.lock", true),
        ("Cargo.lock", "Cargo.lock.bak", false),
        ("?.rs", "src/a.rs", true),
        ("?.rs", "src/ab.rs", false),
        // A last slash: every file below a directory of that name.
        ("evals/", "evals/a.toml", true),
        ("evals/", "crates/x/evals/deep/a.toml", true),
        ("evals/", "evals", false),
        ("evals/", "my-evals/a.toml", false),
        ("eval*/", "evaluations/a.toml", true),
        ("crates/evals/", "crates/evals/a/b.rs", true),
        ("crates/evals/", "x/crates/evals/a.rs", false),
        // Other globs: the whole path from its start.
        ("evals/*.toml", "evals/a.toml", true),
        ("evals/*.toml", "evals/x/a.toml", false),
        ("evals/**/*.toml", "evals/a.toml", true),
        ("evals/**/*.toml", "evals/x/y/a.toml", true),
        ("evals/**", "evals/x/y/a.toml", true),
        ("**/fixtures/**", "a/fixtures/b.json", true),
        ("**/fixtures/**", "fixtures/b.json", true),
        ("docs/*.md", "crates/x/docs/a.md", false),
        ("$SCRATCH/*", "$SCRATCH/out.txt", true),
        ("~/notes/**", "~/notes/a/b.md", true),
        ("/etc/**", "/etc/hosts", true),
        // `*` never crosses a slash.
        ("src/*", "src/a/b.rs", false),
        ("a*b", "axxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxc", false),
        ("*a*a*a*a*a*b", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false),
    ];
    for (glob, path, expected) in table {
        assert_eq!(matches(glob, path), *expected, "{glob} on {path}");
    }
}

#[test]
fn the_load_refuses_a_glob_that_cannot_match_as_meant() {
    for good in ["evals/", "*.eval.toml", "evals/**/*.toml", "**/fixtures/", "/etc/**", "a?c"] {
        assert_eq!(check_glob(good), None, "{good}");
    }
    for bad in ["", "/", "a//b", "../x", "a/./b", "a**", "**b/c", "x\ny", "//"] {
        assert!(check_glob(bad).is_some(), "{bad:?}");
    }
}

#[test]
fn a_group_name_is_short_and_plain() {
    for good in ["evals", "e2e", "golden_files", "web-ui", "A1"] {
        assert_eq!(check_name(good), None, "{good}");
    }
    for bad in ["", "has space", "a.b", "a/b", "über", "x".repeat(25).as_str()] {
        assert!(check_name(bad).is_some(), "{bad:?}");
    }
}
