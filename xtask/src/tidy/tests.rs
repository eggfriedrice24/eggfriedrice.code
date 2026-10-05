use super::{check_contents, check_crate_readmes, check_path};

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
