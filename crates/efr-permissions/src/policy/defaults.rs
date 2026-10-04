//! The read-only commands of [`Policy::defaults`](super::Policy::defaults), as data.
//!
//! Each row is a program, the words that must follow it, the words that must not
//! appear and the most operands. A row allows only what cannot change the machine:
//! options that write a file, run another program, wait forever or change a setting
//! are forbidden, so a command that uses one needs approval. `defaults.md` beside this
//! file is the same table for the docs, and a test keeps the two equal.

use crate::CommandPattern;

/// One row: program, args, forbid, max operands.
type Row = (&'static str, &'static [&'static str], &'static [&'static str], Option<usize>);

/// The subcommands of `systemctl` that only report.
const SYSTEMCTL: &str = "status|list-units|list-unit-files|is-active|is-enabled|is-failed|show|cat";

/// The subcommands of `git` that only report.
const GIT: &str = "status|diff|log|show|rev-parse|ls-files|blame";

/// What turns `git branch` from a listing into a change.
const GIT_BRANCH_FORBID: &[&str] = &[
    "-d",
    "-D",
    "--delete",
    "-m",
    "-M",
    "--move",
    "-c",
    "-C",
    "--copy",
    "-f",
    "--force",
    "-u",
    "--set-upstream-to",
    "--unset-upstream",
    "--edit-description",
    "-t",
    "--track",
    "--no-track",
    "--create-reflog",
];

const ROWS: &[Row] = &[
    ("ls", &[], &[], None),
    ("pwd", &[], &[], None),
    ("cat", &[], &[], None),
    ("head", &[], &[], None),
    // Following a file waits forever and keeps the hidden shell busy.
    ("tail", &[], &["-f", "-F", "--follow"], None),
    ("wc", &[], &[], None),
    // `-C` compiles a magic file into the working directory.
    ("file", &[], &["-C", "--compile"], None),
    ("stat", &[], &[], None),
    ("du", &[], &[], None),
    ("df", &[], &[], None),
    ("lsblk", &[], &[], None),
    // The cache options write the cache file.
    ("blkid", &[], &["-g", "--garbage-collect", "-c", "--cache-file"], None),
    ("findmnt", &[], &["-p", "--poll"], None),
    ("free", &[], &["-s", "--seconds"], None),
    ("uptime", &[], &[], None),
    ("uname", &[], &[], None),
    ("whoami", &[], &[], None),
    ("id", &[], &[], None),
    ("groups", &[], &[], None),
    // An operand or a file sets the host name.
    ("hostname", &[], &["-F", "--file", "-b", "--boot"], Some(0)),
    ("date", &[], &["-s", "--set"], None),
    ("which", &[], &[], None),
    ("type", &[], &[], None),
    ("command", &["-v|-V"], &[], None),
    ("echo", &[], &[], None),
    // `-v` assigns the result to a shell variable, `PATH` among them.
    ("printf", &[], &["-v"], None),
    ("realpath", &[], &[], None),
    ("readlink", &[], &[], None),
    ("basename", &[], &[], None),
    ("dirname", &[], &[], None),
    // `-o` writes the listing to a file; `-R` writes one into every directory.
    ("tree", &[], &["-o", "-R"], None),
    // `--pre` and `--hostname-bin` run a program.
    ("rg", &[], &["--pre", "--hostname-bin"], None),
    ("grep", &[], &[], None),
    ("egrep", &[], &[], None),
    ("fgrep", &[], &[], None),
    ("diff", &[], &[], None),
    ("cmp", &[], &[], None),
    ("sort", &[], &["-o", "--output", "--compress-program"], None),
    // A second operand is the output file.
    ("uniq", &[], &[], Some(1)),
    ("cut", &[], &[], None),
    ("tr", &[], &[], None),
    ("column", &[], &[], None),
    // jq programs can print the environment through `env` and `$ENV`.
    ("jq", &[], &["-i", "--in-place", "env", "ENV"], None),
    // BSD-style `e` shows the environment of each process.
    ("ps", &[], &["e"], None),
    ("pgrep", &[], &[], None),
    ("ss", &[], &["-K", "--kill", "-D", "--diag"], None),
    ("ip", &["addr|address|a"], &[], Some(0)),
    ("ip", &["addr|address|a", "show|list"], &[], None),
    ("ip", &["route|r"], &[], Some(0)),
    ("ip", &["route|r", "show|list"], &[], None),
    ("ip", &["link|l"], &[], Some(0)),
    ("ip", &["link|l", "show|list"], &[], None),
    (
        "journalctl",
        &[],
        &[
            "--vacuum*",
            "--rotate",
            "--flush",
            "--sync",
            "--relinquish-var",
            "--smart-relinquish-var",
            "--setup-keys",
            "--update-catalog",
            "--cursor-file",
            "-f",
            "--follow",
        ],
        None,
    ),
    // `-H` reaches another machine over ssh.
    ("systemctl", &[SYSTEMCTL], &["-H", "--host"], None),
    ("systemctl", &["--user", SYSTEMCTL], &["-H", "--host"], None),
    ("git", &[GIT], &["--output"], None),
    ("git", &["--no-pager", GIT], &["--output"], None),
    // An operand creates a branch.
    ("git", &["branch"], GIT_BRANCH_FORBID, Some(0)),
    ("git", &["--no-pager", "branch"], GIT_BRANCH_FORBID, Some(0)),
    // An operand adds, renames or removes a remote, or reaches it.
    ("git", &["remote"], &[], Some(0)),
    ("git", &["--no-pager", "remote"], &[], Some(0)),
    ("pacman", &["-Q*|--query"], &[], None),
    // Refreshing, upgrading, downloading or cleaning changes the package cache.
    (
        "pacman",
        &["-Ss|-Ssq|-Sqs|-Si|-Sii"],
        &["-y", "-u", "-w", "-c", "--refresh", "--sysupgrade", "--downloadonly", "--clean"],
        None,
    ),
    ("lspci", &[], &[], None),
    ("lsusb", &[], &[], None),
    ("sensors", &[], &["-s", "--set"], None),
    ("nproc", &[], &[], None),
    ("find", &[], &["-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint*", "-fls"], None),
];

/// The read-only commands, in order.
pub(super) fn read_only() -> Vec<CommandPattern> {
    ROWS.iter()
        .map(|(program, args, forbid, max)| {
            let pattern = CommandPattern::new(*program)
                .with_args(args.iter().copied())
                .with_forbid(forbid.iter().copied());
            match max {
                Some(max) => pattern.with_max_operands(*max),
                None => pattern,
            }
        })
        .collect()
}
