//! The lines of the `auto` sandbox: the status line, `efr sandbox check` and
//! `efr sandbox explain`, the exit lines of an approval question, the fallback note,
//! the quarantine question and the notes about files that run code.
//!
//! Plain text only: the view and the commands choose the tone and where it goes. Every
//! path and every text from the daemon or the model passes through [`one_line`], so it
//! cannot drive the terminal or fake a line of a question; each line of a command of
//! several passes through [`command_line`] after its number.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{
    AdminSandboxCheckResult, BlockReason, Blocked, BusKind, BusProxyOff, BusRefusal, ChainStep,
    CheckOutcome, DesktopKind, ExitInfo, ExitKind, ExitRecord, ExitSource, GitCodeFacts, Grant,
    JudgeKind, Launch, Mode, ModeFallback, PathClassName, PauseReason, Presence, PresenceSource,
    ProgramFact, ReportedFile, Risk, SandboxExplainResult, SandboxStatus, SurfaceChange,
    TargetFact, Verdict,
};

use super::card::{self, Card, Row};
use super::{Tone, command_line, numbered_lines, one_line, run_heading};

/// The line under an unsandboxed exit: the exit child runs the whole line with every
/// right the user has.
const FULL_RIGHTS: &str = "the whole line runs with your full rights (files, secrets, network)";

/// The risk line of a card for a run outside the sandbox.
const RISK: &str = "runs with your full rights: files, secrets, network";

/// The mark of a program that a sandboxed call wrote or may have written.
const UNTRUSTED: &str = "untrusted: written in the sandbox";

/// The dim trace of the first contained call of a turn: where it can write. A call
/// with no proxy has no network in the sandbox.
pub(crate) const CONTAINED_IN_PROJECT: &str =
    "sandbox: writes in the project, $SCRATCH, private /tmp; no network";

/// The same trace for a turn outside a registered project.
pub(crate) const CONTAINED: &str = "sandbox: writes in $SCRATCH, private /tmp; no network";

/// What the trace says in place of `no network` when a contained call of the turn ran
/// with efr's network proxy (`SandboxSummary::proxied`, phase D2).
const PROXY_TRACE: &str = "network through efr's proxy";

/// What the trace says in place of `no network` when a contained call of the turn had
/// the whole network (`Grant::OpenNetwork`).
const FULL_TRACE: &str = "full network";

/// What the trace adds when a contained call of the turn had the network view.
const NET_VIEW_TRACE: &str = "host network view (no sending)";

/// The network that the contained calls of a turn had, the most that one call had: the
/// trace names it (v0.0.4, DV21).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TraceNetwork {
    /// No network.
    #[default]
    None,
    /// efr's network proxy (`SandboxSummary::proxied`), also for a job's start.
    Proxy,
    /// The whole network (`Grant::OpenNetwork`).
    Full,
}

/// The dim trace at the end of a turn with a contained call. It says where the calls
/// could write: [`CONTAINED_IN_PROJECT`] in a registered project, else [`CONTAINED`].
/// When the turn wrote in roots that a client names (`wrote`, the `shown` names of the
/// turn's undo roots but `$SCRATCH`), it names them first: `sandbox: wrote in app and
/// lib; could write $SCRATCH, private /tmp; ...`. Then `network` in place of `no
/// network`, and the network view when a call of the turn had it (`Grant::NetView`),
/// because that call saw the host's network. Each name passes through [`one_line`].
pub(crate) fn contained_trace(
    in_project: bool,
    net_view: bool,
    network: TraceNetwork,
    wrote: &[String],
) -> String {
    let mut trace = match wrote {
        [] => if in_project { CONTAINED_IN_PROJECT } else { CONTAINED }.to_owned(),
        names => {
            let names: Vec<String> = names.iter().map(|name| one_line(name)).collect();
            format!(
                "sandbox: wrote in {}; could write $SCRATCH, private /tmp; no network",
                and_list(&names)
            )
        }
    };
    match network {
        TraceNetwork::None => {}
        TraceNetwork::Proxy => trace = trace.replace("no network", PROXY_TRACE),
        TraceNetwork::Full => trace = trace.replace("no network", FULL_TRACE),
    }
    if net_view { format!("{trace}; {NET_VIEW_TRACE}") } else { trace }
}

/// `a`, `a and b`, or `a, b and c`.
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// The quarantine question in one line, for `efr history`.
pub(crate) const SURFACE_QUESTION: &str =
    "question: the last command changed git settings that run programs";

/// The title of the quarantine question's card.
const SURFACE_TITLE: &str = "keep the git settings that the last command changed";

/// `path` with the home directory written as `~`, safe to print.
pub(crate) fn tilde(path: &Path, home: Option<&Path>) -> String {
    let shown = match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    };
    one_line(&shown)
}

/// The value of the `sandbox` row of `efr status`: `ready (Landlock ABI 10, ...)` or
/// `unavailable: <reason>; auto runs as cautious`.
pub(crate) fn status_line(status: &SandboxStatus) -> String {
    if !status.available {
        let reason = status.reason.as_deref().unwrap_or("the probe gave no reason");
        return format!("unavailable: {}; auto runs as cautious", one_line(reason));
    }
    let mut parts = Vec::new();
    if let Some(abi) = status.landlock_abi {
        parts.push(format!("Landlock ABI {abi}"));
    }
    if let Some(version) = &status.bwrap_version {
        parts.push(format!("bubblewrap {}", one_line(version)));
    }
    parts.push(format!("caches {}", status.cache_mode.as_str()));
    parts.push(format!("network {}", network_words(status)));
    format!("ready ({})", parts.join(", "))
}

/// The start of efrd's warning when `network.mode = "proxy"` but the probe's proxy
/// self-test failed.
const PROXY_WARNING: &str = "efr's network proxy is off";

/// What network a contained call has, for `efr status` and `efr sandbox check`:
/// `proxy` (efr's proxy, phase D2), `none`, or `none (proxy self-test failed)` when
/// `network.mode = "proxy"` but the probe's proxy self-test failed, so efrd fell back.
pub(crate) fn network_words(status: &SandboxStatus) -> String {
    let fell_back = status.warnings.iter().any(|warning| warning.starts_with(PROXY_WARNING));
    match status.network_mode.as_str() {
        "none" if fell_back => "none (proxy self-test failed)".to_owned(),
        mode => mode.to_owned(),
    }
}

/// The start of the probe's warning when the network view's self-test failed.
const NETVIEW_WARNING: &str = "the network view's self-test failed";

/// The probe's warning about the network view, when its self-test failed.
fn netview_warning(status: &SandboxStatus) -> Option<&str> {
    status.warnings.iter().map(String::as_str).find(|warning| warning.starts_with(NETVIEW_WARNING))
}

/// The value of the `netview` row of `efr status`: `off` when the user's
/// `network.netview` is false; `yes` when the probe's network view self-test passed,
/// so `ss` and `ip` read forms run in the sandbox with no review; else `no` with the
/// reason.
pub(crate) fn netview_words(status: &SandboxStatus) -> String {
    if status.netview_off {
        return "off (network.netview = false)".to_owned();
    }
    if status.netview {
        return "yes".to_owned();
    }
    let reason = netview_warning(status).unwrap_or("the probe gave no result for it");
    format!("no ({})", one_line(reason))
}

