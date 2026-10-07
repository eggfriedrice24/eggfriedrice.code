//! Exits at the check point: from the engine's decision about a call of the `auto` mode
//! to the launch that runs it, the question the user reads, and the record that a judge
//! would read (phase 3).
//!
//! [`grant`] picks the narrowest launch that covers every exit of a decision. The engine
//! already chose how each write binds its target, so a launch is the union of the
//! grants, or the exit child when one exit cannot run in the sandbox. [`record`] holds
//! only user messages, the action and facts that efr collected itself: never tool
//! output, file contents or the model's reason.

use std::path::{Component, Path, PathBuf};

use efr_permissions::{Cause, Decision, Engine, ExitNeed, PathClass, Requirements, WriteBind};
use efr_protocol::{
    ActionFacts, BusKind, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitRecord,
    ExitSource, Grant, HostFact, Launch, PathClassName, ProgramFact, SandboxSummary, Scope,
    TargetFact, TurnId, Verdict,
};

/// How many refusals in a row without a person stop a turn. Not configurable: the model
/// must not get a fourth try by changing a setting.
pub(crate) const REFUSAL_LIMIT: u8 = 3;

/// What a turn fails with after [`REFUSAL_LIMIT`] refusals in a row.
pub(crate) const REFUSALS_STOPPED: &str = "auto stopped this turn: 3 actions were refused in a \
                                           row. Read the answers, then send a new prompt.";

/// What the model reads when the user says no to an exit.
pub(crate) const EXIT_DENIED: &str = "The user denied the exit; it did not run.";

/// The narrowest launch that covers every exit of `decision`, for a shell call of the
/// `auto` mode that may run.
///
/// No exit, as for a question that a user's `ask` rule raised or a `destructive` exit,
/// keeps the plain sandbox. An exit that cannot run in the sandbox (privilege,
/// persistence, upload, outside, a synced or above-root write, or a write that no bind
/// serves) runs the whole line in the exit child. Every other exit adds its grants for
/// this one call; `desktop_ipc` has none, because the daemon finds the display socket.
pub(crate) fn grant(decision: &Decision) -> Launch {
    let mut grants: Vec<Grant> = Vec::new();
    for need in decision.exits() {
        // NOTE: a floor is refused before any question, so a decision that may run has
        // none; a floor still never opens anything here.
        if need.kind.is_floor() {
            continue;
        }
        if need.runs_unsandboxed() {
            return Launch::Unsandboxed;
        }
        for grant in &need.grants {
            if !grants.contains(grant) {
                grants.push(grant.clone());
            }
        }
    }
    Launch::Contained { grants }
}

/// The kinds of the exits of `decision`, each once, in the order the engine found them.
pub(crate) fn kinds(decision: &Decision) -> Vec<ExitKind> {
    let mut kinds: Vec<ExitKind> = Vec::new();
    for need in decision.exits() {
        if !kinds.contains(&need.kind) {
            kinds.push(need.kind);
        }
    }
    kinds
}

/// The floor kinds that refused `decision`: the exits that no approval opens.
pub(crate) fn floor_kinds(decision: &Decision) -> Vec<ExitKind> {
    let mut kinds: Vec<ExitKind> = Vec::new();
    for reason in decision.deciding() {
        if let Cause::Exit { kind } = reason.cause
            && kind.is_floor()
            && !kinds.contains(&kind)
        {
            kinds.push(kind);
        }
    }
    kinds
}

/// Where the exits of `decision` came from: the model's `needs` when any of them did,
/// else efr's prediction.
pub(crate) fn source(decision: &Decision) -> ExitSource {
    if decision.exits().any(|need| need.source == ExitSource::Needs) {
        ExitSource::Needs
    } else {
        ExitSource::Predicted
    }
}

/// What the approval question shows about the exits of `decision`, which run with
/// `launch` after a "yes".
pub(crate) fn info(
    decision: &Decision,
    requirements: &Requirements,
    launch: &Launch,
    home: &Path,
) -> ExitInfo {
    ExitInfo {
        kinds: kinds(decision),
        launch: launch.clone(),
        grants: launch.grants().to_vec(),
        facts: question_facts(decision, requirements, home),
        model_reason: requirements
            .needs
            .as_ref()
            .and_then(|needs| needs.reason.clone())
            .filter(|reason| !reason.trim().is_empty()),
        judged: None,
        user_only: decision.exits().any(|need| need.user_only),
    }
}

