//! The git code check of a git or gh line that may run outside the sandbox (v0.0.4,
//! DV22, DV23), in the toolbox.
//!
//! - At the facts of the call, efrd first checks the surface guard of every running
//!   contained job (a `.git` file, a `commondir` or an `include.path` can point
//!   anywhere, so not only the jobs of the line's dir), then collects the git code of
//!   the line's repositories with the places where a contained call can write
//!   ([`facts::git_code`]). It keeps the answer for the call.
//! - Right before the exit child starts, after the same guard check and under the plan
//!   lock, efrd collects the git code again. When the parts that a sandboxed command
//!   can write differ from the ones of the question, or the check now fails, the line
//!   does not run, and the model reads why: a new call gets a new question.
//! - A contained call that ran past its timeout can still write in its write roots,
//!   so they count as writable until its launcher wrote its result.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use efr_conversation::CallContext;
use efr_permissions::{AllowRule, GitCodeAsk};
use efr_protocol::{CallId, GitCodeFacts, TurnId};

use super::DaemonToolbox;
use crate::sandbox::SandboxService;
use crate::sandbox::facts::git_code::{self, CONFIG_VARIABLES, CodeInput, Places};
use crate::sandbox::facts::real_path;

/// What the model reads when the git code of an approved line changed before it ran.
pub(crate) const GIT_CODE_CHANGED: &str = "[efr: the git code of this command changed after \
                                           the question, so the command did not run. Run it \
                                           again to get a new question.]";

/// The variables of the hidden shell that choose the program that git runs for ssh.
const SSH_VARIABLES: &[&str] = &["GIT_SSH_COMMAND", "GIT_SSH"];

/// What the toolbox keeps for the git code checks.
#[derive(Debug, Default)]
pub(super) struct GitChecks {
    /// The check of each call that has one, until it runs or its turn ends.
    calls: Mutex<HashMap<CallId, Kept>>,
    /// The call dir and the write roots of each contained call that ran past its
    /// timeout, until its launcher wrote its result or its dir went.
    timed_out: Mutex<Vec<(PathBuf, Vec<PathBuf>)>>,
}

/// One call's check.
#[derive(Debug, Clone)]
struct Kept {
    turn: TurnId,
    asks: Vec<GitCodeAsk>,
    facts: Option<GitCodeFacts>,
}

impl GitChecks {
    fn calls(&self) -> std::sync::MutexGuard<'_, HashMap<CallId, Kept>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Forgets the checks of the calls of `turn`.
    pub(super) fn end_turn(&self, turn: TurnId) {
        self.calls().retain(|_, kept| kept.turn != turn);
    }

    /// Forgets the check of `call`.
    pub(super) fn forget(&self, call: CallId) {
        self.calls().remove(&call);
    }

    /// True when `call` has a check.
    pub(super) fn has(&self, call: CallId) -> bool {
        self.calls().contains_key(&call)
    }

    /// Keeps the write roots of a contained call that runs on past its timeout in
    /// `call_dir`.
    pub(super) fn keep_timed_out(&self, call_dir: PathBuf, roots: Vec<PathBuf>) {
        self.timed_out.lock().unwrap_or_else(PoisonError::into_inner).push((call_dir, roots));
    }

    /// The write roots of each contained call that still runs past its timeout.
    fn timed_out_roots(&self) -> Vec<PathBuf> {
        let mut kept = self.timed_out.lock().unwrap_or_else(PoisonError::into_inner);
        kept.retain(|(dir, _)| dir.is_dir() && !dir.join(efr_sandbox::RESULT_FILE).exists());
        kept.iter().flat_map(|(_, roots)| roots.iter().cloned()).collect()
    }
}

impl DaemonToolbox {
    /// The git code of the repositories of `asks` for the call `call`, which the call
    /// keeps until it runs; `None` for no ask.
    pub(super) async fn git_code_facts(
        &self,
        call: &CallContext,
        sandbox: &SandboxService,
        asks: &[GitCodeAsk],
        shell_path: &str,
    ) -> Option<GitCodeFacts> {
        if asks.is_empty() {
            return None;
        }
        let facts = self.check_git_code(call, sandbox, asks, shell_path).await;
        let kept = Kept { turn: call.turn_id, asks: asks.to_vec(), facts: facts.clone() };
        self.git_checks.calls().insert(call.call_id, kept);
        facts
    }

    /// Checks the git code of `call` again right before it runs outside the sandbox.
    /// `Some` is what the model reads when it must not run: the parts that a sandboxed
    /// command can write differ from the ones of the question, or the check failed now
    /// and did not before.
    pub(super) async fn recheck_git_code(
        &self,
        call: &CallContext,
        sandbox: &SandboxService,
    ) -> Option<String> {
        let kept = self.git_checks.calls().remove(&call.call_id)?;
        let shell_path = sandbox.shell_path(call.conversation_id);
        let now = self.check_git_code(call, sandbox, &kept.asks, &shell_path).await;
        if same(kept.facts.as_ref(), now.as_ref()) {
            return None;
        }
        tracing::info!(call_id = %call.call_id, "the git code of an approved line changed before it ran");
        Some(GIT_CODE_CHANGED.to_owned())
    }

