//! Exits at the check point: from the engine's decision about a call of the `auto` mode
//! to the launch that runs it, who approves it, the question the user reads, and the
//! record that the classifier reads.
//!
//! [`grant`] picks the narrowest launch that covers every exit of a decision. The engine
//! already chose how each write binds its target, so a launch is the union of the
//! grants, or the exit child when one exit cannot run in the sandbox. [`route`] finishes
//! the engine's approver of each exit with the user's messages: an exit that the
//! classifier may judge only when a message names it goes to the user when none does.
//! [`record`] holds only user messages, the action and facts that efr collected itself:
//! never tool output, file contents or the model's reason. The one piece of model text
//! besides the line is the payload of a file tool's floor edit ([`Payload`]), so the
//! classifier judges what efr will write. The one piece of file content is the text of
//! a file that a named persistence form installs, such as the source of a link into a
//! unit dir, which efrd read outside every write root ([`ExitNeed::installs`]). [`effective`] applies the allow rule and the
//! naming rule of efr's auto spec (section 9.4) to the classifier's verdict.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use efr_permissions::exits::{self, Ecosystem, REGISTRY_HOSTS};
use efr_permissions::{
    Approver, Cause, Decision, Egress, Engine, ExitNeed, PathClass, Rebuild, Requirements,
    TargetKind, WriteBind,
};
use efr_protocol::{
    ActionFacts, BusKind, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitRecord,
    ExitSource, Grant, HostFact, Judgement, Launch, Mode, PathClassName, PauseReason, ProgramFact,
    Risk, SandboxSummary, Scope, TargetFact, TurnId, UserAuthorization, Verdict,
};
use efr_store::conversations::{Turn as StoredTurn, TurnStatus};
use serde_json::Value;

/// The time limit of a background job, in minutes, when the call's requirements name
/// none: the default of `jobs.default_minutes`. efrd names the job's own limit, after
/// its default and its cut, in the requirements it declares.
pub(crate) const JOB_MINUTES: u32 = 60;

/// The time limit of the background job that a call with `requirements` starts, or
/// `None` for a call in the foreground.
pub(crate) fn job_minutes(requirements: &Requirements) -> Option<u32> {
    requirements.background.then(|| requirements.max_minutes.unwrap_or(JOB_MINUTES))
}

/// How many refusals in a row without a person stop a turn. Not configurable: the model
/// must not get a fourth try by changing a setting.
pub(crate) const REFUSAL_LIMIT: u8 = 3;

/// What a turn of `mode` fails with after [`REFUSAL_LIMIT`] refusals in a row. It names
/// the mode that stopped the turn, so a full turn never says that `auto` stopped it.
pub(crate) fn refusals_stopped(mode: Mode) -> String {
    let who = match mode {
        Mode::Auto => "auto",
        Mode::Full => "full mode",
        _ => "efr",
    };
    format!(
        "{who} stopped this turn: 3 actions were refused in a row. Read the answers, then send \
         a new prompt."
    )
}

/// How long a question about a delete that full mode guards waits for the user before
/// the delete does not run (efr's full mode, DF5). Not a setting: a longer wait would
/// let the model park the turn on a question nobody reads.
pub(crate) const CRITICAL_WAIT: Duration = Duration::from_secs(120);

/// How many questions about a guarded delete may expire in one turn; after that each
/// later guarded delete of the turn is refused with no question. An answer does not
/// reset the count.
pub(crate) const CRITICAL_EXPIRIES: u8 = 3;

/// What the model reads when nobody answered the question about the guarded delete of
/// `targets`: `waited` is how long the question waited, `None` when nobody was there to
/// ask.
pub(crate) fn critical_expired(targets: &str, waited: Option<Duration>) -> String {
    let why = match waited {
        Some(waited) => {
            format!("the user did not answer in {} seconds", waited.as_secs())
        }
        None => "nobody was there to answer".to_owned(),
    };
    format!(
        "efr did not run this delete of {targets}: {why}. Do not try it another way. Delete \
         narrower paths that you can name, or ask the user to run it."
    )
}

/// What the model reads for a guarded delete of `targets` after
/// [`CRITICAL_EXPIRIES`] questions of the turn expired: it does not run, with no
/// question.
pub(crate) fn critical_refused(targets: &str) -> String {
    format!(
        "efr refuses deletes of {targets} for the rest of this turn. Do not try it another \
         way. Delete narrower paths that you can name, or ask the user to run it."
    )
}

/// How many classifier denials in a row stop the classifier for the rest of the turn.
/// Not configurable, as [`REFUSAL_LIMIT`] is not.
pub(crate) const DENIALS_IN_A_ROW: u8 = 3;

/// How many classifier denials in one turn stop the classifier for the rest of it.
pub(crate) const DENIALS_PER_TURN: u8 = 10;

