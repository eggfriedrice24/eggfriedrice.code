//! The programs and patterns of a simple command that leave the sandbox:
//! privilege, persistence, upload, the network, the buses, the desktop, devices and the
//! destructive idioms.

use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, DesktopKind, ExitKind, Grant};

use super::scan::{OPAQUE, is_assignment};
use super::{Approver, Ecosystem, PRIVILEGE_EXITS};
use crate::command::{PRIVILEGED, git};
use crate::network::{HostList, HostVerdict, host_and_port, is_ip_literal};
use crate::{AutoSupport, Egress};

/// One exit that a simple command needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) kind: ExitKind,
    pub(super) grants: Vec<Grant>,
    pub(super) target: Option<PathBuf>,
    /// Who may approve it: the default of its kind, unless the form says more.
    pub(super) approver: Approver,
    /// The words that name it in a user message.
    pub(super) names: Vec<String>,
    /// The hosts that it reaches, for an `outside` exit of a remote shell or an ssh git
    /// URL; the caller adds the line's hosts to a `host` or `upload` exit.
    pub(super) hosts: Vec<String>,
    /// True for a `bus` exit of a read form of [`bus_read`] (phase E): on a machine
    /// with the filtered bus proxy it is no exit, and the line gets the view
    /// `Grant::BusFilter` of its bus instead.
    pub(super) bus_read: bool,
    /// efr's notes for the question, such as what a publish runs.
    pub(super) notes: Vec<String>,
}

impl Found {
    fn of(kind: ExitKind) -> Self {
        let approver = if kind.user_only() { Approver::User } else { Approver::Classifier };
        Found {
            kind,
            grants: Vec::new(),
            target: None,
            approver,
            names: Vec::new(),
            hosts: Vec::new(),
            bus_read: false,
            notes: Vec::new(),
        }
    }

    /// The `outside` exit of a program that reaches `host` over ssh with the user's ssh
    /// agent and keys (DP20 (e), (f)): the classifier judges it only when a user message
    /// names the host, as written on the line (an alias of `~/.ssh/config` counts as
    /// written). A destination that efr cannot read, such as `$HOST`, is for the user,
    /// and so is a form that the line `steered`: one that can reach another host first
    /// or run a local command, such as `ssh -J` or `ssh -o ProxyCommand=...`.
    fn remote_shell(host: Option<String>, steered: bool) -> Self {
        match host {
            Some(host) => Found {
                approver: if steered { Approver::User } else { Approver::ClassifierWhenNamed },
                names: vec![host.clone()],
                hosts: vec![host],
                ..Found::of(ExitKind::Outside)
            },
            None => Found { approver: Approver::User, ..Found::of(ExitKind::Outside) },
        }
    }

    fn bus(bus: BusKind) -> Self {
        Found { grants: vec![Grant::Bus { bus }], ..Found::of(ExitKind::Bus) }
    }

    /// An `upload` that the classifier judges when a user message names one of `names`
    /// (spec 9.1): the verb of the program, the remote or the host.
    fn upload(names: Vec<String>) -> Self {
        let names = super::names::dedup(names);
        Found { approver: Approver::ClassifierWhenNamed, names, ..Found::of(ExitKind::Upload) }
    }

    /// The `upload` of a package publish, which only the user may approve (v0.0.4,
    /// DV25): it runs project code with the user's rights, which a sandboxed command
    /// can write, and `notes` say which.
    fn publish(verb: &str, notes: &[&str]) -> Self {
        Found {
            approver: Approver::User,
            notes: notes.iter().map(|note| (*note).to_owned()).collect(),
            ..Found::upload(vec![verb.to_owned()])
        }
    }

    /// A `desktop_ipc` exit of `program` with `args`: the classifier judges a read or
    /// reload form of [`DESKTOP_IPC_READ`], the user every other form. Its grant is the
    /// socket of the desktop that the program talks to ([`desktop_kind`]).
    fn desktop(program: &str, args: &[String]) -> Self {
        let approver =
            if is_desktop_read(program, args) { Approver::Classifier } else { Approver::User };
        let grants =
            desktop_kind(program).map(|desktop| Grant::Desktop { desktop }).into_iter().collect();
        Found { approver, grants, ..Found::of(ExitKind::DesktopIpc) }
    }

    /// A `bus` exit of `nmcli` or `iwctl` with `args`, which talk to NetworkManager and
    /// iwd on the system bus: the classifier judges a read form of [`is_bus_read`], the
    /// user every other form. The grant binds the raw system bus, so a "yes" lets the
    /// program change the network. It keeps the exit with the filtered bus proxy of
    /// phase E too: the proxy's default rules hold no method of NetworkManager or iwd.
    fn bus_form(program: &str, args: &[String]) -> Self {
        let approver =
            if is_bus_read(program, args) { Approver::Classifier } else { Approver::User };
        Found { approver, ..Found::bus(BusKind::System) }
    }
}

/// A program that runs another one named among its words: the options that take a
/// value as the next word, and how many words follow the options before the program.
type Wrapper = (&'static str, &'static [&'static str], usize);

/// The wrappers that the engine looks through, and the programs that run another one
/// as another user, so `sudo docker ps` finds `docker` too.
const WRAPPERS: &[Wrapper] = &[
    ("env", &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"], 0),
    ("nice", &["-n", "--adjustment"], 0),
    ("nohup", &[], 0),
    ("time", &["-f", "--format", "-o", "--output"], 0),
    ("timeout", &["-s", "--signal", "-k", "--kill-after"], 1),
    ("command", &[], 0),
    ("builtin", &[], 0),
    ("exec", &["-a"], 0),
    ("stdbuf", &["-i", "-o", "-e", "--input", "--output", "--error"], 0),
    ("ionice", &["-c", "--class", "-n", "--classdata", "-p", "--pid", "-P", "--pgid"], 0),
    ("chrt", &["-T", "-P", "-D"], 1),
    ("taskset", &[], 1),
    ("setsid", &[], 0),
    ("flock", &["-w", "--timeout", "-E", "--conflict-exit-code"], 1),
    (
        "xargs",
        &[
            "-a",
            "--arg-file",
            "-d",
            "--delimiter",
            "-E",
            "-I",
            "-L",
            "-n",
            "--max-args",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
        ],
        0,
    ),
    ("watch", &["-n", "--interval"], 0),
    ("unbuffer", &[], 0),
    ("noglob", &[], 0),
    ("nocorrect", &[], 0),
    ("-", &[], 0),
    (
        "sudo",
        &[
            "-u", "--user", "-g", "--group", "-h", "--host", "-p", "--prompt", "-C", "-D",
            "--chdir", "-r", "--role", "-t", "--type", "-T", "-U",
        ],
        0,
    ),
    ("doas", &["-u", "-C"], 0),
    ("run0", &["-u", "--user", "-g", "--group", "--unit", "--property", "-D", "--chdir"], 0),
    ("pkexec", &["--user"], 0),
];

/// Interpreters: programs that run code from their words, a file or their input, which
/// efr cannot read from the line. A version number at the end of the name does not
/// count, so `python3.12` and `lua5.4` are here too.
const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "mksh",
    "fish",
    "csh",
    "tcsh",
    "python",
    "pypy",
    "perl",
    "ruby",
    "node",
    "nodejs",
    "deno",
    "bun",
    "lua",
    "luajit",
    "php",
    "rscript",
    "tclsh",
    "wish",
    "pwsh",
    "julia",
    "awk",
    "gawk",
    "mawk",
    "nawk",
    "osascript",
];

/// The words after an interpreter that only print its version or help.
const INTERPRETER_INFO: &[&str] = &["--version", "-V", "--help", "-h"];

/// True when `words`, one command, runs an interpreter on code that efr cannot read: an
/// inline program (`python3 -c`, `perl -e`, `sh -c`), a script file, or its input. Only a
/// call that prints the version or the help, such as `python3 --version`, runs none.
pub(super) fn runs_unread_code(words: &[String]) -> bool {
    let Some((first, args)) = words.split_first() else {
        return false;
    };
    let name = base_name(first).to_lowercase();
    let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    INTERPRETERS.contains(&name)
        && !(args.len() == 1 && INTERPRETER_INFO.contains(&args[0].as_str()))
}

/// The base name of a program word, so `/usr/bin/sudo` is `sudo`.
pub(super) fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The commands that `words` runs: itself past its assignments, then the command each
/// wrapper runs, and the commands of `find -exec`, outermost first.
pub(super) fn commands(words: &[String]) -> Vec<&[String]> {
    placed_commands(words).into_iter().map(|(command, _)| command).collect()
}

/// The precommand modifiers and reserved words of zsh after which the shell itself
/// still finds the next program, so a builtin runs there: `time printf` runs the
/// builtin `printf`. After `command`, `exec`, any program and `find -exec`, only a
/// file runs.
const SHELL_PREFIXES: &[&str] = &["builtin", "noglob", "nocorrect", "time"];

/// The builtins of zsh: those of `zsh/main`, which every zsh has, and those of the
/// modules that zsh loads when a word first needs them (`zsh/zle`, `zsh/rlimits`,
/// `zsh/sched`, `zsh/zutil`). Where the shell finds a program, it runs one of these
/// before a file of the same name on the `PATH`, as for `printf`, `echo` and `[`.
const ZSH_BUILTINS: &[&str] = &[
    "-",
    ".",
    ":",
    "[",
    "alias",
    "autoload",
    "bg",
    "bindkey",
    "break",
    "builtin",
    "bye",
    "cd",
    "chdir",
    "command",
    "continue",
    "declare",
    "dirs",
    "disable",
    "disown",
    "echo",
    "emulate",
    "enable",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "float",
    "functions",
    "getln",
    "getopts",
    "hash",
    "history",
    "integer",
    "jobs",
    "kill",
    "let",
    "limit",
    "local",
    "logout",
    "noglob",
    "popd",
    "print",
    "printf",
    "pushd",
    "pushln",
    "pwd",
    "r",
    "read",
    "readonly",
    "rehash",
    "return",
    "sched",
    "set",
    "setopt",
    "shift",
    "source",
    "suspend",
    "test",
    "times",
    "trap",
    "true",
    "ttyctl",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unfunction",
    "unhash",
    "unlimit",
    "unset",
    "unsetopt",
    "vared",
    "wait",
    "whence",
    "where",
    "which",
    "zcompile",
    "zformat",
    "zle",
    "zmodload",
    "zparseopts",
    "zregexparse",
    "zstyle",
];

/// The reserved words of zsh (`man zshmisc`), which the shell reads where a program
/// would stand.
const ZSH_RESERVED: &[&str] = &[
    "do",
    "done",
    "esac",
    "then",
    "elif",
    "else",
    "fi",
    "for",
    "case",
    "if",
    "while",
    "function",
    "repeat",
    "time",
    "until",
    "select",
    "coproc",
    "nocorrect",
    "foreach",
    "end",
    "!",
    "[[",
    "{",
    "}",
];

/// True when the shell runs `word` itself where it finds a program: a builtin or a
/// reserved word of zsh.
pub(super) fn is_shell_word(word: &str) -> bool {
    ZSH_BUILTINS.contains(&word) || ZSH_RESERVED.contains(&word)
}

/// The commands of [`commands`], each with true when the shell itself finds its
/// program, so a builtin can run there: the first one, and one after a word of
/// [`SHELL_PREFIXES`] there. A program behind any other wrapper, or that `find -exec`
/// runs, is a file.
pub(super) fn placed_commands(words: &[String]) -> Vec<(&[String], bool)> {
    let mut found = Vec::new();
    let mut rest = skip_assignments(words);
    let mut by_shell = true;
    while let Some(first) = rest.first() {
        found.push((rest, by_shell));
        let program = base_name(first);
        if program == "find" {
            for inner in find_exec(&rest[1..]) {
                found.extend(placed_commands(inner).into_iter().map(|(inner, _)| (inner, false)));
            }
            break;
        }
        let looks_up = program == "command" && rest.iter().any(|word| word == "-v" || word == "-V");
        match WRAPPERS.iter().find(|(name, _, _)| *name == program) {
            Some((_, values, positional)) if !looks_up => {
                // NOTE: the word itself, not its base name: `/usr/bin/time` is a file.
                by_shell = by_shell && SHELL_PREFIXES.contains(&first.as_str());
                rest = past_wrapper(&rest[1..], values, *positional);
            }
            _ => break,
        }
    }
    found
}

/// True when the zsh functions of the mount view run `command`, which `by_shell` places
/// as [`placed_commands`] does: `df` or `findmnt` by its bare name, where the shell itself
/// finds the program. A path such as `/bin/df`, `command df`, `exec df` and any other
/// wrapper, such as `timeout`, `env` or `watch`, run the file, which shows the
/// sandbox's own mounts.
pub(super) fn runs_mount_view(command: &[String], by_shell: bool) -> bool {
    by_shell && command.first().is_some_and(|program| matches!(program.as_str(), "df" | "findmnt"))
}

/// Reserved words that start a compound command, before the command it runs, such as
/// `do` in `for f in *; do curl ...; done`.
const KEYWORDS: &[&str] = &["do", "then", "else", "elif", "if", "while", "until", "!", "{"];

/// `words` past the assignments and the reserved words before the program.
fn skip_assignments(words: &[String]) -> &[String] {
    let start = words
        .iter()
        .position(|word| !is_assignment(word) && !KEYWORDS.contains(&word.as_str()))
        .unwrap_or(words.len());
    &words[start..]
}

/// The words after a wrapper's options, assignments and positional words.
fn past_wrapper<'a>(words: &'a [String], values: &[&str], positional: usize) -> &'a [String] {
    let mut at = 0;
    while let Some(word) = words.get(at) {
        if word == "--" {
            at += 1;
            break;
        }
        if word.starts_with('-') && word.len() > 1 {
            at += if values.contains(&word.as_str()) { 2 } else { 1 };
        } else if is_assignment(word) {
            at += 1;
        } else {
            break;
        }
    }
    let at = (at + positional).min(words.len());
    &words[at..]
}

