//! `Engine::decide`: requirements in, one decision out.

use efr_protocol::{Origin, Scope};

use crate::decision::{Cause, Decision, Effect, Layer, Reason, Subject};
use crate::path_class::normalize;
use crate::policy::MatchContext;
use crate::{DecisionInput, Locations, PathAccess, PathClass, Policy};

/// The permission engine: the machine's [`Locations`] and the machine [`Policy`].
///
/// `decide` is a pure function of the engine and its input. Each requirement gets its
/// own [`Reason`] and the decision is the strictest of them, in three steps:
///
/// 1. The last matching rule of the machine policy sets the effect; no match denies.
/// 2. The last matching rule of the conversation's policy replaces it, except for
///    secrets and system paths, where it may only make the effect stricter, so an
///    approval collected in one conversation never opens a key or `/etc`.
/// 3. A turn from a remote origin (the phone, or any origin newer than this crate) needs
///    approval for everything outside `$SCRATCH`: an `Allow` becomes `Ask`. An
///    interactive call always needs approval.
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
        let judge = |subject: Subject, class: Option<PathClass>| -> Reason {
            let (effect, cause) = self.by_rules(&subject, class, input, &cx);
            let (effect, cause) = clamp_remote(effect, cause, class, input.origin);
            Reason { subject, effect, cause }
        };

        let requirements = &input.requirements;
        let mut reasons = Vec::new();
        for PathAccess { path, access } in &requirements.paths {
            let reason = match normalize(path) {
                Some(path) => {
                    let class = self.locations.classify_normal(&path, scratch.as_deref());
                    judge(Subject::Path { path, access: *access, class: Some(class) }, Some(class))
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
            reasons.push(judge(Subject::Command { line: line.clone() }, None));
        }
        if requirements.network {
            reasons.push(judge(Subject::Network, None));
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

    /// Steps 1 and 2: the machine policy, then the conversation's policy.
    fn by_rules(
        &self,
        subject: &Subject,
        class: Option<PathClass>,
        input: &DecisionInput,
        cx: &MatchContext<'_>,
    ) -> (Effect, Cause) {
        let machine = match self.policy.last_match(subject, cx) {
            Some((index, effect)) => (effect, Cause::Rule { layer: Layer::Machine, index }),
            None => (Effect::Deny, Cause::NoRule),
        };
        let may_loosen = !matches!(class, Some(PathClass::Secrets | PathClass::System));
        match input.conversation_policy.rules.last_match(subject, cx) {
            Some((index, effect)) if may_loosen || effect >= machine.0 => {
                (effect, Cause::Rule { layer: Layer::Conversation, index })
            }
            _ => machine,
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