/// efr's own facts for the question, one short sentence each: a target that does not
/// exist yet, and what a write grant does beyond a plain bind. What a grant opens is
/// the question's own line, which a client builds from the launch. A target that
/// exists gets no fact: the question names it already, and a line that says so only
/// makes the question longer. The record keeps whether each target exists.
fn question_facts(decision: &Decision, requirements: &Requirements, home: &Path) -> Vec<String> {
    let mut facts: Vec<String> = Vec::new();
    let mut push = |fact: String| {
        if !facts.contains(&fact) {
            facts.push(fact);
        }
    };
    for need in decision.exits() {
        let Some(target) = &need.target else { continue };
        let shown = tilde(target, home);
        let found = requirements.facts.as_ref().and_then(|facts| facts.target(target));
        if matches!(found, Some(None)) {
            push(format!("{shown} does not exist yet"));
        }
        match &need.bind {
            Some(WriteBind::TargetOnly) => {
                push(format!("in-place writes only; a rename over {shown} fails"));
            }
            Some(WriteBind::MakeFile) => push(format!("efr makes the empty file {shown} first")),
            Some(WriteBind::MakeDir) => push(format!("efr makes the directory {shown} first")),
            _ => {}
        }
    }
    if let Some(fact) = narrower_than_outside(decision, home) {
        push(fact);
    }
    facts
}

/// A fact for a question where the model asked to run outside the sandbox, but every
/// other exit that efr finds can run in the sandbox with a grant: the user reads that a
/// narrower grant covers what the line shows. The launch stays what the model asked
/// for; the user decides. efr reads only the line, so a script or a build can still
/// need more, and the fact says what efr found, not what the command does.
fn narrower_than_outside(decision: &Decision, home: &Path) -> Option<String> {
    let asked =
        |need: &ExitNeed| need.kind == ExitKind::Outside && need.source == ExitSource::Needs;
    if !decision.exits().any(asked) {
        return None;
    }
    let mut grants: Vec<String> = Vec::new();
    for need in decision.exits().filter(|need| !asked(need)) {
        // NOTE: a destructive exit runs in the same sandbox and opens nothing. Any
        // other exit without a grant, such as a desktop's socket that the daemon finds,
        // has no words here, so efr says nothing rather than too little.
        let unnamed = need.grants.is_empty() && need.kind != ExitKind::Destructive;
        if need.kind.is_floor() || need.runs_unsandboxed() || unnamed {
            return None;
        }
        for grant in &need.grants {
            let words = grant_words(grant, home)?;
            if !grants.contains(&words) {
                grants.push(words);
            }
        }
    }
    Some(if grants.is_empty() {
        "the line shows nothing that needs more than the sandbox; the model asked for \
         outside"
            .to_owned()
    } else {
        let grants = grants.join(", ");
        format!("a grant of {grants} covers what the line shows; the model asked for outside")
    })
}

/// What `grant` opens, in a few words, or `None` for a grant this build cannot name.
fn grant_words(grant: &Grant, home: &Path) -> Option<String> {
    Some(match grant {
        Grant::Write { path } => format!("{} writable", tilde(path, home)),
        Grant::Host { host, port } => format!("{host}:{port}"),
        Grant::OpenNetwork => "the network".to_owned(),
        Grant::Socket { path } => format!("the socket {}", tilde(path, home)),
        Grant::Bus { bus: BusKind::System } => "the system bus".to_owned(),
        Grant::Bus { bus: BusKind::Session } => "the session bus".to_owned(),
        Grant::Device { path } => format!("the device {}", tilde(path, home)),
        Grant::Unmask { path } => format!("{} readable", tilde(path, home)),
        _ => return None,
    })
}

/// What a turn knows about its own exits and calls, for the record of the next exit and
/// the refusal limit. It lives in the daemon's turn state, so the model cannot reset it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnExits {
    /// Refusals in a row without a person: floor denials, and classifier denials from
    /// phase 3. A user's answer sets it to 0.
    pub(crate) refusals_in_a_row: u8,
    /// Every exit of the turn so far with how it was judged.
    pub(crate) previous: Vec<(ExitKind, Verdict)>,
    /// The names that contained calls of the turn exported, never their values.
    pub(crate) export_names: Vec<String>,
    /// True once a call of the turn changed a git setting or another file that runs
    /// code.
    pub(crate) surface_changed: bool,
}