/// The commands of the `-exec`, `-execdir`, `-ok` and `-okdir` actions of `find`.
fn find_exec(words: &[String]) -> Vec<&[String]> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < words.len() {
        if matches!(words[at].as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
            let start = at + 1;
            let end = words[start..]
                .iter()
                .position(|word| word == ";" || word == "+")
                .map_or(words.len(), |n| start + n);
            found.push(&words[start..end]);
            at = end;
        }
        at += 1;
    }
    found
}

/// The exits of one command, its program first. `hosts` is the host list of efr's
/// proxy, which only [`Egress::Proxy`] reads.
pub(super) fn classify(words: &[String], support: &AutoSupport, hosts: &HostList) -> Vec<Found> {
    let Some((first, args)) = words.split_first() else {
        return Vec::new();
    };
    let program = base_name(first);
    let mut found = Vec::new();
    if PRIVILEGED.contains(&program) || PRIVILEGE_EXITS.contains(&program) {
        found.push(Found::of(ExitKind::Privilege));
    }
    let network = Network { support, hosts, urls: line_urls(words) };
    match program {
        "systemctl" => found.extend(systemctl(args)),
        "loginctl" => found.push(match verb(args, &[]) {
            Some("enable-linger") => Found::of(ExitKind::Persistence),
            // NOTE: with no operand or one user, as `enable-linger`. The route sends it
            // to the classifier only when efrd says the user is its own (persist.rs);
            // more operands need root.
            Some("disable-linger") if args.len() <= 2 => Found::of(ExitKind::Persistence),
            None => Found::bus(BusKind::System),
            Some(verb) if LOGINCTL_READ.contains(&verb) => Found::bus(BusKind::System),
            Some(_) => Found::of(ExitKind::Privilege),
        }),
        "hostnamectl" | "timedatectl" | "localectl" => {
            found.push(match verb(args, &[]) {
                Some(verb) if verb.starts_with("set-") || verb == "hostname" && args.len() > 1 => {
                    Found::of(ExitKind::Privilege)
                }
                _ => Found::bus(BusKind::System),
            });
        }
        "resolvectl" => found.push(Found::bus(BusKind::System)),
        "coredumpctl" => {
            if matches!(verb(args, &[]), Some("info" | "debug" | "gdb" | "dump")) {
                found.push(Found::bus(BusKind::System));
            }
        }
        "busctl" => {
            let bus = if has(args, "--user") { BusKind::Session } else { BusKind::System };
            found.push(match verb(args, &[]) {
                Some("call" | "set-property" | "emit") => Found::of(ExitKind::Privilege),
                _ => Found::bus(bus),
            });
        }
        "gdbus" => {
            let bus = if has(args, "--system") { BusKind::System } else { BusKind::Session };
            found.push(match verb(args, &[]) {
                Some("call" | "emit") => Found::of(ExitKind::Privilege),
                _ => Found::bus(bus),
            });
        }
        "notify-send" => found.push(Found::bus(BusKind::Session)),
        "systemd-run" => found.push(if has(args, "--user") {
            Found::of(ExitKind::Persistence)
        } else {
            Found::of(ExitKind::Privilege)
        }),
        // NOTE: `crontab -l` only reads the user's crontab, but it needs the setgid
        // program, which the sandbox runs without its group. As an `outside` exit the
        // classifier can let the model read the crontab before it writes all of it with
        // `crontab -` (persist.rs).
        "crontab" if args == ["-l"] => found.push(Found::of(ExitKind::Outside)),
        "crontab" | "at" | "batch" => found.push(Found::of(ExitKind::Persistence)),
        "passwd" | "chsh" | "chfn" | "newgrp" => found.push(Found::of(ExitKind::Privilege)),
        "git" => found.extend(git(args, &network)),
        "npm" | "pnpm" | "yarn" | "bun" => found.extend(node(program, args, &network)),
        "cargo" => found.extend(match verb(args, &["--color", "-Z", "--config"]) {
            Some("publish") if !has(args, "--no-verify") => {
                Some(Found::publish("publish", &[CARGO_BUILDS, CARGO_CONFIG]))
            }
            Some(verb @ ("publish" | "yank" | "owner")) => {
                Some(Found::publish(verb, &[CARGO_CONFIG]))
            }
            Some(
                "fetch" | "update" | "install" | "search" | "add" | "login" | "generate-lockfile",
            ) => network.host(),
            _ => None,
        }),
        "go" => found.extend(match verb(args, &[]) {
            Some("get" | "install") => network.host(),
            Some("mod") => {
                if matches!(second_verb(args), Some("download" | "tidy" | "vendor")) {
                    network.host()
                } else {
                    None
                }
            }
            Some("run") if args.iter().any(|arg| arg.contains('@')) => network.host(),
            _ => None,
        }),
        "uv" => found.extend(match verb(args, &["--directory", "--project"]) {
            Some("sync" | "add" | "lock" | "remove") => network.host(),
            Some("pip" | "tool" | "python") => {
                if matches!(second_verb(args), Some("install" | "run" | "upgrade" | "sync")) {
                    network.host()
                } else {
                    None
                }
            }
            _ => None,
        }),
        "pip" | "pip3" => found.extend(match verb(args, &[]) {
            Some("install" | "download" | "wheel" | "index" | "search") => network.host(),
            _ => None,
        }),
        "python" | "python3" if args.first().is_some_and(|arg| arg == "-m") => {
            if let Some((module, rest)) = args[1..].split_first()
                && matches!(module.as_str(), "pip" | "pip3")
            {
                let mut inner = vec![module.clone()];
                inner.extend(rest.iter().cloned());
                found.extend(classify(&inner, support, hosts));
            }
        }
        "gem" => found.extend(match verb(args, &[]) {
            Some("push") => Some(Found::upload(vec!["push".to_owned()])),
            Some("install" | "update" | "fetch") => network.host(),
            _ => None,
        }),
        // NOTE: `twine check` and the other forms that do not upload still reach PyPI,
        // so they get the network of a fetch.
        "twine" => found.extend(match verb(args, &[]) {
            Some(verb @ ("upload" | "register")) => Some(Found::upload(vec![verb.to_owned()])),
            _ => network.host(),
        }),
        "rustup" => found.extend(rustup(args)),
        "gh" => found.extend(gh(args, &network)),
        "scp" | "sftp" => found.push(Found::upload(remote_names(program, args))),
        "rsync" if args.iter().any(|arg| is_remote(arg)) => {
            found.push(Found::upload(remote_names(program, args)));
        }
        "curl" => found.extend(if curl_uploads(args) {
            Some(Found::upload(url_names(args)))
        } else {
            network.host()
        }),
        "wget" => found.extend(if wget_uploads(args) {
            Some(Found::upload(url_names(args)))
        } else {
            network.host()
        }),
        "ssh" | "mosh" | "autossh" => found.extend(
            network.remote_shell(ssh_destination(program, args), ssh_steered(program, args)),
        ),
        "nmcli" | "iwctl" => found.push(Found::bus_form(program, args)),
        program if RAW_NETWORK_PROGRAMS.contains(&program) => found.push(open_network()),
        program if HTTP_NETWORK_PROGRAMS.contains(&program) => found.extend(network.host()),
        program if HOST_VIEW.contains(&program) => {
            found.push(Found { grants: vec![Grant::OpenNetwork], ..Found::of(ExitKind::HostView) });
        }
        program if DESKTOP_IPC.contains(&program) => found.push(Found::desktop(program, args)),
        "niri" if verb(args, &[]) == Some("msg") => found.push(Found::desktop("niri", args)),
        // NOTE: in the sandbox they show the sandbox's own mounts, so they run outside
        // it. With the mount view, the shell's own `df` and `findmnt` show the host's
        // mounts and the caller drops this exit ([`runs_mount_view`]).
        "df" | "findmnt" => found.push(Found::of(ExitKind::Outside)),
        // NOTE: `dd of=/dev/null` overwrites nothing; the sandbox has that node.
        "dd" if dd_targets(words).into_iter().any(|target| {
            !crate::path_class::normalize(Path::new(target))
                .is_some_and(|path| is_sandbox_device(&path))
        }) =>
        {
            found.push(Found::of(ExitKind::Destructive));
        }
        "shred" | "truncate" => found.push(Found::of(ExitKind::Destructive)),
        "find" if has(args, "-delete") => found.push(Found::of(ExitKind::Destructive)),
        _ => {}
    }
    if let Some(bus) = bus_read_form(program, args) {
        for exit in &mut found {
            if exit.kind == ExitKind::Bus && exit.grants == [Grant::Bus { bus }] {
                exit.bus_read = true;
            }
        }
    }
    found.extend(device_operands(args).into_iter().map(device_exit));
    found
}

