//! The git code facts of a line (v0.0.4, DV22): what git runs outside the sandbox when
//! a git or gh line of [`FactRequest::git_code`](efr_permissions::FactRequest) reaches a
//! remote, and whether a contained call can write each part.
//!
//! For each repository, efrd asks git itself, in hardened runs with the git variables
//! of the hidden shells' environment and no ceiling, so it finds what the exit child's
//! git finds: `rev-parse` for the git dir, the common dir and the work tree (through a
//! `.git` file and a `commondir`), and `config --show-origin --list -z` with the system,
//! global, XDG, include and worktree files. The entries of efrd's own `-c` flags are
//! dropped. The parts:
//!
//! - each config key that runs a program (`efr_sandbox::is_code_key`, plus a remote
//!   URL or rewrite through a `<transport>::` helper or `ext::`, `protocol.allow`,
//!   `remote.*.vcs`, `submodule.*.update` with `!`, `gpg.ssh.defaultKeyCommand` and
//!   `hook.*.command`), as the file that holds it, and an `include.path` as its target;
//! - each hook of the effective hooks dir (`core.hooksPath`, else `<common dir>/hooks`)
//!   that is not a sample, and the program of its `#!` line;
//! - the program that each command key starts, and `ssh` from the `PATH`, resolved as
//!   the hidden shell resolves it;
//! - the same for each module git dir below `modules/`, at any depth, when the line can
//!   reach the submodules ([`efr_sandbox::module_git_dirs`]).
//!
//! A part is writable when it lies where a contained call can write ([`Places`]). A run
//! that fails or passes its time, a repository that git does not find, and a module walk
//! that stops early give `checked = false`, which the engine reads as "only you
//! approve", never as an empty list.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use efr_permissions::GitCodeAsk;
use efr_protocol::{GitCodeFact, GitCodeFacts};
use efr_scope::{Git, Home};
use efr_stdx::time::{Clock, Stopwatch};

use super::remotes::{Entry, parse_entries};
use super::{GIT_TIMEOUT, HARDENED, real_path, resolve_program};
use crate::sandbox::fs::DaemonFs;

/// The git variables of the hidden shells' environment that choose the config files
/// that git reads. A sandboxed call can never change them in the hidden shell: the
/// export filter keeps `HOME`, `XDG_*` and `GIT_*` from returning.
pub(crate) const CONFIG_VARIABLES: &[&str] =
    &["HOME", "XDG_CONFIG_HOME", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM", "GIT_CONFIG_NOSYSTEM"];

/// The most bytes of a hook's first line that efrd reads for its `#!` program.
const SHEBANG_BYTES: u64 = 256;

/// Where a contained call can write, for the `writable` of each part.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Places {
    /// Roots where a contained call can write and no pin of efr's holds: the `$SCRATCH`
    /// dirs of every conversation, the write rules and write roots of the config, and
    /// the write roots of each running contained job and of each call that ran past its
    /// timeout. A job's plan is not kept, so a git floor that it pinned counts as
    /// writable too while it runs (check VG1-1).
    pub(crate) open: Vec<PathBuf>,
    /// The registered project roots: writable, except the git floors that every plan
    /// of the project pins.
    pub(crate) projects: Vec<PathBuf>,
}

/// What [`collect`] needs besides the asks.
pub(crate) struct CodeInput<'a> {
    pub(crate) git: &'a Git,
    pub(crate) clock: &'a Arc<dyn Clock>,
    pub(crate) home: &'a Home,
    /// The values of [`CONFIG_VARIABLES`] (and `GIT_SSH_COMMAND`, `GIT_SSH`) in the hidden
    /// shell's environment; a variable that it does not hold is unset for git. `None`
    /// keeps efrd's own environment, which the hidden shells start with.
    pub(crate) env: Option<&'a BTreeMap<String, String>>,
    /// The hidden shell's `PATH`, where a program resolves.
    pub(crate) shell_path: &'a str,
    pub(crate) places: &'a Places,
}

