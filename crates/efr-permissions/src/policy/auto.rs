//! The rules that the `auto` mode adds to the `cautious` ones, as data.
//!
//! Three groups, each allowed to run without a question:
//!
//! - The writer programs (`rm`, `mkdir`, `cp`, `mv` and more), anywhere. The shell
//!   tool declares their operands as writes, so the path rules decide: a write in the
//!   turn's project or `$SCRATCH` is free, a write to the project's root, above it or
//!   anywhere else asks, and a secret or efr's config is denied.
//! - The project's build, test, format and lint tools, only while the hidden shell is
//!   in the project or `$SCRATCH`. They run the project's own code. Options that point
//!   them at another directory, another config or another package registry are
//!   forbidden.
//! - Local git in the project or `$SCRATCH`, and `git fetch` and `git pull` from a
//!   remote that the repository names.
//!
//! Only the rows that fetch the packages a project declares, and `git fetch` and
//! `git pull`, allow network access: a URL or an upload is how a model that follows
//! injected instructions would send data out. Everything else asks: `git push`, a
//! system package manager, `curl`, a script, and every line the engine cannot read.
//!
//! `auto.md` beside this file is the same table for the docs, and a test keeps the two
//! equal.

use super::defaults::{Operands, pattern};
use crate::{Action, Check, CommandPattern, Effect, Resource, Rule};

use self::Operands::{Alone, Any, AtMost};
use self::Place::{Anywhere, Workspace};

/// Where a row's command may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    /// In any directory.
    Anywhere,
    /// In the turn's project or in `$SCRATCH`: one rule for each.
    Workspace,
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
pub(super) struct Row {
    pub(super) program: &'static str,
    pub(super) args: &'static [&'static str],
    pub(super) forbid: &'static [&'static str],
    pub(super) operands: Operands,
    pub(super) check: Option<Check>,
    pub(super) place: Place,
    /// True when the row also allows the network access of a line that runs it.
    pub(super) network: bool,
}

const WRITER: Row = Row {
    program: "",
    args: &[],
    forbid: &[],
    operands: Any,
    check: None,
    place: Anywhere,
    network: false,
};

const TOOL: Row = Row { place: Workspace, ..WRITER };

const FETCH: Row = Row { network: true, ..TOOL };

/// What points cargo at another manifest, another target directory or other code.
const CARGO_FORBID: &[&str] = &["--manifest-path", "--config", "--target-dir", "-Z", "-C"];

/// What points a node package manager at another directory, the global install,
/// another registry or another shell for the scripts.
const NODE_FORBID: &[&str] = &[
    "--script-shell",
    "--prefix",
    "--dir",
    "--cwd",
    "-C",
    "-g",
    "--global",
    "--location",
    "--registry",
    "--userconfig",
];

/// What points go at another directory, another module file or another program, and an
/// operand with `@`, which names a module version to download.
const GO_FORBID: &[&str] = &["-C*", "-modfile*", "-overlay*", "-toolexec*", "-exec*", "@"];

/// What points uv at another project or adds packages from outside it.
const UV_FORBID: &[&str] = &[
    "--directory",
    "--project",
    "--with*",
    "--index*",
    "--extra-index-url",
    "--default-index",
    "--find-links",
];

/// What makes `git fetch` and `git pull` run a program or send data to the remote.
const GIT_REMOTE_FORBID: &[&str] =
    &["--upload-pack", "--exec", "-o", "--server-option", "-s", "--strategy"];

