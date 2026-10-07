//! `Engine::decide`: requirements in, one decision out.

use std::path::{Path, PathBuf};

use efr_protocol::{Mode, Origin, Scope};

use crate::command::{self, SimpleCommand};
use crate::decision::{Cause, Decision, Effect, Layer, Reason, Subject};
use crate::exits::{self, Envelope, ExitInput, ExitNeed, PathFacts};
use crate::path_class::normalize;
use crate::policy::{MatchContext, Part, Target, expand};
use crate::{
    Access, AutoSupport, Construct, DecisionInput, Locations, PathAccess, PathClass, Policy,
    Resource,
};

/// The permission engine: the machine's [`Locations`] and one machine [`Policy`] for
/// each permission mode, the mode's built-in policy ([`Policy::base`]) followed by the
/// user's rules.
///
/// `decide` is a pure function of the engine and its input. Each requirement gets its
/// own [`Reason`] and the decision is the strictest of them, in three steps:
///
/// 1. The last matching rule of the machine policy of the turn's mode sets the effect;
///    no match denies. For a secret, only a rule that names secrets (the class, or an
///    `under` path at or below a secret location) decides; a later rule for a wider
///    resource, such as `read any allow`, may only make the effect stricter.
/// 2. The last matching rule of the conversation's policy replaces it, except for
///    secrets and system paths, where it may only make the effect stricter, so an
///    approval collected in one conversation never opens a key or `/etc`.
/// 3. A turn from a remote origin (the phone, or any origin newer than this crate) runs
///    with at most the `cautious` mode, and needs approval for everything outside
///    `$SCRATCH`: an `Allow` becomes `Ask`. An interactive call always needs approval.
///
/// A change of efr's own settings ([`Requirements::settings`](crate::Requirements),
/// declared only by the settings tool) needs approval in every mode, whatever the
/// rules say, and is denied for a remote origin.
///
/// A command line is judged one simple command at a time, and its effect is the
/// strictest of theirs, so a line is allowed only when every simple command in it is.
/// A line that cannot be split (a command substitution, an output redirection to a
/// file, a builtin such as `eval`, see [`Construct`]) is judged only by the rules for
/// every command line, and a program that runs as another user needs at least approval
/// whatever the rules say. Network access is judged by the rules for every network
/// access and by the network rules of the simple commands of the line.
///
/// A path read with everything below it ([`Access::ReadTree`]) is judged like a read,
/// and needs at least approval when a secret that no rule allows lies below it; a
/// write needs at least approval when such a secret, or efr's configuration, lies below
/// the path. A sealed path ([`Locations::with_sealed_root`], the daemon's own
/// credentials) is denied whatever the rules say, and so is a write of a write-sealed
/// path ([`Locations::with_write_sealed_root`], efr's configuration).
///
/// A call that declares no requirement gets one reason, [`Subject::Nothing`]. It is
/// allowed for the local origins (shell, CLI and proxy) and needs approval for any other,
/// because the engine cannot see what an undeclared call does, and a remote turn fails
/// closed.
///
/// In the `auto` mode a shell call runs in the kernel sandbox: its command line is at
/// least [`Effect::Contain`], and a user's rule that allows it cannot lift the sandbox.
/// A path or network requirement that the built-in rules would ask about is contained
/// too, because the sandbox holds it, and each exit that [`exits::predict`] finds asks
/// on its own reason ([`Cause::Exit`]), or is denied for a floor. A user's `ask` or
/// `deny` rule keeps its own cause. `nested_shell` is denied, because no shell outlives a
/// contained call. `read_file`, `write_file` and edits run in the daemon, outside the
/// sandbox, so they keep the `cautious` path rules, plus an exit for a read of a sandbox
/// mask and for a write to a floor inside a write root.
#[derive(Debug, Clone)]
pub struct Engine {
    locations: Locations,
    manual: Machine,
    cautious: Machine,
    auto: Machine,
    support: AutoSupport,
}

/// The machine policy of one mode, and how many of its rules are built in.
#[derive(Debug, Clone)]
struct Machine {
    policy: Policy,
    built_in: usize,
}

impl Machine {
    fn new(mode: Mode, rules: &Policy) -> Self {
        let base = Policy::base(mode);
        let built_in = base.rules().len();
        Machine { policy: base.then(rules.clone()), built_in }
    }
}