/// The git code of the repositories of `asks`, merged; `None` when `asks` is empty.
pub(crate) async fn collect(asks: &[GitCodeAsk], input: &CodeInput<'_>) -> Option<GitCodeFacts> {
    if asks.is_empty() {
        return None;
    }
    let watch = Stopwatch::start();
    let mut facts = GitCodeFacts { checked: true, items: Vec::new() };
    for ask in asks {
        let found = repository(ask, input).await;
        facts.checked &= found.checked;
        for item in found.items {
            if !facts.items.contains(&item) {
                facts.items.push(item);
            }
        }
    }
    // NOTE: check VG1-3 of the v0.0.4 plan: the cost of the check on an outside git line.
    tracing::debug!(phase = "git_code", elapsed_ms = %watch, checked = facts.checked, items = facts.items.len(), "phase=git_code elapsed_ms={}", watch);
    Some(facts)
}

/// Where git found a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    git_dir: PathBuf,
    common_dir: PathBuf,
    work_tree: Option<PathBuf>,
}

/// The git code of the repository of one ask.
async fn repository(ask: &GitCodeAsk, input: &CodeInput<'_>) -> GitCodeFacts {
    let not_checked = |items| GitCodeFacts { checked: false, items };
    let mut globals: Vec<String> = Vec::new();
    if let Some(git_dir) = &ask.git_dir {
        globals.push(format!("--git-dir={}", git_dir.display()));
    }
    if let Some(work_tree) = &ask.work_tree {
        globals.push(format!("--work-tree={}", work_tree.display()));
    }
    let Some(found) = locate(&ask.dir, &globals, input).await else {
        return not_checked(Vec::new());
    };
    let Some(mut entries) = config(&ask.dir, &globals, input).await else {
        return not_checked(Vec::new());
    };
    if !included(&mut entries, input).await {
        return not_checked(Vec::new());
    }
    let recurse = ask.recurse || recurses(&entries);
    let shown = found.work_tree.clone().unwrap_or_else(|| found.git_dir.clone());
    let mut repos: Vec<(PathBuf, Found, Vec<Entry>)> = vec![(shown, found.clone(), entries)];
    let mut checked = true;
    if recurse {
        let common = found.common_dir.clone();
        let walked =
            tokio::task::spawn_blocking(move || efr_sandbox::module_git_dirs(&common, &DaemonFs))
                .await;
        match walked {
            Ok(Ok(modules)) => {
                for module in modules {
                    let globals = vec![format!("--git-dir={}", module.display())];
                    let Some(entries) = config(&module, &globals, input).await else {
                        checked = false;
                        continue;
                    };
                    let work_tree = module_work_tree(&module, &entries);
                    let found =
                        Found { git_dir: module.clone(), common_dir: module.clone(), work_tree };
                    repos.push((module, found, entries));
                }
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "the module git dirs of a repository are not known");
                checked = false;
            }
            Err(_) => checked = false,
        }
    }
    let env = input.env.cloned().unwrap_or_default();
    let shell_path = input.shell_path.to_owned();
    let places = input.places.clone();
    let home = input.home.path().to_path_buf();
    let items = tokio::task::spawn_blocking(move || {
        let mut items: Vec<GitCodeFact> = Vec::new();
        let pinned = pinned(&places, &repos, &home);
        let cx = Parts {
            env: &env,
            shell_path: &shell_path,
            places: &places,
            pinned: &pinned,
            home: &home,
        };
        for (shown, found, entries) in &repos {
            for item in parts(&cx, shown, found, entries) {
                if !items.contains(&item) {
                    items.push(item);
                }
            }
        }
        items
    })
    .await;
    match items {
        Ok(items) => GitCodeFacts { checked, items },
        Err(_) => not_checked(Vec::new()),
    }
}

