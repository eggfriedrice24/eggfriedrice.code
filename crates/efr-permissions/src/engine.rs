//! `Engine::decide`: requirements in, one decision out.

use std::path::{Path, PathBuf};

use efr_protocol::{Origin, Scope};

use crate::command;
use crate::decision::{Cause, Decision, Effect, Layer, Reason, Subject};
use crate::path_class::normalize;
use crate::policy::{MatchContext, Target, expand};
use crate::{Access, DecisionInput, Locations, PathAccess, PathClass, Policy, Resource};

/// The permission engine: the machine's [`Locations`] and the machine [`Policy`].
///
/// `decide` is a pure function of the engine and its input. Each requirement gets its
/// own [`Reason`] and the decision is the strictest of them, in three steps:
///
/// 1. The last matching rule of the machine policy sets the effect; no match denies.
///    For a secret, only a rule that names secrets (the class, or an `under` path at or
///    below a secret location) decides; a later rule for a wider resource, such as
///    `read any allow`, may only make the effect stricter.
/// 2. The last matching rule of the conversation's policy replaces it, except for
///    secrets and system paths, where it may only make the effect stricter, so an
///    approval collected in one conversation never opens a key or `/etc`.
/// 3. A turn from a remote origin (the phone, or any origin newer than this crate) needs
///    approval for everything outside `$SCRATCH`: an `Allow` becomes `Ask`. An
///    interactive call always needs approval.
///
/// A command line is judged one simple command at a time, and its effect is the
/// strictest of theirs, so a line is allowed only when every simple command in it is.
/// A line that cannot be split (a command substitution, an output redirection to a
/// file, a builtin such as `eval`, see [`Construct`](crate::Construct)) is judged only
/// by the rules for every command line, and a program that runs as another user needs
/// at least approval whatever the rules say.
///
/// A path read with everything below it ([`Access::ReadTree`]) is judged like a read,
/// and needs at least approval when a secret that no rule allows lies below it.
///
/// A call that declares no requirement gets one reason, [`Subject::Nothing`]. It is
/// allowed for the local origins (shell, CLI and proxy) and needs approval for any other,
/// because the engine cannot see what an undeclared call does, and a remote turn fails
/// closed.
#[derive(Debug, Clone)]
pub struct Engine {
    locations: Locations,
    policy: Policy,
}

impl Engine {
    /// An engine for the machine described by `locations`, deciding by `policy`.
    ///
    /// The daemon passes [`Policy::defaults`] followed by the user's configured rules.
    pub fn new(locations: Locations, policy: Policy) -> Self {
        Engine { locations, policy }
    }

    /// An engine with the built-in policy only.
    pub fn with_defaults(locations: Locations) -> Self {
        Engine::new(locations, Policy::defaults())
    }

    /// The machine's locations.
    pub fn locations(&self) -> &Locations {
        &self.locations
    }

    /// The machine policy.
    pub fn policy(&self) -> &Policy {
        &self.policy
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
        };
        let judge = Judge { engine: self, input, cx };

        let requirements = &input.requirements;
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
        if let Some(line) = &requirements.command {
            let dir = requirements.command_dir.as_deref().and_then(normalize);
            reasons.push(judge.command(line, dir.as_deref()));
        }
        if requirements.network {
            let (effect, cause) = judge.by_rules(&Target::Network, None);
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

/// Everything one decision judges its requirements with.
struct Judge<'a> {
    engine: &'a Engine,
    input: &'a DecisionInput,
    cx: MatchContext<'a>,
}

impl Judge<'_> {
    /// One path with its class: the rules, then the secrets below a tree, then the
    /// origin.
    fn path(&self, path: PathBuf, access: Access, class: PathClass) -> Reason {
        let target = Target::Path { path: &path, access, class };
        let (mut effect, mut cause) = self.by_rules(&target, Some(class));
        if access == Access::ReadTree
            && effect == Effect::Allow
            && let Some(secret) = self.unopened_secret_below(&path)
        {
            effect = Effect::Ask;
            cause = Cause::ReachesSecret { secret };
        }
        let (effect, cause) = clamp_remote(effect, cause, Some(class), self.input.origin);
        Reason { subject: Subject::Path { path, access, class: Some(class) }, effect, cause }
    }

    /// The first secret below `dir` that the rules do not allow to read.
    fn unopened_secret_below(&self, dir: &Path) -> Option<PathBuf> {
        self.engine.locations.secrets_below(dir).into_iter().find(|secret| {
            let target =
                Target::Path { path: secret, access: Access::Read, class: PathClass::Secrets };
            self.by_rules(&target, Some(PathClass::Secrets)).0 != Effect::Allow
        })
    }

    /// One command line, starting in `dir` when it is known: each simple command by
    /// the rules, the strictest deciding.
    fn command(&self, line: &str, dir: Option<&Path>) -> Reason {
        let (effect, cause) = match command::analyze(line) {
            Ok(parts) => {
                let several = parts.len() > 1;
                let mut strictest: Option<(Effect, Cause)> = None;
                let mut dir = dir;
                for part in &parts {
                    let privileged = command::privileged(part);
                    let target = Target::Command {
                        words: &part.words,
                        pattern: part.pattern,
                        privileged: privileged.is_some(),
                        dir,
                    };
                    // NOTE: after a change of directory, where the rest runs is unknown.
                    if command::changes_directory(part) {
                        dir = None;
                    }
                    let (mut effect, mut cause) = self.by_rules(&target, None);
                    match (privileged, cause) {
                        (Some(program), _) if effect == Effect::Allow => {
                            effect = Effect::Ask;
                            cause = Cause::Privileged { program: program.to_owned() };
                        }
                        (_, Cause::Rule { layer, index }) if several => {
                            cause = Cause::Part { layer, index, part: part.text() };
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
                    Cause::Rule { layer, index } => Cause::Opaque { construct, layer, index },
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

    /// Steps 1 and 2: the machine policy, then the conversation's policy.
    fn by_rules(&self, target: &Target<'_>, class: Option<PathClass>) -> (Effect, Cause) {
        let secret = class == Some(PathClass::Secrets);
        let machine = match self.decided_by(&self.engine.policy, target, secret, None) {
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
