//! The matcher of the always-allow rules (DE20 to DE25): whether one rule covers every
//! exit of a call, on the real paths of the call's grants.

use std::path::{Path, PathBuf};

use efr_protocol::{ExitKind, ExitSource, Grant};

use super::check::{check, floor_at_or_above, git_dir, real_write_roots, rule_path};
use super::{AllowFloor, AllowRule, AlwaysVerdict};
use crate::decision::{Cause, Decision, Effect, Subject};
use crate::exits::{Approver, ExitNeed, WriteBind};
use crate::network::HostList;
use crate::path_class::normalize;
use crate::policy::expand;
use crate::{Action, Locations, Resource, Rule, persistence_floors};

/// What the matcher reads of the engine.
pub(crate) struct Machine<'a> {
    /// The machine's locations.
    pub(crate) locations: &'a Locations,
    /// The host list of efr's proxy.
    pub(crate) hosts: &'a HostList,
    /// The always rules, each with the reason it is disabled.
    pub(crate) allow: &'a [(AllowRule, Option<AllowFloor>)],
    /// The user's own rules of the `auto` policy, after the built-in ones.
    pub(crate) user_rules: &'a [Rule],
}

/// One grant of an eligible exit, with the facts the matcher needs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    /// One host and port through efr's proxy, the host in lower case.
    Host { host: String, port: u16 },
    /// A write bind of `path`, whose real path is `real`; `dir` for a bind of a whole
    /// dir (`Target`, `Parent`, `MakeDir`).
    Write { path: PathBuf, real: PathBuf, dir: bool },
    /// One socket, which is no link.
    Socket { path: PathBuf },
}

/// Whether a rule of `machine` covers every exit of `decision`, for a turn whose
/// project root is `project` (DE20): `Covered` with the first rule that covers every
/// exit, else `Offer` with the rule that would, else `None`.
pub(crate) fn verdict(
    machine: &Machine<'_>,
    decision: &Decision,
    project: Option<&Path>,
) -> AlwaysVerdict {
    let locations = machine.locations;
    let project = project.and_then(normalize).map(|root| locations.rehome(&root).into_owned());
    let Some(items) = items(machine, decision, project.as_deref()) else {
        return AlwaysVerdict::None;
    };
    for (index, (rule, disabled)) in machine.allow.iter().enumerate() {
        if disabled.is_none() && covers(rule, &items, project.as_deref(), locations) {
            return AlwaysVerdict::Covered { index };
        }
    }
    match offer(&items, project.as_deref(), locations) {
        Some(rule)
            if check(&rule, locations, machine.hosts).is_ok()
                && covers(&rule, &items, project.as_deref(), locations) =>
        {
            AlwaysVerdict::Offer(rule)
        }
        _ => AlwaysVerdict::None,
    }
}