/// The bus whose filtered view the simple command `words` reads, program first, when
/// it is a read form of [`bus_read_form`] (phase E): the read forms of `systemctl`,
/// `systemctl --user`, `loginctl`, `hostnamectl`, `timedatectl` and `localectl`, and
/// `notify-send` (v0.0.4), whose recorded bus methods all lie in the default rules of
/// the filtered bus proxy ([`crate::bus`]).
pub(super) fn bus_read(words: &[String]) -> Option<BusKind> {
    let (first, args) = words.split_first()?;
    bus_read_form(base_name(first), args)
}

/// The verbs of `systemctl` and `systemctl --user` whose bus methods the probe recorded
/// and the default rules hold. `systemctl` with no verb, such as `systemctl --failed`,
/// lists the units too. The other read verbs of [`SYSTEMCTL_READ`] call methods that
/// the defaults leave out, so they keep their `bus` exit.
const SYSTEMCTL_BUS_READ: &[&str] = &[
    "status",
    "list-units",
    "list-unit-files",
    "list-sockets",
    "list-timers",
    "list-jobs",
    "list-dependencies",
    "is-active",
    "is-failed",
    "is-enabled",
    "show",
    "cat",
];

/// The verbs of `hostnamectl`, `timedatectl` and `localectl` that only show the
/// service's properties. Each program with no verb shows them too.
const CTL_BUS_READ: &[&str] = &["status", "show"];

/// The verbs of `timedatectl` that only show the properties of `timesyncd`.
const TIMESYNC_BUS_READ: &[&str] = &["timesync-status", "show-timesync"];

/// The long options of the systemd tools that send the command to another host, a
/// container or another root, where the filtered bus of the call does not lead. `getopt`
/// takes any unique prefix of a long option, so a prefix counts too.
const REMOTE_LONG: &[&str] = &["host", "machine", "root", "image", "image-policy"];

/// The short options of the systemd tools that send the command to another host (`-H`)
/// or a container (`-M`).
const REMOTE_SHORT: &[char] = &['H', 'M'];

/// The bus that `program` with `args` reads through the filtered bus proxy, or `None`
/// when it is no read form of phase E (DE3): `systemctl` and `systemctl --user` with no
/// verb or a verb of [`SYSTEMCTL_BUS_READ`], `loginctl` with no verb or a verb of
/// [`LOGINCTL_READ`], and `hostnamectl`, `timedatectl` and `localectl` with no verb or
/// a verb of [`CTL_BUS_READ`] (and [`TIMESYNC_BUS_READ`] for `timedatectl`). A form
/// with an option of [`REMOTE_LONG`] or [`REMOTE_SHORT`] before or after the verb, or a
/// word that only the shell can read, is no read form. `notify-send` on the session bus
/// is one too (v0.0.4, DV18), except a form that waits for the notice to close or for
/// an action ([`notify_waits`]): the filter lets no signal back, so it would wait for
/// ever.
fn bus_read_form(program: &str, args: &[String]) -> Option<BusKind> {
    if args.iter().any(|word| word.contains([OPAQUE, '$']) || names_remote(word)) {
        return None;
    }
    match program {
        "systemctl" => {
            let bus = if has(args, "--user") { BusKind::Session } else { BusKind::System };
            match verb(args, SYSTEMCTL_VALUES) {
                None => Some(bus),
                Some(verb) if SYSTEMCTL_BUS_READ.contains(&verb) => Some(bus),
                Some(_) => None,
            }
        }
        "loginctl" => match verb(args, &[]) {
            None => Some(BusKind::System),
            Some(verb) if LOGINCTL_READ.contains(&verb) => Some(BusKind::System),
            Some(_) => None,
        },
        "hostnamectl" | "timedatectl" | "localectl" => match verb(args, &[]) {
            None => Some(BusKind::System),
            Some(verb) if CTL_BUS_READ.contains(&verb) => Some(BusKind::System),
            Some(verb) if program == "timedatectl" && TIMESYNC_BUS_READ.contains(&verb) => {
                Some(BusKind::System)
            }
            Some(_) => None,
        },
        "notify-send" if !notify_waits(args) => Some(BusKind::Session),
        _ => None,
    }
}

/// True when `notify-send` with `args` waits for a signal of the notification server:
/// `--wait` (`-w`) and `--action` (`-A`), before a `--` that ends the options. A short
/// cluster that holds `w` or `A` counts too. Stricter than the program: a value such
/// as the `w` of `-awatch` counts, which only keeps an exit.
fn notify_waits(args: &[String]) -> bool {
    for word in args {
        if word == "--" {
            return false;
        }
        if let Some(long) = word.strip_prefix("--") {
            let name = long.split_once('=').map_or(long, |(name, _)| name);
            if matches!(name, "wait" | "action") {
                return true;
            }
        } else if let Some(cluster) = word.strip_prefix('-')
            && cluster.contains(['w', 'A'])
        {
            return true;
        }
    }
    false
}

/// True when `word` is an option that sends a systemd tool to another host, container
/// or root: a long option of which a name of [`REMOTE_LONG`] starts with the given name
/// (`--host`, `--ho=box`, `--root=/mnt`), or a cluster of short options that holds
/// `-H` or `-M` (`-H`, `-Hbox`, `-qM`). Stricter than the tools: a value in a cluster,
/// such as the `M` of `-pMainPID`, counts too, which only keeps an exit.
fn names_remote(word: &str) -> bool {
    if let Some(long) = word.strip_prefix("--") {
        let name = long.split_once('=').map_or(long, |(name, _)| name);
        return !name.is_empty() && REMOTE_LONG.iter().any(|remote| remote.starts_with(name));
    }
    match word.strip_prefix('-') {
        Some(cluster) => cluster.contains(REMOTE_SHORT),
        None => false,
    }
}

/// The `outside` exit of a `rustup` form that installs a toolchain, a target or a
/// component, or updates rustup itself (DP26): in the sandbox it would write only the
/// private layer of `~/.rustup`, and the toolchain would be gone after the call. The
/// install puts programs on the user's `PATH` (`~/.cargo/bin`), which every later build
/// runs, so the line must not steer it: the classifier judges only a form whose
/// toolchains are release channels and whose targets and components are plain names,
/// and only when a user message names `rustup`, `toolchain` or the channel. Every other
/// form, such as a path, a custom toolchain name, a URL or `rustup toolchain link`, is
/// for the user. A line that sets a `RUSTUP_*` variable or `CARGO_HOME` is for the user
/// too ([`sets_rustup_env`]). Any other `rustup` form runs contained.
fn rustup(args: &[String]) -> Option<Found> {
    let form = rustup_install(args)?;
    let safe = !form.link
        && form.toolchains.iter().all(|word| rustup_channel(word))
        && form.operands.iter().all(|word| plain_name(word));
    let mut names = vec!["rustup".to_owned(), "toolchain".to_owned()];
    for word in form.toolchains.iter().filter(|word| rustup_channel(word)) {
        let channel = word.split('-').next().unwrap_or(word);
        if !names.iter().any(|name| name == channel) {
            names.push(channel.to_owned());
        }
    }
    let approver = if safe { Approver::ClassifierWhenNamed } else { Approver::User };
    Some(Found { approver, names, ..Found::of(ExitKind::Outside) })
}