/// What the model reads when the user says no to an exit. It tells the model not to
/// ask again in another form: one "no" must not become a question for each file.
pub(crate) const EXIT_DENIED: &str = "The user said no to this action; it did not run. Do not \
                                      ask again for this action or a similar one in this \
                                      turn. Finish the work that stays in the sandbox, then \
                                      tell the user what you wanted to do and why.";

/// What the model reads when nobody answered a question while the user was away (efr's
/// auto spec, section 9.5).
pub(crate) const AWAY_EXPIRED: &str = "Nobody could approve this now; it did not run. Continue \
                                       without it, and name it in your final answer.";

/// What the model reads when the classifier refused an exit (efr's auto spec, section
/// 7.6), with the classifier's category and rationale.
pub(crate) fn reviewer_denied(judgement: &Judgement) -> String {
    format!(
        "The auto reviewer refused this action ({}): {}. Do not try to reach the same result \
         another way. Ask the user, or choose a step that stays in the sandbox.",
        judgement.category.trim(),
        judgement.rationale.trim().trim_end_matches('.'),
    )
}

/// What a turn fails with when the classifier stopped for the turn and nobody is there
/// to decide an exit that it would have judged.
pub(crate) fn away_stopped(reason: PauseReason, denials: u8) -> String {
    let refused = match reason {
        PauseReason::PerTurn => format!("refused {denials} actions in this turn"),
        _ => format!("refused {denials} actions in a row"),
    };
    format!(
        "auto stopped this turn: the reviewer {refused} and nobody could decide. Read the \
         answers, then send a new prompt."
    )
}

/// The registry exception of the naming rule (efr's auto spec, section 9.4): a package
/// manager's fetch may reach the registries of its ecosystem without a user message that
/// names them, when a user message asks to build, install, update or fetch. False turns
/// it off.
pub(crate) const REGISTRY_EXCEPTION: bool = true;

/// The words of a user message that ask for a package manager's fetch.
const FETCH_WORDS: &[&str] = &[
    "build",
    "builds",
    "building",
    "install",
    "installs",
    "installing",
    "update",
    "updates",
    "updating",
    "fetch",
    "fetches",
    "fetching",
];

/// The narrowest launch that covers every exit of `decision`, for a shell call of the
/// `auto` mode that may run.
///
/// No exit, as for a question that a user's `ask` rule raised or a `destructive` exit,
/// keeps the plain sandbox. An exit that cannot run in the sandbox (privilege,
/// persistence, upload, outside, a synced or above-root write, or a write that no bind
/// serves) runs the whole line in the exit child. Every other exit adds its grants for
/// this one call; a `desktop_ipc` grant names the desktop, and the daemon finds its one
/// control socket for the terminal of the prompt.
///
/// The grants of the views of the line (`Decision::view_grants`) come after the grants
/// of the exits, by the union rule of [`view_fits`]: a view that does not fit the
/// other grants drops, and that part of the line then sees the sandbox's view.
pub(crate) fn grant(decision: &Decision) -> Launch {
    launch(decision.exits(), decision.view_grants())
}

