//! `cargo xtask tidy`: the file rules of CONVENTIONS.md. The shape follows
//! rust-analyzer's `xtask/src/tidy.rs`: walk the repository once, run small pure
//! checks per file, report every violation with its path and line.
//!
//! The allowlists are data at the top of this file, so a crate that legitimately needs
//! an exception adds one line here in the same commit, where a reviewer sees it.

use std::fmt;
use std::path::Path;

use anyhow::Context as _;
use walkdir::WalkDir;

use crate::output;

/// Files in which the token `unsafe` may appear. Everything else is safe Rust.
pub(crate) const UNSAFE_ALLOWLIST: &[&str] = &[
    "crates/efr-pty/src/local_holder.rs",
    "crates/efr-fdpass/src/lib.rs",
    // The sandbox launcher's descriptor numbers (ADR 0007).
    "crates/efr-sbx/src/fds.rs",
];

/// Files in which `cfg(feature = ...)` may appear, one per feature, so feature-gated
/// code stays in places a reader can find.
pub(crate) const CFG_FEATURE_ALLOWLIST: &[&str] = &[
    "crates/efr-daemon/src/screens.rs",
    "crates/efr-daemon/src/shells.rs",
    "crates/efr-credentials/src/lib.rs",
    "crates/efr-screen/src/lib.rs",
    "crates/efr-protocol/src/lib.rs",
];

/// Manifests that may name `anyhow` as a normal dependency. Any manifest may use it
/// under `[dev-dependencies]`, and the root manifest declares it once under
/// `[workspace.dependencies]`.
pub(crate) const ANYHOW_MANIFESTS: &[&str] =
    &["crates/efr-daemon/Cargo.toml", "crates/efr-cli/Cargo.toml", "xtask/Cargo.toml"];

/// Path prefixes in which `efr_test_daemon` may appear outside a `tests/` directory:
/// the crate itself.
pub(crate) const TEST_DAEMON_HOMES: &[&str] = &["crates/efr-test-daemon/"];

/// The files that define these rules name the forbidden tokens, so the token rules
/// skip them. The dash and whitespace rules still apply to them.
const RULE_SOURCES: &[&str] = &["xtask/src/tidy.rs", "xtask/src/tidy/tests.rs"];

/// Directory names never walked.
// `.claude` holds agent worktrees: full checkouts whose copies of this file would
// trip every rule while a parallel run is in progress. `research` is gitignored, so CI
// never sees it; its probe leftovers include overlay work dirs that nobody may read.
const SKIP_DIRS: &[&str] = &["target", ".git", ".claude", "research"];

/// Extensions checked for trailing whitespace.
const WHITESPACE_EXTENSIONS: &[&str] = &["rs", "toml", "md"];

/// Written as escapes so this file passes its own dash rule.
const EM_DASH: char = '\u{2014}';
const EN_DASH: char = '\u{2013}';

/// One broken rule at one place.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Violation {
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) rule: &'static str,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: {}", self.path, self.rule),
            None => write!(f, "{}: {}", self.path, self.rule),
        }
    }
}

fn violation(path: &str, line: Option<usize>, rule: &'static str) -> Violation {
    Violation { path: path.to_owned(), line, rule }
}

/// Rules that depend only on the file's path.
pub(crate) fn check_path(path: &str) -> Vec<Violation> {
    let mut found = Vec::new();
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if file_name == "mod.rs" && (path.starts_with("crates/") || path.starts_with("xtask/")) {
        found.push(violation(
            path,
            None,
            "mod.rs files are not used; name the file after the module",
        ));
    }
    if file_name.ends_with("_tests.rs") {
        found.push(violation(path, None, "unit tests live in foo/tests.rs, not foo_tests.rs"));
    }
    if is_extra_test_binary(path) {
        found.push(violation(
            path,
            None,
            "one integration test binary per crate: a module of tests/it/main.rs",
        ));
    }
    if is_stray_test_snapshot(path) {
        found.push(violation(
            path,
            None,
            "insta snapshots of the integration tests live in tests/it/snapshots/",
        ));
    }
    found
}

/// An insta snapshot under a crate's `tests/` outside `tests/it/snapshots/`, where the
/// modules of the one test binary keep theirs.
fn is_stray_test_snapshot(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["crates", _, "tests", rest @ ..] if path.ends_with(".snap") => {
            !matches!(rest, ["it", "snapshots", _])
        }
        _ => false,
    }
}