impl TurnExits {
    /// Notes how the exits `kinds` were judged.
    pub(crate) fn judged(&mut self, kinds: &[ExitKind], verdict: Verdict) {
        self.previous.extend(kinds.iter().map(|kind| (*kind, verdict)));
    }

    /// Counts a refusal without a person. True when the turn must stop.
    pub(crate) fn refused(&mut self) -> bool {
        self.refusals_in_a_row = self.refusals_in_a_row.saturating_add(1);
        self.refusals_in_a_row >= REFUSAL_LIMIT
    }

    /// A person answered: the count starts again.
    pub(crate) fn answered(&mut self) {
        self.refusals_in_a_row = 0;
    }

    /// Notes what a call through the launcher reported.
    pub(crate) fn ran(&mut self, summary: &SandboxSummary) {
        for name in summary.promoted.iter().chain(&summary.kept_out) {
            if !self.export_names.contains(name) {
                self.export_names.push(name.clone());
            }
        }
        self.surface_changed |= !summary.surface_changes.is_empty();
    }
}

/// The user messages of the conversation up to the turn `turn_id`, from the events of
/// `page`: each prompt and each steering text, oldest first.
pub(crate) fn user_messages(page: &[EventEnvelope], turn_id: TurnId) -> Vec<String> {
    let mut messages = Vec::new();
    for envelope in page {
        match &envelope.event {
            Event::PromptQueued { turn_id: prompt, text, .. } => {
                messages.push(text.clone());
                // NOTE: prompts queued behind this turn are not part of what the user
                // asked so far.
                if *prompt == turn_id {
                    break;
                }
            }
            Event::TurnSteered { text, .. } => messages.push(text.clone()),
            _ => {}
        }
    }
    messages
}

/// `messages` cut to [`ExitRecord::MAX_USER_MESSAGES_BYTES`], newest last: the oldest
/// go first, and a newest message that alone is too long keeps its start.
pub(crate) fn bounded(messages: &[String]) -> Vec<String> {
    let max = ExitRecord::MAX_USER_MESSAGES_BYTES;
    let mut kept: Vec<String> = Vec::new();
    let mut bytes = 0;
    for message in messages.iter().rev() {
        if bytes + message.len() <= max {
            bytes += message.len();
            kept.push(message.clone());
            continue;
        }
        if kept.is_empty() {
            let mut end = max;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            kept.push(message[..end].to_owned());
        }
        break;
    }
    kept.reverse();
    kept
}

/// Everything [`record`] reads about one call.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Action<'a> {
    /// The tool, such as `shell`.
    pub(crate) tool: &'a str,
    /// The decision about the call.
    pub(crate) decision: &'a Decision,
    /// What the call declared, with the daemon's facts.
    pub(crate) requirements: &'a Requirements,
    /// How the call runs after a "yes".
    pub(crate) launch: &'a Launch,
    /// The directory the call starts in.
    pub(crate) cwd: &'a Path,
    /// The turn's scope.
    pub(crate) scope: &'a Scope,
    /// The conversation's `$SCRATCH`.
    pub(crate) scratch: &'a Path,
    /// The engine that decided, for where each path stands.
    pub(crate) engine: &'a Engine,
}