/// The launch of [`grant`] for the exits `exits` and the view grants `views`.
fn launch<'a>(
    exits: impl Iterator<Item = &'a ExitNeed>,
    views: impl Iterator<Item = &'a Grant>,
) -> Launch {
    let mut grants: Vec<Grant> = Vec::new();
    for need in exits {
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
    // NOTE: each view is checked against the exit grants and the views that joined
    // before it, so two views that do not fit each other never join one launch.
    for view in views {
        if !grants.contains(view) && view_fits(view, &grants) {
            grants.push(view.clone());
        }
    }
    Launch::Contained { grants }
}

/// True when the view `view` may join a launch with the grants `grants` (the exits'
/// grants and the views that joined before), as an allow list.
///
/// - `NetView` stays only beside `Write` and `Unmask` grants, which do not depend on
///   the network namespace. With any other grant it drops: the NetView seccomp profile
///   would fail the `socket()` of a network grant, and a device such as `/dev/net/tun`
///   could send frames from the host's namespace with no `socket()` call.
/// - `BusFilter` of a bus (phase E) stays beside every grant except `NetView` and a
///   `Bus` grant of the same bus: an approved exit to the real bus of the call serves
///   the line's reads too, and the sandbox spec refuses the pair.
///
/// A view that this build does not know never joins.
fn view_fits(view: &Grant, grants: &[Grant]) -> bool {
    match view {
        Grant::NetView => {
            grants.iter().all(|grant| matches!(grant, Grant::Write { .. } | Grant::Unmask { .. }))
        }
        Grant::BusFilter { bus } => grants.iter().all(|grant| match grant {
            Grant::NetView => false,
            Grant::Bus { bus: real } => real != bus,
            _ => true,
        }),
        _ => false,
    }
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

/// Where the exits of `decision` came from: a delete in a real cache when any of them
/// did, else the model's `needs` when any of them did, else efr's prediction.
pub(crate) fn source(decision: &Decision) -> ExitSource {
    if decision.exits().any(|need| need.source == ExitSource::CacheDelete) {
        ExitSource::CacheDelete
    } else if decision.exits().any(|need| need.source == ExitSource::Needs) {
        ExitSource::Needs
    } else {
        ExitSource::Predicted
    }
}

/// Who approves `need` once the user's messages of the conversation are known: an exit
/// that the classifier judges only when a message names it goes to the classifier when
/// one of `messages` names it, else to the user. `messages` must be the uncut list, so
/// the route and the record read the same names.
pub(crate) fn approver(need: &ExitNeed, messages: &[String]) -> Approver {
    match need.approver {
        Approver::ClassifierWhenNamed if exits::names_match(&need.names, messages) => {
            Approver::Classifier
        }
        Approver::ClassifierWhenNamed => Approver::User,
        approver => approver,
    }
}

/// Who approves the exits of `decision`: the strictest approver of its exits, after
/// [`approver`]. A payload of a file tool edit that the classifier cannot judge whole
/// ([`Payload::judgeable`]) gives the user. `None` for a decision without exits.
pub(crate) fn route(
    decision: &Decision,
    messages: &[String],
    payload: Option<&Payload>,
) -> Option<Approver> {
    let approver = decision.exits().map(|need| approver(need, messages)).max()?;
    if payload.is_some_and(|payload| !payload.judgeable()) {
        return Some(Approver::User);
    }
    Some(approver)
}

/// The tools that change files in efrd, outside the sandbox.
const FILE_TOOLS: [&str; 3] = ["write_file", "edit", "apply_patch"];

/// What a file tool writes to a file that runs code later (a `persistence` exit, such as
/// `~/.zshrc`), for the record (`ActionFacts::edit`): the new content of `write_file`,
/// the old and the new text of `edit`, the parts of an `apply_patch` for those files.
/// The model wrote it, so it can hold attacker text, as the line can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Payload {
    text: String,
    /// False when efr could not read the whole payload from the input.
    whole: bool,
}

impl Payload {
    /// What marks a payload that the record holds only in part.
    const CUT: &'static str = "\n[efr cut the payload here: it is longer than 8 KiB, so the \
                               user decides]";

    /// True when the classifier may judge it: efr read all of it, and it is at most
    /// [`ActionFacts::MAX_EDIT_BYTES`]. A longer one goes to the user, so the
    /// classifier never allows an edit that it saw only in part.
    pub(crate) fn judgeable(&self) -> bool {
        self.whole && self.text.len() <= ActionFacts::MAX_EDIT_BYTES
    }

    /// The text for the record: all of it, or, for one that the user decides, its start
    /// and a marker, within [`ActionFacts::MAX_EDIT_BYTES`].
    pub(crate) fn recorded(&self) -> String {
        if self.text.len() <= ActionFacts::MAX_EDIT_BYTES {
            return self.text.clone();
        }
        let mut end = ActionFacts::MAX_EDIT_BYTES - Self::CUT.len();
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{}", &self.text[..end], Self::CUT)
    }
}

/// The payload of a file tool call that writes a file that runs code later (an exit of
/// the kind [`ExitKind::Persistence`]), or of a shell line whose named persistence form
/// installs another file ([`ExitNeed::installs`]: each file's path and text); `None`
/// for any other call. A plain write outside the roots, such as a large file in
/// `~/Documents`, has none: efrd copies it before the write too
/// ([`ExitNeed::backup_before_write`]), but the 8 KiB rule of the payload is for code
/// only. `home` and `cwd` resolve a path of a patch as the tool does.
pub(crate) fn payload(
    tool: &str,
    input: &Value,
    decision: &Decision,
    home: &Path,
    cwd: &Path,
) -> Option<Payload> {
    if !FILE_TOOLS.contains(&tool) {
        return installed(decision);
    }
    let targets: Vec<&Path> = decision
        .exits()
        .filter(|need| need.kind == ExitKind::Persistence)
        .filter_map(|need| need.target.as_deref())
        .collect();
    if targets.is_empty() {
        return None;
    }
    let text = |key: &str| input.get(key).and_then(Value::as_str);
    let found = match tool {
        "write_file" => text("content").map(str::to_owned),
        "edit" => match (text("old_string"), text("new_string")) {
            (Some(old), Some(new)) => {
                let all = if input.get("replace_all") == Some(&Value::Bool(true)) {
                    " (every occurrence)"
                } else {
                    ""
                };
                Some(format!("old_string{all}:\n{old}\nnew_string:\n{new}"))
            }
            _ => None,
        },
        _ => patch_text(input).map(|patch| patch_parts(patch, &targets, home, cwd)),
    };
    Some(match found {
        Some(text) => Payload { text, whole: true },
        None => Payload { text: String::new(), whole: false },
    })
}

/// The payload of the files that the exits of `decision` install, each once: its path
/// on a line, then its text. `None` when they install none.
fn installed(decision: &Decision) -> Option<Payload> {
    let mut seen: Vec<&Path> = Vec::new();
    let mut text = String::new();
    for file in decision.exits().flat_map(|need| &need.installs) {
        if seen.contains(&file.path.as_path()) {
            continue;
        }
        seen.push(&file.path);
        text.push_str(&format!("{}:\n{}", file.path.display(), file.text));
        if !text.ends_with('\n') {
            text.push('\n');
        }
    }
    (!seen.is_empty()).then_some(Payload { text, whole: true })
}

/// The text of an `apply_patch` call: the input itself in the freeform form, or its one
/// `input` member in the function form.
fn patch_text(input: &Value) -> Option<&str> {
    match input {
        Value::String(text) => Some(text),
        Value::Object(members) => members.get("input").and_then(Value::as_str),
        _ => None,
    }
}

/// The parts of `patch` that change one of `targets`, each from its `*** Add File:`,
/// `*** Update File:` or `*** Delete File:` line to the next such line. When a target
/// has no part of its own, as the real file behind a link that the patch names by the
/// link, the whole patch: the classifier then reads more, never less.
fn patch_parts(patch: &str, targets: &[&Path], home: &Path, cwd: &Path) -> String {
    const HEADS: [&str; 3] = ["*** Add File: ", "*** Update File: ", "*** Delete File: "];
    let mut parts: Vec<(PathBuf, String)> = Vec::new();
    for line in patch.lines() {
        if let Some(raw) = HEADS.iter().find_map(|head| line.strip_prefix(head)) {
            parts.push((resolve(raw.trim(), home, cwd), String::new()));
        } else if line.starts_with("*** End Patch") {
            continue;
        }
        if let Some((_, text)) = parts.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    let mut kept: Vec<&str> = Vec::new();
    for target in targets {
        let mine: Vec<&str> = parts
            .iter()
            .filter(|(path, _)| path == target)
            .map(|(_, text)| text.as_str())
            .collect();
        if mine.is_empty() {
            return patch.to_owned();
        }
        for text in mine {
            if !kept.contains(&text) {
                kept.push(text);
            }
        }
    }
    kept.concat()
}

/// `raw`, a path as a file tool's input writes it, made absolute as the tools do: `~`
/// is the home directory, and a relative path starts at `cwd`.
fn resolve(raw: &str, home: &Path, cwd: &Path) -> PathBuf {
    let path = if raw == "~" {
        home.to_path_buf()
    } else if let Some(below) = raw.strip_prefix("~/") {
        home.join(below)
    } else {
        cwd.join(raw)
    };
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normal.push("/"),
            Component::Normal(name) => normal.push(name),
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir | Component::Prefix(_) => {}
        }
    }
    normal
}