/// The root of a crate's one integration test binary, which holds its `#![cfg(test)]`.
fn is_test_binary_root(path: &str) -> bool {
    matches!(
        path.split('/').collect::<Vec<_>>().as_slice(),
        ["crates", _, "tests", "it", "main.rs"]
    )
}

/// Cargo makes a test binary of every `tests/*.rs` and `tests/*/main.rs`, and each one
/// links the crate and all its dependencies again, so a crate keeps one, `tests/it/`.
fn is_extra_test_binary(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["crates", _, "tests", file] => file.ends_with(".rs"),
        ["crates", _, "tests", dir, "main.rs"] => *dir != "it",
        _ => false,
    }
}

/// Rules over a text file's contents. `fast` keeps only the rules the pre-commit hook
/// runs: dashes, inline test modules and the unsafe allowlist.
pub(crate) fn check_contents(path: &str, contents: &str, fast: bool) -> Vec<Violation> {
    let mut found = Vec::new();
    let is_rust = path.ends_with(".rs");
    let token_rules = is_rust && !RULE_SOURCES.contains(&path);
    let check_whitespace =
        path.rsplit_once('.').is_some_and(|(_, ext)| WHITESPACE_EXTENSIONS.contains(&ext));

    for (index, line) in contents.lines().enumerate() {
        let line_no = Some(index + 1);
        if line.contains(EM_DASH) || line.contains(EN_DASH) {
            found.push(violation(path, line_no, "em or en dash; use a plain hyphen"));
        }
        if token_rules {
            if is_inline_test_module(line) {
                found.push(violation(
                    path,
                    line_no,
                    "inline test module; use `#[cfg(test)] mod tests;` and foo/tests.rs",
                ));
            }
            if has_token(line, "unsafe") && !UNSAFE_ALLOWLIST.contains(&path) {
                found.push(violation(
                    path,
                    line_no,
                    "unsafe outside the allowlist in xtask/src/tidy.rs",
                ));
            }
        }
        if fast {
            continue;
        }
        if token_rules {
            if is_cfg_feature(line) && !CFG_FEATURE_ALLOWLIST.contains(&path) {
                found.push(violation(
                    path,
                    line_no,
                    "cfg(feature) outside the allowlisted files in xtask/src/tidy.rs",
                ));
            }
            if line.contains("efr_test_daemon") && !test_daemon_may_appear(path) {
                found.push(violation(
                    path,
                    line_no,
                    "efr_test_daemon may only be used from tests/ directories",
                ));
            }
        }
        if check_whitespace && line.ends_with([' ', '\t']) {
            found.push(violation(path, line_no, "trailing whitespace"));
        }
    }

    if !fast && (path == "Cargo.toml" || path.ends_with("/Cargo.toml")) {
        found.extend(check_anyhow(path, contents));
    }
    if !fast
        && is_test_binary_root(path)
        && !contents.lines().any(|line| line.trim() == "#![cfg(test)]")
    {
        found.push(violation(path, None, "tests/it/main.rs holds the #![cfg(test)] line"));
    }
    found
}

/// Every crate directory under `crates/` carries a README (CONVENTIONS.md, module
/// layout). Takes `(crate directory name, has README.md)` pairs.
pub(crate) fn check_crate_readmes(crates: &[(String, bool)]) -> Vec<Violation> {
    crates
        .iter()
        .filter(|(_, has_readme)| !has_readme)
        .map(|(name, _)| violation(&format!("crates/{name}"), None, "crate has no README.md"))
        .collect()
}

fn is_inline_test_module(line: &str) -> bool {
    let compact: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.contains("mod tests {") || compact.contains("mod tests{")
}

/// `cfg(feature = ...)`, `cfg!(feature = ...)` and the same inside `all`, `any`,
/// `not` or `cfg_attr`.
fn is_cfg_feature(line: &str) -> bool {
    if !line.contains("cfg") {
        return false;
    }
    let mut rest = line;
    while let Some(at) = rest.find("feature") {
        let after = rest[at + "feature".len()..].trim_start();
        let before_ok = !rest[..at].ends_with(is_ident_char);
        if before_ok && after.starts_with('=') && !after.starts_with("==") {
            return true;
        }
        rest = &rest[at + "feature".len()..];
    }
    false
}

