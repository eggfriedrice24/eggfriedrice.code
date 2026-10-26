//! What a change of the settings loosens: which keys let more run without a question.
//!
//! The settings tool marks a change that loosens permissions (its question says so),
//! and a reload holds such a change while full mode runs (`reload.rs`, the config
//! hold). Both ask [`loosening`], so the two never drift.

use efr_config::{
    AskDestructive, DbusSettings, IgnoredFiles, JobsSettings, NetworkHost, NetworkSettings,
    RemoteSettings, SandboxSettings, Settings, SnapshotSettings, SudoCache, WriteProjects,
};
use efr_permissions::{Effect, Rule};
use efr_protocol::NetworkMode;

/// The config keys of the change from `before` to `next` that let more run without a
/// question, in this order, each once: a new allow rule or a removed deny or ask rule
/// (`permissions.rules`), a mode toward `full` (the order of `Mode`), the gate of full
/// mode turned on, `per_call` to `keep`, a secret path less, a `[sandbox]` key that
/// opens the sandbox, the classifier turned on, so a model can allow exits that asked
/// the user, undo turned on or covering more, so more destructive lines run without a
/// question, a `[jobs]` key that lets jobs run longer, more often, at a higher
/// priority or with port forwards, a `[network]` key that opens the network view or
/// efr's proxy, a `[dbus]` key that opens the filtered bus (efr's auto spec, section
/// 13.1), or a `[remote]` key that opens efrd to the phone listener. Empty when the
/// change loosens nothing.
pub(crate) fn loosening(before: &Settings, next: &Settings) -> Vec<&'static str> {
    let mut keys = Vec::new();
    let old = before.permissions.rules.rules();
    let new = next.permissions.rules.rules();
    let rules = without(new, old).iter().any(|rule| rule.effect == Effect::Allow)
        || without(old, new).iter().any(|rule| rule.effect != Effect::Allow);
    let permissions = [
        ("permissions.rules", rules),
        ("permissions.mode", next.permissions.mode > before.permissions.mode),
        ("permissions.allow_full", next.permissions.allow_full && !before.permissions.allow_full),
        (
            "shell.sudo_cache",
            before.shell.sudo_cache == SudoCache::PerCall
                && next.shell.sudo_cache == SudoCache::Keep,
        ),
        (
            "permissions.secret_paths",
            before
                .permissions
                .secret_paths
                .iter()
                .any(|path| !next.permissions.secret_paths.contains(path)),
        ),
    ];
    keys.extend(permissions.into_iter().filter(|(_, loosens)| *loosens).map(|(key, _)| key));
    keys.extend(sandbox_loosening(&before.sandbox, &next.sandbox));
    if next.classifier.enabled && !before.classifier.enabled {
        keys.push("classifier.enabled");
    }
    if next.undo.enabled && !before.undo.enabled {
        keys.push("undo.enabled");
    }
    keys.extend(snapshot_loosening(&before.snapshot, &next.snapshot));
    keys.extend(jobs_loosening(&before.jobs, &next.jobs));
    keys.extend(network_loosening(&before.network, &next.network));
    keys.extend(dbus_loosening(&before.dbus, &next.dbus));
    keys.extend(remote_loosening(&before.remote, &next.remote));
    keys
}

/// The `[remote]` keys that open efrd to more peers: the phone listener turned on, or
/// another LocalAPI socket, whose whois decides which peers come in.
fn remote_loosening(before: &RemoteSettings, next: &RemoteSettings) -> Vec<&'static str> {
    keys([
        ("remote.enabled", next.enabled && !before.enabled),
        ("remote.tailscaled_socket", next.tailscaled_socket != before.tailscaled_socket),
    ])
}

/// True when the change from `before` to `next` lets more run without a question
/// ([`loosening`]).
pub(crate) fn loosens(before: &Settings, next: &Settings) -> bool {
    !loosening(before, next).is_empty()
}