/// What efr does with the classifier's `judgement` about the exits of `record` (efr's
/// auto spec, section 9.4).
///
/// The allow rule: an `allow` stands only at a low or medium risk, or at a high risk
/// when a user message authorizes it at least at medium; a critical risk never allows.
/// The naming rule: an exit that writes, edits a file that runs code later or reaches
/// above a root needs a user message that names each target that efr cannot rebuild
/// (one outside a cache, build output and efr's trash, and outside the write roots);
/// a host needs one that names it, except the registries of the package manager's
/// fetch `fetch` when a user message asks to build, install, update or fetch
/// ([`REGISTRY_EXCEPTION`]). An `allow` that breaks a rule becomes `ask_user`. The
/// classes and the names come from efr's facts in the record, never from the line.
pub(crate) fn effective(
    record: &ExitRecord,
    judgement: &Judgement,
    fetch: Option<Ecosystem>,
) -> Verdict {
    match judgement.verdict {
        Verdict::Allow => {}
        Verdict::Deny => return Verdict::Deny,
        _ => return Verdict::AskUser,
    }
    let authorized =
        matches!(judgement.user_authorization, UserAuthorization::Medium | UserAuthorization::High);
    let risk_fits = match judgement.risk {
        Risk::Low | Risk::Medium => true,
        Risk::High => authorized,
        _ => false,
    };
    if !risk_fits || !named(record, fetch) {
        return Verdict::AskUser;
    }
    Verdict::Allow
}