/// The git dir, the common dir and the work tree of the repository that git finds in
/// `dir` with `globals`; `None` when git finds none or does not answer in time.
async fn locate(dir: &Path, globals: &[String], input: &CodeInput<'_>) -> Option<Found> {
    let mut args: Vec<String> = globals.to_vec();
    args.extend(
        [
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
            "--is-bare-repository",
        ]
        .map(str::to_owned),
    );
    let out = run(dir, args, input).await?;
    let text = String::from_utf8(out).ok()?;
    let mut lines = text.lines();
    let git_dir = PathBuf::from(lines.next()?);
    let common_dir = PathBuf::from(lines.next()?);
    let bare = lines.next()? == "true";
    if !git_dir.is_absolute() || !common_dir.is_absolute() {
        return None;
    }
    let work_tree = if bare {
        None
    } else {
        let mut args: Vec<String> = globals.to_vec();
        args.extend(["rev-parse", "--show-toplevel"].map(str::to_owned));
        run(dir, args, input)
            .await
            .and_then(|out| String::from_utf8(out).ok())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    Some(Found { git_dir, common_dir, work_tree })
}

/// The config entries that git reads in `dir` with `globals`, without efrd's own
/// hardening flags; `None` when git fails or does not answer in time.
async fn config(dir: &Path, globals: &[String], input: &CodeInput<'_>) -> Option<Vec<Entry>> {
    let mut args: Vec<String> = globals.to_vec();
    args.extend(["config", "--show-origin", "--includes", "--list", "-z"].map(str::to_owned));
    let out = run(dir, args, input).await?;
    let hardened: Vec<(&str, &str)> =
        HARDENED.chunks(2).filter_map(|pair| pair.get(1)?.split_once('=')).collect();
    let entries = parse_entries(&out)
        .into_iter()
        .filter(|(origin, key, value)| {
            let own =
                hardened.iter().any(|(name, set)| key.eq_ignore_ascii_case(name) && value == set);
            !(origin.starts_with("command line:") && own)
        })
        .map(|(origin, key, value)| {
            // NOTE: git names a config of the repository relative to the dir it runs in,
            // such as `file:.git/config`.
            let origin = match origin.strip_prefix("file:") {
                Some(file) if !file.starts_with('/') => {
                    let path = efr_sandbox::normalize(&dir.join(file)).unwrap_or(dir.join(file));
                    format!("file:{}", path.display())
                }
                _ => origin,
            };
            (origin, key, value)
        })
        .collect();
    Some(entries)
}

/// Adds to `entries` the entries of each include file that they name but git did not
/// read in the listing, such as an `includeIf` whose condition git did not take there
/// (check VG1-2): one more run per file, which reads it whatever its condition, so the
/// check takes the safe side. False when such a run fails.
async fn included(entries: &mut Vec<Entry>, input: &CodeInput<'_>) -> bool {
    let mut todo: Vec<PathBuf> = Vec::new();
    for (origin, key, value) in entries.iter() {
        let lower = key.to_ascii_lowercase();
        if !is_include(&lower) {
            continue;
        }
        let base = origin_file(origin).and_then(|file| file.parent().map(Path::to_path_buf));
        let Some(target) = include_target(value, base.as_deref(), input.home.path()) else {
            continue;
        };
        let read =
            entries.iter().any(|(origin, _, _)| origin_file(origin).as_ref() == Some(&target));
        if !read && target.is_file() && !todo.contains(&target) {
            todo.push(target);
        }
    }
    for target in todo {
        let Some(dir) = target.parent() else { continue };
        let file = target.display().to_string();
        let args = ["config", "--show-origin", "--includes", "--list", "-z", "--file", &file]
            .map(str::to_owned)
            .to_vec();
        let Some(out) = run(dir, args, input).await else { return false };
        entries.extend(parse_entries(&out).into_iter().map(|(origin, key, value)| {
            let origin = match origin_file(&origin) {
                Some(_) => origin,
                None => format!("file:{}", target.display()),
            };
            (origin, key, value)
        }));
    }
    true
}

/// True for a config key (lower case) that names an include file.
fn is_include(key: &str) -> bool {
    key == "include.path" || (key.starts_with("includeif.") && key.ends_with(".path"))
}

/// One hardened git run in `dir` with the hidden shells' git variables and no
/// ceiling: its standard output, or `None` when it fails or passes [`GIT_TIMEOUT`].
async fn run(dir: &Path, args: Vec<String>, input: &CodeInput<'_>) -> Option<Vec<u8>> {
    if !dir.is_dir() {
        return None;
    }
    let mut all: Vec<String> = HARDENED.iter().map(|flag| (*flag).to_owned()).collect();
    all.extend(args);
    let mut command = input.git.command(dir, input.home, &all);
    // NOTE: the exit child's git has no ceiling, so efrd finds the repository that it
    // finds, also one at the home directory.
    command.env_remove("GIT_CEILING_DIRECTORIES").stderr(Stdio::null());
    if let Some(env) = input.env {
        for name in CONFIG_VARIABLES {
            match env.get(*name) {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
    }
    let output = async move {
        let child = tokio::task::spawn_blocking(move || command.spawn())
            .await
            .map_err(std::io::Error::other)??;
        child.wait_with_output().await
    };
    match input.clock.timeout(GIT_TIMEOUT, output).await {
        Ok(Ok(output)) if output.status.success() => {
            let mut out = output.stdout;
            if out.last() == Some(&b'\n') {
                out.pop();
            }
            Some(out)
        }
        Ok(Ok(_)) => None,
        Ok(Err(error)) => {
            tracing::debug!(%error, dir = %dir.display(), "the git code check could not run git");
            None
        }
        Err(_) => {
            tracing::debug!(dir = %dir.display(), "the git code check passed its time");
            None
        }
    }
}

/// True when the config of `entries` makes git reach the submodules on its own.
fn recurses(entries: &[Entry]) -> bool {
    entries.iter().any(|(_, key, value)| {
        let key = key.to_ascii_lowercase();
        let on =
            !matches!(value.trim().to_ascii_lowercase().as_str(), "false" | "no" | "off" | "0");
        on && matches!(
            key.as_str(),
            "submodule.recurse" | "fetch.recursesubmodules" | "push.recursesubmodules"
        )
    })
}

/// The work tree of the module git dir `module`: its `core.worktree`, taken from the
/// module git dir.
fn module_work_tree(module: &Path, entries: &[Entry]) -> Option<PathBuf> {
    let value = last(entries, "core.worktree")?;
    efr_sandbox::normalize(&module.join(value.trim()))
}

/// The last value of `key` (lower case) in `entries`.
fn last<'e>(entries: &'e [Entry], key: &str) -> Option<&'e str> {
    entries
        .iter()
        .rev()
        .find(|(_, name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, _, value)| value.as_str())
}

/// The file of an entry's origin, `file:<path>`; `None` for the command line or a blob.
fn origin_file(origin: &str) -> Option<PathBuf> {
    origin.strip_prefix("file:").map(PathBuf::from).filter(|path| path.is_absolute())
}

/// What the parts of one repository need.
struct Parts<'a> {
    env: &'a BTreeMap<String, String>,
    shell_path: &'a str,
    places: &'a Places,
    pinned: &'a [PathBuf],
    home: &'a Path,
}