/// The note at the start of a turn whose mode fell back, such as `auto is not
/// available here; this turn runs as cautious: Landlock ABI 6 found`. A turn that asked
/// for full mode names only why, because full mode itself needs no sandbox: `this turn
/// runs as auto: a turn that a job notice starts runs with at most auto`.
pub(crate) fn fallback_note(fallback: &ModeFallback, mode: Mode) -> String {
    let reason = one_line(&fallback.reason);
    if fallback.asked == Mode::Full {
        return format!("this turn runs as {mode}: {reason}");
    }
    format!("{} is not available here; this turn runs as {mode}: {reason}", fallback.asked)
}

/// The warning when the user chooses `auto` and the sandbox is not available.
pub(crate) fn fallback_warning(reason: &str) -> String {
    format!(
        "auto needs the sandbox; turns run as cautious: {}. efr sandbox check shows more.",
        one_line(reason.trim_end_matches('.'))
    )
}

/// A time in microseconds as milliseconds with one decimal, such as `3.6 ms`.
fn millis(us: u64) -> String {
    format!("{}.{} ms", us / 1000, us % 1000 / 100)
}

/// The text of `efr sandbox check`: each check with its outcome, the fix of a failed
/// one, the warnings that no check gave, the launch cost and the verdict.
pub(crate) fn check(result: &AdminSandboxCheckResult) -> String {
    let mut out = String::new();
    let mut fixes = Vec::new();
    for check in &result.checks {
        let word = match check.outcome {
            CheckOutcome::Ok => "ok",
            CheckOutcome::Warn => "warn",
            CheckOutcome::Fail => "fail",
            CheckOutcome::Skipped => "skip",
            _ => "?",
        };
        let detail = match (&check.detail, check.outcome) {
            (Some(detail), _) => one_line(detail),
            (None, CheckOutcome::Skipped) => {
                format!("{}: an earlier check failed", one_line(&check.name))
            }
            (None, _) => one_line(&check.name),
        };
        let _ = writeln!(out, "{word:<5} {detail}");
        if let Some(fix) = &check.fix {
            let _ = writeln!(out, "      fix: {}", one_line(fix));
            fixes.push(fix.as_str());
        }
    }
    for warning in &result.status.warnings {
        let shown =
            result.checks.iter().any(|check| check.detail.as_deref() == Some(warning.as_str()));
        if !shown {
            let _ = writeln!(out, "warn  {}", one_line(warning));
        }
    }
    match (result.launch_us, result.snapshot_launch_us) {
        (Some(launch), Some(snapshot)) => {
            let _ = writeln!(
                out,
                "launch {}, with your rc snapshot {}",
                millis(launch),
                millis(snapshot)
            );
        }
        (Some(launch), None) => {
            let _ = writeln!(out, "launch {}", millis(launch));
        }
        _ => {}
    }
    if result.status.available {
        // NOTE: a failed self-test of the network view printed its warning above.
        let status = &result.status;
        let netview = match (status.netview, netview_warning(status)) {
            (false, Some(_)) if !status.netview_off => "no (the warning above says why)".to_owned(),
            _ => netview_words(status),
        };
        let _ = writeln!(out, "netview {netview}");
        let _ = writeln!(out, "network {}", network_words(status));
        let _ = writeln!(out, "bus filter {}", bus_filter_words(status));
        out.push_str("auto: ready\n");
    } else {
        let reason = result.status.reason.as_deref().unwrap_or("the probe gave no reason");
        let _ = writeln!(out, "auto: unavailable: {}; turns run as cautious", one_line(reason));
        if let Some(fix) = result.status.fix.as_deref().filter(|fix| !fixes.contains(fix)) {
            let _ = writeln!(out, "fix: {}", one_line(fix));
        }
    }
    out
}

/// A kind of exit in words, such as `synced write`.
fn kind_words(kind: ExitKind) -> String {
    kind.as_str().replace('_', " ")
}

/// The text of `efr sandbox explain`: the path, the project and the mode, then whether
/// a contained command can read and write it, and why.
pub(crate) fn explain(result: &SandboxExplainResult, home: Option<&Path>) -> String {
    let path = tilde(&result.path, home);
    let place = match &result.project {
        Some(project) => format!("from {}", tilde(project, home)),
        None => "outside a project".to_owned(),
    };
    let reason = one_line(&result.reason);
    let mut out = format!("{path}, {place} in {}:\n", result.mode);
    if result.read {
        out.push_str("  read   yes\n");
    } else {
        let _ = writeln!(out, "  read   no: {reason}");
    }
    let mut write = if result.write { "yes".to_owned() } else { "no".to_owned() };
    // The reason goes on the first line that it explains.
    if result.read {
        let _ = write!(write, ": {reason}");
    }
    if let Some(kind) = result.write_exit.filter(|_| !result.write) {
        if kind.is_floor() {
            let _ = write!(write, "; a write is denied ({})", kind_words(kind));
        } else {
            let _ = write!(write, "; a write is a {} exit", kind_words(kind));
            // NOTE: `user_only` is only the default approver of the kind; when the
            // reviewer may judge a named file, the daemon's reason says so.
            if kind.user_only() && !reason.contains("the reviewer") {
                write.push_str(", user only");
            }
        }
    }
    let _ = writeln!(out, "  write  {write}");
    out
}

/// What a grant opens, such as `full network` or `~/notes writable`.
fn grant_words(grant: &Grant, home: Option<&Path>) -> String {
    match grant {
        Grant::Write { path } => format!("{} writable", tilde(path, home)),
        Grant::Host { host, port } => format!("{}:{port} reachable", one_line(host)),
        Grant::OpenNetwork => "full network".to_owned(),
        Grant::Socket { path } => format!("the socket {}", tilde(path, home)),
        Grant::Bus { bus } => bus_words(*bus).to_owned(),
        Grant::Desktop { desktop } => desktop_words(*desktop),
        Grant::Device { path } => format!("the device {}", tilde(path, home)),
        Grant::Unmask { path } => format!("{} readable", tilde(path, home)),
        Grant::NetView => "host network view (no sending)".to_owned(),
        Grant::BusFilter { bus } => format!("filtered bus: {}", bus_words(*bus)),
        Grant::Unknown => "unknown grant".to_owned(),
        _ => "more access".to_owned(),
    }
}

/// The socket of a desktop grant, such as `the Hyprland socket`.
fn desktop_words(desktop: DesktopKind) -> String {
    format!("the {} socket", desktop.name())
}

/// A `desktop_ipc` exit in words: the socket of its grant, else `the desktop` (an exit
/// recorded before efr named the desktop, or a program with no socket), and what the
/// line does with it. A read or reload form, such as `hyprctl monitors`, says so; any
/// other form can type and run commands.
fn desktop_part(grants: &[Grant], record: Option<&ExitRecord>) -> String {
    let socket = grants
        .iter()
        .find_map(|grant| match grant {
            Grant::Desktop { desktop } => Some(desktop_words(*desktop)),
            _ => None,
        })
        .unwrap_or_else(|| "the desktop".to_owned());
    if record.is_some_and(|record| record.facts.desktop_read) {
        format!("{socket}, to read its state or reload it")
    } else {
        format!("{socket} (it can type and run commands)")
    }
}

fn bus_words(bus: BusKind) -> &'static str {
    match bus {
        BusKind::System => "the system bus",
        BusKind::Session => "the session bus",
        _ => "a message bus",
    }
}