/// The options of `rustup` that take the next word as their value, each with true when
/// that value is a toolchain (else a list of plain names).
const RUSTUP_VALUES: &[(&str, bool)] = &[
    ("--toolchain", true),
    ("--profile", false),
    ("-c", false),
    ("--component", false),
    ("--components", false),
    ("-t", false),
    ("--target", false),
    ("--targets", false),
];

/// The options of `rustup` that take no value and name no file, server or toolchain.
const RUSTUP_FLAGS: &[&str] = &[
    "--no-self-update",
    "--force",
    "--force-non-host",
    "--allow-downgrade",
    "--no-update-default",
    "-v",
    "--verbose",
    "-q",
    "--quiet",
    "-y",
];

/// A `rustup` form that installs, as [`rustup_install`] reads it.
struct RustupInstall {
    /// The toolchain words: the operands of `toolchain install` and `update`, a
    /// `+toolchain` word, the value of `--toolchain`.
    toolchains: Vec<String>,
    /// Every other word: targets, components, the values of the other options, and an
    /// option that efr does not know, which then fails [`plain_name`].
    operands: Vec<String>,
    /// True for `rustup toolchain link`, which installs a toolchain from a path.
    link: bool,
}

/// The words of a `rustup` form that installs (`toolchain install`, `install`, `update`,
/// `target add`, `component add`, `self update`, and `toolchain link`), or `None` for
/// any other form.
fn rustup_install(args: &[String]) -> Option<RustupInstall> {
    let mut form = RustupInstall { toolchains: Vec::new(), operands: Vec::new(), link: false };
    let mut verbs: Vec<&str> = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if let Some(toolchain) = word.strip_prefix('+') {
            form.toolchains.push(toolchain.to_owned());
            continue;
        }
        if word.starts_with('-') {
            let (option, inline) = match word.split_once('=') {
                Some((option, value)) => (option, Some(value.to_owned())),
                None => (word.as_str(), None),
            };
            match RUSTUP_VALUES.iter().find(|(name, _)| *name == option) {
                Some((_, toolchain)) => {
                    let value = inline.or_else(|| words.next().cloned()).unwrap_or_default();
                    let list = if *toolchain { &mut form.toolchains } else { &mut form.operands };
                    list.extend(value.split(',').map(str::to_owned));
                }
                None if inline.is_none() && RUSTUP_FLAGS.contains(&option) => {}
                // NOTE: an option that efr does not know may name a file or a URL.
                None => form.operands.push(word.clone()),
            }
            continue;
        }
        let installs = matches!(
            verbs.as_slice(),
            ["toolchain", "install" | "add" | "update"] | ["install" | "update"]
        );
        if installs {
            form.toolchains.push(word.clone());
        } else if verbs.len() < 2 {
            verbs.push(word);
        } else {
            form.operands.push(word.clone());
        }
    }
    match verbs.as_slice() {
        ["toolchain", "install" | "add" | "update"]
        | ["install" | "update"]
        | ["target" | "component", "add"]
        | ["self", "update"] => Some(form),
        ["toolchain", "link"] => Some(RustupInstall { link: true, ..form }),
        _ => None,
    }
}

/// True when `word` is a release channel of rustup: `stable`, `beta`, `nightly`, `1.N`
/// or `1.N.M`, with an optional date `-YYYY-MM-DD` and an optional target triple of
/// plain names. A path or a custom toolchain name is no channel.
fn rustup_channel(word: &str) -> bool {
    if !plain_name(word) {
        return false;
    }
    let mut parts = word.split('-');
    let channel = parts.next().unwrap_or_default();
    let numbered = || {
        let numbers: Vec<&str> = channel.split('.').collect();
        numbers.first() == Some(&"1")
            && (2..=3).contains(&numbers.len())
            && numbers.iter().all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    if !matches!(channel, "stable" | "beta" | "nightly") && !numbered() {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    let digits =
        |part: &&str, len: usize| part.len() == len && part.chars().all(|c| c.is_ascii_digit());
    let dated =
        rest.len() >= 3 && digits(&rest[0], 4) && digits(&rest[1], 2) && digits(&rest[2], 2);
    let triple = if dated { &rest[3..] } else { &rest[..] };
    triple.iter().all(|part| !part.is_empty() && !part.chars().all(|c| c.is_ascii_digit()))
}

/// True when `word` is a plain name of rustup, such as `rust-src` or
/// `wasm32-unknown-unknown`: letters, digits, `-`, `_` and `.`, not empty and not
/// starting with `.` or `-`. A path, a URL, an option and a word that only the shell
/// reads are no plain name.
fn plain_name(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with(['.', '-'])
        && word.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// True when `words`, one segment of a line, set a `RUSTUP_*` variable or `CARGO_HOME`:
/// an assignment word before a program or behind `env`, or the operand of `export`.
/// `rustup` would then read its toolchains, its server or its home from where the line
/// says (DP26).
pub(super) fn sets_rustup_env(words: &[String]) -> bool {
    words.iter().any(|word| {
        let word = word.trim_start_matches(['\'', '"']);
        word.split_once('=').is_some_and(|(name, _)| {
            let name = name.strip_suffix('+').unwrap_or(name);
            name.starts_with("RUSTUP_") || name == "CARGO_HOME"
        })
    })
}

/// The verbs of `loginctl` that only report.
const LOGINCTL_READ: &[&str] = &[
    "list-sessions",
    "list-users",
    "list-seats",
    "session-status",
    "user-status",
    "seat-status",
    "show-session",
    "show-user",
    "show-seat",
];

/// The host exits of a network need, which depend on the machine's egress.
struct Network<'a> {
    support: &'a AutoSupport,
    /// The host list of efr's proxy, which the engine reads under [`Egress::Proxy`].
    hosts: &'a HostList,
    /// The host and port of each URL of the command ([`line_urls`]).
    urls: Vec<(String, u16)>,
}

impl Network<'_> {
    /// The `host` exit of a program that fetches over HTTP: the whole network for one
    /// call when the machine has no proxy. Under the proxy (phase D2, DP20) a URL host
    /// of the command that the list does not allow and does not deny gets a grant of
    /// its one host and port, so the call does not fail with a 403 first; a host that
    /// the list allows or denies needs nothing, because the proxy decides it. A host
    /// that the proxy cannot serve ([`ProxyRoute::Open`]) gets the whole network. A
    /// line with no URL host has no exit.
    fn host(&self) -> Option<Found> {
        match self.support.egress {
            Egress::None => Some(open_network()),
            Egress::Proxy => {
                let mut grants: Vec<Grant> = Vec::new();
                for (host, port) in &self.urls {
                    match proxy_route(self.hosts, host, *port) {
                        ProxyRoute::Listed | ProxyRoute::Denied => {}
                        ProxyRoute::Open => return Some(open_network()),
                        ProxyRoute::Grant => {
                            let grant = Grant::Host { host: host.clone(), port: *port };
                            if !grants.contains(&grant) {
                                grants.push(grant);
                            }
                        }
                    }
                }
                (!grants.is_empty()).then(|| Found { grants, ..Found::of(ExitKind::Host) })
            }
        }
    }

    /// A remote shell to `host`, or an ssh git URL of `host`: a `host` exit with the
    /// whole network when the machine has no proxy. The proxy carries no ssh, so under
    /// it the command runs only outside the sandbox, and the host must be named
    /// ([`Found::remote_shell`]); a form that the line `steered` is for the user.
    fn remote_shell(&self, host: Option<String>, steered: bool) -> Option<Found> {
        match self.support.egress {
            Egress::None => self.host(),
            Egress::Proxy => Some(Found::remote_shell(host, steered)),
        }
    }
}

/// A `host` exit with the whole network for one call.
fn open_network() -> Found {
    Found { grants: vec![Grant::OpenNetwork], ..Found::of(ExitKind::Host) }
}

/// How a host and a port reach the network under efr's proxy (DP20, DP21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProxyRoute {
    /// The list allows the host on this port: the proxy lets it through.
    Listed,
    /// The list denies the host: the proxy refuses it, and no grant opens it.
    Denied,
    /// The proxy cannot serve it: `localhost`, an IP address (the proxy refuses an IP
    /// address that no `[[network.hosts]]` entry lists, and a local or private one in
    /// every case) or a port other than 443 and 80. Such a host needs the whole network
    /// for the call: a dev server that the user's browser must reach, a service on the
    /// host or the LAN, a client that does not use `HTTPS_PROXY`.
    Open,
    /// A grant of this one host and port for the call.
    Grant,
}

/// The route of `host:port` under the proxy's list `hosts`. `host` is in lower case
/// with no trailing dot, as [`host_and_port`] gives it.
pub(super) fn proxy_route(hosts: &HostList, host: &str, port: u16) -> ProxyRoute {
    match hosts.decide(host, port) {
        HostVerdict::Allowed { .. } => return ProxyRoute::Listed,
        HostVerdict::Denied => return ProxyRoute::Denied,
        HostVerdict::NotListed => {}
    }
    let local = host == "localhost" || host.ends_with(".localhost");
    if local || is_ip_literal(host) || !matches!(port, 443 | 80) {
        ProxyRoute::Open
    } else {
        ProxyRoute::Grant
    }
}

/// Programs that reach the network with no HTTP, whatever their words say: DNS, ping,
/// traceroute and raw sockets. The proxy carries only HTTP and TLS, so they keep the
/// whole network for one call after a review in every phase (DP20 (d)).
const RAW_NETWORK_PROGRAMS: &[&str] = &[
    "nslookup",
    "dig",
    "host",
    "drill",
    "ping",
    "ping6",
    "traceroute",
    "tracepath",
    "mtr",
    "nc",
    "ncat",
    "netcat",
    "socat",
    "telnet",
    "ftp",
];

