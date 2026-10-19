//! The public error types of the crate: [`PermissionsError`] for the engine's inputs,
//! and [`HostError`] for an entry of the network host list. [`BusRuleProblem`] and
//! [`AllowRuleProblem`] are the reasons of two `PermissionsError` variants (phase E).

// NOTE: HostError is a type of its own and not a variant of PermissionsError, because
// the phase D2 plan fixes it as the error of `HostList::compile`: efr-config turns each
// variant into its own message for `config.toml`, and the proxy compiles the same list.

use std::path::PathBuf;

use crate::policy::{Action, Check};

/// Why an entry of `network.allow`, `network.deny` or `[[network.hosts]]` is refused
/// (phase D2). Each variant holds the entry as the config wrote it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HostError {
    /// The entry is `*`, which would allow every host.
    #[error(
        "the network host `*` allows every host; name the hosts or a suffix such as *.example.com"
    )]
    BareStar,

    /// A `*.suffix` entry has fewer than two labels after `*.`, such as `*.com`.
    #[error("the network host {0:?} needs at least two labels after `*.`, such as *.example.com")]
    ShortSuffix(String),

    /// The entry has a character outside ASCII.
    #[error("the network host {0:?} is not ASCII; write the ASCII (xn--) form")]
    NotAscii(String),

    /// The entry is not a host name: a label is empty, longer than 63 bytes or has a
    /// character other than `a-z`, `0-9` and `-`, or the name is longer than 253 bytes.
    #[error("the network host {0:?} is not a valid host name")]
    BadName(String),

    /// An `@name` entry names no preset.
    #[error(
        "the network preset {0:?} is not known; the presets are @crates, @npm, @pypi, @go, @github-read and @pacman-mirrors"
    )]
    UnknownPreset(String),

    /// The entry is an IP address outside `[[network.hosts]]`.
    #[error("the network host {0:?} is an IP address; list it in [[network.hosts]]")]
    IpLiteral(String),

    /// A `[[network.hosts]]` entry has port 0.
    #[error("the network host {0:?} has port 0; a port is 1 to 65535")]
    BadPort(String),
}

/// Every way that building the engine's inputs can fail.
///
/// Deciding itself never fails: a requirement that the engine cannot judge, such as a
/// relative path, is denied with a reason, not returned as an error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PermissionsError {
    /// A location that paths are classified by is not an absolute path.
    #[error("{} is not an absolute path", .path.display())]
    NotAbsolute {
        /// The relative path.
        path: PathBuf,
    },

    /// The home directory is `/`, so user paths cannot be told apart from system paths.
    #[error("the home directory is /, so user paths cannot be told apart from system paths")]
    HomeIsRoot,

    /// A home alias lies inside or above the home directory or another alias, so one
    /// form of a path would sit inside another.
    #[error("the home alias {} lies inside or above another form of the home directory", .alias.display())]
    HomeAliasOverlaps {
        /// The alias.
        alias: PathBuf,
    },

    /// A rule's `under` path is neither absolute nor relative to `~`.
    #[error("rule {index} names {}, which is neither absolute nor under ~", .path.display())]
    RulePathNotAbsolute {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
        /// The path.
        path: PathBuf,
    },

    /// A rule's command pattern names a program that is not one plain word.
    #[error("rule {index} names the program {program:?}, which is not one plain word")]
    RuleProgramInvalid {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
        /// The program.
        program: String,
    },

    /// A rule's command pattern holds an argument that is not one plain word.
    #[error("rule {index} holds the argument {argument:?}, which is not one plain word")]
    RuleArgumentInvalid {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
        /// The argument.
        argument: String,
    },

    /// A rule's command pattern forbids a word that is not one plain word, or ends a
    /// word that is not an option in `*`.
    #[error("rule {index} forbids {word:?}, which is not one plain word or option")]
    RuleForbidInvalid {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
        /// The forbidden word.
        word: String,
    },

    /// A rule's command pattern names a check that reads the words of another program,
    /// such as `sed_print_only` for `rm`.
    #[error(
        "rule {index} checks {program:?} with {check}, which reads the words of another program"
    )]
    RuleCheckInvalid {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
        /// The check.
        check: Check,
        /// The program the rule names.
        program: String,
    },

    /// A rule has the effect `contain`, which only the built-in policy of the `auto`
    /// mode decides: a rule cannot put a call into the sandbox or take it out.
    #[error("rule {index} has the effect contain, which is decided by the auto mode")]
    RuleContain {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
    },

    /// A rule names the envelope roots, which only the built-in policy of the `auto`
    /// mode uses.
    #[error("rule {index} names the envelope roots, which only the auto mode uses")]
    RuleEnvelope {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
    },

    /// A rule pairs an action with a resource that no requirement can match, such as
    /// `execute` with a path class.
    #[error("rule {index} pairs the action {action} with a resource that it never matches")]
    RuleNeverMatches {
        /// The position of the rule in its policy, counted from 0.
        index: usize,
        /// The action.
        action: Action,
    },

    /// A `[[dbus.allow]]` entry breaks a rule of the bus filter (phase E).
    #[error("dbus.allow[{index}] is refused")]
    BusRule {
        /// The position of the entry in `dbus.allow`, counted from 0.
        index: usize,
        /// Why it is refused.
        #[source]
        problem: BusRuleProblem,
    },

    /// A `[[sandbox.allow]]` rule has a bad shape (phase E). A rule that crosses a
    /// floor is not an error here: efrd keeps it, disabled.
    #[error("sandbox.allow[{index}] is refused")]
    AllowRule {
        /// The position of the rule in `sandbox.allow`, counted from 0.
        index: usize,
        /// Why it is refused.
        #[source]
        problem: AllowRuleProblem,
    },
}