/// How long the grants of a "yes" stay open: `for this call`, or for a background job
/// `for the whole job (up to N minutes)`.
fn grants_last(record: Option<&ExitRecord>) -> String {
    match record.map(|record| &record.action) {
        Some(action) if action.background => match action.max_minutes {
            Some(1) => "for the whole job (up to 1 minute)".to_owned(),
            Some(minutes) => format!("for the whole job (up to {minutes} minutes)"),
            None => "for the whole job".to_owned(),
        },
        _ => "for this call".to_owned(),
    }
}

/// The title of the question about a command that full mode still asks for.
const FULL_TITLE: &str = "allow this command, with no sandbox";

/// How a command of full mode runs after a "yes".
const FULL_RUNS: &str = "runs as you, with no sandbox";

/// True for a `shell` call that runs direct: only full mode runs a command line with no
/// sandbox and no launcher (a file tool also runs direct, in every mode).
fn full_shell(info: &ExitInfo, record: Option<&ExitRecord>) -> bool {
    matches!(info.launch, Launch::Direct)
        && record.is_some_and(|record| record.action.tool == "shell")
}

/// How a call runs after a "yes", as the end of the exit line.
fn launch_words(info: &ExitInfo, record: Option<&ExitRecord>, home: Option<&Path>) -> String {
    match &info.launch {
        Launch::Unsandboxed => "runs outside the sandbox".to_owned(),
        Launch::Contained { grants } if grants.is_empty() => "runs in the sandbox".to_owned(),
        Launch::Contained { grants } => {
            let opened: Vec<String> = grants.iter().map(|grant| grant_words(grant, home)).collect();
            format!("runs in the sandbox with {} {}", opened.join(", "), grants_last(record))
        }
        Launch::Direct if full_shell(info, record) => FULL_RUNS.to_owned(),
        // NOTE: in auto only a file tool runs direct: efrd reads or writes the path.
        Launch::Direct => "the file tool runs outside the sandbox".to_owned(),
        _ => "runs as typed".to_owned(),
    }
}

/// The paths of the grants that `pick` selects, else the targets of the record.
fn paths(
    grants: &[Grant],
    record: Option<&ExitRecord>,
    home: Option<&Path>,
    pick: fn(&Grant) -> Option<&Path>,
) -> Vec<String> {
    let granted: Vec<String> =
        grants.iter().filter_map(pick).map(|path| tilde(path, home)).collect();
    if !granted.is_empty() {
        return granted;
    }
    record
        .map(|record| record.facts.targets.iter().map(|target| tilde(&target.path, home)).collect())
        .unwrap_or_default()
}

fn write_path(grant: &Grant) -> Option<&Path> {
    match grant {
        Grant::Write { path } => Some(path),
        _ => None,
    }
}

fn unmask_path(grant: &Grant) -> Option<&Path> {
    match grant {
        Grant::Unmask { path } => Some(path),
        _ => None,
    }
}

fn socket_path(grant: &Grant) -> Option<&Path> {
    match grant {
        Grant::Socket { path } => Some(path),
        _ => None,
    }
}

fn device_path(grant: &Grant) -> Option<&Path> {
    match grant {
        Grant::Device { path } => Some(path),
        _ => None,
    }
}

/// `verb` and the paths, or `fallback` when there are none.
fn with_paths(verb: &str, paths: &[String], fallback: &str) -> String {
    if paths.is_empty() { fallback.to_owned() } else { format!("{verb} {}", paths.join(", ")) }
}

/// The first program word of the line, as the line has it.
fn first_program(record: Option<&ExitRecord>) -> Option<String> {
    record.and_then(|record| record.facts.programs.first()).map(|program| one_line(&program.word))
}

/// The program of the first command that runs outside the sandbox, such as `df` for
/// `uname -a; df -h`: the command that leaves, not the first of the line. A record from
/// an efr before `outside_commands` names the first program of the line.
fn outside_program(record: Option<&ExitRecord>) -> Option<String> {
    let Some(command) = record.and_then(|record| record.facts.outside_commands.first()) else {
        return first_program(record);
    };
    command.split_whitespace().find(|word| !is_assignment(word)).map(one_line)
}

/// True for a word that sets a variable for its command, such as `LC_ALL=C`.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// One exit kind in words, such as `write ~/.zshrc` or `network`, with what the
/// grants open.
fn kind_part(
    kind: ExitKind,
    grants: &[Grant],
    record: Option<&ExitRecord>,
    home: Option<&Path>,
) -> String {
    let writes = || paths(grants, record, home, write_path);
    match kind {
        ExitKind::Write | ExitKind::Persistence => {
            with_paths("write", &writes(), &kind_words(kind))
        }
        ExitKind::SyncedWrite => {
            format!("{} (a synced folder)", with_paths("write", &writes(), "write"))
        }
        ExitKind::AboveRoot => {
            format!("{} (at or above a project root)", with_paths("write", &writes(), "write"))
        }
        ExitKind::MaskedRead => {
            with_paths("read", &paths(grants, record, home, unmask_path), "a masked read")
        }
        ExitKind::Destructive => {
            let targets = paths(grants, record, home, |_| None);
            with_paths("destroy or overwrite", &targets, "a destructive change")
        }
        ExitKind::Host => {
            let hosts: Vec<String> = record
                .map(|record| record.facts.hosts.iter().map(|host| one_line(&host.host)).collect())
                .unwrap_or_default();
            if hosts.is_empty() {
                "network".to_owned()
            } else {
                format!("network ({})", hosts.join(", "))
            }
        }
        ExitKind::HostView => "a view of the host's network".to_owned(),
        ExitKind::Socket => {
            with_paths("the socket", &paths(grants, record, home, socket_path), "a socket")
        }
        ExitKind::DesktopIpc => desktop_part(grants, record),
        ExitKind::Bus => grants
            .iter()
            .find_map(|grant| match grant {
                Grant::Bus { bus } => Some(bus_words(*bus).to_owned()),
                _ => None,
            })
            .unwrap_or_else(|| "a message bus".to_owned()),
        ExitKind::Device => {
            with_paths("the device", &paths(grants, record, home, device_path), "a device")
        }
        ExitKind::Outside => outside_program(record).unwrap_or_else(|| "the whole line".to_owned()),
        ExitKind::Privilege => format!(
            "{} (you may need to type your password)",
            first_program(record).unwrap_or_else(|| "more rights".to_owned())
        ),
        ExitKind::Upload => {
            let patterns: Vec<String> = record
                .map(|record| record.facts.upload_patterns.iter().map(|p| one_line(p)).collect())
                .unwrap_or_default();
            if patterns.is_empty() {
                "upload".to_owned()
            } else {
                format!("upload ({})", patterns.join(", "))
            }
        }
        ExitKind::Secret => "a secret (denied)".to_owned(),
        ExitKind::Config => "efr's config (denied)".to_owned(),
        _ => kind_words(kind),
    }
}

/// A program word with the path it resolves to, and the untrusted mark when the
/// sandbox wrote it or may have, such as ` in a write root (untrusted: ...)`.
fn program(program: &ProgramFact, home: Option<&Path>) -> (String, Option<String>) {
    let word = one_line(&program.word);
    let text = match &program.resolved {
        Some(resolved) if resolved.is_absolute() => format!("{word} {}", tilde(resolved, home)),
        // A builtin, a function or an alias of the user's shell.
        Some(resolved) => format!("{word} ({})", tilde(resolved, home)),
        None => format!("{word} (not found)"),
    };
    let mut marks = Vec::new();
    if program.in_write_root {
        marks.push("in a write root");
    }
    if program.changed_this_turn {
        marks.push("changed this turn");
    }
    let mark = (!marks.is_empty()).then(|| format!(" {} ({UNTRUSTED})", marks.join(", ")));
    (text, mark)
}