/// Programs that fetch over HTTP whatever their words say: package runners and
/// downloaders. Under the proxy they follow the host list ([`Network::host`]).
const HTTP_NETWORK_PROGRAMS: &[&str] =
    &["checkupdates", "npx", "bunx", "pnpx", "uvx", "pipx", "aria2c", "yt-dlp"];

/// Programs that show the host's network, which only the host's namespace has. `nmcli`
/// and `iwctl` are not here: they ask NetworkManager and iwd on the system bus, so they
/// are `bus` exits ([`Found::bus_form`]).
const HOST_VIEW: &[&str] = &["ss", "ip", "iw", "netstat", "ifconfig"];

/// The options of `nmcli` before its object that only change the format of the answer
/// and take no value.
const NMCLI_FORMAT: &[&str] = &["-t", "--terse", "-p", "--pretty"];

/// The options of `nmcli` before its object that take the next word as a value and
/// only change the format of the answer.
const NMCLI_FORMAT_VALUES: &[&str] = &["-f", "--fields", "-g", "--get-values", "-c", "--colors"];

/// The forms of `nmcli` that only read, after the options of [`NMCLI_FORMAT`] and
/// [`NMCLI_FORMAT_VALUES`]: its words exactly. `nmcli general hostname NAME` and
/// `nmcli radio wifi off` change the machine, so a value after these words is no read.
const NMCLI_READ: &[&[&str]] = &[
    &[],
    &["general"],
    &["general", "status"],
    &["general", "permissions"],
    &["general", "hostname"],
    &["device"],
    &["device", "status"],
    &["device", "show"],
    &["connection"],
    &["connection", "show"],
    &["radio"],
    &["radio", "all"],
    &["radio", "wifi"],
    &["radio", "wwan"],
];

/// The forms of `iwctl` that only read: its words exactly, where `*` stands for one
/// device name. `iwctl` alone starts its interactive shell.
const IWCTL_READ: &[&[&str]] = &[
    &["device", "list"],
    &["adapter", "list"],
    &["known-networks", "list"],
    &["station", "*", "show"],
    &["station", "*", "get-networks"],
];

/// True when `program` with `args` is a read form of `nmcli` ([`NMCLI_READ`]) or
/// `iwctl` ([`IWCTL_READ`]). Any other form, an option that the lists do not name, and
/// a word that only the shell knows are no read.
fn is_bus_read(program: &str, args: &[String]) -> bool {
    if args.iter().any(|word| word.contains([OPAQUE, '$'])) {
        return false;
    }
    let same = |form: &[&str], words: &[String]| {
        form.len() == words.len()
            && form.iter().zip(words).all(|(want, word)| match *want {
                "*" => !word.starts_with('-') && !word.is_empty(),
                want => want == word,
            })
    };
    match program {
        "nmcli" => {
            let mut rest = args;
            loop {
                match rest.first().map(String::as_str) {
                    Some(word) if NMCLI_FORMAT.contains(&word) => rest = &rest[1..],
                    Some(word) if NMCLI_FORMAT_VALUES.contains(&word) && rest.len() > 1 => {
                        rest = &rest[2..];
                    }
                    _ => break,
                }
            }
            NMCLI_READ.iter().any(|form| same(form, rest))
        }
        "iwctl" => IWCTL_READ.iter().any(|form| same(form, args)),
        _ => false,
    }
}

/// Programs that talk to the desktop's display server or compositor, which can type
/// into the desktop or run commands there.
const DESKTOP_IPC: &[&str] = &[
    "xrandr",
    "swaymsg",
    "i3-msg",
    "hyprctl",
    "wlr-randr",
    "xdotool",
    "ydotool",
    "wtype",
    "wmctrl",
    "xprop",
    "xdpyinfo",
    "xclip",
    "xsel",
    "wl-copy",
    "wl-paste",
];

/// The forms of the desktop programs that only read the state of the display or reload
/// the user's own config (spec 7.1): the program, then its exact words. A word that
/// ends in `*` matches every word that starts with the rest. The classifier judges
/// these forms; every other form of a desktop program types, runs programs or is
/// unknown, and only the user approves it. The clipboard readers (`wl-paste`,
/// `xclip -o`, `xsel`) are not here: a clipboard can hold a password.
pub const DESKTOP_IPC_READ: &[(&str, &[&str])] = &[
    ("hyprctl", &["clients"]),
    ("hyprctl", &["monitors"]),
    ("hyprctl", &["workspaces"]),
    ("hyprctl", &["activewindow"]),
    ("hyprctl", &["version"]),
    ("hyprctl", &["reload"]),
    ("swaymsg", &["-t", "get_*"]),
    ("i3-msg", &["-t", "get_*"]),
    ("xrandr", &[]),
    ("xrandr", &["--verbose"]),
    ("xrandr", &["--query"]),
    ("xrandr", &["-q"]),
    ("wlr-randr", &[]),
    ("niri", &["msg", "outputs"]),
    ("niri", &["msg", "workspaces"]),
    ("niri", &["msg", "windows"]),
];

/// The desktop whose socket `program` talks to, or `None` for a program that talks to
/// no desktop socket: `ydotool` talks to its own daemon, `ydotoold`, which only the
/// user may start, so its exit opens nothing.
fn desktop_kind(program: &str) -> Option<DesktopKind> {
    Some(match program {
        "hyprctl" => DesktopKind::Hyprland,
        "swaymsg" => DesktopKind::Sway,
        "i3-msg" => DesktopKind::I3,
        "niri" => DesktopKind::Niri,
        "wlr-randr" | "wtype" | "wl-copy" | "wl-paste" => DesktopKind::Wayland,
        "xrandr" | "xdotool" | "wmctrl" | "xprop" | "xdpyinfo" | "xclip" | "xsel" => {
            DesktopKind::X11
        }
        _ => return None,
    })
}

/// The options that a form of [`DESKTOP_IPC_READ`] may carry besides its words: they
/// only change the format of the answer. Any other option, such as `hyprctl --batch`,
/// refuses the match.
const DESKTOP_READ_OPTIONS: &[&str] = &["-j", "--json", "-r", "--raw"];

/// True when `program` with `args` is a form of [`DESKTOP_IPC_READ`].
fn is_desktop_read(program: &str, args: &[String]) -> bool {
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|word| !DESKTOP_READ_OPTIONS.contains(word))
        .collect();
    DESKTOP_IPC_READ.iter().filter(|(name, _)| *name == program).any(|(_, form)| {
        form.len() == words.len()
            && form.iter().zip(&words).all(|(want, word)| match want.strip_suffix('*') {
                Some(prefix) => word.starts_with(prefix) && word.len() > prefix.len(),
                None => want == word,
            })
    })
}

/// The ecosystem whose registry the command fetches from, when it is a package
/// manager's fetch: `cargo fetch` or `cargo build`, `npm ci` or `npm install`,
/// `pip install`, `uv sync`, `go mod download`, `pacman -Sw`.
pub(super) fn package_fetch(words: &[String]) -> Option<Ecosystem> {
    let (first, args) = words.split_first()?;
    let verb = verb(args, &[]);
    match base_name(first) {
        "cargo" => matches!(
            verb,
            Some(
                "fetch"
                    | "build"
                    | "check"
                    | "test"
                    | "update"
                    | "generate-lockfile"
                    | "add"
                    | "install"
            )
        )
        .then_some(Ecosystem::Crates),
        "npm" | "pnpm" | "yarn" | "bun" => {
            matches!(verb, None | Some("ci" | "install" | "i" | "add" | "update"))
                .then_some(Ecosystem::Npm)
        }
        "pip" | "pip3" => matches!(verb, Some("install" | "download")).then_some(Ecosystem::Pypi),
        "uv" => matches!(verb, Some("sync" | "add" | "lock" | "pip")).then_some(Ecosystem::Pypi),
        "python" | "python3" if args.first().is_some_and(|arg| arg == "-m") => {
            package_fetch(&args[1..])
        }
        "go" => match verb {
            Some("mod") => matches!(second_verb(args), Some("download" | "tidy" | "vendor"))
                .then_some(Ecosystem::Go),
            Some("get" | "install" | "build" | "test") => Some(Ecosystem::Go),
            _ => None,
        },
        "pacman" => args
            .iter()
            .any(|arg| {
                arg.strip_prefix("-S").is_some_and(|letters| letters.contains('w'))
                    || arg == "--downloadonly"
            })
            .then_some(Ecosystem::Pacman),
        _ => None,
    }
}

/// The words of the paths that a destructive command of `words` deletes or overwrites,
/// or `None` when it works on the repository of its directory (`git reset --hard`).
/// An empty list means that efr cannot name the paths.
pub(super) fn destructive_targets(words: &[String]) -> Option<Vec<&str>> {
    let Some((first, args)) = words.split_first() else {
        return Some(Vec::new());
    };
    let operands = |values: &[&str]| -> Vec<&str> {
        let mut found = Vec::new();
        let mut words = args.iter();
        while let Some(word) = words.next() {
            if values.contains(&word.as_str()) {
                words.next();
            } else if !word.starts_with('-') {
                found.push(word.as_str());
            }
        }
        found
    };
    match base_name(first) {
        "git" => None,
        "dd" => Some(dd_targets(words)),
        "shred" => Some(operands(&["-n", "--iterations", "-s", "--size"])),
        "truncate" => Some(operands(&["-s", "--size", "-r", "--reference"])),
        "find" => {
            let roots: Vec<&str> = args
                .iter()
                .take_while(|word| !word.starts_with('-') && *word != "(" && *word != "!")
                .map(String::as_str)
                .collect();
            Some(if roots.is_empty() { vec!["."] } else { roots })
        }
        "rm" => Some(recursive_rm_operands(words)),
        _ => Some(Vec::new()),
    }
}

/// The device nodes that every contained call has (bwrap `--dev`).
const SANDBOX_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/console",
    "/dev/ptmx",
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/core",
];