impl Parts<'_> {
    fn fact(&self, repo: &Path, item: String, file: PathBuf) -> GitCodeFact {
        let writable = writable(self.places, self.pinned, &file);
        GitCodeFact { repo: repo.to_path_buf(), item, file, writable }
    }

    /// The fact of the program that `command` starts, run from `dir`, for `item`.
    fn program(&self, repo: &Path, item: &str, command: &str, dir: &Path) -> Option<GitCodeFact> {
        let word = first_word(command)?;
        let resolved = resolve_program(&word, Some(dir), self.shell_path)?;
        let name = word.rsplit('/').next().unwrap_or(&word);
        Some(self.fact(repo, format!("{name} ({item})"), resolved))
    }
}

/// The parts of one repository: its code keys, its hooks and the programs they run.
fn parts(cx: &Parts<'_>, repo: &Path, found: &Found, entries: &[Entry]) -> Vec<GitCodeFact> {
    let mut items: Vec<GitCodeFact> = Vec::new();
    let dir = found.work_tree.as_deref().unwrap_or(&found.git_dir);
    let mut ssh_command = cx.env.contains_key("GIT_SSH_COMMAND") || cx.env.contains_key("GIT_SSH");
    for (origin, key, value) in entries {
        let lower = key.to_ascii_lowercase();
        if !is_code(&lower, value) {
            continue;
        }
        let holder = origin_file(origin).unwrap_or_else(|| PathBuf::from(origin.clone()));
        if is_include(&lower) {
            // NOTE: the target is a config file that git reads; a relative path is taken
            // from the dir of the file that names it.
            let base = origin_file(origin).and_then(|file| file.parent().map(Path::to_path_buf));
            if let Some(target) = include_target(value, base.as_deref(), cx.home) {
                items.push(cx.fact(repo, key.clone(), target));
            }
            items.push(cx.fact(repo, key.clone(), holder));
            continue;
        }
        items.push(cx.fact(repo, key.clone(), holder));
        if lower == "core.sshcommand" {
            ssh_command = true;
        }
        if let Some(command) = command_of(&lower, value) {
            items.extend(cx.program(repo, key, &command, dir));
        }
    }
    if !ssh_command && let Some(ssh) = resolve_program("ssh", Some(dir), cx.shell_path) {
        items.push(cx.fact(repo, "ssh (PATH)".to_owned(), ssh));
    }
    let hooks = hooks_dir(found, entries, cx.home);
    for (name, hook) in hooks_in(&hooks) {
        items.push(cx.fact(repo, format!("hooks/{name}"), hook.clone()));
        if let Some(command) = shebang(&hook) {
            items.extend(cx.program(repo, &format!("hooks/{name}"), &command, dir));
        }
    }
    items
}