/// The heading of an exit's approval: the whole line from the record, so nothing of it
/// hides behind a summary, with each line of a command of several on its own
/// ([`run_heading`]); `None` without a record, and for an exit of a file tool, which
/// has no line: its summary names the path.
pub(crate) fn exit_heading(record: Option<&ExitRecord>) -> Option<Vec<String>> {
    let action = &record?.action;
    if action.line.is_empty() {
        return None;
    }
    Some(run_heading(&action.tool, &action.line))
}

/// The lines that an approval question for an exit shows between its heading and the
/// question, each as pieces with their tone: what leaves the sandbox and how the call
/// runs after a "yes" (plain), the full-rights warning of an unsandboxed run (in the
/// warning role), every program word of such a run (plain, with the untrusted mark in
/// the warning role), efr's own facts, and the model's reason, labelled as the model's
/// (muted). Only the facts that matter most stand out.
pub(crate) fn exit_lines(
    info: &ExitInfo,
    record: Option<&ExitRecord>,
    home: Option<&Path>,
) -> Vec<Vec<(String, Tone)>> {
    let mut parts: Vec<String> = Vec::new();
    for kind in &info.kinds {
        let part = kind_part(*kind, &info.grants, record, home);
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    let unsandboxed = matches!(info.launch, Launch::Unsandboxed);
    // NOTE: privilege and outside exits name no target; the program is what leaves.
    let program_only = !info.kinds.is_empty()
        && info.kinds.iter().all(|kind| matches!(kind, ExitKind::Privilege | ExitKind::Outside));
    let first = if unsandboxed && program_only {
        format!("runs outside the sandbox: {}", parts.join("; "))
    } else if full_shell(info, record) {
        format!("full mode asks: {}; {FULL_RUNS}", parts.join("; "))
    } else {
        format!("leaves the sandbox: {}; {}", parts.join("; "), launch_words(info, record, home))
    };
    let mut lines = vec![vec![(first, Tone::Plain)]];
    if unsandboxed {
        lines.push(vec![(FULL_RIGHTS.to_owned(), Tone::Attention)]);
        let programs: Vec<(String, Option<String>)> = record
            .map(|record| record.facts.programs.iter().map(|p| program(p, home)).collect())
            .unwrap_or_default();
        if !programs.is_empty() {
            let mut line = vec![("programs: ".to_owned(), Tone::Plain)];
            for (at, (text, mark)) in programs.into_iter().enumerate() {
                let separator = if at == 0 { "" } else { "; " };
                line.push((format!("{separator}{text}"), Tone::Plain));
                line.extend(mark.map(|mark| (mark, Tone::Attention)));
            }
            lines.push(line);
        }
    }
    if let Some(judged) = &info.judged {
        lines.push(vec![(reviewer_row(judged.verdict, &judged.rationale), Tone::Dim)]);
    }
    let mut facts: Vec<String> = info.facts.iter().map(|fact| one_line(fact)).collect();
    if info.user_only {
        facts.push("only you can allow this".to_owned());
    }
    if !facts.is_empty() {
        lines.push(vec![(format!("efr: {}", facts.join("; ")), Tone::Dim)]);
    }
    if let Some(reason) = &info.model_reason {
        lines.push(vec![(format!("the model says: \"{}\"", one_line(reason)), Tone::Dim)]);
    }
    lines
}

/// The card of an approval for an exit. The title says what a "yes" allows: to run
/// outside the sandbox, or what the sandbox opens for this call, or for the whole job
/// of a background call. The rows show:
///
/// - the whole line of the call from its record, each line of a command on its own,
///   so nothing of it hides behind a summary; `subject` when the record has no line
///   (a file tool, or no record): the rows of the daemon's summary;
/// - for a run outside the sandbox, the full-rights warning (in the `warning` role);
/// - why the call leaves the sandbox, such as `why: write ~/.zshrc` (muted);
/// - for a run outside the sandbox, every program word as a plain name, with the path
///   it resolves to and the untrusted mark (in the `warning` role) only for a program
///   that the sandbox wrote or may have written (muted);
/// - what the reviewer thought before it handed the question over, from the question
///   (`judged`), else `reviewed`, the row of the verdict that the view saw (muted);
/// - efr's own facts after `efr:`, and the model's reason as `the model says: "..."`
///   (muted).
pub(crate) fn exit_card(
    info: &ExitInfo,
    record: Option<&ExitRecord>,
    home: Option<&Path>,
    subject: Vec<Row>,
    reviewed: Option<&str>,
) -> Card {
    let unsandboxed = matches!(info.launch, Launch::Unsandboxed);
    let title = match &info.launch {
        Launch::Unsandboxed => "allow outside the sandbox".to_owned(),
        Launch::Contained { grants } if grants.is_empty() => "allow in the sandbox".to_owned(),
        Launch::Contained { grants } => {
            let opened: Vec<String> = grants.iter().map(|grant| grant_words(grant, home)).collect();
            format!("allow {} {}", opened.join(", "), grants_last(record))
        }
        Launch::Direct if full_shell(info, record) => FULL_TITLE.to_owned(),
        Launch::Direct => "allow the file tool outside the sandbox".to_owned(),
        _ => "allow this call".to_owned(),
    };
    let mut rows = match record.map(|record| record.action.line.as_str()) {
        Some(line) if !line.is_empty() => card::command_rows(line),
        _ => subject,
    };
    if unsandboxed {
        rows.push(Row::text(RISK, Tone::Attention));
    }
    let mut parts: Vec<String> = Vec::new();
    for kind in &info.kinds {
        // NOTE: an outside exit names only the first program, which the line shows.
        if *kind == ExitKind::Outside {
            continue;
        }
        let part = kind_part(*kind, &info.grants, record, home);
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    if !parts.is_empty() {
        rows.push(Row::text(format!("why: {}", parts.join("; ")), Tone::Dim));
    }
    if unsandboxed && let Some(record) = record.filter(|record| !record.facts.programs.is_empty()) {
        let mut pieces = vec![("programs: ".to_owned(), Tone::Dim)];
        for (at, fact) in record.facts.programs.iter().enumerate() {
            let separator = if at == 0 { "" } else { ", " };
            let (text, mark) = program_name(fact, home);
            pieces.push((format!("{separator}{text}"), Tone::Dim));
            pieces.extend(mark.map(|mark| (mark, Tone::Attention)));
        }
        rows.push(Row::Text(pieces));
    }
    let judged = info.judged.as_ref().map(|judged| reviewer_row(judged.verdict, &judged.rationale));
    if let Some(row) = judged.or_else(|| reviewed.map(str::to_owned)) {
        rows.push(Row::text(row, Tone::Dim));
    }
    let mut facts: Vec<String> = info.facts.iter().map(|fact| one_line(fact)).collect();
    if info.user_only {
        facts.push("only you can allow this".to_owned());
    }
    if !facts.is_empty() {
        rows.push(Row::text(format!("efr: {}", facts.join("; ")), Tone::Dim));
    }
    if let Some(reason) = &info.model_reason {
        rows.push(Row::text(format!("the model says: \"{}\"", one_line(reason)), Tone::Dim));
    }
    Card { title, rows }
}

/// A program word as the card of an exit names it: the word alone, `(not found)`
/// after a word that resolves to nothing, and for a program that the sandbox wrote or
/// may have written the path it resolves to and the mark, such as ` (untrusted: in a
/// write root, changed this turn)`.
fn program_name(program: &ProgramFact, home: Option<&Path>) -> (String, Option<String>) {
    let word = one_line(&program.word);
    let mut marks = Vec::new();
    if program.in_write_root {
        marks.push("in a write root");
    }
    if program.changed_this_turn {
        marks.push("changed this turn");
    }
    if marks.is_empty() {
        let text = if program.resolved.is_none() { format!("{word} (not found)") } else { word };
        return (text, None);
    }
    let text = match &program.resolved {
        Some(resolved) if resolved.is_absolute() && tilde(resolved, home) != word => {
            format!("{word} {}", tilde(resolved, home))
        }
        _ => word,
    };
    (text, Some(format!(" ({UNTRUSTED}; {})", marks.join(", "))))
}

/// The card of the quarantine question: the git settings that the last call changed,
/// each on its own row.
pub(crate) fn surface_card(changes: &[SurfaceChange], home: Option<&Path>) -> Card {
    let rows = changes
        .iter()
        .map(|change| Row::text(surface_change(change, home).trim().to_owned(), Tone::Plain))
        .collect();
    Card { title: SURFACE_TITLE.to_owned(), rows }
}

/// One line for an exit in `efr history`: the heading's line and every exit line,
/// joined.
pub(crate) fn exit_summary(
    info: &ExitInfo,
    record: Option<&ExitRecord>,
    home: Option<&Path>,
) -> String {
    let lines: Vec<String> = exit_lines(info, record, home)
        .into_iter()
        .map(|line| line.into_iter().map(|(text, _)| text).collect())
        .collect();
    lines.join("; ")
}

/// An exit as its `exit_requested` event gives it: the view and the history keep one
/// per call, for the lines of its question and of the reviewer.
#[derive(Debug, Clone)]
pub(crate) struct Requested {
    pub(crate) kinds: Vec<ExitKind>,
    pub(crate) grants: Vec<Grant>,
    pub(crate) source: ExitSource,
    pub(crate) record: ExitRecord,
}

impl Requested {
    /// The exit of an `exit_requested` event.
    pub(crate) fn new(
        kinds: &[ExitKind],
        grants: &[Grant],
        source: ExitSource,
        record: &ExitRecord,
    ) -> Requested {
        Requested { kinds: kinds.to_vec(), grants: grants.to_vec(), source, record: record.clone() }
    }
}

/// What an exit opens, in a few words, for the reviewer's lines: each kind with its
/// targets, such as `write ~/.local/share/app`; for the deletes of a tool cache, how many
/// paths and where, because they can be thousands.
pub(crate) fn exit_what(exit: &Requested, home: Option<&Path>) -> String {
    exit_parts(exit, home).join("; ")
}

/// The parts of [`exit_what`], each kind once: `this call` when the exit names none.
pub(crate) fn exit_parts(exit: &Requested, home: Option<&Path>) -> Vec<String> {
    let record = Some(&exit.record);
    if exit.source == ExitSource::CacheDelete {
        let paths: Vec<&Path> =
            exit.record.facts.targets.iter().map(|target| target.path.as_path()).collect();
        let count = plural(paths.len(), "path", "paths");
        return vec![match common_dir(&paths) {
            Some(dir) => format!("delete {count} in {}", tilde(&dir, home)),
            None => format!("delete {count} in a tool cache"),
        }];
    }
    let mut parts: Vec<String> = Vec::new();
    for kind in &exit.kinds {
        let part = kind_part(*kind, &exit.grants, record, home);
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        parts.push("this call".to_owned());
    }
    parts
}

/// How many actions full mode let through in a turn shows at most at its end.
const FULL_SHOWN: usize = 3;

/// The muted line at the end of a turn in which full mode let `actions` through with no
/// review and no question, such as `efr saw 2 actions that full mode let through: write
/// ~/notes.txt, upload`: the first three, then `...`. `None` when there were none, so a
/// turn never says `0 actions`. It names only what efr saw on a line, not what a script
/// did.
pub(crate) fn full_let_through(actions: &[String]) -> Option<String> {
    if actions.is_empty() {
        return None;
    }
    let mut shown: Vec<&str> = actions.iter().take(FULL_SHOWN).map(String::as_str).collect();
    if actions.len() > FULL_SHOWN {
        shown.push("...");
    }
    Some(format!(
        "efr saw {} that full mode let through: {}",
        plural(actions.len(), "action", "actions"),
        shown.join(", ")
    ))
}

/// `count` and the noun in the singular or the plural, such as `1 path` or `12 paths`.
fn plural(count: usize, one: &str, more: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { more })
}

/// The dir that holds every path of `paths`: the parent of a single path, else the
/// longest common start of their parents; `None` for no paths or nothing in common.
fn common_dir(paths: &[&Path]) -> Option<std::path::PathBuf> {
    let mut parents = paths.iter().map(|path| path.parent().unwrap_or(path));
    let mut common = parents.next()?.to_path_buf();
    for parent in parents {
        while !parent.starts_with(&common) {
            if !common.pop() {
                return None;
            }
        }
    }
    (!common.as_os_str().is_empty()).then_some(common)
}

/// What the reviewer reviews, for the status row (`reviewing: <subject>`): the line of
/// the call, else what the exit opens (a file tool has no line).
pub(crate) fn reviewing(
    exit: Option<&Requested>,
    kinds: &[ExitKind],
    home: Option<&Path>,
) -> String {
    let subject = match exit {
        Some(exit) if !exit.record.action.line.trim().is_empty() => {
            one_line(exit.record.action.line.trim())
        }
        Some(exit) => exit_what(exit, home),
        None => {
            let kinds: Vec<String> = kinds.iter().map(|kind| kind_words(*kind)).collect();
            kinds.join(", ")
        }
    };
    format!("reviewing: {subject}")
}

/// A risk in words, such as `low risk`.
fn risk_words(risk: Risk) -> &'static str {
    match risk {
        Risk::Low => "low risk",
        Risk::Medium => "medium risk",
        Risk::High => "high risk",
        Risk::Critical => "critical risk",
        _ => "unknown risk",
    }
}

