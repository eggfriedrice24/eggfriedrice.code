//! The one public error type of the crate.

use std::path::PathBuf;

use crate::policy::Action;

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