/// True for a config key (lower case) with `value` that runs a program or chooses one
/// (DV22): the keys of `efr_sandbox::is_code_key` and the extra ones of this check.
fn is_code(key: &str, value: &str) -> bool {
    if efr_sandbox::is_code_key(key, value) {
        return true;
    }
    let (section, rest) = key.split_once('.').unwrap_or((key, ""));
    let variable = rest.rsplit('.').next().unwrap_or(rest);
    let helper = value.contains("::");
    match (section, variable) {
        ("remote", "url" | "pushurl") => helper,
        ("url", "insteadof" | "pushinsteadof") => helper,
        ("protocol", "allow") => true,
        ("remote", "vcs") => true,
        ("submodule", "update") => value.trim_start().starts_with('!'),
        ("gpg", "defaultkeycommand") => key == "gpg.ssh.defaultkeycommand",
        ("hook", "command") => true,
        // NOTE: `git send-email` runs these; a server with a `/` is a sendmail-like
        // program (v0.0.4, DV22).
        ("sendemail", "tocmd" | "cccmd" | "headercmd" | "sendmailcmd") => true,
        ("sendemail", "smtpserver") => value.contains('/'),
        _ => false,
    }
}

/// The command line that the code key `key` (lower case) with `value` starts, when it
/// starts one that efrd can name.
fn command_of(key: &str, value: &str) -> Option<String> {
    let value = value.trim();
    let (section, rest) = key.split_once('.')?;
    let variable = rest.rsplit('.').next().unwrap_or(rest);
    if let Some(shell) = value.strip_prefix('!') {
        return Some(shell.to_owned());
    }
    match (section, variable) {
        ("credential", "helper") => {
            let word = first_word(value)?;
            Some(if word.contains('/') {
                value.to_owned()
            } else {
                format!("git-credential-{value}")
            })
        }
        ("remote", "vcs") => Some(format!("git-remote-{value}")),
        ("remote", "uploadpack" | "receivepack") => Some(value.to_owned()),
        ("remote" | "url", "url" | "pushurl" | "insteadof" | "pushinsteadof") => {
            let (transport, address) = value.split_once("::")?;
            if transport == "ext" {
                Some(address.to_owned())
            } else {
                Some(format!("git-remote-{transport}"))
            }
        }
        ("core", "fsmonitor") if value.contains('/') => Some(value.to_owned()),
        ("sendemail", "tocmd" | "cccmd" | "headercmd" | "sendmailcmd") => Some(value.to_owned()),
        ("sendemail", "smtpserver") if value.contains('/') => Some(value.to_owned()),
        (
            "core" | "sequence" | "diff" | "gpg" | "uploadpack" | "filter" | "merge" | "hook",
            "sshcommand"
            | "pager"
            | "editor"
            | "askpass"
            | "gitproxy"
            | "external"
            | "program"
            | "packobjectshook"
            | "alternaterefscommand"
            | "textconv"
            | "command"
            | "driver"
            | "clean"
            | "smudge"
            | "process"
            | "defaultkeycommand",
        ) => Some(value.to_owned()),
        _ => None,
    }
}

/// The first word of the shell command `command`, without quotes, past `env` and its
/// assignments; `None` when only the shell can read it.
fn first_word(command: &str) -> Option<String> {
    let mut words = command.split_whitespace().map(|word| word.trim_matches(['\'', '"']));
    loop {
        let word = words.next()?;
        if word.contains('$') || word.contains('`') {
            return None;
        }
        let base = word.rsplit('/').next().unwrap_or(word);
        if base == "env" || (word.contains('=') && !word.contains('/')) || word.starts_with('-') {
            continue;
        }
        return Some(word.to_owned());
    }
}