/// The line of the reviewer's verdict on an exit: `reviewer allowed: <what> for this
/// call (<risk>; <rationale>)` (`for the whole job` for a background call) or `reviewer refused: <line>: <rationale>` (efr's auto
/// spec, 14.3). `None` for an unsure verdict, which the question's card shows. The
/// rationale is the reviewer model's text, so it passes through [`one_line`].
pub(crate) fn reviewed(
    verdict: Verdict,
    risk: Option<Risk>,
    rationale: Option<&str>,
    exit: Option<&Requested>,
    home: Option<&Path>,
) -> Option<String> {
    let rationale = rationale.map(one_line).filter(|text| !text.trim().is_empty());
    let what = exit.map_or_else(|| "this call".to_owned(), |exit| exit_what(exit, home));
    match verdict {
        Verdict::Allow => {
            let why: Vec<String> =
                risk.map(|risk| risk_words(risk).to_owned()).into_iter().chain(rationale).collect();
            let mut line = match exit {
                Some(exit) if exit.record.action.background => {
                    format!("reviewer allowed: {what} for the whole job")
                }
                Some(_) => format!("reviewer allowed: {what} for this call"),
                None => format!("reviewer allowed: {what}"),
            };
            if !why.is_empty() {
                let _ = write!(line, " ({})", why.join("; "));
            }
            Some(line)
        }
        Verdict::Deny => {
            let subject = match exit {
                Some(exit) if !exit.record.action.line.trim().is_empty() => {
                    one_line(exit.record.action.line.trim())
                }
                _ => what,
            };
            Some(match rationale {
                Some(rationale) => format!("reviewer refused: {subject}: {rationale}"),
                None => format!("reviewer refused: {subject}"),
            })
        }
        _ => None,
    }
}