impl Engine {
    /// An engine for the machine described by `locations`, deciding in each mode by the
    /// mode's built-in policy followed by `rules`, the user's configured rules, which
    /// win where they match. The three policies are built here, once.
    pub fn with_rules(locations: Locations, rules: Policy) -> Self {
        Engine {
            manual: Machine::new(Mode::Manual, &rules),
            cautious: Machine::new(Mode::Cautious, &rules),
            auto: Machine::new(Mode::Auto, &rules),
            locations,
            support: AutoSupport::default(),
        }
    }

    /// Sets what the machine's `auto` sandbox supports beyond phase 1, which turns some
    /// exits into routine calls.
    #[must_use]
    pub fn with_support(mut self, support: AutoSupport) -> Self {
        self.support = support;
        self
    }

    /// What the machine's `auto` sandbox supports.
    pub fn support(&self) -> &AutoSupport {
        &self.support
    }

    /// An engine that decides by `policy` alone in every mode, with no built-in rule,
    /// for tests of what happens when no rule matches.
    #[cfg(test)]
    pub(crate) fn with_policy(locations: Locations, policy: Policy) -> Self {
        let machine = Machine { policy, built_in: 0 };
        Engine {
            locations,
            manual: machine.clone(),
            cautious: machine.clone(),
            auto: machine,
            support: AutoSupport::default(),
        }
    }

    /// An engine with the built-in policies only.
    pub fn with_defaults(locations: Locations) -> Self {
        Engine::with_rules(locations, Policy::empty())
    }

    /// The machine's locations.
    pub fn locations(&self) -> &Locations {
        &self.locations
    }

    /// The machine policy of `mode`. A mode newer than this crate gets the `manual`
    /// policy.
    pub fn policy(&self, mode: Mode) -> &Policy {
        &self.machine(mode).policy
    }

    fn machine(&self, mode: Mode) -> &Machine {
        match mode {
            Mode::Cautious => &self.cautious,
            Mode::Auto => &self.auto,
            // NOTE: manual, and any mode added to the protocol after this crate.
            _ => &self.manual,
        }
    }

    /// True when `resource` names secrets: the class itself, or an `under` path at or
    /// below a secret location. `any`, the project and an `under` path above a secret
    /// only reach secrets on the way to everything else. Only such a rule decides a
    /// secret, so the settings tool refuses to write one.
    pub fn names_secrets(&self, resource: &Resource) -> bool {
        match resource {
            Resource::Class(class) => *class == PathClass::Secrets,
            Resource::Under(root) => expand(root, self.locations.home()).is_some_and(|root| {
                self.locations.classify_normal(&root, None) == PathClass::Secrets
            }),
            Resource::Any | Resource::Project | Resource::Command(_) | Resource::Envelope => false,
        }
    }

