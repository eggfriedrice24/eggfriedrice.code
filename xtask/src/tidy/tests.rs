use std::path::Path;

use super::{check_contents, check_crate_readmes, check_path, skipped_dir};

fn rules(path: &str, contents: &str) -> Vec<&'static str> {
    check_contents(path, contents, false).into_iter().map(|v| v.rule).collect()
}

fn fast_rules(path: &str, contents: &str) -> Vec<&'static str> {
    check_contents(path, contents, true).into_iter().map(|v| v.rule).collect()
}

#[test]
fn mod_rs_under_crates_fails() {
    assert_eq!(check_path("crates/efr-store/src/writer/mod.rs").len(), 1);
    assert_eq!(check_path("xtask/src/tidy/mod.rs").len(), 1);
}

#[test]
fn mod_rs_outside_crates_is_not_checked() {
    assert!(check_path("research/mod.rs").is_empty());
}

#[test]
fn sibling_tests_file_fails() {
    assert_eq!(check_path("crates/efr-store/src/writer_tests.rs").len(), 1);
}

#[test]
fn tests_rs_in_module_directory_is_clean() {
    assert!(check_path("crates/efr-store/src/writer/tests.rs").is_empty());
}

#[test]
fn a_second_integration_test_binary_fails() {
    assert_eq!(check_path("crates/efr-daemon/tests/hello.rs").len(), 1);
    assert_eq!(check_path("crates/efr-daemon/tests/smoke/main.rs").len(), 1);
}

#[test]
fn a_snapshot_of_the_integration_tests_outside_tests_it_snapshots_fails() {
    assert_eq!(check_path("crates/efr-cli/tests/snapshots/smoke__help.snap").len(), 1);
    assert_eq!(check_path("crates/efr-cli/tests/it/smoke__help.snap").len(), 1);
    assert_eq!(check_path("crates/efr-cli/tests/it/snapshots/deeper/x.snap").len(), 1);
    assert!(check_path("crates/efr-cli/src/snapshots/efr__x.snap").is_empty());
}

#[test]
fn the_root_of_the_integration_test_binary_needs_its_cfg_test_line() {
    let root = "crates/efr-daemon/tests/it/main.rs";
    assert_eq!(rules(root, "mod hello;\n"), ["tests/it/main.rs holds the #![cfg(test)] line"]);
    assert!(rules(root, "//! Tests.\n\n#![cfg(test)]\n\nmod hello;\n").is_empty());
    assert!(rules("crates/efr-daemon/tests/it/hello.rs", "use x;\n").is_empty());
    assert!(fast_rules(root, "mod hello;\n").is_empty(), "a full rule");
}

#[test]
fn modules_of_the_one_integration_test_binary_are_clean() {
    assert!(check_path("crates/efr-daemon/tests/it/main.rs").is_empty());
    assert!(check_path("crates/efr-daemon/tests/it/hello.rs").is_empty());
    assert!(check_path("crates/efr-daemon/tests/it/hello/cases.rs").is_empty());
    assert!(check_path("crates/efr-cli/tests/it/snapshots/it__smoke__help.snap").is_empty());
}

#[test]
fn inline_test_module_fails() {
    let found = rules("crates/efr-stdx/src/fs.rs", "#[cfg(test)]\nmod  tests {\n}\n");
    assert_eq!(found.len(), 1);
}

#[test]
fn out_of_line_test_module_is_clean() {
    assert!(rules("crates/efr-stdx/src/fs.rs", "#[cfg(test)]\nmod tests;\n").is_empty());
}

#[test]
fn unsafe_outside_allowlist_fails() {
    let found = rules("crates/efr-stdx/src/fs.rs", "let x = unsafe { f() };\n");
    assert_eq!(found.len(), 1);
}

#[test]
fn unsafe_in_allowlisted_file_is_clean() {
    let found =
        rules("crates/efr-pty/src/local_holder.rs", "// SAFETY: fd is open.\nunsafe { f() };\n");
    assert!(found.is_empty());
}

#[test]
fn unsafe_as_part_of_an_identifier_is_clean() {
    let found = rules("crates/efr-pty/src/lib.rs", "#[allow(unsafe_code)]\nlet is_unsafe = 1;\n");
    assert!(found.is_empty());
}

#[test]
fn cfg_feature_outside_allowlist_fails() {
    let found = rules("crates/efr-shell/src/run.rs", "#[cfg(all(unix, feature = \"x\"))]\n");
    assert_eq!(found.len(), 1);
    let found = rules("crates/efr-shell/src/run.rs", "if cfg!(feature = \"x\") {}\n");
    assert_eq!(found.len(), 1);
}

#[test]
fn cfg_feature_in_allowlisted_file_is_clean() {
    let found = rules("crates/efr-daemon/src/screens.rs", "#[cfg(feature = \"screen-ghostty\")]\n");
    assert!(found.is_empty());
}

#[test]
fn cfg_without_feature_is_clean() {
    assert!(rules("crates/efr-shell/src/run.rs", "#[cfg(target_os = \"linux\")]\n").is_empty());
}