/// The directories below `/dev` that every contained call has, with files of its own
/// at any depth.
const SANDBOX_DEVICE_DIRS: &[&str] = &["/dev/shm", "/dev/mqueue"];

/// The directories below `/dev` that every contained call has, whose entries are a
/// terminal or an open file of the process: `/dev/pts/0`, `/dev/fd/2`.
const SANDBOX_ENTRY_DIRS: &[&str] = &["/dev/pts", "/dev/fd"];

/// True when `path`, in normal form, is a device node that every contained call has,
/// or lies in such a directory: `/dev/null`, `/dev/stderr`, `/dev/fd/2`. A write there
/// stays in the sandbox and needs no exit. A path below a node, such as
/// `/dev/fd/3/x`, is not one ([`through_open_file`]).
pub(super) fn is_sandbox_device(path: &Path) -> bool {
    SANDBOX_DEVICES.iter().any(|node| path == Path::new(node))
        || SANDBOX_DEVICE_DIRS.iter().any(|dir| path.starts_with(dir))
        || SANDBOX_ENTRY_DIRS
            .iter()
            .any(|dir| path.strip_prefix(dir).is_ok_and(|entry| entry.components().count() <= 1))
}

/// True when `path`, in normal form, lies below a node that every contained call has,
/// such as `/dev/fd/3/authorized_keys` or `/dev/stdout/x`. Such a node can be a link to
/// an open file of the process, and that file can be any directory, so efr cannot tell
/// where the path leads: it is no device, and the daemon looks nothing up through it,
/// because it would follow a descriptor of its own.
pub(super) fn through_open_file(path: &Path) -> bool {
    let below = |node: &Path| path.starts_with(node) && path != node;
    SANDBOX_DEVICES.iter().any(|node| below(Path::new(node)))
        || SANDBOX_ENTRY_DIRS
            .iter()
            .any(|dir| path.strip_prefix(dir).is_ok_and(|entry| entry.components().count() > 1))
}

/// True when `path`, in normal form, lies below `/dev` and is not a node that every
/// contained call has, nor a path through one: a real device, such as `/dev/sda`, that
/// only a `device` grant opens.
pub(super) fn is_host_device(path: &Path) -> bool {
    path.starts_with("/dev")
        && path != Path::new("/dev")
        && !is_sandbox_device(path)
        && !through_open_file(path)
}

/// The `device` exit of the host device `path`.
pub(super) fn device_exit(path: PathBuf) -> Found {
    Found {
        grants: vec![Grant::Device { path: path.clone() }],
        target: Some(path),
        ..Found::of(ExitKind::Device)
    }
}

/// The operands under `/dev` that name a node the sandbox does not have, also as the
/// value of `name=value` (`dd if=/dev/sda`) or `--option=value`.
fn device_operands(args: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for arg in args {
        let value = match arg.split_once('=') {
            Some((_, value)) if !arg.starts_with("/dev/") => value,
            _ => arg.as_str(),
        };
        let Some(path) = crate::path_class::normalize(Path::new(value)) else {
            continue;
        };
        if is_host_device(&path) && !found.contains(&path) {
            found.push(path);
        }
    }
    found
}

/// True when `args` holds `word`.
fn has(args: &[String], word: &str) -> bool {
    args.iter().any(|arg| arg == word)
}

/// The first word of `args` that is not an option, past the options in `values` that
/// take the next word as their value.
fn verb<'a>(args: &'a [String], values: &[&str]) -> Option<&'a str> {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if values.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            return Some(word);
        }
    }
    None
}

/// The second word of `args` that is not an option, such as `download` in
/// `go mod download`.
fn second_verb(args: &[String]) -> Option<&str> {
    args.iter().filter(|word| !word.starts_with('-')).nth(1).map(String::as_str)
}

/// The options of `systemctl` that take the next word as their value.
const SYSTEMCTL_VALUES: &[&str] = &[
    "-t",
    "--type",
    "-p",
    "--property",
    "-H",
    "--host",
    "-M",
    "--machine",
    "-n",
    "--lines",
    "-o",
    "--output",
    "-s",
    "--signal",
    "--kill-whom",
    "--root",
    "--state",
    "--what",
    "--job-mode",
    "--timestamp",
    "--message",
    "--image",
    "--drop-in",
];

/// The verbs of `systemctl` that only report.
const SYSTEMCTL_READ: &[&str] = &[
    "list-units",
    "list-unit-files",
    "list-sockets",
    "list-timers",
    "list-paths",
    "list-jobs",
    "list-dependencies",
    "list-automounts",
    "list-machines",
    "status",
    "show",
    "cat",
    "help",
    "is-active",
    "is-failed",
    "is-enabled",
    "is-system-running",
    "get-default",
    "show-environment",
];

/// The verbs of `systemctl --user` whose change outlives the session.
const SYSTEMCTL_PERSIST: &[&str] = &[
    "enable",
    "reenable",
    "link",
    "edit",
    "set-environment",
    "unset-environment",
    "import-environment",
    "preset",
    "preset-all",
    "mask",
    "add-wants",
    "add-requires",
    "set-property",
    "revert",
    "set-default",
];

fn systemctl(args: &[String]) -> Option<Found> {
    let user = has(args, "--user");
    let bus = if user { BusKind::Session } else { BusKind::System };
    match verb(args, SYSTEMCTL_VALUES) {
        None => Some(Found::bus(bus)),
        Some(verb) if SYSTEMCTL_READ.contains(&verb) => Some(Found::bus(bus)),
        // NOTE: only the user approves it in phase A, also when a message names the unit
        // (spec 9.1); the unit's names fill the record.
        Some(verb) if user && SYSTEMCTL_PERSIST.contains(&verb) => {
            Some(Found { names: unit_names(args, verb), ..Found::of(ExitKind::Persistence) })
        }
        Some(_) if user => Some(Found::bus(bus)),
        Some(_) => Some(Found::of(ExitKind::Privilege)),
    }
}

/// The units that `systemctl` with `args` names after `verb`, each with and without
/// its suffix (`syncthing.service`, `syncthing`).
fn unit_names(args: &[String], verb: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut words = args.iter();
    let mut after_verb = false;
    while let Some(word) = words.next() {
        if SYSTEMCTL_VALUES.contains(&word.as_str()) {
            words.next();
            continue;
        }
        if word.starts_with('-') {
            continue;
        }
        if !after_verb {
            after_verb = word == verb;
            continue;
        }
        names.push(word.clone());
        match word.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => names.push(stem.to_owned()),
            _ => names.push(format!("{word}.service")),
        }
    }
    super::names::dedup(names)
}

fn git(args: &[String], network: &Network<'_>) -> Vec<Found> {
    // NOTE: the positions come from the one table of git's global options, so
    // `git --attr-source HEAD push` is a push (DL28).
    let Some(at) = git::exact(args).sub else {
        return Vec::new();
    };
    let sub = &args[at];
    let rest = &args[at + 1..];
    let destructive = || vec![Found::of(ExitKind::Destructive)];
    match sub.as_str() {
        "push" | "send-email" | "send-pack" => {
            let mut names = vec![sub.clone()];
            if let Some(remote) = rest.iter().find(|word| !word.starts_with('-')) {
                names.extend(host_of(remote).or_else(|| Some(remote.clone())));
            }
            vec![Found::upload(names)]
        }
        "fetch" | "pull" | "clone" | "ls-remote" => {
            // NOTE: an ssh URL on the line, such as `git clone git@host:o/r.git`; a
            // remote name reads its URLs from the facts (remotes.rs).
            match operands_of(rest).into_iter().find(|word| is_ssh_url(word)) {
                Some(url) => {
                    let steered = git_steers_ssh(&args[..at], rest);
                    network.remote_shell(ssh_url_host(url), steered).into_iter().collect()
                }
                None => network.host().into_iter().collect(),
            }
        }
        "remote" if matches!(verb(rest, &[]), Some("update" | "prune" | "show")) => {
            network.host().into_iter().collect()
        }
        "submodule" if rest.iter().any(|word| word == "update" || word == "sync") => {
            network.host().into_iter().collect()
        }
        "reset" if has(rest, "--hard") => destructive(),
        "clean" if rest.iter().any(|word| forces(word)) => destructive(),
        "checkout" if checkout_discards(rest) => destructive(),
        "restore" if restores_work_tree(rest) => destructive(),
        "switch" if rest.iter().any(|word| word == "--discard-changes" || forces(word)) => {
            destructive()
        }
        "stash" if matches!(verb(rest, &[]), Some("drop" | "clear")) => destructive(),
        "branch" if deletes_by_force(rest) => destructive(),
        "reflog" if matches!(verb(rest, &[]), Some("expire" | "delete")) => destructive(),
        "gc" if rest.iter().any(|word| word.starts_with("--prune")) => destructive(),
        _ => Vec::new(),
    }
}

/// The words of `rest` that are no option.
fn operands_of(rest: &[String]) -> Vec<&str> {
    rest.iter().map(String::as_str).filter(|word| !word.starts_with('-')).collect()
}

/// True when git reaches the URL `word` over ssh: `ssh://`, `git+ssh://`, `ssh+git://`,
/// or the scp form `[user@]host:path`, with a `:` before any `/`.
fn is_ssh_url(word: &str) -> bool {
    match word.split_once("://") {
        Some((scheme, _)) => matches!(scheme, "ssh" | "git+ssh" | "ssh+git"),
        None => is_remote(word),
    }
}

/// The host of the repository operand of a git push, the first word of `rest` that is
/// no option, when it is an ssh URL in the scp form (`git@host:o/r.git`). A URL with a
/// scheme names its host through [`line_hosts`].
pub(super) fn scp_url_host(rest: &[String]) -> Option<String> {
    let operand = rest.iter().find(|word| !word.starts_with('-'))?;
    (!operand.contains("://") && is_ssh_url(operand)).then(|| ssh_url_host(operand)).flatten()
}

