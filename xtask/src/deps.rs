//! `cargo xtask deps`: the dependency rule of ARCHITECTURE.md, checked on the
//! `cargo metadata` graph so it cannot drift into a convention a reviewer remembers.
//!
//! Four checks:
//! 1. every workspace member's direct normal dependencies on other workspace crates are
//!    on its allowlist (an unknown member fails);
//! 2. no forbidden edge exists, transitively, over normal dependencies;
//! 3. crates that only one owner may reach (libghostty-vt, rusqlite) are reachable from
//!    no other member except through that owner;
//! 4. dev-only crates appear as dev-dependencies only under the members allowed to
//!    use them.
//!
//! A new edge is a one-line change to the tables below plus a line in the crate's
//! README saying why.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use anyhow::Context as _;
use cargo_metadata::{CargoOpt, DependencyKind, MetadataCommand};

use crate::output;

/// Workspace crate to the workspace crates it may depend on as normal dependencies.
/// Mirrors the "Allowed workspace dependencies" column of the crate table.
pub(crate) const ALLOWED: &[(&str, &[&str])] = &[
    // Tier 0
    ("efr-stdx", &[]),
    ("efr-protocol", &["efr-stdx"]),
    // The pure patch engine of apply_patch and the edit tool: no IO, no runtime.
    ("efr-patch", &[]),
    // Tier 1
    ("efr-store", &["efr-protocol", "efr-stdx"]),
    ("efr-credentials", &["efr-stdx"]),
    ("efr-permissions", &["efr-protocol"]),
    ("efr-scope", &["efr-protocol", "efr-stdx"]),
    ("efr-holder", &["efr-protocol", "efr-stdx"]),
    ("efr-http", &["efr-stdx"]),
    ("efr-screen", &["efr-protocol", "efr-stdx"]),
    ("efr-provider", &["efr-protocol", "efr-stdx"]),
    ("efr-test-support", &["efr-protocol", "efr-store", "efr-provider", "efr-stdx"]),
    // Markdown and render events in, ANSI out; no IO (structure document, addendum A).
    ("efr-render", &[]),
    // The pure logic of the auto sandbox; no efr-stdx, because it reaches tokio.
    ("efr-sandbox", &["efr-protocol"]),
    // Tier 2
    ("efr-screen-vt100", &["efr-screen"]),
    ("efr-screen-ghostty", &["efr-screen"]),
    ("efr-pty", &["efr-holder", "efr-stdx"]),
    ("efr-shell", &["efr-holder", "efr-screen", "efr-protocol", "efr-sandbox", "efr-stdx"]),
    ("efr-tools", &["efr-shell", "efr-scope", "efr-patch", "efr-protocol", "efr-stdx"]),
    ("efr-provider-openai", &["efr-provider", "efr-http", "efr-protocol", "efr-stdx"]),
    // The Anthropic Messages API client; the key arrives through TokenSource.
    ("efr-provider-anthropic", &["efr-provider", "efr-http", "efr-protocol", "efr-stdx"]),
    ("efr-oauth-openai", &["efr-http", "efr-credentials", "efr-provider", "efr-stdx"]),
    // The sandbox launcher: efr-sbx run, inner, bridge and probe. No async runtime.
    ("efr-sbx", &["efr-sandbox", "efr-protocol"]),
    // efr's own snapshot store: hardened git through efr-scope's Git::command (phase 4
    // of the auto spec, section 15.1).
    ("efr-snapshot", &["efr-scope", "efr-protocol", "efr-stdx"]),
    // The config file's schema, shared by efrd and efr; efr-tools must never reach it,
    // because it reaches efr-permissions.
    ("efr-config", &["efr-permissions", "efr-protocol", "efr-stdx"]),
    // Tier 3
    (
        "efr-conversation",
        // No efr-tools edge: efr-tools depends on efr-shell, and efr-conversation ->
        // efr-shell is forbidden through any chain. The conversation drives tools through
        // its own Toolbox trait, which efr-daemon implements over the tool registry.
        &["efr-provider", "efr-permissions", "efr-scope", "efr-store", "efr-protocol", "efr-stdx"],
    ),
    ("efr-transport", &["efr-protocol", "efr-stdx"]),
    ("efr-client", &["efr-protocol", "efr-stdx"]),
    // Tier 4: the daemon composes every library crate except the client side and the
    // test crates.
    (
        "efr-daemon",
        &[
            "efr-stdx",
            "efr-protocol",
            "efr-store",
            "efr-credentials",
            "efr-permissions",
            "efr-scope",
            "efr-holder",
            "efr-http",
            "efr-screen",
            "efr-provider",
            "efr-screen-vt100",
            "efr-screen-ghostty",
            "efr-pty",
            "efr-shell",
            "efr-tools",
            "efr-provider-openai",
            "efr-provider-anthropic",
            "efr-oauth-openai",
            "efr-config",
            "efr-conversation",
            "efr-transport",
            "efr-sandbox",
            "efr-snapshot",
            "efr-patch",
        ],
    ),
    ("efr-cli", &["efr-client", "efr-config", "efr-render", "efr-protocol", "efr-stdx"]),
    // Tier T
    ("efr-test-daemon", &["efr-daemon", "efr-test-support", "efr-client", "efr-protocol"]),
    // Tooling
    // config-docs writes docs/config.md and docs/config.schema.json from efr-config.
    ("xtask", &["efr-config"]),
];