#[test]
fn test_daemon_outside_tests_fails() {
    let found = rules("crates/efr-daemon/src/run.rs", "use efr_test_daemon::TestDaemon;\n");
    assert_eq!(found.len(), 1);
}

#[test]
fn test_daemon_inside_tests_is_clean() {
    assert!(
        rules("crates/efr-daemon/tests/send.rs", "use efr_test_daemon::TestDaemon;\n").is_empty()
    );
    assert!(rules("crates/efr-test-daemon/src/lib.rs", "//! efr_test_daemon docs\n").is_empty());
}

#[test]
fn a_pure_module_with_a_runtime_or_io_fails() {
    for line in [
        "use tokio::sync::mpsc;\n",
        "pub async fn apply() {}\n",
        "let text = std::fs::read_to_string(path)?;\n",
        "use std::io::Read;\n",
        "use efr_stdx::fs::write_atomic;\n",
        "use crate::journal::WriteJournal;\n",
    ] {
        assert_eq!(rules("crates/efr-tools/src/patch/apply.rs", line).len(), 1, "{line}");
        assert_eq!(rules("crates/efr-tools/src/patch.rs", line).len(), 1, "{line}");
    }
}

#[test]
fn a_pure_module_within_itself_is_clean() {
    let lines = "use std::collections::HashMap;\nuse crate::patch::{Patch, PatchError};\n\
                 use super::seek;\n// tokio and std::fs are named in comments\n";
    assert!(rules("crates/efr-tools/src/patch/apply/update.rs", lines).is_empty());
    assert!(rules("crates/efr-tools/src/patch/tests.rs", "use crate::patchwork;\n").len() == 1);
}

#[test]
fn the_rest_of_the_crate_of_a_pure_module_is_not_checked() {
    let lines = "use tokio::fs;\nuse crate::journal::WriteJournal;\n";
    assert!(rules("crates/efr-tools/src/apply_patch.rs", lines).is_empty());
    assert!(rules("crates/efr-tools/src/patches.rs", lines).is_empty());
}

#[test]
fn dashes_fail_in_any_text_file() {
    assert_eq!(rules("README.md", "a \u{2014} b\n").len(), 1);
    assert_eq!(rules("docs/adr/0001.md", "1\u{2013}2\n").len(), 1);
    assert_eq!(rules("shell/zsh/efr.plugin.zsh", "# x \u{2014} y\n").len(), 1);
}

#[test]
fn plain_hyphen_is_clean() {
    assert!(rules("README.md", "a - b\n").is_empty());
}

#[test]
fn trailing_whitespace_fails_in_checked_extensions() {
    assert_eq!(rules("README.md", "line \n").len(), 1);
    assert_eq!(rules("Cargo.toml", "[package]\t\n").len(), 1);
    assert_eq!(rules("xtask/src/main.rs", "fn main() {} \n").len(), 1);
}

#[test]
fn trailing_whitespace_is_ignored_elsewhere() {
    assert!(rules("shell/zsh/efr.plugin.zsh", "echo hi \n").is_empty());
}

#[test]
fn anyhow_in_library_manifest_fails() {
    let manifest = "[package]\nname = \"efr-store\"\n\n[dependencies]\nanyhow.workspace = true\n";
    assert_eq!(rules("crates/efr-store/Cargo.toml", manifest).len(), 1);
    let manifest = "[dependencies.anyhow]\nworkspace = true\n";
    assert_eq!(rules("crates/efr-store/Cargo.toml", manifest).len(), 1);
}

#[test]
fn anyhow_as_dev_dependency_is_clean() {
    let manifest = "[dev-dependencies]\nanyhow = { workspace = true }\n";
    assert!(rules("crates/efr-store/Cargo.toml", manifest).is_empty());
}

#[test]
fn anyhow_in_binary_and_root_manifests_is_clean() {
    let manifest = "[dependencies]\nanyhow.workspace = true\n";
    assert!(rules("crates/efr-cli/Cargo.toml", manifest).is_empty());
    assert!(rules("xtask/Cargo.toml", manifest).is_empty());
    let root = "[workspace.dependencies] # every crate once\nanyhow = \"1\"\n";
    assert!(rules("Cargo.toml", root).is_empty());
}

#[test]
fn fast_mode_skips_the_slower_rules() {
    assert!(fast_rules("README.md", "line \n").is_empty());
    assert!(fast_rules("crates/efr-shell/src/run.rs", "#[cfg(feature = \"x\")]\n").is_empty());
    assert_eq!(fast_rules("README.md", "a \u{2014} b\n").len(), 1);
}

#[test]
fn crate_without_readme_fails() {
    let crates = vec![("efr-stdx".to_owned(), true), ("efr-protocol".to_owned(), false)];
    let found = check_crate_readmes(&crates);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "crates/efr-protocol");
}

#[test]
fn the_walk_skips_the_installed_and_generated_dirs_of_the_app() {
    for dir in ["app/node_modules", "app/.expo", "app/ios", "app/android", "target", ".claude"] {
        assert!(skipped_dir(Path::new(dir)), "{dir}");
    }
    for dir in ["", "app", "app/src", "app/src/core", "crates/efr-daemon/src", "docs/ios"] {
        assert!(!skipped_dir(Path::new(dir)), "{dir}");
    }
}