    /// Decides whether the tool call described by `input` runs, waits for approval or
    /// is refused.
    pub fn decide(&self, input: &DecisionInput) -> Decision {
        let requirements = &input.requirements;
        let mode = effective_mode(input.mode, input.origin);
        let auto = mode == Mode::Auto;
        let shell = requirements.command.is_some();
        let scratch = self.locations.scratch_root(&input.conversation_policy.scratch);
        let project_root = match &input.scope {
            Scope::Project(id) => self.locations.widening_project_root(id),
            _ => None,
        };
        let envelope: Vec<PathBuf> = if auto && shell {
            self.locations.envelope_roots().map(Path::to_path_buf).collect()
        } else {
            Vec::new()
        };
        let cx = MatchContext {
            home: self.locations.home(),
            home_aliases: self.locations.home_aliases(),
            project_root,
            scratch: scratch.as_deref(),
            envelope: &envelope,
        };
        // NOTE: `read_file` and `write_file` run in the daemon, outside the sandbox, so
        // in `auto` they keep the cautious path rules: the auto policy would let them
        // write a project's `.git` or the host's real `/tmp`.
        let machine = if auto && !shell { &self.cautious } else { self.machine(mode) };
        let judge = Judge { engine: self, machine, input, cx, contain: auto && shell };

        let analyzed = requirements.command.as_deref().map(command::analyze);
        let commands = match &analyzed {
            Some(Ok(commands)) => commands.as_slice(),
            Some(Err(_)) | None => &[],
        };
        let start = requirements.command_dir.as_deref().and_then(normalize);
        let parts = parts(commands, start.as_deref());

        let mut reasons = Vec::new();
        for PathAccess { path, access } in &requirements.paths {
            let reason = match normalize(path) {
                Some(path) => {
                    let class = self.locations.classify_normal(&path, scratch.as_deref());
                    judge.path(path, *access, class)
                }
                None => Reason {
                    subject: Subject::Path { path: path.clone(), access: *access, class: None },
                    effect: Effect::Deny,
                    cause: Cause::NotAbsolute,
                },
            };
            reasons.push(reason);
        }
        if let (Some(line), Some(analyzed)) = (&requirements.command, &analyzed) {
            let analyzed =
                analyzed.as_ref().map(|commands| (commands.as_slice(), parts.as_slice()));
            reasons.push(judge.command(line, analyzed));
        }
        if requirements.network {
            let (effect, cause) = judge.network(commands, &parts);
            let (effect, cause) = clamp_remote(effect, cause, None, input.origin);
            reasons.push(Reason { subject: Subject::Network, effect, cause });
        }
        if requirements.interactive {
            // NOTE: nobody must be there for a contained call; an exit that needs a
            // person, such as sudo, asks on its own reason.
            let (effect, cause) = if judge.contain {
                (Effect::Contain, Cause::Contained)
            } else {
                (Effect::Ask, Cause::Interactive)
            };
            reasons.push(Reason { subject: Subject::Interactive, effect, cause });
        }
        if requirements.nested && auto {
            // NOTE: a nested shell receives the model's line raw, and when none runs the
            // trusted zsh would run it outside the sandbox.
            reasons.push(Reason {
                subject: Subject::NestedShell,
                effect: Effect::Deny,
                cause: Cause::NestedShell,
            });
        }
        if let Some(change) = &requirements.settings {
            // NOTE: no rule is read: a rule that allows writing the file must not let
            // the model change its own permissions without the user, and a remote turn
            // never changes them.
            let (effect, cause) = if is_local(input.origin) {
                (Effect::Ask, Cause::SettingsChange)
            } else {
                (Effect::Deny, Cause::RemoteSettings { origin: input.origin })
            };
            let subject =
                Subject::Settings { summary: change.summary.clone(), loosens: change.loosens };
            reasons.push(Reason { subject, effect, cause });
        }
        if auto {
            let found = match &requirements.command {
                Some(line) => exits::predict(&ExitInput {
                    line,
                    command_dir: requirements.command_dir.as_deref(),
                    paths: &requirements.paths,
                    network: requirements.network,
                    needs: requirements.needs.as_ref(),
                    facts: requirements.facts.as_ref(),
                    locations: &self.locations,
                    project_root,
                    scratch: scratch.as_deref(),
                    support: &self.support,
                }),
                None => self.tool_exits(&requirements.paths, project_root, scratch.as_deref()),
            };
            reasons.extend(found.into_iter().map(exit_reason));
        }
        if reasons.is_empty() {
            let (effect, cause) =
                clamp_remote(Effect::Allow, Cause::NoRequirements, None, input.origin);
            reasons.push(Reason { subject: Subject::Nothing, effect, cause });
        }
        Decision::from_reasons(reasons)
    }

    /// Where `path` stands for a call of the `auto` mode in a turn of `scope` whose
    /// `$SCRATCH` is `scratch`: its class, and whether it lies in a write root, on a
    /// floor or in a synced folder. `None` when `path` is relative. The conversation
    /// builds the facts of an exit record from it, so the record and the decision use
    /// one set of roots.
    pub fn path_facts(&self, path: &Path, scope: &Scope, scratch: &Path) -> Option<PathFacts> {
        let path = normalize(path)?;
        let path = self.locations.rehome(&path).into_owned();
        let scratch = self.locations.scratch_root(scratch);
        let project_root = match scope {
            Scope::Project(id) => self.locations.widening_project_root(id),
            _ => None,
        };
        let envelope = Envelope::new(&self.locations, project_root, scratch.as_deref());
        Some(PathFacts {
            class: self.locations.classify_normal(&path, scratch.as_deref()),
            in_write_root: envelope.in_root(&path),
            floor: envelope.is_floor(&path),
            synced: self.locations.synced_roots().iter().any(|root| path.starts_with(root)),
        })
    }