    /// One check of the git code of `asks`: the guard of every running contained job
    /// first, then the git runs.
    async fn check_git_code(
        &self,
        call: &CallContext,
        sandbox: &SandboxService,
        asks: &[GitCodeAsk],
        shell_path: &str,
    ) -> Option<GitCodeFacts> {
        // NOTE: a break stops the job and moves the plant to its quarantine before git
        // reads it (DV23).
        if let Some(jobs) = &self.jobs {
            jobs.check_guards(None).await;
        }
        let places = self.places(call, sandbox).await;
        let env = self.git_env(call, sandbox).await;
        let input = CodeInput {
            git: sandbox.git(),
            clock: &self.clock,
            home: &self.home,
            env: env.as_ref(),
            shell_path,
            places: &places,
        };
        git_code::collect(asks, &input).await
    }

    /// Where a contained call can write now: the `$SCRATCH` dirs of every conversation,
    /// the write rules and write roots of the config, the write roots of each running
    /// contained job and of each call past its timeout, and the registered projects.
    async fn places(&self, call: &CallContext, sandbox: &SandboxService) -> Places {
        let settings = std::sync::Arc::clone(&self.settings.borrow());
        let home = self.home.path();
        let mut open: Vec<PathBuf> = Vec::new();
        // NOTE: every conversation's `$SCRATCH` lies in `$D/scratch` (`$D` is the second
        // of efr's roots), as this call's does.
        let base = match self.efr_roots.get(1) {
            Some(data) => Some(data.join(crate::state::SCRATCH_DIR)),
            None => call.scratch.parent().map(Path::to_path_buf),
        };
        open.extend(base);
        for rule in &settings.sandbox.allow {
            if let AllowRule::Write { under, .. } = rule {
                open.push(efr_sandbox::expand_home(Path::new(under), home));
            }
        }
        open.extend(
            settings.sandbox.write_roots.iter().map(|root| efr_sandbox::expand_home(root, home)),
        );
        if let Some(jobs) = &self.jobs {
            for job in jobs.running().into_iter().filter(|job| job.contained) {
                open.extend(job.write_roots);
            }
        }
        open.extend(self.git_checks.timed_out_roots());
        let projects = sandbox.projects().await;
        let canonical = |roots: Vec<PathBuf>| -> Vec<PathBuf> {
            let mut out: Vec<PathBuf> = Vec::new();
            for root in roots.into_iter().filter(|root| root.is_absolute()) {
                for form in [real_path(&root), Some(root)].into_iter().flatten() {
                    if !out.contains(&form) {
                        out.push(form);
                    }
                }
            }
            out
        };
        let (open, projects) =
            tokio::task::spawn_blocking(move || (canonical(open), canonical(projects)))
                .await
                .unwrap_or_default();
        Places { open, projects }
    }

    /// The git variables of the hidden shell of `call`: a test's job environment, else
    /// the `env` file that the zsh integration writes at each prompt. `None` when no
    /// shell wrote one yet: git then runs with efrd's start environment, which is the
    /// one that the hidden shells start with.
    async fn git_env(
        &self,
        call: &CallContext,
        sandbox: &SandboxService,
    ) -> Option<BTreeMap<String, String>> {
        let wanted = |name: &str| CONFIG_VARIABLES.contains(&name) || SSH_VARIABLES.contains(&name);
        if let Some(env) = &self.job_env {
            return Some(
                env.iter()
                    .filter(|(name, _)| wanted(name))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            );
        }
        let file = sandbox.shell_dir(call.conversation_id).join(efr_sandbox::ENV_FILE);
        let bytes = tokio::task::spawn_blocking(move || {
            crate::sandbox::fs::DaemonFs::read(&file, efr_sandbox::MAX_ENV_BYTES).ok()
        })
        .await
        .ok()
        .flatten()
        .filter(|bytes| !bytes.is_empty())?;
        Some(
            bytes
                .split(|byte| *byte == 0)
                .filter_map(|entry| std::str::from_utf8(entry).ok()?.split_once('='))
                .filter(|(name, _)| wanted(name))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        )
    }
}

/// True when two checks agree: both checked or both not, with the same parts that a
/// sandboxed command can write.
fn same(before: Option<&GitCodeFacts>, now: Option<&GitCodeFacts>) -> bool {
    let shape = |facts: Option<&GitCodeFacts>| {
        facts.map(|facts| {
            let mut parts: Vec<(String, PathBuf)> = facts
                .items
                .iter()
                .filter(|item| item.writable)
                .map(|item| (item.item.clone(), item.file.clone()))
                .collect();
            parts.sort();
            parts.dedup();
            (facts.checked, parts)
        })
    };
    shape(before) == shape(now)
}

#[cfg(test)]
mod tests;