/// The record of the exits of `action`: the user messages, the action, and the facts
/// that efr collected itself.
pub(crate) fn record(action: Action<'_>, messages: &[String], turn: &TurnExits) -> ExitRecord {
    let messages = bounded(messages);
    let home = action.engine.locations().home();
    let named = |forms: &[String]| {
        messages.iter().any(|message| forms.iter().any(|form| message.contains(form.as_str())))
    };
    let mut targets: Vec<TargetFact> = Vec::new();
    for need in action.decision.exits() {
        let Some(path) = &need.target else { continue };
        if targets.iter().any(|known| known.path == *path) {
            continue;
        }
        let place = action.engine.path_facts(path, action.scope, action.scratch);
        let exists = action
            .requirements
            .facts
            .as_ref()
            .and_then(|facts| facts.target(path))
            .is_none_or(|kind| kind.is_some());
        targets.push(TargetFact {
            path: path.clone(),
            class: place.map(|place| class_name(place.class)),
            in_write_root: place.is_some_and(|place| place.in_write_root),
            floor: place.is_some_and(|place| place.floor),
            synced: place.is_some_and(|place| place.synced),
            exists,
            named_in_user_messages: named(&path_forms(path, home)),
        });
    }
    let mut hosts: Vec<HostFact> = Vec::new();
    let mut add_host = |host: String| {
        if !host.is_empty() && !hosts.iter().any(|known| known.host == host) {
            let named_in_user_messages = named(std::slice::from_ref(&host));
            hosts.push(HostFact {
                host,
                on_allow_list: false,
                named_in_user_messages,
                refused_by_proxy: false,
            });
        }
    };
    for need in action.decision.exits() {
        for grant in &need.grants {
            if let Grant::Host { host, .. } = grant {
                add_host(host.to_lowercase());
            }
        }
    }
    for host in action.requirements.needs.iter().flat_map(|needs| &needs.hosts) {
        add_host(host.trim().to_lowercase());
    }
    let programs = action
        .requirements
        .facts
        .iter()
        .flat_map(|facts| &facts.programs)
        .map(|(word, resolved, changed)| ProgramFact {
            word: word.clone(),
            resolved: resolved.clone(),
            // NOTE: a relative path names no file: the shell runs the word itself.
            in_write_root: resolved
                .as_deref()
                .filter(|resolved| resolved.is_absolute())
                .is_some_and(|resolved| {
                    action
                        .engine
                        .path_facts(resolved, action.scope, action.scratch)
                        .is_some_and(|place| place.in_write_root)
                }),
            changed_this_turn: *changed,
        })
        .collect();
    let upload_patterns = action
        .decision
        .exits()
        .filter(|need| need.kind == ExitKind::Upload)
        .map(|need: &ExitNeed| need.part.clone())
        .collect();
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: messages.clone(),
        action: ActionFacts {
            tool: action.tool.to_owned(),
            line: action.requirements.command.clone().unwrap_or_default(),
            cwd: action.cwd.to_path_buf(),
            scope: action.scope.clone(),
            exits: kinds(action.decision),
            grants: action.launch.grants().to_vec(),
            source: source(action.decision),
        },
        facts: ExitFacts {
            targets,
            hosts,
            programs,
            upload_patterns,
            repo_surface_changed_this_turn: turn.surface_changed,
            git_status: None,
            snapshot_covers: None,
            sandbox_export_names: turn.export_names.clone(),
            previous_exits_this_turn: turn.previous.clone(),
            refusals_in_a_row: turn.refusals_in_a_row,
        },
    }
}

/// The forms of `path` that count as naming it in a user message: the absolute form,
/// the `~` form and its last two components.
fn path_forms(path: &Path, home: &Path) -> Vec<String> {
    let mut forms = vec![path.display().to_string()];
    let shown = tilde(path, home);
    if !forms.contains(&shown) {
        forms.push(shown);
    }
    let names: Vec<&str> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    if let [.., parent, name] = names.as_slice() {
        forms.push(format!("{parent}/{name}"));
    }
    forms
}

/// `path` with the home directory written as `~`.
pub(crate) fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The protocol's name of the engine's path class.
fn class_name(class: PathClass) -> PathClassName {
    match class {
        PathClass::Scratch => PathClassName::Scratch,
        PathClass::UserConfig => PathClassName::UserConfig,
        PathClass::UserData => PathClassName::UserData,
        PathClass::System => PathClassName::System,
        PathClass::Secrets => PathClassName::Secrets,
    }
}

/// The changes that a call reported and the launcher moved to quarantine, for the
/// question before the next call.
pub(crate) fn quarantined(summary: &SandboxSummary) -> Vec<efr_protocol::SurfaceChange> {
    summary.surface_changes.iter().filter(|change| change.quarantined).cloned().collect()
}

/// The quarantined changes as the model reads them, such as
/// `/home/u/p/app/.git/commondir (core.fsmonitor)`.
pub(crate) fn change_names(changes: &[efr_protocol::SurfaceChange]) -> String {
    let names: Vec<String> = changes
        .iter()
        .map(|change| match &change.key {
            Some(key) => format!("{} ({key})", change.path.display()),
            None => change.path.display().to_string(),
        })
        .collect();
    names.join("; ")
}

/// The directory a call starts in, for its record: where the command starts, else the
/// hidden shell, else the user's directory.
pub(crate) fn start_dir(
    requirements: &Requirements,
    shell_cwd: Option<&Path>,
    cwd: &Path,
) -> PathBuf {
    requirements.command_dir.as_deref().or(shell_cwd).unwrap_or(cwd).to_path_buf()
}

#[cfg(test)]
mod tests;