/// The `[dbus]` keys that let more bus calls run without a question (phase E): the
/// filtered bus turned on, or a `[[dbus.allow]]` method that no entry of `before` names
/// on the same bus and service.
fn dbus_loosening(before: &DbusSettings, next: &DbusSettings) -> Vec<&'static str> {
    let known = |bus, name: &str, call: &String| {
        before
            .allow
            .iter()
            .any(|entry| entry.bus == bus && entry.name == name && entry.call.contains(call))
    };
    let new_call = next
        .allow
        .iter()
        .any(|entry| entry.call.iter().any(|call| !known(entry.bus, &entry.name, call)));
    keys([("dbus.proxy", next.proxy && !before.proxy), ("dbus.allow", new_call)])
}

/// The `[network]` keys that let more lines run without a question: the network view
/// turned on, so `ss` and the read forms of `ip` run in the host's network namespace
/// with no review; or, for efr's proxy (phase D2), `mode` to `proxy`, an added `allow`
/// entry, a removed `deny` entry, a `[[network.hosts]]` entry that no entry of `before`
/// covers (a new host or port, or `http` or `private` that becomes true), or a higher
/// `max_upload_bytes`.
fn network_loosening(before: &NetworkSettings, next: &NetworkSettings) -> Vec<&'static str> {
    fn adds(before: &[String], next: &[String]) -> bool {
        next.iter().any(|item| !before.contains(item))
    }
    let covered = |host: &NetworkHost| {
        before.hosts.iter().any(|known| {
            known.host == host.host
                && known.port == host.port
                && (known.http || !host.http)
                && (known.private || !host.private)
        })
    };
    keys([
        ("network.netview", next.netview && !before.netview),
        ("network.mode", next.mode == NetworkMode::Proxy && before.mode != NetworkMode::Proxy),
        ("network.allow", adds(&before.allow, &next.allow)),
        ("network.deny", adds(&next.deny, &before.deny)),
        ("network.hosts", next.hosts.iter().any(|host| !covered(host))),
        ("network.max_upload_bytes", next.max_upload_bytes > before.max_upload_bytes),
    ])
}

/// The `[jobs]` keys that let the model's background jobs do more: jobs turned on, a
/// longer default or longest time limit, more jobs at one time, more turns that notices
/// start, more wakes of a watch pattern, a higher CPU priority (a lower nice value), or
/// the port forwards of contained jobs turned on, which open a host port to a server of
/// the model (DL12).
fn jobs_loosening(before: &JobsSettings, next: &JobsSettings) -> Vec<&'static str> {
    keys([
        ("jobs.enabled", next.enabled && !before.enabled),
        ("jobs.default_minutes", next.default_minutes > before.default_minutes),
        ("jobs.max_minutes", next.max_minutes > before.max_minutes),
        ("jobs.max_running", next.max_running > before.max_running),
        ("jobs.max_running_daemon", next.max_running_daemon > before.max_running_daemon),
        ("jobs.max_wakes_per_hour", next.max_wakes_per_hour > before.max_wakes_per_hour),
        (
            "jobs.max_wakes_per_hour_daemon",
            next.max_wakes_per_hour_daemon > before.max_wakes_per_hour_daemon,
        ),
        ("jobs.max_wakes_per_watch", next.max_wakes_per_watch > before.max_wakes_per_watch),
        ("jobs.nice", next.nice < before.nice),
        ("jobs.forward_ports", next.forward_ports && !before.forward_ports),
    ])
}

/// The `[snapshot]` keys that make efr's snapshots cover more, so more lines that
/// delete or overwrite files count as undoable and run without a question: snapshots
/// turned on, small ignored files taken, or a higher size or count limit.
fn snapshot_loosening(before: &SnapshotSettings, next: &SnapshotSettings) -> Vec<&'static str> {
    keys([
        ("snapshot.enabled", next.enabled && !before.enabled),
        (
            "snapshot.ignored",
            next.ignored == IgnoredFiles::Small && before.ignored != IgnoredFiles::Small,
        ),
        ("snapshot.max_file_mib", next.max_file_mib > before.max_file_mib),
        // NOTE: a root with more files than `max_files` has no snapshot, so a higher
        // limit covers more roots.
        ("snapshot.max_files", next.max_files > before.max_files),
        ("snapshot.mirror_max_mib", next.mirror_max_mib > before.mirror_max_mib),
        ("snapshot.home_max_files", next.home_max_files > before.home_max_files),
        ("snapshot.home_max_mib", next.home_max_mib > before.home_max_mib),
    ])
}