/// True when the record's user messages name every target and host that the naming
/// rule of [`effective`] asks a name for.
fn named(record: &ExitRecord, fetch: Option<Ecosystem>) -> bool {
    let needs_targets =
        record.action.exits.iter().any(|kind| {
            matches!(kind, ExitKind::Write | ExitKind::Persistence | ExitKind::AboveRoot)
        });
    let rebuilt = |class: Option<PathClassName>| {
        matches!(
            class,
            Some(PathClassName::Cache | PathClassName::BuildOutput | PathClassName::Trash)
        )
    };
    if needs_targets
        && record.facts.targets.iter().any(|target| {
            !target.in_write_root && !rebuilt(target.class) && !target.named_in_user_messages
        })
    {
        return false;
    }
    let registries: &[&str] = match fetch {
        Some(ecosystem) if REGISTRY_EXCEPTION && asks_to_fetch(&record.user_messages) => {
            REGISTRY_HOSTS
                .iter()
                .find(|(known, _)| *known == ecosystem)
                .map_or(&[], |(_, hosts)| hosts)
        }
        _ => &[],
    };
    // NOTE: `refused_by_proxy` is never a name: it shows only that a command tried the
    // host. A project's own remote counts as named; an upload still needs its verb in
    // a user message, which its route checks.
    record.facts.hosts.iter().all(|host| {
        host.on_allow_list
            || host.named_in_user_messages
            || host.project_remote
            || registries.contains(&host.host.as_str())
    })
}

/// True when a message of `messages` asks to build, install, update or fetch.
fn asks_to_fetch(messages: &[String]) -> bool {
    let words: Vec<String> = FETCH_WORDS.iter().map(|word| (*word).to_owned()).collect();
    exits::names_match(&words, messages)
}

/// The package ecosystem whose registries a `host` exit of `decision` fetches from, for
/// the registry exception of [`effective`].
pub(crate) fn package_fetch(decision: &Decision) -> Option<Ecosystem> {
    decision.exits().find_map(|need| need.package_fetch)
}

/// What the approval question shows about the exits of `decision`, which run with
/// `launch` after a "yes".
pub(crate) fn info(
    decision: &Decision,
    requirements: &Requirements,
    launch: &Launch,
    home: &Path,
    approver: Approver,
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
        user_only: approver != Approver::Classifier,
        always: None,
    }
}

/// efr's own facts for the question, one short sentence each: how long a background
/// job and its grants last, a target that does not
/// exist yet, a target that is a directory, and what a write grant does beyond a plain
/// bind. What a grant opens is the question's own line, which a client builds from the
/// launch. A file that exists gets no fact: the question names it already, and a line
/// that says so only makes the question longer. A directory does: a write grant of it
/// opens every file below it, which its name alone does not say. The record keeps
/// whether each target exists.
fn question_facts(decision: &Decision, requirements: &Requirements, home: &Path) -> Vec<String> {
    let mut facts: Vec<String> = Vec::new();
    let mut push = |fact: String| {
        if !facts.contains(&fact) {
            facts.push(fact);
        }
    };
    // NOTE: a job's grants are not one-shot: they stay open while nobody watches it.
    if let Some(minutes) = job_minutes(requirements) {
        push(crate::job_lifetime_fact(minutes));
    }
    for need in decision.exits() {
        let Some(target) = &need.target else { continue };
        let shown = tilde(target, home);
        match requirements.facts.as_ref().and_then(|facts| facts.target(target)) {
            Some(None) => push(format!("{shown} does not exist yet")),
            Some(Some(TargetKind::Dir)) => {
                let opened = need.grants.iter().any(|grant| match grant {
                    Grant::Write { path } => path == target,
                    _ => false,
                });
                push(if opened {
                    format!("{shown} is a directory: the call can write every file in it")
                } else {
                    format!("{shown} is a directory")
                });
            }
            _ => {}
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
        // other exit without a grant, such as `ydotool`, which talks to its own daemon,
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
        Grant::Desktop { desktop } => format!("the {} socket", desktop.name()),
        Grant::Device { path } => format!("the device {}", tilde(path, home)),
        Grant::Unmask { path } => format!("{} readable", tilde(path, home)),
        Grant::NetView => "the host's network view".to_owned(),
        _ => return None,
    })
}

/// What a turn knows about its own exits and calls, for the record of the next exit and
/// the limits on refusals and denials. It lives in the daemon's turn state, so the model
/// cannot reset it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnExits {
    /// Floor refusals in a row without a person's answer. A user's answer sets it to 0;
    /// a classifier allow does not.
    pub(crate) refusals_in_a_row: u8,
    /// Classifier denials since its last allow or the user's last answer.
    pub(crate) denials_in_a_row: u8,
    /// Classifier denials in the turn.
    pub(crate) denials_this_turn: u8,
    /// Why the classifier stopped for the rest of the turn, with the denials that the
    /// limit counted; `None` while it judges.
    pub(crate) paused: Option<(PauseReason, u8)>,
    /// Every exit of the turn so far with how it was judged.
    pub(crate) previous: Vec<(ExitKind, Verdict)>,
    /// The names that contained calls of the turn exported, never their values.
    pub(crate) export_names: Vec<String>,
    /// True once a call of the turn changed a git setting or another file that runs
    /// code.
    pub(crate) surface_changed: bool,
    /// True in a turn that a job notice started, not a prompt: nobody asked for its
    /// steps now (phase C).
    pub(crate) wake_turn: bool,
    /// Questions about a delete that full mode guards that expired in this turn, also
    /// those denied at once because nobody was there. An answer does not reset it; at
    /// [`CRITICAL_EXPIRIES`] each later guarded delete is refused with no question.
    pub(crate) critical_expired: u8,
}