/// True when `token` appears with no identifier character on either side, so
/// `unsafe_code` and `is_unsafe` do not count as `unsafe`.
fn has_token(line: &str, token: &str) -> bool {
    line.match_indices(token).any(|(at, _)| {
        let before_ok = !line[..at].ends_with(is_ident_char);
        let after_ok = !line[at + token.len()..].starts_with(is_ident_char);
        before_ok && after_ok
    })
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn test_daemon_may_appear(path: &str) -> bool {
    let in_tests_dir = path.split('/').rev().skip(1).any(|component| component == "tests");
    in_tests_dir || TEST_DAEMON_HOMES.iter().any(|home| path.starts_with(home))
}

/// `anyhow` belongs to binaries, the xtask and tests. A line-based scan is enough
/// because manifests in this repository use one key per line.
fn check_anyhow(path: &str, contents: &str) -> Vec<Violation> {
    let manifest_may_use = ANYHOW_MANIFESTS.contains(&path);
    let mut section = String::new();
    let mut found = Vec::new();
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if let Some(header) = line.strip_prefix('[') {
            let name = header.trim_start_matches('[').split(']').next().unwrap_or_default();
            section = name.trim().to_owned();
            if let Some(table) = section.strip_suffix(".anyhow")
                && !anyhow_section_allowed(table, manifest_may_use)
            {
                found.push(violation(path, Some(index + 1), ANYHOW_RULE));
            }
            continue;
        }
        let names_anyhow = line
            .strip_prefix("anyhow")
            .is_some_and(|rest| rest.trim_start().starts_with(['=', '.']));
        let in_dependency_table = section.ends_with("dependencies");
        if names_anyhow
            && in_dependency_table
            && !anyhow_section_allowed(&section, manifest_may_use)
        {
            found.push(violation(path, Some(index + 1), ANYHOW_RULE));
        }
    }
    found
}

const ANYHOW_RULE: &str =
    "anyhow is only for efr-daemon, efr-cli, xtask and dev-dependencies; use thiserror";

fn anyhow_section_allowed(section: &str, manifest_may_use: bool) -> bool {
    section == "workspace.dependencies"
        || section.ends_with("dev-dependencies")
        || (manifest_may_use && section.ends_with("dependencies"))
}

/// Walks the repository and runs every rule.
pub(crate) fn run(root: &Path, fast: bool) -> anyhow::Result<bool> {
    let mut violations = Vec::new();
    let mut files = 0_usize;

    let walker = WalkDir::new(root).sort_by_file_name().into_iter().filter_entry(|entry| {
        !(entry.file_type().is_dir()
            && entry.file_name().to_str().is_some_and(|name| SKIP_DIRS.contains(&name)))
    });
    for entry in walker {
        let entry = entry.context("walking the repository")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root).context("path outside the repository")?;
        let Some(path) = relative.to_str() else {
            violations.push(violation(&relative.to_string_lossy(), None, "path is not UTF-8"));
            continue;
        };
        files += 1;
        violations.extend(check_path(path));
        let bytes = std::fs::read(entry.path()).with_context(|| format!("reading {path}"))?;
        // Binary files cannot hold the text rules' violations in a meaningful way.
        if let Ok(contents) = String::from_utf8(bytes) {
            violations.extend(check_contents(path, &contents, fast));
        }
    }

    if !fast {
        violations.extend(check_crate_readmes(&crate_dirs(root)?));
    }

    for found in &violations {
        output::line(&format!("tidy: {found}"));
    }
    let mode = if fast { "fast" } else { "full" };
    output::line(&format!("tidy ({mode}): {files} files checked, {} violations", violations.len()));
    Ok(violations.is_empty())
}

fn crate_dirs(root: &Path) -> anyhow::Result<Vec<(String, bool)>> {
    let crates = root.join("crates");
    if !crates.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(&crates).context("reading crates/")? {
        let entry = entry.context("reading crates/")?;
        if entry.file_type().context("reading crates/")?.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            dirs.push((name, entry.path().join("README.md").is_file()));
        }
    }
    dirs.sort();
    Ok(dirs)
}

#[cfg(test)]
mod tests;
