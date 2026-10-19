//! The rules of the filtered bus proxy (phase E, efr's auto spec 11.1).
//!
//! A contained call that reads the state of the machine through the message bus, such
//! as `systemctl --failed`, gets its own `xdg-dbus-proxy` in `--filter` mode. The proxy
//! lets through only the methods that a `--call` rule names. This module holds those
//! rules as data, with no process and no file:
//!
//! - [`DEFAULT_RULES`]: the read methods that the probe recorded for the read forms of
//!   `systemctl`, `loginctl`, `hostnamectl`, `timedatectl` and `localectl`. Each rule
//!   names an exact member; the proxy has no member globs.
//! - [`BusRule`]: one `[[dbus.allow]]` entry of `config.toml`. [`check_rules`] refuses an
//!   entry that could open more than one exact method, a protected service, or a
//!   method that changes the machine.
//! - [`BusFilter`]: the defaults of one bus plus the user's entries, and the arguments
//!   of the proxy ([`BusFilter::proxy_args`]). It never gives `--talk`, `--own`, `--see`
//!   or `--broadcast`: `--talk` beside call rules opens every method of a name.
//!
//! The proxy checks no arguments, so a rule for `Properties.Get` allows every property
//! at its path. Only the path narrows a rule.

use efr_protocol::BusKind;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{BusRuleProblem, PermissionsError};

/// One `[[dbus.allow]]` entry: more methods of one service on one bus. The entries add
/// to the defaults and never replace them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BusRule {
    /// The bus: `system` or `session`.
    pub bus: BusKind,
    /// The well-known name of the service, such as `org.freedesktop.timedate1`.
    pub name: String,
    /// The methods, each `IFACE.MEMBER` (at every path) or `IFACE.MEMBER@PATH`, where
    /// PATH may end in `/*` for the path and every path below it.
    pub call: Vec<String>,
}

/// One row of the defaults: the methods of one service on one bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultRule {
    /// The bus.
    pub bus: BusKind,
    /// The well-known name of the service.
    pub name: &'static str,
    /// The methods, each `IFACE.MEMBER@PATH`.
    pub calls: &'static [&'static str],
}