/// Edges that must not exist, directly or through any chain of normal dependencies.
/// The target may be a workspace crate or a third-party crate.
pub(crate) const FORBIDDEN: &[(&str, &str)] = &[
    ("efr-tools", "efr-permissions"),
    ("efr-provider-openai", "efr-oauth-openai"),
    ("efr-provider-anthropic", "efr-oauth-openai"),
    ("efr-conversation", "efr-shell"),
    ("efr-conversation", "efr-transport"),
    ("efr-transport", "efr-store"),
    ("efr-protocol", "tokio"),
    ("efr-test-support", "efr-daemon"),
    ("efr-sandbox", "tokio"),
    ("efr-sbx", "tokio"),
    ("efr-patch", "tokio"),
];

/// Third-party crates that exactly one workspace crate may reach: (crate, owner).
/// Every other member may reach them only through the owner.
pub(crate) const EXCLUSIVE: &[(&str, &str)] = &[
    ("libghostty-vt", "efr-screen-ghostty"),
    ("libghostty-vt-sys", "efr-screen-ghostty"),
    ("rusqlite", "efr-store"),
];

/// Workspace crates that may appear as a dev-dependency only under the listed
/// members. efr-daemon is reached from tests only through efr-test-daemon.
pub(crate) const DEV_DEPENDENTS: &[(&str, &[&str])] =
    &[("efr-test-daemon", &["efr-daemon", "efr-cli"]), ("efr-daemon", &[])];

/// The dependency graph by package name. Several versions of one third-party crate
/// collapse into one node, which is what the rules are about.
#[derive(Debug, Default)]
pub(crate) struct Graph {
    pub(crate) members: BTreeSet<String>,
    pub(crate) normal: BTreeMap<String, BTreeSet<String>>,
    pub(crate) dev: BTreeMap<String, BTreeSet<String>>,
}

impl Graph {
    pub(crate) fn add_normal(&mut self, from: &str, to: &str) {
        self.normal.entry(from.to_owned()).or_default().insert(to.to_owned());
    }

    pub(crate) fn add_dev(&mut self, from: &str, to: &str) {
        self.dev.entry(from.to_owned()).or_default().insert(to.to_owned());
    }

    fn normal_deps(&self, name: &str) -> impl Iterator<Item = &String> {
        self.normal.get(name).into_iter().flatten()
    }

