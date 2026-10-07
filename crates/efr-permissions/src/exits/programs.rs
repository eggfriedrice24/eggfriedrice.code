//! The programs and patterns of a simple command that leave the sandbox:
//! privilege, persistence, upload, the network, the buses, the desktop, devices and the
//! destructive idioms.

use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, ExitKind, Grant};

use super::PRIVILEGE_EXITS;
use super::scan::is_assignment;
use crate::command::PRIVILEGED;
use crate::{AutoSupport, Egress};

/// One exit that a simple command needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) kind: ExitKind,
    pub(super) grants: Vec<Grant>,
    pub(super) target: Option<PathBuf>,
}

impl Found {
    fn of(kind: ExitKind) -> Self {
        Found { kind, grants: Vec::new(), target: None }
    }

    fn bus(bus: BusKind) -> Self {
        Found { kind: ExitKind::Bus, grants: vec![Grant::Bus { bus }], target: None }
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

/// The exits of one command, its program first.
pub(super) fn classify(words: &[String], support: &AutoSupport) -> Vec<Found> {
    let Some((first, args)) = words.split_first() else {
        return Vec::new();
    };
    let program = base_name(first);
    let mut found = Vec::new();
    if PRIVILEGED.contains(&program) || PRIVILEGE_EXITS.contains(&program) {
        found.push(Found::of(ExitKind::Privilege));
    }
    let network = Network { support };
    match program {
        "systemctl" => found.extend(systemctl(args)),
        "loginctl" => found.push(match verb(args, &[]) {
            Some("enable-linger") => Found::of(ExitKind::Persistence),
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
        "crontab" | "at" | "batch" => found.push(Found::of(ExitKind::Persistence)),
        "passwd" | "chsh" | "chfn" | "newgrp" => found.push(Found::of(ExitKind::Privilege)),
        "git" => found.extend(git(args, &network)),
        "npm" | "pnpm" | "yarn" | "bun" => found.extend(node(program, args, &network)),
        "cargo" => found.extend(match verb(args, &["--color", "-Z", "--config"]) {
            Some("publish" | "yank" | "owner") => Some(Found::of(ExitKind::Upload)),
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
                found.extend(classify(&inner, support));
            }
        }
        "gem" => found.extend(match verb(args, &[]) {
            Some("push") => Some(Found::of(ExitKind::Upload)),
            Some("install" | "update" | "fetch") => network.host(),
            _ => None,
        }),
        "twine" => found.push(match verb(args, &[]) {
            Some("upload" | "register") => Found::of(ExitKind::Upload),
            _ => Found::of(ExitKind::Host),
        }),
        "gh" => found.extend(gh(args, &network)),
        "scp" | "sftp" => found.push(Found::of(ExitKind::Upload)),
        "rsync" if args.iter().any(|arg| is_remote(arg)) => {
            found.push(Found::of(ExitKind::Upload));
        }
        "curl" => found.extend(if curl_uploads(args) {
            Some(Found::of(ExitKind::Upload))
        } else {
            network.host()
        }),
        "wget" => found.extend(if wget_uploads(args) {
            Some(Found::of(ExitKind::Upload))
        } else {
            network.host()
        }),
        "ssh" | "mosh" | "autossh" => found.extend(network.remote_shell()),
        program if NETWORK_PROGRAMS.contains(&program) => found.extend(network.host()),
        program if HOST_VIEW.contains(&program) => found.push(Found {
            kind: ExitKind::HostView,
            grants: vec![Grant::OpenNetwork],
            target: None,
        }),
        program if DESKTOP_IPC.contains(&program) => found.push(Found::of(ExitKind::DesktopIpc)),
        "niri" if verb(args, &[]) == Some("msg") => found.push(Found::of(ExitKind::DesktopIpc)),
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
    found.extend(device_operands(args).into_iter().map(device_exit));
    found
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
}

impl Network<'_> {
    /// A `host` exit with the whole network for one call in phase 1; nothing once the
    /// proxy decides (phase 2).
    fn host(&self) -> Option<Found> {
        match self.support.egress {
            Egress::None => {
                Some(Found { kind: ExitKind::Host, grants: vec![Grant::OpenNetwork], target: None })
            }
            Egress::Proxy => None,
        }
    }

    /// A remote shell: a `host` exit in phase 1. The proxy carries no ssh, so from
    /// phase 2 it runs only outside the sandbox.
    fn remote_shell(&self) -> Option<Found> {
        match self.support.egress {
            Egress::None => self.host(),
            Egress::Proxy => Some(Found::of(ExitKind::Outside)),
        }
    }
}

/// Programs that reach the network whatever their words say.
const NETWORK_PROGRAMS: &[&str] = &[
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
    "checkupdates",
    "npx",
    "bunx",
    "pnpx",
    "uvx",
    "pipx",
    "aria2c",
    "yt-dlp",
];

/// Programs that show the host's network, which only the host's namespace has.
const HOST_VIEW: &[&str] = &["ss", "ip", "nmcli", "iw", "netstat", "ifconfig", "iwctl"];

/// Programs that talk to the desktop's display server or compositor, which can type
/// into the desktop or run commands there.
const DESKTOP_IPC: &[&str] = &[
    "xrandr",
    "swaymsg",
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

/// The directories below `/dev` that every contained call has.
const SANDBOX_DEVICE_DIRS: &[&str] = &["/dev/pts", "/dev/shm", "/dev/fd", "/dev/mqueue"];

/// True when `path`, in normal form, is a device node that every contained call has,
/// or lies in such a directory: `/dev/null`, `/dev/stderr`, `/dev/fd/2`. A write there
/// stays in the sandbox and needs no exit.
pub(super) fn is_sandbox_device(path: &Path) -> bool {
    SANDBOX_DEVICES.iter().any(|node| path == Path::new(node))
        || SANDBOX_DEVICE_DIRS.iter().any(|dir| path.starts_with(dir))
}

/// True when `path`, in normal form, lies below `/dev` and is not a node that every
/// contained call has: a real device, such as `/dev/sda`, that only a `device` grant
/// opens.
pub(super) fn is_host_device(path: &Path) -> bool {
    path.starts_with("/dev") && path != Path::new("/dev") && !is_sandbox_device(path)
}

/// The `device` exit of the host device `path`.
pub(super) fn device_exit(path: PathBuf) -> Found {
    Found {
        kind: ExitKind::Device,
        grants: vec![Grant::Device { path: path.clone() }],
        target: Some(path),
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
        Some(verb) if user && SYSTEMCTL_PERSIST.contains(&verb) => {
            Some(Found::of(ExitKind::Persistence))
        }
        Some(_) if user => Some(Found::bus(bus)),
        Some(_) => Some(Found::of(ExitKind::Privilege)),
    }
}

/// The options of `git` itself that take the next word as their value.
const GIT_VALUES: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

fn git(args: &[String], network: &Network<'_>) -> Vec<Found> {
    let mut at = 0;
    while let Some(word) = args.get(at) {
        if GIT_VALUES.contains(&word.as_str()) {
            at += 2;
        } else if word.starts_with('-') {
            at += 1;
        } else {
            break;
        }
    }
    let Some(sub) = args.get(at) else {
        return Vec::new();
    };
    let rest = &args[at + 1..];
    let destructive = || vec![Found::of(ExitKind::Destructive)];
    match sub.as_str() {
        "push" | "send-email" | "send-pack" => vec![Found::of(ExitKind::Upload)],
        "fetch" | "pull" | "clone" | "ls-remote" => network.host().into_iter().collect(),
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

fn node(program: &str, args: &[String], network: &Network<'_>) -> Option<Found> {
    match verb(args, &["--prefix", "-C", "--dir", "--cwd", "--filter"]) {
        Some("publish") => Some(Found::of(ExitKind::Upload)),
        Some("npm") if program == "yarn" && args.iter().any(|arg| arg == "publish") => {
            Some(Found::of(ExitKind::Upload))
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
    if writes { Some(Found::of(ExitKind::Upload)) } else { network.host() }
}

/// True for an `rsync` operand on another host: `host:path`, `user@host:path` or
/// `rsync://`.
fn is_remote(arg: &str) -> bool {
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
