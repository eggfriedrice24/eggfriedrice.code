//! REPLs and shells in a container: programs whose prompt reads command lines like a
//! shell's, though they are not one.
//!
//! A REPL counts when it gets neither a script nor code to run and reads its input from
//! the terminal, or when an option asks for its prompt anyway (`python -i`). A
//! container's command counts when `exec` gives it a terminal (`-it`) and it is a shell,
//! a REPL or missing.

use super::{argv_starts_shell, parse, parse_all};

/// True when `program` with `args` leaves a REPL or a shell in a container at its
/// prompt; `None` when `program` is neither a REPL nor a container's client.
pub(super) fn starts_repl(
    program: &str,
    args: &[&str],
    terminal_input: bool,
    depth: usize,
) -> Option<bool> {
    let starts = match program {
        "node" | "nodejs" => node(args, terminal_input),
        "irb" => irb(args, terminal_input),
        "ghci" => ghci(args, terminal_input),
        "psql" => psql(args, terminal_input),
        "mysql" | "mariadb" => mysql(args, terminal_input),
        "sqlite3" => sqlite3(args, terminal_input),
        "docker" | "podman" => container_exec(args, terminal_input, depth),
        "kubectl" => kubectl_exec(args, terminal_input, depth),
        name if versioned(name, "python") => python(args, terminal_input),
        name if versioned(name, "lua") => lua(args, terminal_input),
        _ => return None,
    };
    Some(starts)
}

