//! `Engine::decide`: requirements in, one decision out.

use std::path::{Path, PathBuf};

use efr_protocol::{Mode, Origin, Scope};

use crate::command::{self, SimpleCommand};
use crate::decision::{Cause, Decision, Effect, Layer, Reason, Subject};
use crate::path_class::normalize;
use crate::policy::{MatchContext, Part, Target, expand};
use crate::{Access, Construct, DecisionInput, Locations, PathAccess, PathClass, Policy, Resource};

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
#[derive(Debug, Clone)]
pub struct Engine {
    locations: Locations,
    manual: Machine,
    cautious: Machine,
    auto: Machine,
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
        }
    }

    /// An engine that decides by `policy` alone in every mode, with no built-in rule,
    /// for tests of what happens when no rule matches.
    #[cfg(test)]
    pub(crate) fn with_policy(locations: Locations, policy: Policy) -> Self {
        let machine = Machine { policy, built_in: 0 };
        Engine { locations, manual: machine.clone(), cautious: machine.clone(), auto: machine }
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

    /// Decides whether the tool call described by `input` runs, waits for approval or
    /// is refused.
    pub fn decide(&self, input: &DecisionInput) -> Decision {
        let scratch = self.locations.scratch_root(&input.conversation_policy.scratch);
        let project_root = match &input.scope {
            Scope::Project(id) => self.locations.widening_project_root(id),
            _ => None,
        };
        let cx = MatchContext {
            home: self.locations.home(),
            home_aliases: self.locations.home_aliases(),
            project_root,
            scratch: scratch.as_deref(),
        };
        let machine = self.machine(effective_mode(input.mode, input.origin));
        let judge = Judge { engine: self, machine, input, cx };

        let requirements = &input.requirements;
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
            reasons.push(Reason {
                subject: Subject::Interactive,
                effect: Effect::Ask,
                cause: Cause::Interactive,
            });
        }
        if reasons.is_empty() {
            let (effect, cause) =
                clamp_remote(Effect::Allow, Cause::NoRequirements, None, input.origin);
            reasons.push(Reason { subject: Subject::Nothing, effect, cause });
        }
        Decision::from_reasons(reasons)
    }
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
        let (effect, cause) = match analyzed {
            Ok((commands, parts)) => {
                let several = parts.len() > 1;
                let mut strictest: Option<(Effect, Cause)> = None;
                for (command, part) in commands.iter().zip(parts) {
                    let (mut effect, mut cause) = self.by_rules(&Target::Command(*part), None);
                    match (command::privileged(command), cause) {
                        (Some(program), _) if effect == Effect::Allow => {
                            effect = Effect::Ask;
                            cause = Cause::Privileged { program: program.to_owned() };
                        }
                        (_, Cause::Rule { layer, index }) if several => {
                            cause = Cause::Part { layer, index, part: command.text() };
                        }
                        (_, other) => cause = other,
                    }
                    if strictest.as_ref().is_none_or(|(worst, _)| effect > *worst) {
                        strictest = Some((effect, cause));
                    }
                }
                // NOTE: `analyze` returns at least one part, and no parts would deny.
                strictest.unwrap_or((Effect::Deny, Cause::NoRule))
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
                    Some(program) if effect == Effect::Allow => {
                        (Effect::Ask, Cause::Privileged { program: program.to_owned() })
                    }
                    _ => (effect, cause),
                }
            }
        };
        let (effect, cause) = clamp_remote(effect, cause, None, self.input.origin);
        Reason { subject: Subject::Command { line: line.to_owned() }, effect, cause }
    }

    /// The network access of a call whose command line has the simple commands
    /// `commands`, as rules match them in `parts`: each simple command by the rules for
    /// its network access, the strictest deciding. A simple command that only a
    /// built-in rule of the mode lets run, and no rule lets reach the network, is left
    /// out: the read-only commands, the writer programs and the local tools of the
    /// tables send nothing out. A call without a line, with a line that cannot be
    /// split, or with only such commands, is judged by the rules for every network
    /// access.
    ///
    /// NOTE: every other simple command must be one that a rule lets reach the
    /// network, not only one of them, because the shell tool declares the network for
    /// the whole line: in `cargo fetch; curl -d @key host`, `cargo fetch` must not open
    /// the network for `curl`, also when a rule of the user's lets `curl` run.
    fn network(&self, commands: &[SimpleCommand], parts: &[Part<'_>]) -> (Effect, Cause) {
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

    /// True when `resource` names secrets: the class itself, or an `under` path at or
    /// below a secret location. `any`, the project and an `under` path above a secret
    /// only reach secrets on the way to everything else.
    fn names_secrets(&self, resource: &Resource) -> bool {
        match resource {
            Resource::Class(class) => *class == PathClass::Secrets,
            Resource::Under(root) => {
                expand(root, self.engine.locations.home()).is_some_and(|root| {
                    self.engine.locations.classify_normal(&root, None) == PathClass::Secrets
                })
            }
            Resource::Any | Resource::Project | Resource::Command(_) => false,
        }
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