/// The row of a question's card that says what the reviewer thought before it handed
/// the question over, such as `reviewer: unsure; no message of yours names this host`.
pub(crate) fn reviewer_row(verdict: Verdict, rationale: &str) -> String {
    let opinion = match verdict {
        Verdict::Allow => "it would allow this, but efr's rules ask you",
        Verdict::Deny => "it would refuse this",
        _ => "unsure",
    };
    let rationale = one_line(rationale.trim());
    if rationale.is_empty() {
        format!("reviewer: {opinion}")
    } else {
        format!("reviewer: {opinion}; {rationale}")
    }
}

/// The row of a question's card when no reviewer model judged before efr handed the
/// question over: every step of the chain failed, timed out or refused, such as
/// `reviewer: no answer (skipped gpt-6-luna: parse error, gpt-6.1-sol: refused)`.
pub(crate) fn no_answer_row(skipped: &[String]) -> String {
    if skipped.is_empty() {
        return "reviewer: no answer".to_owned();
    }
    let skipped: Vec<String> = skipped.iter().map(|text| one_line(text)).collect();
    format!("reviewer: no answer (skipped {})", skipped.join(", "))
}

/// The note when the reviewer stops for the rest of a turn after `denials` refusals
/// (efr's auto spec, 9.5 and 14.7): each later exit then asks the user.
pub(crate) fn paused(reason: PauseReason, denials: u8) -> String {
    let refused = match reason {
        PauseReason::InARow => format!("it refused {denials} actions in a row"),
        _ => format!("it refused {denials} actions"),
    };
    format!("reviewer paused for this turn: {refused}; efr asks you for the rest")
}

/// The answer line of a question that nobody answered while the user was away
/// (`approval_expired` with `away`). efr does not know how long the daemon waited.
pub(crate) const AWAY_EXPIRED: &str =
    "approval expired: nobody answered while you were away; the call did not run";

/// The row of a question's card for a call outside the sandbox that may wait for a
/// password at the terminal (the hard or the soft flag), when its exits do not say so
/// already: `rsync` over ssh, `scp` or `git` over the network.
pub(crate) const PASSWORD_ROW: &str = "you may need to type your password";

/// The row of a question's card while efr counts the user away: the question does not
/// wait long, and an answer counts as input.
pub(crate) const AWAY_ROW: &str =
    "efr counts you away: this question waits only a short time, then the call does not run";

/// The note after efrd deleted paths in the real tool caches, such as `freed 1.2 GB in
/// ~/.cache/go-build (12 paths)`.
pub(crate) fn cache_freed(paths: &[std::path::PathBuf], freed: u64, home: Option<&Path>) -> String {
    let paths: Vec<&Path> = paths.iter().map(std::path::PathBuf::as_path).collect();
    let place =
        common_dir(&paths).map_or_else(|| "the tool caches".to_owned(), |dir| tilde(&dir, home));
    let count = plural(paths.len(), "path", "paths");
    format!("freed {} in {place} ({count})", super::size(freed))
}

/// The name of the file that efrd writes next to a backup copy, and in place of one for
/// a file that did not exist before the call (the efr-daemon backup store).
const BACKUP_META: &str = "meta.json";

/// The line at the end of a turn for a file outside the project that runs code later
/// and that a call changed (`home_file_changed`): `changed ~/.zshrc; backup at <path>`.
/// A file that did not exist before has no copy, only the record of efrd's backup
/// store, so the line says so. A file changed through a link names the path that the
/// user knows, then its `real` file: `changed ~/.config/hypr/x.conf (real file
/// ~/dotfiles/hypr/x.conf); ...`.
pub(crate) fn home_changed(
    path: &Path,
    real: Option<&Path>,
    backup: Option<&Path>,
    home: Option<&Path>,
) -> String {
    let mut shown = tilde(path, home);
    if let Some(real) = real.filter(|real| *real != path) {
        let _ = write!(shown, " (real file {})", tilde(real, home));
    }
    let Some(backup) = backup else {
        return format!("changed {shown}");
    };
    let meta = Some(std::ffi::OsStr::new(BACKUP_META));
    if backup.file_name() == meta && path.file_name() != meta {
        return format!(
            "changed {shown}, which did not exist before; record at {}",
            tilde(backup, home)
        );
    }
    format!("changed {shown}; backup at {}", tilde(backup, home))
}

/// The line at the end of a turn of a file outside the roots that efr's home store
/// holds or could not copy (phase B): `changed ~/.zshrc; efr undo reverts it` for a
/// file that runs code later, `changed ~/.aws/config; efr undo cannot revert it
/// (masked)` for one that efr could not copy, with `why` when the turn's undo names it.
/// `None` for a covered file that runs no code: the line of the turn counts it.
pub(crate) fn home_undo_line(
    path: &Path,
    real: Option<&Path>,
    floor: bool,
    covered: bool,
    why: Option<&str>,
    home: Option<&Path>,
) -> Option<String> {
    let mut shown = tilde(path, home);
    if let Some(real) = real.filter(|real| *real != path) {
        let _ = write!(shown, " (real file {})", tilde(real, home));
    }
    match (covered, why) {
        (true, _) if floor => Some(format!("changed {shown}; efr undo reverts it")),
        (true, _) => None,
        (false, Some(why)) => {
            Some(format!("changed {shown}; efr undo cannot revert it ({})", one_line(why)))
        }
        (false, None) => Some(format!("changed {shown}; efr undo cannot revert it")),
    }
}

/// A change of the user's presence, for `efr history`, such as `away (efr away)`.
pub(crate) fn presence_changed(presence: Presence, source: PresenceSource) -> String {
    let state = match presence {
        Presence::Away => "away",
        _ => "back",
    };
    let why = match source {
        PresenceSource::User => "efr away",
        PresenceSource::Timeout => "no input for a while",
        PresenceSource::Input => "input",
        _ => "another reason",
    };
    format!("{state} ({why})")
}

/// Where an exit came from, in words.
fn source_words(source: ExitSource) -> &'static str {
    match source {
        ExitSource::Predicted => "predicted from the line",
        ExitSource::Needs => "the model asked with needs",
        ExitSource::CacheDelete => "deletes in a private tool cache",
        _ => "from an unknown source",
    }
}

/// `yes` facts of a target, in words.
fn target_words(target: &TargetFact, home: Option<&Path>) -> String {
    let mut words = Vec::new();
    if let Some(class) = target.class {
        words.push(class_words(class));
    }
    words.push(if target.exists { "exists" } else { "missing" });
    if target.in_write_root {
        words.push("in a write root");
    }
    if target.floor {
        words.push("a floor");
    }
    if target.synced {
        words.push("in a synced folder");
    }
    if target.named_in_user_messages {
        words.push("named by the user");
    }
    if target.backup_before_write {
        words.push("efr keeps a backup copy before a write");
    }
    format!("target {}: {}", tilde(&target.path, home), words.join(", "))
}

