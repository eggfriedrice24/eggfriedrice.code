//! The public error types of the crate: [`PermissionsError`] for the engine's inputs,
//! and [`HostError`] for an entry of the network host list.

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
}