const ROWS: &[Row] = &[
    // The writer programs. Their operands are declared writes, which the path rules
    // judge; `cp` and `ln` also write into a target directory.
    Row { program: "rm", ..WRITER },
    Row { program: "rmdir", ..WRITER },
    Row { program: "mkdir", ..WRITER },
    Row { program: "touch", ..WRITER },
    Row { program: "mv", ..WRITER },
    Row { program: "cp", ..WRITER },
    Row { program: "ln", ..WRITER },
    Row { program: "chmod", ..WRITER },
    Row { program: "truncate", ..WRITER },
    Row { program: "tee", ..WRITER },
    // Build, test, format and lint tools in the project or $SCRATCH.
    Row {
        program: "cargo",
        args: &["build|check|test|fetch|update"],
        forbid: CARGO_FORBID,
        ..FETCH
    },
    Row {
        program: "cargo",
        args: &["clippy|fmt|doc|run|bench|nextest|tree|metadata"],
        // `--open` starts a browser.
        forbid: &["--manifest-path", "--config", "--target-dir", "-Z", "-C", "--open"],
        ..TOOL
    },
    // `VAR=value` operands and `--set` replace the variables of the recipes, and
    // `--command` and the shell options run a command that no recipe names.
    Row {
        program: "just",
        forbid: &[
            "-f",
            "--justfile",
            "-d",
            "--working-directory",
            "-g",
            "--global-justfile",
            "--set",
            "-c",
            "--command",
            "--shell*",
            "--dotenv-path",
            "=",
        ],
        ..TOOL
    },
    Row {
        program: "make",
        forbid: &[
            "-C",
            "--directory",
            "-f",
            "--file",
            "--makefile",
            "-I",
            "--include-dir",
            "--eval",
            "-E",
            "=",
        ],
        ..TOOL
    },
    // Installing what the project's manifest and lock file declare. An operand would
    // name another package to download.
    Row {
        program: "npm",
        args: &["install|ci"],
        forbid: NODE_FORBID,
        operands: AtMost(0),
        ..FETCH
    },
    Row { program: "npm", args: &["run|test|build|lint"], forbid: NODE_FORBID, ..TOOL },
    Row {
        program: "pnpm",
        args: &["install|ci"],
        forbid: NODE_FORBID,
        operands: AtMost(0),
        ..FETCH
    },
    Row { program: "pnpm", args: &["run|test|build|lint"], forbid: NODE_FORBID, ..TOOL },
    Row {
        program: "yarn",
        args: &["install|ci"],
        forbid: NODE_FORBID,
        operands: AtMost(0),
        ..FETCH
    },
    Row { program: "yarn", args: &["run|test|build|lint"], forbid: NODE_FORBID, ..TOOL },
    Row {
        program: "bun",
        args: &["install|ci"],
        forbid: NODE_FORBID,
        operands: AtMost(0),
        ..FETCH
    },
    Row { program: "bun", args: &["run|test|build|lint"], forbid: NODE_FORBID, ..TOOL },
    Row { program: "go", args: &["build|test|vet|fmt|run"], forbid: GO_FORBID, ..TOOL },
    Row { program: "go", args: &["mod", "tidy"], forbid: GO_FORBID, ..TOOL },
    Row {
        program: "go",
        args: &["mod", "download"],
        forbid: GO_FORBID,
        operands: AtMost(0),
        ..FETCH
    },
    Row { program: "pytest", forbid: &["--rootdir", "-c", "--config-file", "-p"], ..TOOL },
    Row { program: "uv", args: &["run"], forbid: UV_FORBID, ..TOOL },
    Row { program: "uv", args: &["sync"], forbid: UV_FORBID, operands: AtMost(0), ..FETCH },
    Row { program: "ruff", ..TOOL },
    Row { program: "mypy", ..TOOL },
    Row { program: "rustfmt", ..TOOL },
    Row { program: "prettier", ..TOOL },
    Row { program: "eslint", ..TOOL },
    Row { program: "tsc", ..TOOL },
    Row { program: "zig", args: &["build"], forbid: &["-p", "--prefix*", "--build-file"], ..TOOL },
    // Local git in the project or $SCRATCH.
    Row { program: "git", args: &["add"], ..TOOL },
    Row { program: "git", args: &["commit"], ..TOOL },
    // `--discard-changes` and `--force` throw away changes in the work tree.
    Row {
        program: "git",
        args: &["switch"],
        forbid: &["--discard-changes", "-f", "--force"],
        ..TOOL
    },
    // A branch only: a path, `.`, `--` or a pathspec would throw away changes.
    Row {
        program: "git",
        args: &["checkout"],
        forbid: &[
            "-p",
            "--patch",
            "-f",
            "--force",
            "--ours",
            "--theirs",
            "-m",
            "--merge",
            "--conflict",
            "--overlay",
            "--no-overlay",
            "--pathspec-from-file",
        ],
        operands: AtMost(2),
        check: Some(Check::RefNames),
        ..TOOL
    },
    // The index only: `--worktree` would throw away changes in the work tree.
    Row {
        program: "git",
        args: &["restore", "--staged|-S"],
        forbid: &["-W", "--worktree", "-p", "--patch"],
        ..TOOL
    },
    Row { program: "git", args: &["stash"], operands: Alone, ..TOOL },
    Row { program: "git", args: &["stash", "push|list|show|apply|pop"], ..TOOL },
    // A strategy is a program named git-merge-<strategy> on the PATH.
    Row { program: "git", args: &["merge"], forbid: &["-s", "--strategy"], ..TOOL },
    // Not interactive, and no `--exec`, which runs a command after each commit.
    Row {
        program: "git",
        args: &["rebase"],
        forbid: &["-i", "--interactive", "-x", "--exec", "--edit-todo", "-s", "--strategy"],
        ..TOOL
    },
    Row { program: "git", args: &["cherry-pick"], forbid: &["-s", "--strategy"], ..TOOL },
    // Creating a tag: not deleting, moving or signing one.
    Row {
        program: "git",
        args: &["tag"],
        forbid: &[
            "-d",
            "--delete",
            "-f",
            "--force",
            "-s",
            "--sign",
            "-u",
            "--local-user",
            "-v",
            "--verify",
        ],
        ..TOOL
    },
    Row { program: "git", args: &["mv"], ..TOOL },
    Row { program: "git", args: &["rm"], ..TOOL },
    Row { program: "git", args: &["worktree", "add|list"], ..TOOL },
    // From a remote the repository names: an operand that is no ref name, such as a
    // URL, asks.
    Row {
        program: "git",
        args: &["fetch"],
        forbid: GIT_REMOTE_FORBID,
        check: Some(Check::RefNames),
        ..FETCH
    },
    Row {
        program: "git",
        args: &["pull"],
        forbid: GIT_REMOTE_FORBID,
        check: Some(Check::RefNames),
        ..FETCH
    },
];