impl TurnExits {
    /// Notes how the exits `kinds` were judged.
    pub(crate) fn judged(&mut self, kinds: &[ExitKind], verdict: Verdict) {
        self.previous.extend(kinds.iter().map(|kind| (*kind, verdict)));
    }

    /// Counts a floor refusal without a person. True when the turn must stop.
    pub(crate) fn refused(&mut self) -> bool {
        self.refusals_in_a_row = self.refusals_in_a_row.saturating_add(1);
        self.refusals_in_a_row >= REFUSAL_LIMIT
    }

    /// A person answered: both counts in a row start again.
    pub(crate) fn answered(&mut self) {
        self.refusals_in_a_row = 0;
        self.denials_in_a_row = 0;
    }

    /// The classifier allowed an exit: its denials in a row start again. The floor
    /// refusals do not: only a person resets them.
    pub(crate) fn allowed(&mut self) {
        self.denials_in_a_row = 0;
    }

    /// Counts a classifier denial. When it reaches a limit, the classifier stops for
    /// the rest of the turn, and the reason comes back once, with the denials that the
    /// limit counted.
    pub(crate) fn denied(&mut self) -> Option<(PauseReason, u8)> {
        self.denials_in_a_row = self.denials_in_a_row.saturating_add(1);
        self.denials_this_turn = self.denials_this_turn.saturating_add(1);
        if self.paused.is_some() {
            return None;
        }
        let paused = if self.denials_in_a_row >= DENIALS_IN_A_ROW {
            (PauseReason::InARow, self.denials_in_a_row)
        } else if self.denials_this_turn >= DENIALS_PER_TURN {
            (PauseReason::PerTurn, self.denials_this_turn)
        } else {
            return None;
        };
        self.paused = Some(paused);
        Some(paused)
    }

    /// Counts a question about a guarded delete that expired.
    pub(crate) fn critical_expired(&mut self) {
        self.critical_expired = self.critical_expired.saturating_add(1);
    }

    /// True once [`CRITICAL_EXPIRIES`] questions about guarded deletes expired in this
    /// turn: each later guarded delete is refused with no question.
    pub(crate) fn critical_closed(&self) -> bool {
        self.critical_expired >= CRITICAL_EXPIRIES
    }

    /// True once the classifier stopped for the rest of the turn.
    pub(crate) fn classifier_off(&self) -> bool {
        self.paused.is_some()
    }