/// The grants of the exits of `decision` as items, when a rule may cover the call at
/// all: an `auto` shell call that asks only for its exits, each of them eligible, with a
/// real path for each write and socket grant, past the apply checks (b) and (c) of
/// DE20. `None` when no rule may cover it.
fn items(machine: &Machine<'_>, decision: &Decision, project: Option<&Path>) -> Option<Vec<Item>> {
    let shell =
        decision.reasons().iter().any(|reason| matches!(reason.subject, Subject::Command { .. }));
    let only_exits = decision.deciding().all(|reason| matches!(reason.cause, Cause::Exit { .. }));
    if !shell || decision.effect() != Effect::Ask || !only_exits {
        return None;
    }
    let locations = machine.locations;
    let mut items: Vec<Item> = Vec::new();
    for need in decision.exits() {
        if !eligible(need) {
            return None;
        }
        for grant in &need.grants {
            let item = match grant {
                Grant::Host { host, port } => Item::Host { host: lower_host(host), port: *port },
                Grant::Write { path } => {
                    let path = normal(path, locations)?;
                    let real = real_path(decision, &path, locations)?;
                    // NOTE: the load check refuses a rule with a `.git` part or below a
                    // floor, but the dir of a rule can hold a git dir or a floor, and a
                    // link can lead into one. A write there is never covered (DE24).
                    let floor =
                        |path: &Path| git_dir(path) || floor_at_or_above(path, locations).is_some();
                    if floor(&path) || floor(&real) {
                        return None;
                    }
                    let dir = matches!(
                        need.bind,
                        Some(WriteBind::Target | WriteBind::Parent(_) | WriteBind::MakeDir)
                    );
                    Item::Write { path, real, dir }
                }
                Grant::Socket { path } => {
                    let path = normal(path, locations)?;
                    let real = real_path(decision, &path, locations)?;
                    // NOTE: a link can lead anywhere when the call starts, and a socket
                    // in a write root is one that a contained call can replace by a link.
                    let mut roots = real_write_roots(locations);
                    roots.extend(project.map(Path::to_path_buf));
                    if real != path || roots.iter().any(|root| path.starts_with(root)) {
                        return None;
                    }
                    Item::Socket { path }
                }
                _ => return None,
            };
            if !items.contains(&item) {
                items.push(item);
            }
        }
    }
    // NOTE: an eligible exit has a grant, so no items means no exits.
    if items.is_empty() {
        return None;
    }
    if items.iter().any(|item| over_a_floor(item, locations)) {
        return None;
    }
    if items.iter().any(|item| user_rule_names(machine.user_rules, item, project, locations)) {
        return None;
    }
    Some(items)
}

/// True when an always rule may cover `need` at all (DE20): a `host`, `write` or
/// `socket` exit that the classifier judges on its own, that runs in the sandbox with
/// narrow grants only (one host and port, a contained write bind, one socket), and that
/// no delete in a cache made.
fn eligible(need: &ExitNeed) -> bool {
    if need.approver != Approver::Classifier
        || need.runs_unsandboxed()
        || need.source == ExitSource::CacheDelete
        || need.grants.is_empty()
    {
        return false;
    }
    match need.kind {
        ExitKind::Host => need.grants.iter().all(|grant| matches!(grant, Grant::Host { .. })),
        ExitKind::Write => {
            matches!(
                need.bind,
                Some(
                    WriteBind::Target
                        | WriteBind::Parent(_)
                        | WriteBind::TargetOnly
                        | WriteBind::MakeFile
                        | WriteBind::MakeDir
                )
            ) && need.grants.iter().all(|grant| matches!(grant, Grant::Write { .. }))
        }
        ExitKind::Socket => need.grants.iter().all(|grant| matches!(grant, Grant::Socket { .. })),
        _ => false,
    }
}

/// True when `item` is a dir bind with a floor, a synced folder or a registered project
/// at or below it, by its path or its real path, whether the floor exists or not
/// (DE20 (b)): the plan binds only the floors that exist, so the call could make a
/// missing one.
fn over_a_floor(item: &Item, locations: &Locations) -> bool {
    let Item::Write { path, real, dir: true } = item else { return false };
    let home = locations.home();
    let projects = locations.projects_below(Path::new("/"));
    let mut guarded = persistence_floors(home)
        .into_iter()
        .chain(locations.floor_roots().cloned())
        .chain(locations.synced_roots().iter().cloned())
        .chain(projects.into_iter().map(Path::to_path_buf));
    guarded.any(|floor| floor.starts_with(path) || floor.starts_with(real))
}

/// True when a user rule with the effect `deny` or `ask` names a path of `item`, or
/// what lies at or above it (DE20 (c)): `exits::predict` reads no user rule, so the
/// matcher must. A rule for every path (`any`) names every write and socket, and a rule
/// for the network of every call names every host.
fn user_rule_names(
    rules: &[Rule],
    item: &Item,
    project: Option<&Path>,
    locations: &Locations,
) -> bool {
    let home = locations.home();
    rules.iter().filter(|rule| matches!(rule.effect, Effect::Deny | Effect::Ask)).any(|rule| {
        let paths: Vec<&Path> = match item {
            Item::Host { .. } => {
                return matches!(rule.action, Action::Any | Action::Network)
                    && matches!(rule.resource, Resource::Any);
            }
            Item::Write { path, real, .. } => vec![path, real],
            Item::Socket { path } => vec![path],
        };
        if !matches!(rule.action, Action::Any | Action::Write) {
            return false;
        }
        let overlaps =
            |dir: &Path| paths.iter().any(|path| path.starts_with(dir) || dir.starts_with(path));
        match &rule.resource {
            Resource::Any => true,
            Resource::Under(dir) => expand(dir, home)
                .map(|dir| locations.rehome(&dir).into_owned())
                .is_some_and(|dir| overlaps(&dir)),
            Resource::Project => project.is_some_and(overlaps),
            Resource::Class(class) => {
                paths.iter().any(|path| locations.classify_normal(path, None) == *class)
            }
            Resource::Command(_) | Resource::Envelope => false,
        }
    })
}