impl Row {
    /// The command pattern of the row, without a place.
    pub(super) fn pattern(&self) -> CommandPattern {
        let pattern = pattern(self.program, self.args, self.forbid, self.operands);
        match self.check {
            Some(check) => pattern.with_check(check),
            None => pattern,
        }
    }

    /// The rules of the row: for each place, an execute rule, then a network rule when
    /// the row fetches.
    pub(super) fn rules(&self) -> Vec<Rule> {
        let places: &[Option<&str>] = match self.place {
            Anywhere => &[None],
            Workspace => &[Some("project"), Some("scratch")],
        };
        let mut rules = Vec::new();
        for place in places {
            let pattern = match place {
                Some(under) => self.pattern().with_under(under),
                None => self.pattern(),
            };
            rules.push(Rule::new(
                Action::Execute,
                Resource::Command(pattern.clone()),
                Effect::Allow,
            ));
            if self.network {
                rules.push(Rule::new(Action::Network, Resource::Command(pattern), Effect::Allow));
            }
        }
        rules
    }
}

/// The rows, in order, for the test that keeps `auto.md` equal to them.
#[cfg(test)]
pub(super) fn rows() -> &'static [Row] {
    ROWS
}

/// The rules of the table, in order.
pub(super) fn rules() -> Vec<Rule> {
    ROWS.iter().flat_map(Row::rules).collect()
}