/// The `[sandbox]` keys that open the sandbox more: another bwrap, all projects
/// writable, a new write root, cache, kept or promoted variable or rebuildable dir, a
/// mask glob or synced folder less, `ask_destructive` from `always` to
/// `unless_undoable`, or a new always-allow rule (`sandbox.allow`).
fn sandbox_loosening(before: &SandboxSettings, next: &SandboxSettings) -> Vec<&'static str> {
    fn adds<T: PartialEq>(before: &[T], next: &[T]) -> bool {
        next.iter().any(|item| !before.contains(item))
    }
    keys([
        ("sandbox.bwrap", next.bwrap != before.bwrap),
        (
            "sandbox.write_projects",
            next.write_projects == WriteProjects::All
                && before.write_projects != WriteProjects::All,
        ),
        ("sandbox.write_roots", adds(&before.write_roots, &next.write_roots)),
        ("sandbox.caches", adds(&before.caches, &next.caches)),
        ("sandbox.mask_globs", adds(&next.mask_globs, &before.mask_globs)),
        ("sandbox.env_keep", adds(&before.env_keep, &next.env_keep)),
        ("sandbox.promote_env", adds(&before.promote_env, &next.promote_env)),
        ("sandbox.synced_dirs", adds(&next.synced_dirs, &before.synced_dirs)),
        ("sandbox.rebuildable", adds(&before.rebuildable, &next.rebuildable)),
        (
            "sandbox.ask_destructive",
            next.ask_destructive == AskDestructive::UnlessUndoable
                && before.ask_destructive != AskDestructive::UnlessUndoable,
        ),
        ("sandbox.allow", adds(&before.allow, &next.allow)),
    ])
}

/// The keys of `rows` that loosen.
fn keys<const N: usize>(rows: [(&'static str, bool); N]) -> Vec<&'static str> {
    rows.into_iter().filter(|(_, loosens)| *loosens).map(|(key, _)| key).collect()
}

/// The rules of `rules` that `other` does not hold, each counted as often as it occurs.
fn without<'a>(rules: &'a [Rule], other: &[Rule]) -> Vec<&'a Rule> {
    let mut left: Vec<&Rule> = other.iter().collect();
    let mut extra = Vec::new();
    for rule in rules {
        match left.iter().position(|known| *known == rule) {
            Some(at) => {
                left.swap_remove(at);
            }
            None => extra.push(rule),
        }
    }
    extra
}

/// True when `next` opens the sandbox more than `before`.
#[cfg(test)]
pub(crate) fn sandbox_loosens(before: &SandboxSettings, next: &SandboxSettings) -> bool {
    !sandbox_loosening(before, next).is_empty()
}

/// True when `next` opens the filtered bus more than `before`.
#[cfg(test)]
pub(crate) fn dbus_loosens(before: &DbusSettings, next: &DbusSettings) -> bool {
    !dbus_loosening(before, next).is_empty()
}

/// True when `next` opens the network more than `before`.
#[cfg(test)]
pub(crate) fn network_loosens(before: &NetworkSettings, next: &NetworkSettings) -> bool {
    !network_loosening(before, next).is_empty()
}

/// True when `next` lets jobs do more than `before`.
#[cfg(test)]
pub(crate) fn jobs_loosens(before: &JobsSettings, next: &JobsSettings) -> bool {
    !jobs_loosening(before, next).is_empty()
}

#[cfg(test)]
mod tests;
