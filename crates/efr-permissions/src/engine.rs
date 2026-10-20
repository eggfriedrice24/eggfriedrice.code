//! `Engine::decide`: requirements in, one decision out.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{ExitKind, ExitSource, Grant, Mode, Origin, Scope};

use crate::always::{self, AllowFloor, AllowRule, AlwaysVerdict};
use crate::command::{self, SimpleCommand};
use crate::decision::{Cause, Decision, Effect, Layer, Reason, Subject};
use crate::exits::{self, Approver, Envelope, ExitInput, ExitNeed, PathFacts, Rebuild};
use crate::jobs::job_refusal;
use crate::path_class::normalize;
use crate::policy::{MatchContext, Part, Target, expand};
use crate::{
    Access, AutoSupport, Construct, CriticalDelete, DecisionInput, HostList, Locations, PathAccess,
    PathClass, Policy, Resource,
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
/// A delete or a move of a file ([`Requirements::destructive`](crate::Requirements))
/// needs approval in every mode and from every origin, whatever the rules say. In
/// `auto` on a machine with undo ([`AutoSupport::undo`]) it runs with no question when
/// efr holds every file that the call writes, so `efr undo` can bring it back (phase
/// B). Its paths are judged as writes as well, so a denied path stays denied.
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
/// on its own reason ([`Cause::Exit`]), or is denied for a floor. Each exit names who
/// may approve it ([`Approver`](crate::Approver)). A user's `ask` or `deny` rule keeps
/// its own cause. `nested_shell` is denied, because no shell outlives a contained call.
/// `read_file`, `write_file` and edits run in the daemon, outside the sandbox, so they
/// keep the `cautious` path rules, plus an exit for a read of a sandbox mask, for a
/// write to a floor inside a write root, and for a write that a built-in rule asks
/// about outside the turn's project and `$SCRATCH` ([`Cause::ExitDecides`]).
///
/// A shell call that starts a background job ([`Requirements::background`]) runs on
/// after its turn, while nobody watches it (phase C). Its exits are those of the same
/// line in the foreground, except that no delete of it counts as covered by undo. An
/// exit that no job may hold ([`job_refusal`]) is denied with [`Cause::NotAsJob`] in
/// every mode. Outside `auto` a job asks the user ([`Cause::JobAsks`]), whatever the
/// rules say.
///
/// Full mode ([`Mode::Full`]) runs a call as the user, with no sandbox and no question.
/// The engine decides it as an `auto` call on a machine with no sandbox
/// ([`AutoSupport::for_full`]), so it finds the same exits, and then lets run what the
/// sandbox would hold and what a built-in row asks about. Each exit that runs keeps its
/// reason, so [`Decision::exits`] lists it for the record. A floor stays denied; a
/// privileged program, a `privilege`, `persistence` or `destructive` exit, a change of
/// efr's settings, a file tool delete that undo does not cover and a user's or a
/// conversation's `ask` rule still ask. A delete of a tree that full mode guards
/// ([`exits::critical_deletes`]) asks too, with [`Cause::CriticalDelete`], whatever the
/// rules say. In a background job such a delete and a `destructive` exit are refused
/// with [`Cause::NotAsJob`]. `nested_shell` runs.
///
/// [`Requirements::background`]: crate::Requirements::background
#[derive(Debug, Clone)]
pub struct Engine {
    locations: Locations,
    manual: Machine,
    cautious: Machine,
    auto: Machine,
    support: AutoSupport,
    /// The host list of efr's proxy (phase D2). Nothing reads it yet.
    hosts: Arc<HostList>,
    /// The always-allow rules of `sandbox.allow` (phase E), in the file's order, each
    /// with the floor that disables it.
    allow: Arc<[(AllowRule, Option<AllowFloor>)]>,
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
            hosts: Arc::default(),
            allow: Arc::new([]),
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

    /// The engine with the host list of efr's proxy (phase D2). The default list is
    /// empty.
    #[must_use]
    pub fn with_hosts(mut self, hosts: Arc<HostList>) -> Self {
        self.hosts = hosts;
        self
    }

    /// The host list of efr's proxy: the global allow and deny lists that the engine
    /// of phase D2 reads under `Egress::Proxy`.
    pub fn hosts(&self) -> &HostList {
        &self.hosts
    }

    /// The engine with the always-allow rules of `sandbox.allow` (phase E, efr's auto
    /// spec 11.2). Each rule goes through [`Engine::check_allow`] against this engine's
    /// locations and host list, so the daemon sets the rules last; a rule that fails a
    /// floor check stays in the list, disabled with its reason ([`AllowFloor`]), and
    /// never covers a call. It never fails for a floor (DE44): a rule can turn bad with
    /// no edit, such as when a new project or a new dotfile link appears.
    #[must_use]
    pub fn with_allow(mut self, rules: Vec<AllowRule>) -> Self {
        let checked: Vec<(AllowRule, Option<AllowFloor>)> = rules
            .into_iter()
            .map(|rule| {
                let disabled = self.check_allow(&rule).err();
                (rule, disabled)
            })
            .collect();
        self.allow = checked.into();
        self
    }

    /// The floor check of one always-allow rule (DE23 to DE25, efr's auto spec 11.3),
    /// the same that [`Engine::with_allow`] runs: its shape, then what its kind may never
    /// name. A `host` rule may not name a host that `network.deny` holds. A `write` rule
    /// may not name `~` or a dir above it or outside it, a secret, efr's config or own
    /// dirs, a mask, a synced folder, a git dir, a floor or a dir below one, a dir above a
    /// built-in floor of the home directory (such as `~/.config`), a write root or a dir
    /// above one, or a dir that holds more than one project. A `socket` rule may not name
    /// a service manager's or a container engine's socket, a bus, a credential agent, a
    /// desktop socket, a socket in a write root, a mask, a secret or efr's own dirs. The
    /// daemon calls it before it writes a rule (DE26).
    pub fn check_allow(&self, rule: &AllowRule) -> Result<(), AllowFloor> {
        always::check(rule, &self.locations, &self.hosts)
    }

    /// The always-allow rules, in the file's order, each with `None` or the floor that
    /// disables it.
    pub fn allow_rules(&self) -> &[(AllowRule, Option<AllowFloor>)] {
        &self.allow
    }

    /// Whether an always-allow rule covers every exit of `decision`, the decision of a
    /// shell call in a turn whose project root is `project` (DE20).
    ///
    /// Only a decision of an `auto` shell call that asks for its exits alone, each a
    /// `host`, `write` or `socket` exit that the classifier judges on its own, that runs
    /// in the sandbox with narrow grants (one host and port through efr's proxy, a
    /// contained write bind, one socket) and that no delete in a cache made, may give
    /// `Covered` or `Offer`. Every write and socket grant needs its real path from the
    /// call's facts ([`CallFacts::real_paths`](crate::CallFacts)): a grant with no fact,
    /// a socket that is a link or lies in a write root, a dir grant with a floor, a
    /// synced folder or a registered project at or below it (also a floor that does not
    /// exist yet), and a grant that a user's `deny` or `ask` rule names, are never
    /// covered, and no rule is offered for them. One rule must cover every exit:
    /// `Covered` names the first that does. Else `Offer` holds the rule that an `a`
    /// answer would save, which passes [`Engine::check_allow`].
    pub fn always(&self, decision: &Decision, project: Option<&Path>) -> AlwaysVerdict {
        let machine = &self.auto;
        let machine = always::Machine {
            locations: &self.locations,
            hosts: &self.hosts,
            allow: &self.allow,
            user_rules: machine.policy.rules().get(machine.built_in..).unwrap_or_default(),
        };
        always::verdict(&machine, decision, project)
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
            hosts: Arc::default(),
            allow: Arc::new([]),
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

    /// The machine policy of `mode`: full mode has the policy of `auto`. A mode newer
    /// than this crate gets the `manual` policy.
    pub fn policy(&self, mode: Mode) -> &Policy {
        &self.machine(mode).policy
    }

    fn machine(&self, mode: Mode) -> &Machine {
        match mode {
            Mode::Cautious => &self.cautious,
            // NOTE: full mode decides by the rows of `auto`, then settles (DF1).
            Mode::Auto | Mode::Full => &self.auto,
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
        // NOTE: full mode decides as `auto` with the support of a machine with no
        // sandbox, and then settles the reasons (efr's full mode, DF1): the prediction,
        // the records, the home copies and the undo cover are those of `auto`.
        let full = mode == Mode::Full;
        let predicts = auto || full;
        let shell = requirements.command.is_some();
        let base_support = if full { self.support.for_full() } else { self.support };
        let scratch = self.locations.scratch_root(&input.conversation_policy.scratch);
        let project_root = match &input.scope {
            Scope::Project(id) => self.locations.widening_project_root(id),
            _ => None,
        };
        let envelope: Vec<PathBuf> = if predicts && shell {
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
        // in `auto` and in full mode they keep the cautious path rules: the auto policy
        // would let them write a project's `.git` or the host's real `/tmp`.
        let machine = if predicts && !shell { &self.cautious } else { self.machine(mode) };
        let judge = Judge {
            engine: self,
            machine,
            input,
            cx,
            contain: predicts && shell,
            support: base_support,
        };

        let analyzed = requirements.command.as_deref().map(command::analyze);
        let commands = match &analyzed {
            Some(Ok(commands)) => commands.as_slice(),
            Some(Err(_)) | None => &[],
        };
        let start = requirements.command_dir.as_deref().and_then(normalize);
        let parts = parts(commands, start.as_deref());

        let mut reasons = Vec::new();
        // NOTE: a file tool in `auto` or full mode runs in efrd: its exits come from its
        // paths, and a write that a built-in rule asks about is an exit that decides it.
        let tool_envelope = (predicts && !shell)
            .then(|| Envelope::new(&self.locations, project_root, scratch.as_deref()));
        let mut tool_found: Vec<ExitNeed> = Vec::new();
        for PathAccess { path, access } in &requirements.paths {
            let reason = match normalize(path) {
                Some(path) => {
                    let class = self.locations.classify_normal(&path, scratch.as_deref());
                    let reason = judge.path(path.clone(), *access, class);
                    match &tool_envelope {
                        Some(envelope) => {
                            let path = self.locations.rehome(&path);
                            let (reason, need) = judge.tool_exit(envelope, &path, *access, reason);
                            tool_found.extend(need);
                            reason
                        }
                        None => reason,
                    }
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
        let undoable = tool_envelope
            .as_ref()
            .is_some_and(|envelope| self.undoable_tool_call(envelope, requirements));
        if requirements.destructive && !undoable {
            // NOTE: no rule is read: a person approves each delete and move, unless the
            // turn runs in `auto` and `efr undo` can bring back every file that the
            // call writes (phase B). The paths are judged as writes on their own
            // reasons, so a path that is denied stays denied.
            reasons.push(Reason {
                subject: Subject::Destructive,
                effect: Effect::Ask,
                cause: Cause::Destructive,
            });
        }
        // NOTE: a background job (phase C). Its start call answers at once, and an undo
        // of its turn needs `--force` while the job may still write, so no delete of it
        // counts as covered: a destructive part stays an exit for the classifier.
        let job = requirements.background && shell;
        let support = if job { AutoSupport { undo: false, ..base_support } } else { base_support };
        let exit_input = requirements.command.as_deref().map(|line| ExitInput {
            line,
            command_dir: requirements.command_dir.as_deref(),
            paths: &requirements.paths,
            network: requirements.network,
            needs: requirements.needs.as_ref(),
            facts: requirements.facts.as_ref(),
            locations: &self.locations,
            project_root,
            scratch: scratch.as_deref(),
            support: &support,
            hosts: &self.hosts,
        });
        if predicts {
            let found = match &exit_input {
                Some(input) => exits::predict(input),
                None => exits::link_names(
                    exits::named_targets(tool_found, self.locations.home()),
                    requirements.facts.as_ref(),
                ),
            };
            if job {
                reasons.extend(found.iter().filter(|need| job_refusal(need)).map(not_as_job));
            }
            reasons.extend(found.into_iter().map(exit_reason));
            // NOTE: a view is no exit: no kind, no record, no classifier and no
            // question. The launch unions its grant with the grants of the exits.
            if let Some(input) = &exit_input {
                reasons.extend(exits::views(input).into_iter().map(view_reason));
            }
        } else if let (true, Some(input)) = (job, &exit_input) {
            // NOTE: outside `auto` the user approves each job before it starts, and no
            // rule decides it. The kinds that no job may hold stay refused in every mode,
            // also after a "yes".
            let found = exits::predict(input);
            reasons.extend(found.iter().filter(|need| job_refusal(need)).map(not_as_job));
            reasons.push(Reason {
                subject: Subject::Job,
                effect: Effect::Ask,
                cause: Cause::JobAsks,
            });
        }
        if let (true, Some(input)) = (full, &exit_input) {
            // NOTE: no rule is read: only the user approves a delete of a tree that full
            // mode guards, and no job may hold one (DF4, DF5).
            reasons.extend(
                exits::critical_deletes(input)
                    .into_iter()
                    .map(|delete| critical_reason(delete, job)),
            );
        }
        if full {
            reasons = crate::full::settle(reasons, job, |cause| judge.built_in(cause));
        }
        if reasons.is_empty() {
            let (effect, cause) =
                clamp_remote(Effect::Allow, Cause::NoRequirements, None, input.origin);
            reasons.push(Reason { subject: Subject::Nothing, effect, cause });
        }
        let notes = match (&exit_input, auto) {
            (Some(input), true) => exits::notes(input),
            _ => Vec::new(),
        };
        // NOTE: the always-allow matcher reads the real paths of a shell call's grants
        // (phase E); a file tool's decision never has them, so no rule covers it.
        let real_paths = match (&requirements.facts, auto && shell) {
            (Some(facts), true) => facts.real_paths.clone(),
            _ => Vec::new(),
        };
        Decision::from_reasons(reasons).with_notes(notes).with_real_paths(real_paths)
    }

    /// True when `efr undo` can bring back every file that a file tool call of
    /// `requirements` writes, deletes or moves, so its delete or move needs no question
    /// in `auto` (phase B, plan PB21): the machine has undo
    /// ([`AutoSupport::undo`]), and each path it writes is covered, the destination of
    /// a move too: by the turn's snapshot inside the turn's project and `$SCRATCH`, by
    /// the home store elsewhere ([`exits::tool_fact_requests`]).
    fn undoable_tool_call(
        &self,
        envelope: &Envelope<'_>,
        requirements: &crate::Requirements,
    ) -> bool {
        let Some(facts) = requirements.facts.as_ref().filter(|_| self.support.undo) else {
            return false;
        };
        let mut writes =
            requirements.paths.iter().filter(|access| access.access == Access::Write).peekable();
        writes.peek().is_some()
            && writes.all(|access| {
                let Some(path) = normalize(&access.path) else { return false };
                let ask = envelope.tool_cover_ask(&path);
                let allowed = match &ask {
                    crate::CoverAsk::Path(_) => envelope.may_cover(&ask),
                    _ => true,
                };
                allowed && facts.coverage(&ask).is_covered()
            })
    }

    /// The decision about deleting `paths` in the real tool caches, after a contained
    /// call of a turn of `scope`, whose `$SCRATCH` is `scratch`, deleted them in the
    /// private overlay of a cache (spec 5.2).
    ///
    /// Each path below a cache root is one `write` exit with the source
    /// [`ExitSource::CacheDelete`](efr_protocol::ExitSource::CacheDelete) that the
    /// classifier judges, with no grant and no bind: efrd deletes the files itself, and
    /// a cache needs no user message that names it. An engine secret is a `secret`
    /// floor, and efr's config or efr's own state a `config` floor. A path outside every
    /// cache, a floor of a cache such as `~/.cargo/bin` or `~/.cargo/config.toml`, and a
    /// mask are refused with [`Cause::NotCacheFile`]. efrd checks each path again
    /// against its own pins before it deletes one.
    pub fn decide_cache_delete(
        &self,
        paths: &[PathBuf],
        scope: &Scope,
        scratch: &Path,
    ) -> Decision {
        let scratch = self.locations.scratch_root(scratch);
        let project_root = match scope {
            Scope::Project(id) => self.locations.widening_project_root(id),
            _ => None,
        };
        let envelope = Envelope::new(&self.locations, project_root, scratch.as_deref());
        let mut reasons = Vec::new();
        let mut found = Vec::new();
        for declared in paths {
            let Some(path) =
                normalize(declared).map(|path| self.locations.rehome(&path).into_owned())
            else {
                reasons.push(Reason {
                    subject: Subject::Path {
                        path: declared.clone(),
                        access: Access::Write,
                        class: None,
                    },
                    effect: Effect::Deny,
                    cause: Cause::NotAbsolute,
                });
                continue;
            };
            let kind = if self.locations.is_sealed(&path) || self.locations.is_secret(&path) {
                Some(ExitKind::Secret)
            } else if self.locations.is_write_sealed(&path) || self.locations.is_efr_own(&path) {
                Some(ExitKind::Config)
            } else if self.locations.rebuild(&path) == Some(Rebuild::Cache)
                && !envelope.is_floor(&path)
                && !envelope.is_masked(&path)
            {
                Some(ExitKind::Write)
            } else {
                None
            };
            match kind {
                Some(kind) => {
                    let mut need =
                        ExitNeed::new(kind, path.display().to_string(), ExitSource::CacheDelete);
                    need.approver = Approver::Classifier;
                    need.target = Some(path);
                    exits::set_copies(&mut need, None, &self.support);
                    found.push(need);
                }
                None => {
                    let class = self.locations.classify_normal(&path, scratch.as_deref());
                    reasons.push(Reason {
                        subject: Subject::Path { path, access: Access::Write, class: Some(class) },
                        effect: Effect::Deny,
                        cause: Cause::NotCacheFile,
                    });
                }
            }
        }
        let found = exits::named_targets(found, self.locations.home());
        reasons.extend(found.into_iter().map(exit_reason));
        if reasons.is_empty() {
            reasons.push(Reason {
                subject: Subject::Nothing,
                effect: Effect::Deny,
                cause: Cause::NotCacheFile,
            });
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
        let class = self.locations.classify_normal(&path, scratch.as_deref());
        let floor = envelope.is_floor(&path);
        let masked = envelope.is_masked(&path);
        let guarded = floor || class == PathClass::Secrets || masked;
        Some(PathFacts {
            class,
            in_write_root: envelope.in_root(&path),
            floor,
            synced: self.locations.synced_roots().iter().any(|root| path.starts_with(root)),
            masked,
            rebuild: if guarded { None } else { self.locations.rebuild(&path) },
        })
    }
}

/// The refusal of the part of a background job's line that `need` leaves the sandbox
/// for: no job may hold it ([`job_refusal`]).
fn not_as_job(need: &ExitNeed) -> Reason {
    Reason {
        subject: Subject::JobPart { part: need.part.clone(), kind: need.kind },
        effect: Effect::Deny,
        cause: Cause::NotAsJob,
    }
}

/// The reason of a delete that full mode guards: a question, or in a background job a
/// refusal, since nobody answers a job's question.
fn critical_reason(delete: CriticalDelete, job: bool) -> Reason {
    let (effect, cause) =
        if job { (Effect::Deny, Cause::NotAsJob) } else { (Effect::Ask, Cause::CriticalDelete) };
    Reason { subject: Subject::CriticalDelete { delete }, effect, cause }
}

/// The reason of one view of the host, such as `Grant::NetView`: contained, so it never
/// raises the effect of the call.
fn view_reason(grant: Grant) -> Reason {
    Reason { subject: Subject::View { grant }, effect: Effect::Contain, cause: Cause::Contained }
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
    /// True for a shell call in `auto` or full mode: in `auto` it runs in the sandbox,
    /// and full mode lets what the sandbox would hold run.
    contain: bool,
    /// What the machine supports for this call: the `auto` sandbox's, or in full mode
    /// [`AutoSupport::for_full`].
    support: AutoSupport,
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

    /// For a `read_file`, `write_file`, edit or patch in `auto`, which efrd runs outside
    /// the sandbox: the reason of one path and its exit. A read of a sandbox mask is a
    /// `masked_read` exit. A write that a built-in rule asks about is an exit of its own
    /// kind (spec 7.4), which decides it: the path's reason becomes `Contain` with
    /// [`Cause::ExitDecides`], so a call whose exit got lost still asks. A user's or a
    /// conversation's rule keeps its cause and makes no exit. An allowed write to a
    /// floor inside a write root is a `persistence` exit.
    fn tool_exit(
        &self,
        envelope: &Envelope<'_>,
        path: &Path,
        access: Access,
        reason: Reason,
    ) -> (Reason, Option<ExitNeed>) {
        let facts = self.input.requirements.facts.as_ref();
        let support = &self.support;
        match access {
            Access::Write if reason.effect == Effect::Ask && self.built_in(&reason.cause) => {
                let need = envelope.tool_write_exit(path, facts, support);
                (
                    Reason { effect: Effect::Contain, cause: Cause::ExitDecides, ..reason },
                    Some(need),
                )
            }
            Access::Write if reason.effect != Effect::Allow => (reason, None),
            Access::Read | Access::ReadTree | Access::Write => {
                let need = envelope.tool_exit(path, access, facts, support);
                (reason, need)
            }
        }
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
            if *access == Access::Write && self.engine.locations.is_efr_own(path) {
                return (Effect::Deny, Cause::EfrOwn);
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