fn class_words(class: PathClassName) -> &'static str {
    match class {
        PathClassName::Scratch => "scratch",
        PathClassName::UserConfig => "user config",
        PathClassName::UserData => "user data",
        PathClassName::System => "system",
        PathClassName::Secrets => "secrets",
        PathClassName::Cache => "a tool cache",
        PathClassName::BuildOutput => "build output",
        PathClassName::Trash => "efr's trash",
        _ => "another class",
    }
}

fn verdict_words(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Allow => "allowed",
        Verdict::Deny => "denied",
        Verdict::AskUser => "asked the user",
        _ => "judged",
    }
}

/// The lines of an `exit_requested` event in `efr history --verbose`: the kinds, the
/// grants and every fact of its record. Names and counts only, as the record holds
/// them; the user messages are counted, not repeated.
pub(crate) fn exit_record(
    kinds: &[ExitKind],
    grants: &[Grant],
    source: ExitSource,
    record: &ExitRecord,
    home: Option<&Path>,
) -> Vec<String> {
    let kinds: Vec<String> = kinds.iter().map(|kind| kind_words(*kind)).collect();
    let mut first = format!("exit requested: {} ({})", kinds.join(", "), source_words(source));
    if !grants.is_empty() {
        let opened: Vec<String> = grants.iter().map(|grant| grant_words(grant, home)).collect();
        let _ = write!(first, "; a yes opens {}", opened.join(", "));
    }
    let facts = &record.facts;
    let mut lines = vec![first];
    match numbered_lines(&record.action.line, "    ") {
        Some(numbered) => {
            lines.push(format!("  line: {} lines:", numbered.len()));
            lines.extend(numbered);
        }
        None => lines.push(format!("  line: {}", command_line(record.action.line.trim()))),
    }
    // NOTE: the payload of a file tool's edit is model-written text; its size only.
    if let Some(edit) = &record.action.edit {
        lines.push(format!("  edit: {} that the reviewer reads", super::size(edit.len() as u64)));
    }
    lines.push(format!("  cwd: {}", tilde(&record.action.cwd, home)));
    lines.extend(facts.targets.iter().map(|target| format!("  {}", target_words(target, home))));
    for host in &facts.hosts {
        let mut words = Vec::new();
        if host.on_allow_list {
            words.push("on the allow list");
        }
        if host.named_in_user_messages {
            words.push("named by the user");
        }
        if host.refused_by_proxy {
            words.push("refused by the proxy");
        }
        if words.is_empty() {
            words.push("not named by the user");
        }
        lines.push(format!("  host {}: {}", one_line(&host.host), words.join(", ")));
    }
    lines.extend(facts.programs.iter().map(|fact| format!("  program {}", program(fact, home).0)));
    if !facts.upload_patterns.is_empty() {
        let patterns: Vec<String> = facts.upload_patterns.iter().map(|p| one_line(p)).collect();
        lines.push(format!("  upload patterns: {}", patterns.join(", ")));
    }
    if facts.repo_surface_changed_this_turn {
        lines.push("  a git setting or a file that runs code changed this turn".to_owned());
    }
    if let Some(git) = facts.git_status {
        lines.push(format!(
            "  git status: {} modified, {} untracked, {} staged",
            git.modified, git.untracked, git.staged
        ));
    }
    if let Some(code) = &facts.git_code {
        lines.extend(git_code_lines(code, home));
    }
    if !facts.sandbox_export_names.is_empty() {
        let names: Vec<String> = facts.sandbox_export_names.iter().map(|n| one_line(n)).collect();
        lines.push(format!("  exported in the sandbox: {}", names.join(", ")));
    }
    if !facts.previous_exits_this_turn.is_empty() {
        let earlier: Vec<String> = facts
            .previous_exits_this_turn
            .iter()
            .map(|(kind, verdict)| format!("{} {}", kind_words(*kind), verdict_words(*verdict)))
            .collect();
        lines.push(format!("  earlier exits this turn: {}", earlier.join(", ")));
    }
    if facts.refusals_in_a_row > 0 {
        lines.push(format!("  refusals in a row: {}", facts.refusals_in_a_row));
    }
    if facts.denials_in_a_row > 0 || facts.denials_this_turn > 0 {
        lines.push(format!(
            "  reviewer refusals: {} in a row, {} in this turn",
            facts.denials_in_a_row, facts.denials_this_turn
        ));
    }
    lines.push(format!("  user messages: {}", record.user_messages.len()));
    lines
}

/// The lines of the git code fact of an exit record (v0.0.4, DV22): whether efr could
/// check it, then each part that a sandboxed command can write, and the count of the
/// others.
fn git_code_lines(code: &GitCodeFacts, home: Option<&Path>) -> Vec<String> {
    let mut lines = Vec::new();
    if !code.checked {
        lines.push("  git code: efr could not check it".to_owned());
    }
    let mut others = 0;
    for item in &code.items {
        if item.writable {
            lines.push(format!(
                "  git code: {} in {}, which a sandboxed command can write",
                one_line(&item.item),
                tilde(&item.file, home)
            ));
        } else {
            others += 1;
        }
    }
    if others > 0 {
        lines.push(format!(
            "  git code: {} that the sandbox cannot write",
            plural(others, "part", "parts")
        ));
    }
    lines
}

/// The line of an `exit_judged` event in `efr history --verbose`.
pub(crate) fn exit_judged(judge: JudgeKind, verdict: Verdict, category: Option<&str>) -> String {
    let judge = match judge {
        JudgeKind::User => "the user",
        JudgeKind::Floor => "a floor",
        JudgeKind::Classifier => "the reviewer",
        JudgeKind::Always => "an always-allow rule",
        JudgeKind::PresenceExpired => "nobody, while you were away",
        JudgeKind::Full => "full mode",
        _ => "another judge",
    };
    let mut line = format!("exit {} by {judge}", verdict_words(verdict));
    if let Some(category) = category {
        let _ = write!(line, ": {}", one_line(category));
    }
    line
}

/// How the reviewer came to a verdict, for `efr history --verbose`: the model, the
/// step of its chain, the time, whether the verdict came from the turn's cache, and
/// the models that the chain skipped; `None` when the event names none of these.
pub(crate) fn judge_details(
    model: Option<&str>,
    chain_step: Option<ChainStep>,
    latency_ms: Option<u64>,
    cached: bool,
    skipped: &[String],
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(model) = model {
        parts.push(format!("model {}", one_line(model)));
    }
    if let Some(step) = chain_step {
        let step = match step {
            ChainStep::ClassifierModel => "the reviewer model",
            ChainStep::TurnModel => "the turn's model",
            ChainStep::User => "handed to you",
            _ => "another step",
        };
        parts.push(step.to_owned());
    }
    if let Some(latency) = latency_ms {
        parts.push(format!("{latency} ms"));
    }
    if cached {
        parts.push("from the turn's cache".to_owned());
    }
    if !skipped.is_empty() {
        let skipped: Vec<String> = skipped.iter().map(|text| one_line(text)).collect();
        parts.push(format!("skipped {}", skipped.join(", ")));
    }
    (!parts.is_empty()).then(|| format!("  {}", parts.join("; ")))
}

/// One change of the quarantine question, such as `  .git/commondir (core.fsmonitor);
/// moved to quarantine`.
pub(crate) fn surface_change(change: &SurfaceChange, home: Option<&Path>) -> String {
    let what = change.what();
    let mut line = format!("  {} ({})", tilde(&change.path, home), one_line(&what));
    if change.quarantined {
        line.push_str("; moved to quarantine");
    }
    line
}