/// The default rules (DE4): the read methods that the probe recorded on systemd 262,
/// with exact members only. No glob, no `IFACE.*` and no rule with a path alone.
///
/// Left out on purpose: `Manager.GetUnitProcesses` (the command lines of every process
/// of a unit; `systemctl status` with no unit asks for every host process) and
/// `Manager.GetUnitByPID` (it reads a pid of the call's namespace as a host pid). On the
/// session bus no `Properties` rule exists at `/org/freedesktop/systemd1`, so the user
/// manager's `Environment`, which can hold tokens, stays out.
pub const DEFAULT_RULES: &[DefaultRule] = &[
    DefaultRule {
        bus: BusKind::System,
        name: "org.freedesktop.hostname1",
        calls: &[
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/hostname1",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/hostname1",
        ],
    },
    DefaultRule {
        bus: BusKind::System,
        name: "org.freedesktop.systemd1",
        calls: &[
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/systemd1",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListUnits@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListUnitsByPatterns@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListUnitFiles@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListUnitFilesByPatterns@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.GetUnitFileState@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.GetUnit@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.GetUnitByPIDFD@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListJobs@/org/freedesktop/systemd1",
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/systemd1/unit/*",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/systemd1/unit/*",
        ],
    },
    DefaultRule {
        bus: BusKind::System,
        name: "org.freedesktop.login1",
        calls: &[
            "org.freedesktop.login1.Manager.ListSessionsEx@/org/freedesktop/login1",
            "org.freedesktop.login1.Manager.ListSessions@/org/freedesktop/login1",
            "org.freedesktop.login1.Manager.ListUsers@/org/freedesktop/login1",
            "org.freedesktop.login1.Manager.ListSeats@/org/freedesktop/login1",
            "org.freedesktop.login1.Manager.GetSession@/org/freedesktop/login1",
            "org.freedesktop.login1.Manager.GetUser@/org/freedesktop/login1",
            "org.freedesktop.login1.Manager.GetSeat@/org/freedesktop/login1",
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/login1/*",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/login1/*",
        ],
    },
    DefaultRule {
        bus: BusKind::System,
        name: "org.freedesktop.timedate1",
        calls: &[
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/timedate1",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/timedate1",
        ],
    },
    DefaultRule {
        bus: BusKind::System,
        name: "org.freedesktop.locale1",
        calls: &[
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/locale1",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/locale1",
        ],
    },
    DefaultRule {
        bus: BusKind::System,
        name: "org.freedesktop.timesync1",
        calls: &[
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/timesync1",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/timesync1",
        ],
    },
    DefaultRule {
        bus: BusKind::Session,
        name: "org.freedesktop.systemd1",
        calls: &[
            "org.freedesktop.systemd1.Manager.ListUnits@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListUnitsByPatterns@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListUnitFiles@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListUnitFilesByPatterns@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.GetUnitFileState@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.GetUnit@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.GetUnitByPIDFD@/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager.ListJobs@/org/freedesktop/systemd1",
            "org.freedesktop.DBus.Properties.Get@/org/freedesktop/systemd1/unit/*",
            "org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/systemd1/unit/*",
        ],
    },
];

/// The services of the defaults. A `[[dbus.allow]]` entry for one of them may add only
/// a read member ([`READ_MEMBER_PREFIXES`], `Properties.Get` and `GetAll`).
pub const DEFAULT_NAMES: &[&str] = &[
    "org.freedesktop.systemd1",
    "org.freedesktop.login1",
    "org.freedesktop.hostname1",
    "org.freedesktop.timedate1",
    "org.freedesktop.locale1",
    "org.freedesktop.timesync1",
];

/// The starts of the member names that a `[[dbus.allow]]` entry may add for a service of
/// [`DEFAULT_NAMES`], on every interface. Matched with case, and only as a whole word:
/// the member is the word alone or the next letter is upper case, so `CanReboot`
/// matches `Can` and `CancelJob` does not.
pub const READ_MEMBER_PREFIXES: &[&str] = &["Get", "List", "Can", "Lookup"];

/// The starts of the member names that no `[[dbus.allow]]` entry may add for any other
/// service, matched without case: `KExec` and `Kexec` both match. This is a floor, not
/// a full guard: for other services efr cannot tell a read from a write, so the user
/// owns each method that they add.
pub const WRITE_MEMBER_PREFIXES: &[&str] = &[
    "Set",
    "Unset",
    "Start",
    "Stop",
    "Restart",
    "TryRestart",
    "Reload",
    "Kill",
    "Terminate",
    "Enable",
    "Disable",
    "Mask",
    "Unmask",
    "Link",
    "Enqueue",
    "Reboot",
    "PowerOff",
    "Halt",
    "Suspend",
    "Hibernate",
    "HybridSleep",
    "Sleep",
    "KExec",
    "SoftReboot",
    "SwitchRoot",
    "Lock",
    "Unlock",
    "Inhibit",
    "Attach",
    "Bind",
    "Mount",
    "Freeze",
    "Thaw",
    "Clean",
    "Abandon",
    "Create",
    "Release",
    "Activate",
    "Exit",
    "Reexecute",
    "Dump",
];

/// The services that no `[[dbus.allow]]` entry may name, each with every name below it:
/// secrets, the portals, polkit, Flatpak (`HostCommand` runs code on the host), dconf.
/// `org.kde.kwalletd` also matches `org.kde.kwalletd5` and `org.kde.kwalletd6`.
pub const FLOOR_NAMES: &[&str] = &[
    "org.freedesktop.secrets",
    "org.freedesktop.portal",
    "org.freedesktop.impl.portal",
    "org.freedesktop.PolicyKit1",
    "org.freedesktop.Flatpak",
    "org.kde.kwalletd",
    "ca.desrt.dconf",
];

/// The bus driver, which no rule may name.
const BUS_DRIVER: &str = "org.freedesktop.DBus";

/// The interface of every object's properties.
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// The rules of one bus, checked, in the order that the arguments take them: the
/// defaults, then the user's entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BusFilter {
    bus: BusKind,
    /// Each rule as `(name, IFACE.MEMBER[@PATH])`, each once.
    rules: Vec<(String, String)>,
}

impl BusFilter {
    /// The defaults of `bus` ([`DEFAULT_RULES`]) plus `extra`; an entry of `extra` for
    /// another bus is skipped. Fails with the index of the first entry of `extra` that
    /// [`check_rules`] refuses, whatever its bus.
    pub fn compile(bus: BusKind, extra: &[BusRule]) -> Result<BusFilter, PermissionsError> {
        check_rules(extra)?;
        let mut rules: Vec<(String, String)> = Vec::new();
        let mut add = |name: &str, call: &str| {
            let rule = (name.to_owned(), call.to_owned());
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        };
        for row in DEFAULT_RULES.iter().filter(|row| row.bus == bus) {
            for call in row.calls {
                add(row.name, call);
            }
        }
        for entry in extra.iter().filter(|entry| entry.bus == bus) {
            for call in &entry.call {
                add(&entry.name, call);
            }
        }
        Ok(BusFilter { bus, rules })
    }

    /// The bus of these rules.
    pub fn bus(&self) -> BusKind {
        self.bus
    }

    /// The options of `xdg-dbus-proxy`: `--filter`, `--log`, then one
    /// `--call=NAME=IFACE.MEMBER[@PATH]` per rule. Never `--talk`, `--own`, `--see` or
    /// `--broadcast`. The caller adds the bus address, the socket path and `--fd`.
    pub fn proxy_args(&self) -> Vec<String> {
        let mut args = vec!["--filter".to_owned(), "--log".to_owned()];
        args.extend(self.rules.iter().map(|(name, call)| format!("--call={name}={call}")));
        args
    }

    /// True when a rule lets through a call of `member` (`IFACE.MEMBER`) to `name` at
    /// `path`, as the proxy matches it: a rule path that ends in `/*` matches that path
    /// and every path below it.
    pub fn allows(&self, name: &str, member: &str, path: &str) -> bool {
        self.rules.iter().any(|(rule_name, call)| {
            rule_name == name
                && match call.split_once('@') {
                    None => call == member,
                    Some((rule_member, rule_path)) => {
                        rule_member == member && path_matches(rule_path, path)
                    }
                }
        })
    }
}

/// True when the rule path `rule` matches the object path `path`.
fn path_matches(rule: &str, path: &str) -> bool {
    match rule.strip_suffix("/*") {
        Some(base) => {
            path == base
                || path
                    .strip_prefix(base)
                    .is_some_and(|rest| rest.starts_with('/') || base.is_empty())
        }
        None => rule == path,
    }
}

/// Checks `rules` as a load of `config.toml` does (DE5): the index and the problem of
/// the first entry that is refused.
pub fn check_rules(rules: &[BusRule]) -> Result<(), PermissionsError> {
    for (index, rule) in rules.iter().enumerate() {
        check_rule(rule).map_err(|problem| PermissionsError::BusRule { index, problem })?;
    }
    Ok(())
}

/// The checks of one entry: first its name, then each call.
fn check_rule(rule: &BusRule) -> Result<(), BusRuleProblem> {
    let name = rule.name.as_str();
    if name.starts_with(':') {
        return Err(BusRuleProblem::UniqueName);
    }
    if name.contains('*') {
        return Err(BusRuleProblem::Glob);
    }
    if !is_bus_name(name) {
        return Err(BusRuleProblem::BadName);
    }
    if name.eq_ignore_ascii_case(BUS_DRIVER) {
        return Err(BusRuleProblem::BusDriver);
    }
    if is_floor_name(name) {
        return Err(BusRuleProblem::FloorName { name: name.to_owned() });
    }
    if rule.call.is_empty() {
        return Err(BusRuleProblem::EmptyCall);
    }
    let default_name = DEFAULT_NAMES.contains(&name);
    for call in &rule.call {
        check_call(call, default_name)?;
    }
    Ok(())
}

/// The checks of one call entry.
fn check_call(call: &str, default_name: bool) -> Result<(), BusRuleProblem> {
    let (method, path) = match call.split_once('@') {
        Some((method, path)) => (method, Some(path)),
        None => (call, None),
    };
    if method.is_empty() || (method == "*" && path.is_some()) {
        return Err(BusRuleProblem::PathOnly);
    }
    if method.ends_with(".*") {
        return Err(BusRuleProblem::InterfaceWildcard);
    }
    let path_glob = path.is_some_and(|path| path.strip_suffix("/*").unwrap_or(path).contains('*'));
    if method.contains('*') || path_glob {
        return Err(BusRuleProblem::Glob);
    }
    let bad = || BusRuleProblem::BadCall { call: call.to_owned() };
    let (interface, member) = method.rsplit_once('.').ok_or_else(bad)?;
    if !is_interface_name(interface) || !is_member_name(member) {
        return Err(bad());
    }
    if let Some(path) = path {
        let base = path.strip_suffix("/*").unwrap_or(path);
        if !(is_object_path(base) || path == "/*") {
            return Err(bad());
        }
    }
    if interface.eq_ignore_ascii_case(PROPERTIES) && member.eq_ignore_ascii_case("Set") {
        return Err(BusRuleProblem::PropertiesSet);
    }
    if default_name {
        let properties_read = interface == PROPERTIES && matches!(member, "Get" | "GetAll");
        let read = READ_MEMBER_PREFIXES.iter().any(|prefix| {
            // NOTE: the prefix must end a word, so `CanSuspend` reads and `Cancel`
            // (`Job.Cancel`, `Manager.CancelJob`) does not.
            member.strip_prefix(prefix).is_some_and(|rest| {
                rest.bytes().next().is_none_or(|next| next.is_ascii_uppercase())
            })
        });
        if !(properties_read || read) {
            return Err(BusRuleProblem::NotARead { member: method.to_owned() });
        }
    } else if WRITE_MEMBER_PREFIXES.iter().any(|prefix| starts_with_ignore_case(member, prefix)) {
        return Err(BusRuleProblem::WriteMember { member: method.to_owned() });
    }
    Ok(())
}

/// True when `name` is on [`FLOOR_NAMES`], or below one of them, without case.
fn is_floor_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    FLOOR_NAMES.iter().any(|floor| {
        let floor = floor.to_ascii_lowercase();
        if floor == "org.kde.kwalletd" {
            return lower.starts_with(&floor);
        }
        lower == floor || lower.strip_prefix(&floor).is_some_and(|rest| rest.starts_with('.'))
    })
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// A well-known bus name of the D-Bus grammar: at least two elements of
/// `[A-Za-z0-9_-]`, none empty or starting with a digit, at most 255 bytes.
fn is_bus_name(name: &str) -> bool {
    let element = |element: &str| {
        element.bytes().next().is_some_and(|first| !first.is_ascii_digit())
            && element
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    };
    name.len() <= 255 && name.contains('.') && name.split('.').all(element)
}

/// An interface name of the D-Bus grammar: at least two elements of `[A-Za-z0-9_]`,
/// none empty or starting with a digit, at most 255 bytes.
fn is_interface_name(name: &str) -> bool {
    name.len() <= 255 && name.contains('.') && name.split('.').all(is_member_name)
}

/// A member name of the D-Bus grammar: `[A-Za-z_][A-Za-z0-9_]*`, at most 255 bytes.
fn is_member_name(name: &str) -> bool {
    name.len() <= 255
        && name.bytes().next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// An object path of the D-Bus grammar: `/`, or `/` and elements of `[A-Za-z0-9_]`
/// joined by `/`, with no empty element and no `/` at the end.
fn is_object_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    path.strip_prefix('/').is_some_and(|rest| {
        rest.split('/').all(|element| {
            !element.is_empty()
                && element.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    })
}

#[cfg(test)]
mod tests;