/// True for `name` and its versioned forms, such as `python3` and `python3.12`.
fn versioned(program: &str, name: &str) -> bool {
    program
        .strip_prefix(name)
        .is_some_and(|version| version.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

fn python(args: &[&str], terminal_input: bool) -> bool {
    let parsed = parse(args, "cmWX", &["check-hash-based-pycs"]);
    if parsed.has('i') {
        return true;
    }
    let runs_nothing = parsed.has('V') || parsed.has('h') || parsed.has_long("version");
    !runs_nothing
        && !parsed.has('c')
        && !parsed.has('m')
        && no_script(&parsed.operands)
        && terminal_input
}

fn node(args: &[&str], terminal_input: bool) -> bool {
    let parsed = parse(
        args,
        "epr",
        &["eval", "print", "require", "import", "input-type", "env-file", "title", "loader"],
    );
    if parsed.has('i') || parsed.has_long("interactive") {
        return true;
    }
    let runs_code = ['e', 'p', 'c', 'v', 'h'].into_iter().any(|letter| parsed.has(letter))
        || ["eval", "print", "check", "version", "help", "test", "run"]
            .into_iter()
            .any(|name| parsed.has_long(name));
    !runs_code && no_script(&parsed.operands) && terminal_input
}

fn irb(args: &[&str], terminal_input: bool) -> bool {
    let parsed = parse(args, "Ir", &["prompt", "prompt-mode", "inf-ruby-mode"]);
    let runs_nothing = parsed.has('v') || parsed.has_long("version") || parsed.has_long("help");
    !runs_nothing && parsed.operands.is_empty() && terminal_input
}

/// ghci gives its prompt after loading the modules it is given, unless `-e` evaluates
/// an expression. Its options are words of several letters after one dash, so they are
/// not read as letters.
fn ghci(args: &[&str], terminal_input: bool) -> bool {
    let runs_nothing = args
        .iter()
        .any(|arg| matches!(*arg, "-e" | "--version" | "--numeric-version" | "--help" | "--info"));
    !runs_nothing && terminal_input
}

fn lua(args: &[&str], terminal_input: bool) -> bool {
    let parsed = parse(args, "el", &[]);
    if parsed.has('i') {
        return true;
    }
    !parsed.has('e') && !parsed.has('v') && no_script(&parsed.operands) && terminal_input
}

fn psql(args: &[&str], terminal_input: bool) -> bool {
    let parsed = parse_all(
        args,
        "cdfhpUvoPLTFR",
        &[
            "command",
            "dbname",
            "file",
            "host",
            "port",
            "username",
            "variable",
            "set",
            "output",
            "pset",
            "log-file",
            "table-attr",
            "field-separator",
            "record-separator",
        ],
    );
    let runs_nothing = ['c', 'f', 'l', 'V', '?'].into_iter().any(|letter| parsed.has(letter))
        || ["command", "file", "list", "version", "help"]
            .into_iter()
            .any(|name| parsed.has_long(name));
    !runs_nothing && terminal_input
}

fn mysql(args: &[&str], terminal_input: bool) -> bool {
    // NOTE: `-p` takes its password only attached (`-psecret`), and `-p` alone asks for
    // it, so the word is left out rather than read as letters.
    let args: Vec<&str> = args.iter().copied().filter(|arg| !arg.starts_with("-p")).collect();
    let parsed = parse_all(
        &args,
        "ehuPDS",
        &["execute", "host", "user", "port", "database", "socket", "login-path"],
    );
    let runs_nothing = ['e', 'V', '?'].into_iter().any(|letter| parsed.has(letter))
        || ["execute", "version", "help"].into_iter().any(|name| parsed.has_long(name));
    !runs_nothing && terminal_input
}

/// sqlite3 takes a database and then SQL to run instead of its prompt. Its options are
/// words after one dash, anywhere before the SQL.
fn sqlite3(args: &[&str], terminal_input: bool) -> bool {
    const VALUED: &[&str] =
        &["cmd", "init", "separator", "newline", "nullvalue", "mmap", "vfs", "maxsize", "heap"];
    let mut operands = 0;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let Some(name) = word.strip_prefix('-').map(|name| name.trim_start_matches('-')) else {
            operands += 1;
            continue;
        };
        if matches!(name, "version" | "help") {
            return false;
        }
        if VALUED.contains(&name) {
            words.next();
        }
    }
    operands <= 1 && terminal_input
}

/// `docker exec` and `podman exec`, also as `container exec`, with a terminal (`-it`)
/// for a shell, a REPL or no command.
fn container_exec(args: &[&str], terminal_input: bool, depth: usize) -> bool {
    let global = parse(
        args,
        "cHl",
        &["config", "context", "host", "log-level", "connection", "url", "identity", "root"],
    );
    let rest = match global.operands.as_slice() {
        ["exec", rest @ ..] | ["container", "exec", rest @ ..] => rest,
        _ => return false,
    };
    let parsed = parse(rest, "euw", &["env", "env-file", "user", "workdir", "detach-keys"]);
    let interactive = parsed.has('i') || parsed.has_long("interactive");
    let tty = parsed.has('t') || parsed.has_long("tty");
    let detached = parsed.has('d') || parsed.has_long("detach");
    if !(interactive && tty && terminal_input) || detached {
        return false;
    }
    match parsed.operands.split_first() {
        Some((_, [])) => true,
        Some((_, command)) => argv_starts_shell(command, true, depth),
        None => false,
    }
}

/// `kubectl exec` with a terminal (`-it`) for a shell, a REPL or no command, which
/// follows `--`.
fn kubectl_exec(args: &[&str], terminal_input: bool, depth: usize) -> bool {
    let (options, after) = match args.iter().position(|arg| *arg == "--") {
        Some(at) => (&args[..at], Some(&args[at + 1..])),
        None => (args, None),
    };
    let parsed = parse_all(
        options,
        "cfns",
        &["container", "filename", "namespace", "server", "context", "kubeconfig", "cluster"],
    );
    let ["exec", _pod, before @ ..] = parsed.operands.as_slice() else {
        return false;
    };
    let interactive = parsed.has('i') || parsed.has_long("stdin");
    let tty = parsed.has('t') || parsed.has_long("tty");
    if !(interactive && tty && terminal_input) {
        return false;
    }
    let command = after.unwrap_or(before);
    command.is_empty() || argv_starts_shell(command, true, depth)
}

/// True when a REPL gets no script, or `-`, which on a terminal is the terminal.
fn no_script(operands: &[&str]) -> bool {
    matches!(operands, [] | ["-", ..])
}