/// The host of the ssh URL `word`, in lower case; `None` when only the shell can read
/// it.
fn ssh_url_host(word: &str) -> Option<String> {
    let host = match word.split_once("://") {
        Some((_, rest)) => {
            let authority = rest.split('/').next().unwrap_or_default();
            let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
            host.rsplit_once(':').map_or(host, |(host, _)| host).to_owned()
        }
        None => host_of(word)?,
    };
    let host = host.trim_end_matches('.').to_lowercase();
    (!host.is_empty() && !host.contains(['/', OPAQUE, '$'])).then_some(host)
}

/// True for `-f`, `--force`, or a cluster of short options that holds `f`.
fn forces(word: &str) -> bool {
    word == "--force"
        || word
            .strip_prefix('-')
            .is_some_and(|letters| !letters.starts_with('-') && letters.contains('f'))
}

/// True when `git checkout` with `args` overwrites files of the work tree: paths after
/// `--`, the operand `.`, or a forced switch.
fn checkout_discards(args: &[String]) -> bool {
    let after_dashes =
        args.iter().position(|word| word == "--").is_some_and(|at| at + 1 < args.len());
    after_dashes || has(args, ".") || args.iter().any(|word| forces(word))
}

/// True when `git restore` with `args` restores the work tree, which is the default
/// unless only `--staged` is given.
fn restores_work_tree(args: &[String]) -> bool {
    let staged = args.iter().any(|word| word == "--staged" || word == "-S");
    let worktree = args.iter().any(|word| word == "--worktree" || word == "-W");
    worktree || !staged
}

/// True when `git branch` with `args` deletes a branch whatever its state.
fn deletes_by_force(args: &[String]) -> bool {
    let delete = args.iter().any(|word| {
        word == "--delete"
            || word
                .strip_prefix('-')
                .is_some_and(|letters| !letters.starts_with('-') && letters.contains(['d', 'D']))
    });
    let force = args.iter().any(|word| {
        word == "-D"
            || word
                .strip_prefix('-')
                .is_some_and(|letters| !letters.starts_with('-') && letters.contains('D'))
    });
    force || (delete && args.iter().any(|word| forces(word)))
}

/// The verbs of the node package managers that fetch from a registry.
const NODE_FETCH: &[&str] = &[
    "install",
    "i",
    "in",
    "ci",
    "clean-install",
    "add",
    "update",
    "up",
    "upgrade",
    "outdated",
    "view",
    "info",
    "show",
    "audit",
    "exec",
    "x",
    "dlx",
    "dedupe",
    "install-test",
    "it",
    "login",
    "search",
    "create",
    "fetch",
    "import",
];

/// The note of a node package publish (DV25).
const NODE_PUBLISH: &str = "runs the project's prepublishOnly and prepack scripts";

/// The note of `cargo publish` without `--no-verify` (DV25).
const CARGO_BUILDS: &str = "builds the crate (build.rs) with your rights";

/// The note of every cargo upload verb (DV25).
const CARGO_CONFIG: &str = "reads .cargo/config.toml, which can name a credential program";

fn node(program: &str, args: &[String], network: &Network<'_>) -> Option<Found> {
    match verb(args, &["--prefix", "-C", "--dir", "--cwd", "--filter"]) {
        Some("publish") => Some(Found::publish("publish", &[NODE_PUBLISH])),
        Some("npm") if program == "yarn" && args.iter().any(|arg| arg == "publish") => {
            Some(Found::publish("publish", &[NODE_PUBLISH]))
        }
        // NOTE: `yarn` alone installs.
        None if program == "yarn" => network.host(),
        Some(verb) if NODE_FETCH.contains(&verb) => network.host(),
        _ => None,
    }
}

fn gh(args: &[String], network: &Network<'_>) -> Option<Found> {
    let mut words = args.iter().filter(|word| !word.starts_with('-'));
    let group = words.next().map(String::as_str);
    let action = words.next().map(String::as_str);
    let writes = match (group, action) {
        (Some("release"), Some("create" | "upload" | "edit" | "delete")) => true,
        (Some("pr"), Some("create" | "merge" | "comment" | "edit" | "review" | "close")) => true,
        (Some("issue"), Some("create" | "comment" | "edit" | "close")) => true,
        (Some("gist"), Some("create" | "edit")) => true,
        (Some("repo"), Some("create" | "fork" | "edit" | "delete" | "sync")) => true,
        (Some("api"), _) => args.iter().any(|word| {
            matches!(word.as_str(), "-f" | "-F" | "--field" | "--raw-field" | "--input")
                || word.starts_with("-X")
                || word.starts_with("--method")
        }),
        _ => false,
    };
    if !writes {
        return network.host();
    }
    let mut names: Vec<String> = group.into_iter().map(str::to_owned).collect();
    if group == Some("pr") {
        names.push("pull request".to_owned());
    }
    Some(Found::upload(names))
}

/// The options of `ssh` that take a value as the next word.
const SSH_VALUE_LETTERS: &str = "BbcDEeFIiJLlmOopQRSWw";

/// The options of `autossh` that take a value as the next word: those of `ssh`, and
/// `-M port` (for ssh `-M` is a flag).
const AUTOSSH_VALUE_LETTERS: &str = "BbcDEeFIiJLlMmOopQRSWw";

/// The options of `ssh` whose value can reach another host than the destination, read
/// more config, or run a local command: a config file (`-F`), a jump host (`-J`) and a
/// forward of the standard streams (`-W`).
const SSH_STEERING_LETTERS: &str = "FJW";

/// The `-o` keys of `ssh`, in lower case, that can reach another host than the
/// destination, read more config, or run a local command.
const SSH_STEERING_KEYS: &[&str] = &[
    "proxycommand",
    "proxyjump",
    "hostname",
    "localcommand",
    "permitlocalcommand",
    "knownhostscommand",
    "match",
    "include",
];

/// True when `ssh`, `mosh` or `autossh` with `args` can reach another host than its
/// destination or run a command on this machine (DP20 (e)): an option of
/// [`SSH_STEERING_LETTERS`], an `-o` with a key of [`SSH_STEERING_KEYS`], or `mosh
/// --ssh` or `--client`. ssh also reads options after the destination, up to the
/// remote command.
fn ssh_steered(program: &str, args: &[String]) -> bool {
    if program == "mosh" {
        return args.iter().take_while(|word| *word != "--").any(|word| {
            let option = word.split_once('=').map_or(word.as_str(), |(option, _)| option);
            matches!(option, "--ssh" | "--client")
        });
    }
    let values = if program == "autossh" { AUTOSSH_VALUE_LETTERS } else { SSH_VALUE_LETTERS };
    let mut words = args.iter();
    let mut destination = false;
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|letters| !letters.is_empty()) else {
            if destination {
                // NOTE: the remote command.
                break;
            }
            destination = true;
            continue;
        };
        let Some((at, letter)) = letters.char_indices().find(|(_, c)| values.contains(*c)) else {
            continue;
        };
        let inline = &letters[at + letter.len_utf8()..];
        let value = if inline.is_empty() {
            words.next().map(String::as_str).unwrap_or_default()
        } else {
            inline
        };
        if SSH_STEERING_LETTERS.contains(letter) {
            return true;
        }
        if letter == 'o' {
            let key = value.trim_start().split(['=', ' ', '\t']).next().unwrap_or_default();
            if SSH_STEERING_KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                return true;
            }
        }
    }
    false
}

/// True when the git command with the options `global` (before its subcommand) and
/// `rest` (after it) can run its ssh transport with a command or a config of the line,
/// or reach more hosts than its URL: a config value of the line (`-c`, `--config-env`,
/// `clone -c` or `--config`), another exec path, a template dir with hooks, or the
/// submodules of the clone, whose URLs come from the remote.
fn git_steers_ssh(global: &[String], rest: &[String]) -> bool {
    fn option(word: &str) -> &str {
        word.split_once('=').map_or(word, |(option, _)| option)
    }
    global.iter().any(|word| matches!(option(word), "-c" | "--config-env" | "--exec-path"))
        || rest.iter().any(|word| {
            matches!(
                option(word),
                "-c" | "--config" | "--template" | "--recurse-submodules" | "--recursive"
            )
        })
}

/// True when the segment `words` sets a variable that changes how git or ssh reach a
/// remote or which command they run on this machine: a `GIT_*` name, such as
/// `GIT_SSH_COMMAND` or `GIT_CONFIG_COUNT`, an `SSH_*` name, such as `SSH_ASKPASS`, or
/// an `AUTOSSH_*` name, such as `AUTOSSH_PATH`. It reads assignment words and the
/// words of `export` and `env` alike.
pub(super) fn sets_ssh_env(words: &[String]) -> bool {
    words.iter().any(|word| {
        let word = word.trim_start_matches(['\'', '"']);
        word.split_once('=').is_some_and(|(name, _)| {
            let name = name.strip_suffix('+').unwrap_or(name);
            ["GIT_", "SSH_", "AUTOSSH_"].iter().any(|prefix| name.starts_with(prefix))
        })
    })
}

/// The hosts that the command `words` names: the host of each URL, of each remote
/// operand of `scp`, `sftp` and `rsync`, and the destination of `ssh`, `mosh` and
/// `autossh`, each once, in lower case.
pub(super) fn line_hosts(words: &[String]) -> Vec<String> {
    let Some((first, args)) = words.split_first() else {
        return Vec::new();
    };
    let program = base_name(first);
    let mut hosts: Vec<String> =
        args.iter().filter(|arg| arg.contains("://")).filter_map(|arg| host_of(arg)).collect();
    match program {
        "scp" | "sftp" | "rsync" => {
            hosts.extend(
                args.iter().filter(|arg| !arg.contains("://")).filter_map(|arg| host_of(arg)),
            );
        }
        "ssh" | "mosh" | "autossh" => hosts.extend(ssh_destination(program, args)),
        // NOTE: a raw network program names its host as a plain operand, such as
        // `nslookup github.com` or `dig @1.1.1.1 x.test`; a word with no dot, such as a
        // port or a count, names none.
        program if RAW_NETWORK_PROGRAMS.contains(&program) => hosts.extend(
            args.iter()
                .map(|arg| arg.trim_start_matches('@'))
                .filter(|arg| !arg.starts_with(['-', '+']) && !arg.contains("://"))
                .filter(|arg| arg.contains('.') || *arg == "localhost")
                .filter_map(|arg| host_and_port(arg).map(|(host, _)| host)),
        ),
        _ => {}
    }
    // NOTE: a host that only the shell knows, such as `$HOST`, names nothing efr can
    // check.
    hosts.retain(|host| !host.contains([OPAQUE, '$']));
    super::names::dedup(hosts)
}

