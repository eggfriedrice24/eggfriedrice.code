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
    AdminSandboxCheckResult, BlockReason, Blocked, BusKind, CheckOutcome, ExitInfo, ExitKind,
    ExitRecord, ExitSource, Grant, JudgeKind, Launch, Mode, ModeFallback, PathClassName,
    ProgramFact, ReportedFile, SandboxExplainResult, SandboxStatus, SurfaceChange, TargetFact,
    Verdict,
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

/// The dim trace of the first contained call of a turn: where it can write. Phase 1
/// has no network in the sandbox.
pub(crate) const CONTAINED_IN_PROJECT: &str =
    "sandbox: writes in the project, $SCRATCH, private /tmp; no network";

/// The same trace for a turn outside a registered project.
pub(crate) const CONTAINED: &str = "sandbox: writes in $SCRATCH, private /tmp; no network";

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
    parts.push(format!("network {}", status.network_mode.as_str()));
    format!("ready ({})", parts.join(", "))
}

/// The note at the start of a turn whose mode fell back, such as `auto is not
/// available here; this turn runs as cautious: Landlock ABI 6 found`.
pub(crate) fn fallback_note(fallback: &ModeFallback, mode: Mode) -> String {
    format!(
        "{} is not available here; this turn runs as {mode}: {}",
        fallback.asked,
        one_line(&fallback.reason)
    )
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
            if kind.user_only() {
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
        Grant::Device { path } => format!("the device {}", tilde(path, home)),
        Grant::Unmask { path } => format!("{} readable", tilde(path, home)),
        _ => "more access".to_owned(),
    }
}

fn bus_words(bus: BusKind) -> &'static str {
    match bus {
        BusKind::System => "the system bus",
        BusKind::Session => "the session bus",
        _ => "a message bus",
    }
}

/// How a call runs after a "yes", as the end of the exit line.
fn launch_words(info: &ExitInfo, home: Option<&Path>) -> String {
    match &info.launch {
        Launch::Unsandboxed => "runs outside the sandbox".to_owned(),
        Launch::Contained { grants } if grants.is_empty() => "runs in the sandbox".to_owned(),
        Launch::Contained { grants } => {
            let opened: Vec<String> = grants.iter().map(|grant| grant_words(grant, home)).collect();
            format!("runs in the sandbox with {} for this call", opened.join(", "))
        }
        // NOTE: in auto only a file tool runs direct: efrd reads or writes the path.
        Launch::Direct => "the file tool runs outside the sandbox".to_owned(),
        _ => "runs as typed".to_owned(),
    }
}

/// The paths of the grants that `pick` selects, else the targets of the record.
fn paths(
    info: &ExitInfo,
    record: Option<&ExitRecord>,
    home: Option<&Path>,
    pick: fn(&Grant) -> Option<&Path>,
) -> Vec<String> {
    let granted: Vec<String> =
        info.grants.iter().filter_map(pick).map(|path| tilde(path, home)).collect();
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

/// One exit kind in words, such as `write ~/.zshrc` or `network`.
fn kind_part(
    kind: ExitKind,
    info: &ExitInfo,
    record: Option<&ExitRecord>,
    home: Option<&Path>,
) -> String {
    let writes = || paths(info, record, home, write_path);
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
            with_paths("read", &paths(info, record, home, unmask_path), "a masked read")
        }
        ExitKind::Destructive => {
            let targets = paths(info, record, home, |_| None);
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
            with_paths("the socket", &paths(info, record, home, socket_path), "a socket")
        }
        ExitKind::DesktopIpc => "the desktop (it can type and run commands)".to_owned(),
        ExitKind::Bus => info
            .grants
            .iter()
            .find_map(|grant| match grant {
                Grant::Bus { bus } => Some(bus_words(*bus).to_owned()),
                _ => None,
            })
            .unwrap_or_else(|| "a message bus".to_owned()),
        ExitKind::Device => {
            with_paths("the device", &paths(info, record, home, device_path), "a device")
        }
        ExitKind::Outside => first_program(record).unwrap_or_else(|| "the whole line".to_owned()),
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
        let part = kind_part(*kind, info, record, home);
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
    } else {
        format!("leaves the sandbox: {}; {}", parts.join("; "), launch_words(info, home))
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
/// outside the sandbox, or what the sandbox opens for this call. The rows show:
///
/// - the whole line of the call from its record, each line of a command on its own,
///   so nothing of it hides behind a summary; `subject` when the record has no line
///   (a file tool, or no record): the rows of the daemon's summary;
/// - for a run outside the sandbox, the full-rights warning (in the `warning` role);
/// - why the call leaves the sandbox, such as `why: write ~/.zshrc` (muted);
/// - for a run outside the sandbox, every program word as a plain name, with the path
///   it resolves to and the untrusted mark (in the `warning` role) only for a program
///   that the sandbox wrote or may have written (muted);
/// - efr's own facts after `efr:`, and the model's reason as `the model says: "..."`
///   (muted).
pub(crate) fn exit_card(
    info: &ExitInfo,
    record: Option<&ExitRecord>,
    home: Option<&Path>,
    subject: Vec<Row>,
) -> Card {
    let unsandboxed = matches!(info.launch, Launch::Unsandboxed);
    let title = match &info.launch {
        Launch::Unsandboxed => "allow outside the sandbox".to_owned(),
        Launch::Contained { grants } if grants.is_empty() => "allow in the sandbox".to_owned(),
        Launch::Contained { grants } => {
            let opened: Vec<String> = grants.iter().map(|grant| grant_words(grant, home)).collect();
            format!("allow {} for this call", opened.join(", "))
        }
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
        let part = kind_part(*kind, info, record, home);
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

/// Where an exit came from, in words.
fn source_words(source: ExitSource) -> &'static str {
    match source {
        ExitSource::Predicted => "predicted from the line",
        ExitSource::Needs => "the model asked with needs",
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
    format!("target {}: {}", tilde(&target.path, home), words.join(", "))
}

fn class_words(class: PathClassName) -> &'static str {
    match class {
        PathClassName::Scratch => "scratch",
        PathClassName::UserConfig => "user config",
        PathClassName::UserData => "user data",
        PathClassName::System => "system",
        PathClassName::Secrets => "secrets",
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
    lines.push(format!("  user messages: {}", record.user_messages.len()));
    lines
}

/// The line of an `exit_judged` event in `efr history --verbose`.
pub(crate) fn exit_judged(judge: JudgeKind, verdict: Verdict, category: Option<&str>) -> String {
    let judge = match judge {
        JudgeKind::User => "the user",
        JudgeKind::Floor => "a floor",
        JudgeKind::Classifier => "the reviewer",
        JudgeKind::Always => "an always-allow rule",
        _ => "another judge",
    };
    let mut line = format!("exit {} by {judge}", verdict_words(verdict));
    if let Some(category) = category {
        let _ = write!(line, ": {}", one_line(category));
    }
    line
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
/// on the allow list)`.
pub(crate) fn blocked(blocked: &Blocked) -> String {
    let reason = match blocked.reason {
        BlockReason::NotAllowed => "not on the allow list",
        BlockReason::PrivateAddress => "a private address",
        BlockReason::SniMismatch => "the TLS name differs from the host",
        BlockReason::UploadLimit => "over the upload limit",
        BlockReason::Method => "a method that is not allowed",
        _ => "refused",
    };
    format!("network: blocked {}:{} ({reason})", one_line(&blocked.host), blocked.port)
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