/// Why a `[[dbus.allow]]` entry is refused (phase E): it could open more than one exact
/// method, it names a protected service, or it adds a method that changes the machine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BusRuleProblem {
    /// A name or a call holds `*` where the proxy reads a glob.
    #[error("it holds `*`; name each method in full, such as org.freedesktop.DBus.Properties.Get")]
    Glob,

    /// A call names every method of an interface (`IFACE.*`).
    #[error("a call names every method of an interface (IFACE.*); name each method")]
    InterfaceWildcard,

    /// A call names only a path (`@PATH` or `*@PATH`), which allows every method there.
    #[error("a call names only a path, which allows every method there; name each method")]
    PathOnly,

    /// The entry has no call.
    #[error("the call list is empty")]
    EmptyCall,

    /// The name is not a well-known bus name.
    #[error("the name is not a valid bus name, such as org.freedesktop.timedate1")]
    BadName,

    /// The name is a unique name (`:1.42`), which changes with each connection.
    #[error(
        "the name is a unique name (:1.x), which changes with each connection; name the service"
    )]
    UniqueName,

    /// The name is the bus driver, `org.freedesktop.DBus`.
    #[error("the bus driver org.freedesktop.DBus is never allowed")]
    BusDriver,

    /// The name is a protected service, or a name below one: secrets, a portal, polkit,
    /// Flatpak, kwallet or dconf.
    #[error("{name} is a protected service (secrets, portals, polkit, Flatpak, kwallet or dconf)")]
    FloorName {
        /// The name as the entry wrote it.
        name: String,
    },

    /// A call is `org.freedesktop.DBus.Properties.Set`, which changes a service.
    #[error("org.freedesktop.DBus.Properties.Set changes a service and is never allowed")]
    PropertiesSet,

    /// A call adds a member that does not read to a service of the defaults: there a
    /// member must start with `Get`, `List`, `Can` or `Lookup`.
    #[error(
        "{member} is not a read method; for this service a method must start with Get, List, Can or Lookup"
    )]
    NotARead {
        /// The call's `IFACE.MEMBER`.
        member: String,
    },

    /// A call adds a member whose name starts with a word that changes the machine,
    /// such as `Start`, `Set` or `PowerOff`, matched without case.
    #[error("{member} can change the machine and is never allowed")]
    WriteMember {
        /// The call's `IFACE.MEMBER`.
        member: String,
    },

    /// A call is not `IFACE.MEMBER` or `IFACE.MEMBER@PATH` of the D-Bus grammar.
    #[error("{call:?} is not IFACE.MEMBER or IFACE.MEMBER@PATH")]
    BadCall {
        /// The call as the entry wrote it.
        call: String,
    },
}

/// Why a `[[sandbox.allow]]` rule has a bad shape (phase E). These checks need no fact
/// about the machine; efrd runs the floor checks when it loads the rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AllowRuleProblem {
    /// The host is not a host name: a label is empty, longer than 63 bytes or has a
    /// character other than `a-z`, `0-9` and `-`, or the name is not ASCII.
    #[error("the host {host:?} is not a valid host name")]
    BadHost {
        /// The host as the rule wrote it.
        host: String,
    },

    /// The host is a pattern, such as `*.example.com`. A rule names one host.
    #[error("the host {host:?} is a pattern; a rule names one host")]
    HostPattern {
        /// The host as the rule wrote it.
        host: String,
    },

    /// The host is an IP address.
    #[error("the host {host:?} is an IP address; a rule names a host name")]
    IpLiteral {
        /// The host as the rule wrote it.
        host: String,
    },

    /// The host is `localhost` or a name below it.
    #[error("the host {host:?} is this machine; a rule cannot open it")]
    Localhost {
        /// The host as the rule wrote it.
        host: String,
    },

    /// The port is 0.
    #[error("the port is 0; a port is 1 to 65535")]
    PortZero,

    /// A path is not absolute and does not start with `~/`.
    #[error("the path {path:?} is not absolute and does not start with ~/")]
    BadPath {
        /// The path as the rule wrote it.
        path: String,
    },

    /// A path has a `.` or `..` part.
    #[error("the path {path:?} has a . or .. part; write the path in full")]
    DotDot {
        /// The path as the rule wrote it.
        path: String,
    },
}