    /// The exits of a `read_file`, `write_file` or edit in `auto`: a read of a sandbox
    /// mask, and a write to a floor inside a write root.
    fn tool_exits(
        &self,
        paths: &[PathAccess],
        project_root: Option<&Path>,
        scratch: Option<&Path>,
    ) -> Vec<ExitNeed> {
        let envelope = Envelope::new(&self.locations, project_root, scratch);
        paths
            .iter()
            .filter_map(|PathAccess { path, access }| {
                let path = normalize(path)?;
                envelope.tool_exit(&self.locations.rehome(&path), *access)
            })
            .collect()
    }
}

/// The reason of one exit: a question, or a denial for a floor.
fn exit_reason(need: ExitNeed) -> Reason {
    let kind = need.kind;
    let effect = if kind.is_floor() { Effect::Deny } else { Effect::Ask };
    Reason { subject: Subject::Exit { need }, effect, cause: Cause::Exit { kind } }
}

/// The permission mode that a turn from `origin` runs with when it asks for `mode`: at
/// most [`Mode::Cautious`] for a remote origin, any origin other than the shell, the
/// CLI and the proxy, so a phone can never run the `auto` list.
pub fn effective_mode(mode: Mode, origin: Origin) -> Mode {
    if is_local(origin) { mode } else { mode.min(Mode::Cautious) }
}

/// The simple commands of a line as rules match them, each with the directory it starts
/// in: `start` until a change of directory, then unknown.
fn parts<'a>(commands: &'a [SimpleCommand], start: Option<&'a Path>) -> Vec<Part<'a>> {
    let mut dir = start;
    commands
        .iter()
        .map(|command| {
            let part = Part {
                words: &command.words,
                pattern: command.pattern,
                privileged: command::privileged(command).is_some(),
                dir,
            };
            // NOTE: after a change of directory, where the rest runs is unknown.
            if command::changes_directory(command) {
                dir = None;
            }
            part
        })
        .collect()
}

/// Everything one decision judges its requirements with.
struct Judge<'a> {
    engine: &'a Engine,
    /// The machine policy of the turn's effective mode.
    machine: &'a Machine,
    input: &'a DecisionInput,
    cx: MatchContext<'a>,
    /// True for a shell call in `auto`: it runs in the sandbox.
    contain: bool,
}