    /// The counts that a turn which job notices start goes on with, from this one: the
    /// refusals and classifier denials, and the pause (phase C).
    pub(crate) fn carried(&self) -> TurnExits {
        TurnExits {
            refusals_in_a_row: self.refusals_in_a_row,
            denials_in_a_row: self.denials_in_a_row,
            denials_this_turn: self.denials_this_turn,
            paused: self.paused,
            ..TurnExits::default()
        }
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
/// `page`: each prompt and each steering text, oldest first. A prompt that the user
/// withdrew is left out, and so is a steer that an interrupt sent again as a prompt,
/// which counts once, as that prompt, or that the user took back.
///
/// A job notice is never one: it is a `job_notice` event, which `page` does not hold.
pub(crate) fn user_messages(page: &[EventEnvelope], turn_id: TurnId) -> Vec<String> {
    messages_until(page, Some(turn_id), &HashSet::new())
}

/// The user messages before a turn that job notices started (phase C), from the events
/// of `page`: every prompt and steer of the conversation before the turn, as
/// [`user_messages`] reads them. A wake turn starts only when no prompt waits, so a
/// prompt of `waiting`, the turns that wait, came after the wake turn started: it is
/// no earlier request, and it is left out.
pub(crate) fn wake_user_messages(page: &[EventEnvelope], waiting: &HashSet<TurnId>) -> Vec<String> {
    messages_until(page, None, waiting)
}

/// The turns of `turns` that wait to start.
pub(crate) fn waiting_turns(turns: &[StoredTurn]) -> HashSet<TurnId> {
    turns
        .iter()
        .filter(|turn| matches!(turn.status, TurnStatus::Queued | TurnStatus::Held))
        .map(|turn| turn.id)
        .collect()
}

/// The user messages of `page` up to the prompt of `until`, without the prompts of the
/// turns `skipped`.
fn messages_until(
    page: &[EventEnvelope],
    until: Option<TurnId>,
    skipped: &HashSet<TurnId>,
) -> Vec<String> {
    let withdrawn: HashSet<TurnId> = page
        .iter()
        .filter_map(|envelope| match envelope.event {
            Event::PromptWithdrawn { turn_id, .. } => Some(turn_id),
            _ => None,
        })
        .collect();
    let left = crate::history::left_steers(page);
    let mut messages = Vec::new();
    for envelope in page {
        match &envelope.event {
            Event::PromptQueued { turn_id: prompt, .. }
                if withdrawn.contains(prompt) || skipped.contains(prompt) => {}
            Event::TurnSteered { .. } if left.contains(&envelope.seq) => {}
            Event::PromptQueued { turn_id: prompt, text, .. } => {
                messages.push(text.clone());
                // NOTE: prompts queued behind this turn are not part of what the user
                // asked so far.
                if Some(*prompt) == until {
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
    /// What a file tool writes to a floor file, when it writes one.
    pub(crate) payload: Option<&'a Payload>,
}

/// The record of the exits of `action`: the user messages, the action, and the facts
/// that efr collected itself.
///
/// `messages` is every user message of the conversation. The record keeps the newest
/// ones within [`ExitRecord::MAX_USER_MESSAGES_BYTES`] ([`bounded`]), but whether a
/// message names a target or a host is checked on all of them, with the same check as
/// the route ([`approver`]), so the two never disagree.
pub(crate) fn record(action: Action<'_>, messages: &[String], turn: &TurnExits) -> ExitRecord {
    let home = action.engine.locations().home();
    let mut targets: Vec<TargetFact> = Vec::new();
    for need in action.decision.exits() {
        let Some(path) = &need.target else { continue };
        let forms = if need.names.is_empty() { path_forms(path, home) } else { need.names.clone() };
        let named = exits::names_match(&forms, messages);
        if let Some(known) = targets.iter_mut().find(|known| known.path == *path) {
            known.named_in_user_messages |= named;
            known.backup_before_write |= need.backup_before_write;
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
            class: place.map(|place| match place.rebuild {
                Some(rebuild) => rebuild_name(rebuild).unwrap_or_else(|| class_name(place.class)),
                None => class_name(place.class),
            }),
            in_write_root: place.is_some_and(|place| place.in_write_root),
            floor: place.is_some_and(|place| place.floor),
            synced: place.is_some_and(|place| place.synced),
            exists,
            named_in_user_messages: named,
            backup_before_write: need.backup_before_write,
            masked: place.is_some_and(|place| place.masked),
        });
    }
    let hosts = host_facts(action.decision, action.requirements, action.engine, messages);
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
    let desktop_read = desktop_read(action.decision);
    let upload_patterns = action
        .decision
        .exits()
        .filter(|need| need.kind == ExitKind::Upload)
        .map(|need: &ExitNeed| need.part.clone())
        .collect();
    // NOTE: the command that leaves, not the first of the line: `uname; df -h` runs
    // `df -h` outside, and a label of `uname` would name the wrong program. Only a
    // predicted need names a command: the part of `needs.outside` is the model's field.
    let mut outside_commands: Vec<String> = Vec::new();
    let leaving =
        |need: &&ExitNeed| need.kind == ExitKind::Outside && need.source == ExitSource::Predicted;
    for need in action.decision.exits().filter(leaving) {
        if !outside_commands.contains(&need.part) {
            outside_commands.push(need.part.clone());
        }
    }
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: bounded(messages),
        action: ActionFacts {
            tool: action.tool.to_owned(),
            line: action.requirements.command.clone().unwrap_or_default(),
            cwd: action.cwd.to_path_buf(),
            scope: action.scope.clone(),
            exits: kinds(action.decision),
            grants: action.launch.grants().to_vec(),
            source: source(action.decision),
            edit: action.payload.map(Payload::recorded),
            background: action.requirements.background,
            max_minutes: job_minutes(action.requirements),
            watch: action.requirements.watch,
            wake_turn: turn.wake_turn,
        },
        facts: ExitFacts {
            targets,
            hosts,
            programs,
            upload_patterns,
            outside_commands,
            desktop_read,
            repo_surface_changed_this_turn: turn.surface_changed,
            git_status: None,
            snapshot_covers: snapshot_covers(action.decision),
            sandbox_export_names: turn.export_names.clone(),
            previous_exits_this_turn: turn.previous.clone(),
            refusals_in_a_row: turn.refusals_in_a_row,
            denials_in_a_row: turn.denials_in_a_row,
            denials_this_turn: turn.denials_this_turn,
        },
    }
}

/// The host facts of the exits of `decision`, each host once (efr's auto spec, 9.4 and
/// DP23): the hosts of the exits' `Grant::Host` grants and of
/// [`ExitNeed::hosts`](efr_permissions::ExitNeed), which hold the hosts that the line
/// names, the hosts behind a git remote name and the model's `needs.hosts` in their
/// normal form.
///
/// `on_allow_list` holds only under efr's proxy, for a host that only `host` exits
/// name, each with at least one grant and `Grant::Host` grants only (never
/// `Grant::OpenNetwork`), on each of
/// their ports, when no `upload` or `outside` exit names the host. So the list never
/// lifts the name for an exit that opens the whole network, for an upload or for a run
/// outside the sandbox, and never with no proxy. `project_remote` holds for a host of a
/// remote of a pinned project git dir that did not change since the conversation's first
/// call (`RemoteFact::project`), never for a submodule. `refused_by_proxy` holds for a
/// host that the proxy refused in the conversation's last call or its jobs: a fact,
/// never a name.
fn host_facts(
    decision: &Decision,
    requirements: &Requirements,
    engine: &Engine,
    messages: &[String],
) -> Vec<HostFact> {
    let facts = requirements.facts.as_ref();
    let proxy = engine.support().egress == Egress::Proxy;
    let mut order: Vec<String> = Vec::new();
    for need in decision.exits() {
        let granted = need.grants.iter().filter_map(|grant| match grant {
            Grant::Host { host, .. } => Some(host.to_lowercase()),
            _ => None,
        });
        for host in granted.chain(need.hosts.iter().cloned()) {
            if !host.is_empty() && !order.contains(&host) {
                order.push(host);
            }
        }
    }
    let wide = |need: &ExitNeed| {
        matches!(need.kind, ExitKind::Upload | ExitKind::Outside)
            || need.grants.iter().any(|grant| !matches!(grant, Grant::Host { .. }))
    };
    let on_allow_list = |host: &str| {
        let naming: Vec<&ExitNeed> = decision
            .exits()
            .filter(|need| {
                need.hosts.iter().any(|known| known == host)
                    || need.grants.iter().any(
                        |grant| matches!(grant, Grant::Host { host: known, .. } if known == host),
                    )
            })
            .collect();
        proxy
            && !naming.is_empty()
            && naming.iter().all(|need| need.kind == ExitKind::Host && !wide(need))
            && naming.iter().all(|need| {
                // NOTE: an exit with no grant has no port to check the list on.
                !need.grants.is_empty()
                    && need.grants.iter().all(|grant| match grant {
                        Grant::Host { port, .. } => engine.hosts().allows(host, *port),
                        _ => false,
                    })
            })
    };
    let project_remote = |host: &str| {
        facts.is_some_and(|facts| {
            facts.remotes.iter().flat_map(|(_, remotes)| remotes).any(|remote| {
                remote.project
                    && !remote.submodule
                    && remote.fetch.iter().chain(&remote.push).any(|url| url.host == host)
            })
        })
    };
    let refused = |host: &str| {
        facts.is_some_and(|facts| facts.refused_hosts.iter().any(|known| known == host))
    };
    order
        .into_iter()
        .map(|host| HostFact {
            on_allow_list: on_allow_list(&host),
            named_in_user_messages: exits::names_match(std::slice::from_ref(&host), messages),
            refused_by_proxy: refused(&host),
            project_remote: project_remote(&host),
            host,
        })
        .collect()
}

/// Whether efr copies every file of the exits of `decision` before the call, so that
/// `efr undo` can bring it back: false when one exit's copy is not covered, such as a
/// masked file or a directory over the limits. `None` when no exit has a file to copy.
fn snapshot_covers(decision: &Decision) -> Option<bool> {
    decision
        .exits()
        .filter_map(|need| need.covered)
        .fold(None, |all, covered| Some(all.unwrap_or(true) && covered))
}

/// True when `decision` has a `desktop_ipc` exit and the classifier may judge each
/// one: the engine gives that approver only to a read or reload form of
/// `DESKTOP_IPC_READ`, such as `hyprctl monitors`. A form that types or runs commands,
/// and a socket that the model names, are the user's.
fn desktop_read(decision: &Decision) -> bool {
    let mut desktop = decision.exits().filter(|need| need.kind == ExitKind::DesktopIpc).peekable();
    desktop.peek().is_some() && desktop.all(|need| need.approver == Approver::Classifier)
}

/// The protocol's class of a path that efr or a tool can make again, for the naming
/// rule; `None` for a kind that this build does not know, which keeps the engine's
/// class.
fn rebuild_name(rebuild: Rebuild) -> Option<PathClassName> {
    match rebuild {
        Rebuild::Cache => Some(PathClassName::Cache),
        Rebuild::BuildOutput => Some(PathClassName::BuildOutput),
        Rebuild::Trash => Some(PathClassName::Trash),
        _ => None,
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