/// The note about changes that the surface guard reported but did not quarantine;
/// `None` when every change went to quarantine, which the question shows.
pub(crate) fn surface_changed(changes: &[SurfaceChange], home: Option<&Path>) -> Option<String> {
    let reported: Vec<String> = changes
        .iter()
        .filter(|change| !change.quarantined)
        .map(|change| format!("{} ({})", tilde(&change.path, home), one_line(&change.what())))
        .collect();
    (!reported.is_empty())
        .then(|| format!("sandbox: the last command changed {}", reported.join(", ")))
}

/// The note after the quarantine question was answered, expired or interrupted.
pub(crate) fn surface_answered(keep: bool, origin: Option<&str>) -> String {
    match (keep, origin) {
        (true, Some(origin)) => format!("the git change was kept, from {origin}"),
        (true, None) => "the git change was kept".to_owned(),
        (false, Some(origin)) => format!("the git change stays in quarantine, from {origin}"),
        (false, None) => "no answer; the git change stays in quarantine".to_owned(),
    }
}

/// The turn-end report of files that run code later outside the sandbox.
pub(crate) fn surface_report(files: &[ReportedFile]) -> Vec<String> {
    let names: Vec<String> = files
        .iter()
        .map(|file| {
            let path = one_line(&file.path.display().to_string());
            match &file.detail {
                Some(detail) => format!("{path} ({})", one_line(detail)),
                None => path,
            }
        })
        .collect();
    vec![
        "efr: this turn changed files that run code later outside the sandbox:".to_owned(),
        format!("  {}", names.join(", ")),
        "  check them before you run the project yourself".to_owned(),
    ]
}

/// A connection that the proxy refused, such as `network: blocked evil.example:443 (not
/// on the allow list)`, with the count when the proxy refused it more than once.
pub(crate) fn blocked(blocked: &Blocked) -> String {
    format!("network: blocked {}", blocked_entry(blocked))
}

/// The lines of a call's refusals: one [`blocked`] line each, then `network: and N more
/// refused` for the refusals that the list leaves out (`blocked_more`).
pub(crate) fn blocked_lines(blocked: &[Blocked], more: u32) -> Vec<String> {
    let mut lines: Vec<String> = blocked.iter().map(self::blocked).collect();
    if more > 0 {
        lines.push(format!("network: and {more} more refused"));
    }
    lines
}

/// The refusals of a job in one piece of a line, such as `network: blocked a.test:443
/// (not on the allow list), b.test:443 (not TLS) and 2 more`, or `network: 2 refused`
/// when only the count is left; `None` when there are none.
pub(crate) fn blocked_joined(blocked: &[Blocked], more: u32) -> Option<String> {
    if blocked.is_empty() {
        return (more > 0).then(|| format!("network: {more} refused"));
    }
    let entries: Vec<String> = blocked.iter().map(blocked_entry).collect();
    let mut text = format!("network: blocked {}", entries.join(", "));
    if more > 0 {
        let _ = write!(text, " and {more} more");
    }
    Some(text)
}

/// `host:port (reason)`, with `; N times` in the parentheses for a count over one.
fn blocked_entry(blocked: &Blocked) -> String {
    let reason = match blocked.reason {
        BlockReason::NotAllowed => "not on the allow list",
        BlockReason::PrivateAddress => "a private address",
        BlockReason::SniMismatch => "the TLS name differs from the host",
        BlockReason::UploadLimit => "over the upload limit",
        BlockReason::Method => "a method that is not allowed",
        BlockReason::IpLiteral => "ip address",
        BlockReason::NotTls => "not TLS",
        BlockReason::BadRequest => "bad request",
        BlockReason::Limit => "too many connections",
        BlockReason::Denied => "denied for this command",
        BlockReason::Unknown => "unknown reason",
        _ => "refused",
    };
    let host = one_line(&blocked.host);
    match blocked.count {
        0 | 1 => format!("{host}:{} ({reason})", blocked.port),
        count => format!("{host}:{} ({reason}; {count} times)", blocked.port),
    }
}

/// The lines of the bus calls that a call's bus filter refused (phase E), such as
/// `bus: refused system org.freedesktop.systemd1 Manager.StartUnit`, with `; N times`
/// when the filter refused it more than once, then `bus: and N more refused` for the
/// refusals that the list leaves out. A member shows without the name of its service
/// in front. The names come from the command's own messages, so each part passes
/// through [`one_line`]; entries that then read the same, such as two paths of one
/// method, show as one line with their counts added.
pub(crate) fn bus_refused_lines(refused: &[BusRefusal], more: u32) -> Vec<String> {
    let mut lines: Vec<(String, u64)> = Vec::new();
    for entry in refused {
        let name = one_line(&entry.name);
        let member = one_line(&entry.member);
        let member = member
            .strip_prefix(&name)
            .and_then(|rest| rest.strip_prefix('.'))
            .filter(|rest| !rest.is_empty())
            .map_or(member.clone(), str::to_owned);
        let bus = match entry.bus {
            BusKind::System => "system",
            BusKind::Session => "session",
            _ => "unknown",
        };
        let mut line = format!("bus: refused {bus} {name}");
        if !member.is_empty() {
            let _ = write!(line, " {member}");
        }
        let count = u64::from(entry.count.max(1));
        match lines.iter_mut().find(|(shown, _)| *shown == line) {
            Some((_, total)) => *total = total.saturating_add(count),
            None => lines.push((line, count)),
        }
    }
    let mut out: Vec<String> = lines
        .into_iter()
        .map(|(line, count)| if count > 1 { format!("{line}; {count} times") } else { line })
        .collect();
    if more > 0 {
        out.push(format!("bus: and {more} more refused"));
    }
    out
}

/// The value of the `bus filter` row of `efr status` and `efr sandbox check` (phase
/// E): `on` when efrd runs the filtered bus proxy, else `off` with the reason, such as
/// `off (dbus.proxy = false)`. A daemon from before the filter says neither, so it
/// shows `off`.
pub(crate) fn bus_filter_words(status: &SandboxStatus) -> String {
    if status.bus_proxy {
        return "on".to_owned();
    }
    let reason = match status.bus_proxy_off {
        None => return "off".to_owned(),
        Some(BusProxyOff::Config) => "dbus.proxy = false",
        Some(BusProxyOff::NoProgram) => "xdg-dbus-proxy is not installed",
        Some(BusProxyOff::SelfTest) => "its self-test failed",
        Some(BusProxyOff::OldVersion) => "xdg-dbus-proxy is too old",
        Some(_) => "unknown reason",
    };
    format!("off ({reason})")
}

/// The note about background jobs that stopped when a contained call ended.
pub(crate) fn background_stopped(names: &[String]) -> Option<String> {
    let names: Vec<String> = names.iter().map(|name| one_line(name)).collect();
    (!names.is_empty())
        .then(|| format!("sandbox: stopped when the call ended: {}", names.join(", ")))
}

/// The note about a call whose sandbox could not start (efr's auto spec, section 14.7).
pub(crate) fn setup_failed(reason: Option<&str>) -> Option<String> {
    reason.map(|reason| {
        format!("sandbox: could not start: {}; efr checks the sandbox again", one_line(reason))
    })
}

/// The note about processes of an approved exit that efr could not end.
pub(crate) fn survivors(names: &[String]) -> Option<String> {
    let names: Vec<String> = names.iter().map(|name| one_line(name)).collect();
    (!names.is_empty()).then(|| {
        format!(
            "sandbox: could not end {}; the next call gets a new hidden shell",
            names.join(", ")
        )
    })
}

#[cfg(test)]
mod tests;