impl Judge<'_> {
    /// One path with its class: the rules, then what lies below it, then the origin.
    fn path(&self, path: PathBuf, access: Access, class: PathClass) -> Reason {
        let target = Target::Path { path: &path, access, class };
        let (mut effect, mut cause) = self.by_rules(&target, Some(class));
        if effect == Effect::Allow {
            let below = match access {
                Access::Read => None,
                Access::ReadTree => self
                    .unopened_secret_below(&path, Access::Read)
                    .map(|secret| Cause::ReachesSecret { secret }),
                // NOTE: a write of a directory may replace or remove everything below it.
                Access::Write => match self.engine.locations.write_sealed_below(&path) {
                    Some(root) => Some(Cause::ReachesWriteSealed { root: root.to_path_buf() }),
                    None => self
                        .unopened_secret_below(&path, Access::Write)
                        .map(|secret| Cause::ReachesSecret { secret }),
                },
            };
            if let Some(below) = below {
                effect = Effect::Ask;
                cause = below;
            }
        }
        let (effect, cause) = self.contained(effect, cause);
        let (effect, cause) = clamp_remote(effect, cause, Some(class), self.input.origin);
        Reason { subject: Subject::Path { path, access, class: Some(class) }, effect, cause }
    }

    /// The first secret below `dir` that the rules do not allow for `access`.
    fn unopened_secret_below(&self, dir: &Path, access: Access) -> Option<PathBuf> {
        self.engine.locations.secrets_below(dir).into_iter().find(|secret| {
            let target = Target::Path { path: secret, access, class: PathClass::Secrets };
            self.by_rules(&target, Some(PathClass::Secrets)).0 != Effect::Allow
        })
    }

    /// One command line: each simple command by the rules, the strictest deciding, or
    /// the construct that keeps the line from being split.
    fn command(
        &self,
        line: &str,
        analyzed: Result<(&[SimpleCommand], &[Part<'_>]), &Construct>,
    ) -> Reason {
        let mut deciding = Vec::new();
        let (effect, cause) = match analyzed {
            Ok((commands, parts)) => {
                let several = parts.len() > 1;
                let mut strictest: Option<(Effect, Cause)> = None;
                let mut effects = Vec::with_capacity(parts.len());
                for (command, part) in commands.iter().zip(parts) {
                    let (mut effect, mut cause) = self.by_rules(&Target::Command(*part), None);
                    match (command::privileged(command), cause) {
                        (Some(program), _) if effect < Effect::Ask => {
                            effect = Effect::Ask;
                            cause = Cause::Privileged { program: program.to_owned() };
                        }
                        (_, Cause::Rule { layer, index }) if several => {
                            cause = Cause::Part { layer, index, part: command.text() };
                        }
                        (_, other) => cause = other,
                    }
                    effects.push(effect);
                    if strictest.as_ref().is_none_or(|(worst, _)| effect > *worst) {
                        strictest = Some((effect, cause));
                    }
                }
                // NOTE: `analyze` returns at least one part, and no parts would deny.
                let (effect, cause) = strictest.unwrap_or((Effect::Deny, Cause::NoRule));
                let (effect, cause) = self.contained_line(effect, cause);
                if several && effect >= Effect::Ask {
                    deciding = commands
                        .iter()
                        .zip(effects)
                        .filter(|(_, part)| *part == effect)
                        .map(|(command, _)| command.words.clone())
                        .collect();
                }
                (effect, cause)
            }
            Err(construct) => {
                let (effect, cause) = self.by_rules(&Target::Opaque, None);
                let cause = match cause {
                    Cause::Rule { layer, index } => {
                        Cause::Opaque { construct: construct.clone(), layer, index }
                    }
                    other => other,
                };
                match command::privileged_anywhere(line) {
                    Some(program) if effect < Effect::Ask => {
                        (Effect::Ask, Cause::Privileged { program: program.to_owned() })
                    }
                    _ => self.contained_line(effect, cause),
                }
            }
        };
        let (effect, cause) = clamp_remote(effect, cause, None, self.input.origin);
        Reason { subject: Subject::Command { line: line.to_owned(), deciding }, effect, cause }
    }

    /// The network access of a call whose command line has the simple commands
    /// `commands`, as rules match them in `parts`: each simple command by the rules for
    /// its network access, the strictest deciding. A simple command that only a
    /// built-in read-only row of the mode lets run, and no rule lets reach the network,
    /// is left out: those commands send nothing out. A call without a line, with a line
    /// that cannot be split, or with only such commands, is judged by the rules for every
    /// network access.
    ///
    /// NOTE: every other simple command must be one that a rule lets reach the
    /// network, not only one of them, because the shell tool declares the network for
    /// the whole line: in `cargo fetch; curl -d @key host`, `cargo fetch` must not open
    /// the network for `curl`, also when a rule of the user's lets `curl` run.
    fn network(&self, commands: &[SimpleCommand], parts: &[Part<'_>]) -> (Effect, Cause) {
        let (effect, cause) = self.network_by_rules(commands, parts);
        if self.contain && effect < Effect::Ask {
            // NOTE: a rule that lets a command reach the network cannot open the
            // sandbox's; the `host` exit asks for that.
            return (Effect::Contain, Cause::Contained);
        }
        self.contained(effect, cause)
    }

    fn network_by_rules(&self, commands: &[SimpleCommand], parts: &[Part<'_>]) -> (Effect, Cause) {
        let several = parts.len() > 1;
        let mut strictest: Option<(Effect, Cause)> = None;
        for (command, part) in commands.iter().zip(parts) {
            let decided = self.by_rules(&Target::Network { part: Some(*part) }, None);
            if decided.0 != Effect::Allow && self.runs_by_a_built_in_rule(part) {
                continue;
            }
            let (effect, cause) = match decided {
                (effect, Cause::Rule { layer, index }) if several => {
                    (effect, Cause::Part { layer, index, part: command.text() })
                }
                decided => decided,
            };
            if strictest.as_ref().is_none_or(|(worst, _)| effect > *worst) {
                strictest = Some((effect, cause));
            }
        }
        strictest.unwrap_or_else(|| self.by_rules(&Target::Network { part: None }, None))
    }

    /// For a shell call in `auto`: a question that a built-in rule asks, or that a secret
    /// or efr's config below the path asks, is contained instead, because the sandbox
    /// masks the secret, keeps the config read-only and holds the rest. A user's `ask`
    /// or `deny` keeps its cause.
    fn contained(&self, effect: Effect, cause: Cause) -> (Effect, Cause) {
        let below = matches!(cause, Cause::ReachesSecret { .. } | Cause::ReachesWriteSealed { .. });
        if self.contain && effect == Effect::Ask && (below || self.built_in(&cause)) {
            (Effect::Contain, Cause::Contained)
        } else {
            (effect, cause)
        }
    }

    /// For a shell call in `auto`: a command line runs at least contained, also when a
    /// rule allows it. A question or a denial keeps its cause.
    fn contained_line(&self, effect: Effect, cause: Cause) -> (Effect, Cause) {
        if self.contain && effect < Effect::Ask {
            (Effect::Contain, Cause::Contained)
        } else {
            (effect, cause)
        }
    }

    /// True when `cause` is a built-in rule of the mode.
    fn built_in(&self, cause: &Cause) -> bool {
        match cause {
            Cause::Rule { layer, index }
            | Cause::Part { layer, index, .. }
            | Cause::Opaque { layer, index, .. } => {
                *layer == Layer::Machine && *index < self.machine.built_in
            }
            _ => false,
        }
    }

    /// True when a built-in rule of the mode lets the simple command `part` run, and no
    /// rule of the user's or of the conversation decided it.
    fn runs_by_a_built_in_rule(&self, part: &Part<'_>) -> bool {
        matches!(
            self.by_rules(&Target::Command(*part), None),
            (Effect::Allow, Cause::Rule { layer: Layer::Machine, index })
                if index < self.machine.built_in
        )
    }

    /// Steps 1 and 2: the machine policy, then the conversation's policy. A sealed
    /// path, and a write of a write-sealed path, is denied before any rule is read.
    fn by_rules(&self, target: &Target<'_>, class: Option<PathClass>) -> (Effect, Cause) {
        if let Target::Path { path, access, .. } = target {
            if self.engine.locations.is_sealed(path) {
                return (Effect::Deny, Cause::Sealed);
            }
            if *access == Access::Write && self.engine.locations.is_write_sealed(path) {
                return (Effect::Deny, Cause::WriteSealed);
            }
        }
        let secret = class == Some(PathClass::Secrets);
        let machine = match self.decided_by(&self.machine.policy, target, secret, None) {
            Some((index, effect)) => (effect, Cause::Rule { layer: Layer::Machine, index }),
            None => (Effect::Deny, Cause::NoRule),
        };
        let may_loosen = !matches!(class, Some(PathClass::Secrets | PathClass::System));
        let conversation = &self.input.conversation_policy.rules;
        match self.decided_by(conversation, target, secret, Some(machine.0)) {
            Some((index, effect)) if may_loosen || effect >= machine.0 => {
                (effect, Cause::Rule { layer: Layer::Conversation, index })
            }
            _ => machine,
        }
    }

    /// The rule of `policy` that decides `target`: the last match, or for a secret the
    /// last rule that names secrets, made stricter by any later match. `earlier` is the
    /// effect of the policy before this one.
    fn decided_by(
        &self,
        policy: &Policy,
        target: &Target<'_>,
        secret: bool,
        earlier: Option<Effect>,
    ) -> Option<(usize, Effect)> {
        if secret {
            policy.last_match_for_secret(target, &self.cx, earlier, |resource| {
                self.names_secrets(resource)
            })
        } else {
            policy.last_match(target, &self.cx)
        }
    }

    fn names_secrets(&self, resource: &Resource) -> bool {
        self.engine.names_secrets(resource)
    }
}

/// Step 3 for one requirement: outside `$SCRATCH`, a remote turn needs approval. A
/// requirement without a class, such as a command or no requirement at all, is outside.
fn clamp_remote(
    effect: Effect,
    cause: Cause,
    class: Option<PathClass>,
    origin: Origin,
) -> (Effect, Cause) {
    if effect == Effect::Allow && !is_local(origin) && class != Some(PathClass::Scratch) {
        (Effect::Ask, Cause::RemoteOrigin { origin })
    } else {
        (effect, cause)
    }
}

/// Origins on this machine. Anything else, including an origin added to the protocol
/// after this crate was written, is treated as remote, so the engine fails closed.
fn is_local(origin: Origin) -> bool {
    matches!(origin, Origin::Shell | Origin::Cli | Origin::Proxy)
}

#[cfg(test)]
mod tests;
