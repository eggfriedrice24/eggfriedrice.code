//! The read-only commands of [`Policy::defaults`](super::Policy::defaults), as data.
//!
//! Each row is a program, the words that must follow it, the words that must not
//! appear, and the fewest and most operands, or nothing at all after the args. A row allows only what cannot change the
//! machine or print the environment:
//! options that write a file, run another program, wait forever or change a setting
//! are forbidden, so a command that uses one needs approval. `defaults.md` beside this
//! file is the same table for the docs, and a test keeps the two equal.

use self::Operands::{Alone, Any, AtLeast, AtMost};
use crate::CommandPattern;

/// The fewest and the most operands of a row, or nothing at all after its args.
#[derive(Debug, Clone, Copy)]
enum Operands {
    Any,
    AtMost(usize),
    AtLeast(usize),
    /// No operand and no option after the args.
    Alone,
}

/// One row: program, args, forbid, operands.
type Row = (&'static str, &'static [&'static str], &'static [&'static str], Operands);

/// The subcommands of `systemctl` that only report. `show` is a row of its own.
const SYSTEMCTL: &str = "status|list-units|list-unit-files|is-active|is-enabled|is-failed|cat";

/// What lets `systemctl show` reach another machine or name a property.
const SYSTEMCTL_SHOW_FORBID: &[&str] = &["-H", "--host", "-p", "-P", "--property"];

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
    ("ls", &[], &[], Any),
    ("pwd", &[], &[], Any),
    ("cat", &[], &[], Any),
    ("head", &[], &[], Any),
    // Following a file waits forever and keeps the hidden shell busy.
    ("tail", &[], &["-f", "-F", "--follow"], Any),
    ("wc", &[], &[], Any),
    // `-C` compiles a magic file into the working directory.
    ("file", &[], &["-C", "--compile"], Any),
    ("stat", &[], &[], Any),
    ("du", &[], &[], Any),
    ("df", &[], &[], Any),
    ("lsblk", &[], &[], Any),
    // The cache options write the cache file.
    ("blkid", &[], &["-g", "--garbage-collect", "-c", "--cache-file"], Any),
    ("findmnt", &[], &["-p", "--poll"], Any),
    ("free", &[], &["-s", "--seconds"], Any),
    ("uptime", &[], &[], Any),
    ("uname", &[], &[], Any),
    ("whoami", &[], &[], Any),
    ("id", &[], &[], Any),
    ("groups", &[], &[], Any),
    // An operand or a file sets the host name.
    ("hostname", &[], &["-F", "--file", "-b", "--boot"], AtMost(0)),
    ("date", &[], &["-s", "--set"], Any),
    ("which", &[], &[], Any),
    ("type", &[], &[], Any),
    ("command", &["-v|-V"], &[], Any),
    ("echo", &[], &[], Any),
    // `-v` assigns the result to a shell variable, `PATH` among them.
    ("printf", &[], &["-v"], Any),
    ("realpath", &[], &[], Any),
    ("readlink", &[], &[], Any),
    ("basename", &[], &[], Any),
    ("dirname", &[], &[], Any),
    // `-o` writes the listing to a file; `-R` writes one into every directory.
    ("tree", &[], &["-o", "-R"], Any),
    // `--pre` and `--hostname-bin` run a program.
    ("rg", &[], &["--pre", "--hostname-bin"], Any),
    ("grep", &[], &[], Any),
    ("egrep", &[], &[], Any),
    ("fgrep", &[], &[], Any),
    ("diff", &[], &[], Any),
    ("cmp", &[], &[], Any),
    // `--files0-from` prints the files that another file names, which `$SCRATCH` may
    // hold without an approval.
    ("sort", &[], &["-o", "--output", "--compress-program", "--files0-from"], Any),
    // A second operand is the output file.
    ("uniq", &[], &[], AtMost(1)),
    ("cut", &[], &[], Any),
    ("tr", &[], &[], Any),
    ("column", &[], &[], Any),
    // jq programs can print the environment through `env` and `$ENV`. A program read
    // from a file (`-f`), or a module that `include` or `import` loads from a library
    // path (`-L`), is text the line does not show, and `$SCRATCH` may hold it without
    // an approval.
    (
        "jq",
        &[],
        &[
            "-i",
            "--in-place",
            "-f",
            "--from-file",
            "-L",
            "--library-path",
            "env",
            "ENV",
            "include",
            "import",
        ],
        Any,
    ),
    // BSD-style `e` shows the environment of each process. procps reads the whole line
    // again as BSD syntax when one word is not valid UNIX syntax, dashed clusters
    // included, so `ps -ex` and `ps -e -x` show it too: an `e` may stand in no word but
    // a long option. The `environ` field shows it as well; `-o environ` holds an `e`,
    // and `--format=environ` is the long option that would hide one.
    ("ps", &[], &["e", "-e", "--format"], Any),
    // The UNIX forms with `-e`, alone: nothing after them can send the line to BSD.
    ("ps", &["-e|-ef|-eF|-ely|-eLf|-ejH"], &[], Alone),
    ("pgrep", &[], &[], Any),
    ("ss", &[], &["-K", "--kill", "-D", "--diag"], Any),
    ("ip", &["addr|address|a"], &[], AtMost(0)),
    ("ip", &["addr|address|a", "show|list"], &[], Any),
    ("ip", &["route|r"], &[], AtMost(0)),
    ("ip", &["route|r", "show|list"], &[], Any),
    ("ip", &["link|l"], &[], AtMost(0)),
    ("ip", &["link|l", "show|list"], &[], Any),
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
        Any,
    ),
    // `-H` reaches another machine over ssh.
    ("systemctl", &[SYSTEMCTL], &["-H", "--host"], Any),
    ("systemctl", &["--user", SYSTEMCTL], &["-H", "--host"], Any),
    // Without a unit, `show` prints the service manager's environment, as `env` would;
    // a property's name given as a separate word would count as the unit.
    ("systemctl", &["show"], SYSTEMCTL_SHOW_FORBID, AtLeast(1)),
    ("systemctl", &["--user", "show"], SYSTEMCTL_SHOW_FORBID, AtLeast(1)),
    ("git", &[GIT], &["--output"], Any),
    ("git", &["--no-pager", GIT], &["--output"], Any),
    // An operand creates a branch.
    ("git", &["branch"], GIT_BRANCH_FORBID, AtMost(0)),
    ("git", &["--no-pager", "branch"], GIT_BRANCH_FORBID, AtMost(0)),
    // An operand adds, renames or removes a remote, or reaches it.
    ("git", &["remote"], &[], AtMost(0)),
    ("git", &["--no-pager", "remote"], &[], AtMost(0)),
    ("pacman", &["-Q*|--query"], &[], Any),
    // Refreshing, upgrading, downloading or cleaning changes the package cache.
    (
        "pacman",
        &["-Ss|-Ssq|-Sqs|-Si|-Sii"],
        &["-y", "-u", "-w", "-c", "--refresh", "--sysupgrade", "--downloadonly", "--clean"],
        Any,
    ),
    ("lspci", &[], &[], Any),
    ("lsusb", &[], &[], Any),
    ("sensors", &[], &["-s", "--set"], Any),
    ("nproc", &[], &[], Any),
    ("find", &[], &["-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint*", "-fls"], Any),
];

/// The read-only commands, in order.
pub(super) fn read_only() -> Vec<CommandPattern> {
    ROWS.iter()
        .map(|(program, args, forbid, operands)| {
            let pattern = CommandPattern::new(*program)
                .with_args(args.iter().copied())
                .with_forbid(forbid.iter().copied());
            match operands {
                Any => pattern,
                AtMost(max) => pattern.with_max_operands(*max),
                AtLeast(min) => pattern.with_min_operands(*min),
                Alone => pattern.with_max_operands(0).with_max_options(0),
            }
        })
        .collect()
}