/// The program of the `#!` line of the hook `path`, with `env`'s program word.
fn shebang(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut head = String::new();
    file.take(SHEBANG_BYTES).read_to_string(&mut head).ok()?;
    let line = head.lines().next()?.strip_prefix("#!")?;
    Some(line.trim().to_owned())
}

/// The target of an `include.path` value: `~/` from the home, a relative path from
/// `base`, the dir of the file that names it.
fn include_target(value: &str, base: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let value = value.trim();
    let path = if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else if value.starts_with('/') {
        PathBuf::from(value)
    } else {
        base?.join(value)
    };
    efr_sandbox::normalize(&path)
}

/// The hooks dir that git uses for `found`: `core.hooksPath` (`~/` from the home, a
/// relative path from the work tree, or from the git dir of a bare repository), else
/// the `hooks` of the common dir.
fn hooks_dir(found: &Found, entries: &[Entry], home: &Path) -> PathBuf {
    let Some(value) = last(entries, "core.hookspath").map(str::trim).filter(|v| !v.is_empty())
    else {
        return found.common_dir.join("hooks");
    };
    let path = if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else if value.starts_with('/') {
        PathBuf::from(value)
    } else {
        found.work_tree.as_deref().unwrap_or(&found.git_dir).join(value)
    };
    efr_sandbox::normalize(&path).unwrap_or(path)
}

/// Each hook in `dir` that is not a sample, by name, sorted.
fn hooks_in(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut hooks: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let kind = entry.file_type().ok()?;
            let file = kind.is_file() || kind.is_symlink();
            (file && !name.ends_with(".sample")).then(|| (name, entry.path()))
        })
        .collect();
    hooks.sort();
    hooks
}

/// The git floors that every plan pins for the registered projects of `places` among
/// `repos`: `config`, `config.worktree` and `hooks` of the top `.git` dir of each
/// project, of each of its worktree git dirs and of each module git dir below it, and
/// the include files and hooks paths that a pinned config names.
fn pinned(places: &Places, repos: &[(PathBuf, Found, Vec<Entry>)], home: &Path) -> Vec<PathBuf> {
    let mut git_dirs: Vec<PathBuf> = Vec::new();
    for root in &places.projects {
        let top = root.join(".git");
        if !std::fs::symlink_metadata(&top).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(top.join("worktrees")) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    git_dirs.push(entry.path());
                }
            }
        }
        // NOTE: a walk that stops early pins no module here: the plan then cannot
        // start, and efrd takes the safe side.
        if let Ok(modules) = efr_sandbox::module_git_dirs(&top, &DaemonFs) {
            git_dirs.extend(modules);
        }
        git_dirs.push(top);
    }
    let mut pins: Vec<PathBuf> = Vec::new();
    for git_dir in &git_dirs {
        for name in ["config", "config.worktree", "hooks"] {
            pins.push(git_dir.join(name));
        }
    }
    let surface: Vec<PathBuf> = pins.clone();
    for (_, found, entries) in repos {
        for (origin, key, value) in entries {
            let Some(file) = origin_file(origin) else { continue };
            if !surface.contains(&file) {
                continue;
            }
            let lower = key.to_ascii_lowercase();
            if is_include(&lower) {
                pins.extend(include_target(value, file.parent(), home));
            }
        }
        if let Some((origin, _, _)) =
            entries.iter().rev().find(|(_, key, _)| key.eq_ignore_ascii_case("core.hookspath"))
            && origin_file(origin).is_some_and(|file| surface.contains(&file))
        {
            pins.push(hooks_dir(found, entries, home));
        }
    }
    pins.into_iter().map(|pin| real_path(&pin).unwrap_or(pin)).collect()
}

/// True when a contained call can write `file`, by its own path or by its real path.
fn writable(places: &Places, pinned: &[PathBuf], file: &Path) -> bool {
    if !file.is_absolute() {
        return false;
    }
    let real = real_path(file);
    [Some(file.to_path_buf()), real].into_iter().flatten().any(|path| {
        let open = places.open.iter().any(|root| path.starts_with(root));
        let project = places.projects.iter().any(|root| path.starts_with(root));
        open || (project && !pinned.iter().any(|pin| path.starts_with(pin)))
    })
}

#[cfg(test)]
mod tests;