/// The host and port of each URL of the command `words` (an operand with `://`), each
/// once, as [`host_and_port`] reads it: port 80 for `http://`, else 443 unless the URL
/// names one. A URL host that only the shell knows, such as `$HOST`, names nothing.
pub(super) fn line_urls(words: &[String]) -> Vec<(String, u16)> {
    let mut urls: Vec<(String, u16)> = Vec::new();
    for word in words.iter().skip(1).filter(|word| word.contains("://")) {
        if word.contains([OPAQUE, '$']) {
            continue;
        }
        if let Some(url) = host_and_port(word)
            && !urls.contains(&url)
        {
            urls.push(url);
        }
    }
    urls
}

/// The host of the destination of `ssh` with `args`: the first word after the options,
/// without a user or a port.
fn ssh_destination(program: &str, args: &[String]) -> Option<String> {
    let values = if program == "autossh" { AUTOSSH_VALUE_LETTERS } else { SSH_VALUE_LETTERS };
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            return words.next().and_then(|word| destination_host(word));
        }
        if let Some(letters) = word.strip_prefix('-').filter(|letters| !letters.is_empty()) {
            let takes = letters.char_indices().find(|(_, c)| values.contains(*c));
            if takes.is_some_and(|(at, c)| at + c.len_utf8() == letters.len()) {
                words.next();
            }
            continue;
        }
        return destination_host(word);
    }
    None
}

/// The host of an `ssh` destination: `user@host`, `host`, or `ssh://user@host:port`.
fn destination_host(word: &str) -> Option<String> {
    if word.contains("://") {
        return host_of(word);
    }
    let host = word.rsplit_once('@').map_or(word, |(_, host)| host);
    let host = host.trim().trim_end_matches('.').to_lowercase();
    (!host.is_empty() && !host.contains(['/', OPAQUE, '$'])).then_some(host)
}

/// The names of an upload with `scp`, `sftp` or `rsync`: the program and the host of
/// each remote operand.
fn remote_names(program: &str, args: &[String]) -> Vec<String> {
    let mut names = vec![program.to_owned()];
    names.extend(args.iter().filter(|arg| !arg.starts_with('-')).filter_map(|arg| host_of(arg)));
    names
}

/// The names of an upload with `curl` or `wget`: the verb and the host of each URL.
fn url_names(args: &[String]) -> Vec<String> {
    let mut names = vec!["upload".to_owned()];
    names.extend(args.iter().filter(|arg| arg.contains("://")).filter_map(|arg| host_of(arg)));
    names
}

/// The host of a URL (`https://user@host:8443/x`), of a remote operand (`user@host:path`,
/// `host:path`) or of an `rsync://` address, in lower case.
fn host_of(word: &str) -> Option<String> {
    if word.contains("://") {
        return host_and_port(word).map(|(host, _)| host);
    }
    if !is_remote(word) {
        return None;
    }
    let (authority, _) = word.split_once(':')?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// True for an `rsync` operand on another host: `host:path`, `user@host:path` or
/// `rsync://`.
// NOTE: `efr-tools/src/shell_tool/prompts.rs` has a copy for its soft flag, since that
// crate cannot name this one; `remote_rows_match_the_classifier` in both crates holds
// the same rows.
pub(super) fn is_remote(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }
    if arg.starts_with("rsync://") {
        return true;
    }
    match arg.find(':') {
        Some(colon) => !arg[..colon].contains('/') && colon > 0,
        None => false,
    }
}

/// The short options of `curl` that take a value, so the letters after them are the
/// value and not options.
const CURL_VALUE_LETTERS: &str = "AbcCEeHKmorUuwxYyzPQt";

/// True when `curl` with `args` sends data: a body, a form, a file, or a method that
/// writes.
fn curl_uploads(args: &[String]) -> bool {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (long, None),
            };
            if name.starts_with("data")
                || matches!(name, "form" | "form-string" | "upload-file" | "json")
            {
                return true;
            }
            if name == "request" {
                let method = value.or_else(|| words.next().cloned()).unwrap_or_default();
                if writes(&method) {
                    return true;
                }
            }
            continue;
        }
        let Some(letters) = word.strip_prefix('-') else {
            continue;
        };
        for (at, letter) in letters.char_indices() {
            match letter {
                'd' | 'F' | 'T' => return true,
                'X' => {
                    let rest = &letters[at + 1..];
                    let method = if rest.is_empty() {
                        words.next().cloned().unwrap_or_default()
                    } else {
                        rest.to_owned()
                    };
                    if writes(&method) {
                        return true;
                    }
                    break;
                }
                letter if CURL_VALUE_LETTERS.contains(letter) => break,
                _ => {}
            }
        }
    }
    false
}

/// True for an HTTP method that sends data.
fn writes(method: &str) -> bool {
    matches!(method.to_ascii_uppercase().as_str(), "POST" | "PUT" | "PATCH")
}

/// True when `wget` with `args` sends data.
fn wget_uploads(args: &[String]) -> bool {
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word.starts_with("--post-") || word.starts_with("--body-") {
            return true;
        }
        if let Some(method) = word.strip_prefix("--method") {
            let method = match method.strip_prefix('=') {
                Some(value) => value.to_owned(),
                None => words.next().cloned().unwrap_or_default(),
            };
            if writes(&method) {
                return true;
            }
        }
    }
    false
}

/// The operands of `rm` when it removes directories with what they hold: `-r`, `-R`
/// or `--recursive`. Empty for any other `rm`.
pub(super) fn recursive_rm_operands(words: &[String]) -> Vec<&str> {
    let Some((first, args)) = words.split_first() else {
        return Vec::new();
    };
    if base_name(first) != "rm" {
        return Vec::new();
    }
    let mut recursive = false;
    let mut operands = Vec::new();
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && arg.starts_with("--") {
            recursive |= arg == "--recursive";
        } else if options && arg.starts_with('-') && arg.len() > 1 {
            recursive |= arg.contains(['r', 'R']);
        } else {
            operands.push(arg.as_str());
        }
    }
    if recursive { operands } else { Vec::new() }
}

/// The `of=` targets of `dd`, which it overwrites.
pub(super) fn dd_targets(words: &[String]) -> Vec<&str> {
    match words.split_first() {
        Some((first, args)) if base_name(first) == "dd" => {
            args.iter().filter_map(|arg| arg.strip_prefix("of=")).collect()
        }
        _ => Vec::new(),
    }
}

/// The files that `sed -i` edits in place: its operands after the script, or all of
/// them when `-e`, `-f` or a long form gives the script.
pub(super) fn sed_in_place_targets(words: &[String]) -> Vec<&str> {
    let Some((first, args)) = words.split_first() else {
        return Vec::new();
    };
    if base_name(first) != "sed" {
        return Vec::new();
    }
    let mut in_place = false;
    let mut script_given = false;
    let mut operands = Vec::new();
    let mut words = args.iter();
    while let Some(arg) = words.next() {
        if let Some(long) = arg.strip_prefix("--") {
            in_place |= long.starts_with("in-place");
            if long == "expression" || long == "file" {
                script_given = true;
                words.next();
            }
            script_given |= long.starts_with("expression=") || long.starts_with("file=");
        } else if let Some(letters) = arg.strip_prefix('-').filter(|letters| !letters.is_empty()) {
            // NOTE: `-i` takes an optional suffix attached to it, so the letters after
            // it are the suffix, not options.
            for (at, letter) in letters.char_indices() {
                match letter {
                    'i' => {
                        in_place = true;
                        break;
                    }
                    'e' | 'f' => {
                        script_given = true;
                        if at + 1 == letters.len() {
                            words.next();
                        }
                        break;
                    }
                    _ => {}
                }
            }
        } else {
            operands.push(arg.as_str());
        }
    }
    if !in_place {
        return Vec::new();
    }
    if script_given { operands } else { operands.into_iter().skip(1).collect() }
}

/// The operands of `ln`, so the engine knows which writes make a link: a link in a
/// floor is no file edit, and only the user approves it.
pub(super) fn link_paths(words: &[String]) -> Vec<&str> {
    match words.split_first() {
        Some((first, args)) if base_name(first) == "ln" => {
            args.iter().filter(|arg| !arg.starts_with('-')).map(String::as_str).collect()
        }
        _ => Vec::new(),
    }
}

/// The paths that the command makes as exactly what it names, so efrd may make them
/// first for a write grant: `tee` and `touch` make files, `mkdir`,
/// `git worktree add` and `git clone` make directories.
pub(super) fn made_paths(words: &[String]) -> Vec<(&str, bool)> {
    let Some((first, args)) = words.split_first() else {
        return Vec::new();
    };
    let operands = || args.iter().filter(|arg| !arg.starts_with('-')).map(String::as_str);
    match base_name(first) {
        "tee" | "touch" => operands().map(|path| (path, false)).collect(),
        "mkdir" => operands().map(|path| (path, true)).collect(),
        "git" => {
            let words: Vec<&str> = operands().collect();
            match words.as_slice() {
                ["worktree", "add", path, ..] => vec![(*path, true)],
                ["clone", _, path] => vec![(*path, true)],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}