    /// Shortest chain of normal dependencies from `from` to `to` that never enters
    /// `avoid`, as the list of crate names along it.
    fn path(&self, from: &str, to: &str, avoid: Option<&str>) -> Option<Vec<String>> {
        let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
        let mut seen: BTreeSet<&str> = BTreeSet::from([from]);
        let mut queue: VecDeque<&str> = VecDeque::from([from]);
        while let Some(current) = queue.pop_front() {
            for next in self.normal_deps(current) {
                let next = next.as_str();
                if Some(next) == avoid || !seen.insert(next) {
                    continue;
                }
                parent.insert(next, current);
                if next == to {
                    let mut chain = vec![to.to_owned()];
                    let mut at = to;
                    while let Some(&up) = parent.get(at) {
                        chain.push(up.to_owned());
                        at = up;
                    }
                    chain.reverse();
                    return Some(chain);
                }
                queue.push_back(next);
            }
        }
        None
    }
}

/// Runs every check and returns the violations, one sentence each.
pub(crate) fn check(graph: &Graph) -> Vec<String> {
    let allowed: BTreeMap<&str, &[&str]> = ALLOWED.iter().copied().collect();
    let mut violations = Vec::new();

    for member in &graph.members {
        let Some(allowed_deps) = allowed.get(member.as_str()) else {
            violations.push(format!(
                "{member} is a workspace member with no entry in xtask/src/deps.rs ALLOWED"
            ));
            continue;
        };
        for dep in graph.normal_deps(member) {
            if graph.members.contains(dep) && !allowed_deps.contains(&dep.as_str()) {
                violations
                    .push(format!("{member} -> {dep} is not an allowed workspace dependency"));
            }
        }
    }

    for (from, to) in FORBIDDEN {
        if !graph.members.contains(*from) {
            continue;
        }
        if let Some(chain) = graph.path(from, to, None) {
            violations.push(format!("forbidden edge {from} -> {to}: {}", chain.join(" -> ")));
        }
    }

    for (target, owner) in EXCLUSIVE {
        for member in graph.members.iter().filter(|m| m.as_str() != *owner) {
            if let Some(chain) = graph.path(member, target, Some(owner)) {
                violations.push(format!(
                    "only {owner} may depend on {target}, but {}",
                    chain.join(" -> ")
                ));
            }
        }
    }

    for (dev_crate, dependents) in DEV_DEPENDENTS {
        for (member, deps) in &graph.dev {
            if deps.contains(*dev_crate) && !dependents.contains(&member.as_str()) {
                violations.push(format!(
                    "{member} has {dev_crate} as a dev-dependency; only [{}] may",
                    dependents.join(", ")
                ));
            }
        }
    }

    violations
}

/// Reads `cargo metadata` with all features, so optional edges such as
/// efr-daemon's `screen-ghostty` feature are checked too.
fn load(root: &Path) -> anyhow::Result<Graph> {
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .features(CargoOpt::AllFeatures)
        .exec()
        .context("running cargo metadata")?;
    let names: BTreeMap<_, _> =
        metadata.packages.iter().map(|p| (p.id.clone(), p.name.to_string())).collect();
    let name_of = |id| names.get(id).cloned().context("package id missing from cargo metadata");

    let mut graph = Graph::default();
    for id in &metadata.workspace_members {
        graph.members.insert(name_of(id)?);
    }
    let resolve = metadata.resolve.as_ref().context("cargo metadata returned no resolve graph")?;
    for node in &resolve.nodes {
        let from = name_of(&node.id)?;
        for dep in &node.deps {
            let to = name_of(&dep.pkg)?;
            for kind in &dep.dep_kinds {
                match kind.kind {
                    DependencyKind::Normal => graph.add_normal(&from, &to),
                    DependencyKind::Development if graph.members.contains(&from) => {
                        graph.add_dev(&from, &to);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(graph)
}

pub(crate) fn run(root: &Path, json: bool) -> anyhow::Result<bool> {
    let graph = load(root)?;
    let violations = check(&graph);
    if json {
        let report = serde_json::json!({
            "members": graph.members,
            "violations": violations,
        });
        output::line(&serde_json::to_string_pretty(&report)?);
    } else {
        for violation in &violations {
            output::line(&format!("deps: {violation}"));
        }
        output::line(&format!(
            "deps: {} workspace members checked, {} violations",
            graph.members.len(),
            violations.len()
        ));
    }
    Ok(violations.is_empty())
}

#[cfg(test)]
mod tests;