/// True when `rule` covers every item (DE23 to DE25).
fn covers(rule: &AllowRule, items: &[Item], project: Option<&Path>, locations: &Locations) -> bool {
    match rule {
        AllowRule::Host { host, port } => {
            let host = lower_host(host);
            items.iter().all(|item| {
                matches!(item, Item::Host { host: known, port: at } if *known == host && at == port)
            })
        }
        AllowRule::Write { under, project: scope } => {
            if let Some(scope) = scope {
                let scope = rule_path(scope, locations);
                let same = scope.is_some_and(|scope| {
                    project == Some(scope.as_path()) && locations.is_project_root(&scope)
                });
                if !same {
                    return false;
                }
            }
            let Some(under) = rule_path(under, locations) else { return false };
            items
                .iter()
                .all(|item| matches!(item, Item::Write { real, .. } if real.starts_with(&under)))
        }
        AllowRule::Socket { path } => {
            let Some(path) = rule_path(path, locations) else { return false };
            items.iter().all(|item| matches!(item, Item::Socket { path: known } if *known == path))
        }
    }
}

/// The rule that an `a` answer would save for `items`: one host and port, one socket,
/// or the real path of the one write bind that holds every other one, limited to the
/// turn's project when it has one. `None` for a mix of kinds or of hosts and sockets.
fn offer(items: &[Item], project: Option<&Path>, locations: &Locations) -> Option<AllowRule> {
    let home = locations.home();
    match items.first()? {
        Item::Host { host, port } => Some(AllowRule::Host { host: host.clone(), port: *port }),
        Item::Socket { path } => Some(AllowRule::Socket { path: tilde(path, home)? }),
        Item::Write { .. } => {
            let reals: Vec<&PathBuf> = items
                .iter()
                .map(|item| match item {
                    Item::Write { real, .. } => Some(real),
                    _ => None,
                })
                .collect::<Option<_>>()?;
            let top = reals.iter().find(|top| reals.iter().all(|real| real.starts_with(top)))?;
            let project = match project {
                Some(root) => Some(tilde(root, home)?),
                None => None,
            };
            Some(AllowRule::Write { under: tilde(top, home)?, project })
        }
    }
}

/// `path` in normal form, under the home directory when it lies under an alias.
fn normal(path: &Path, locations: &Locations) -> Option<PathBuf> {
    normalize(path).map(|path| locations.rehome(&path).into_owned())
}

/// The real path of `path` from the facts of `decision`, in the same form.
fn real_path(decision: &Decision, path: &Path, locations: &Locations) -> Option<PathBuf> {
    decision.real_paths().iter().find_map(|(known, real)| {
        (normal(known, locations).as_deref() == Some(path)).then(|| normal(real, locations))?
    })
}

/// `host` in lower case, without a final dot.
fn lower_host(host: &str) -> String {
    let lower = host.to_ascii_lowercase();
    lower.strip_suffix('.').map(str::to_owned).unwrap_or(lower)
}

/// `path` as a rule writes it: below `~/` when it lies in the home directory. `None`
/// for a path that is no UTF-8 text.
fn tilde(path: &Path, home: &Path) -> Option<String> {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => Some("~".to_owned()),
        Ok(rest) => Some(format!("~/{}", rest.to_str()?)),
        Err(_) => path.to_str().map(str::to_owned),
    }
}
