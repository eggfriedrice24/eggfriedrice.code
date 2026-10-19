//! Always-allow rules in a turn (phase E, efr's auto spec 11.2, DE19 to DE21).
//!
//! [`Turn::always`] runs in `Turn::ask` before the classifier: when an `auto` call asks
//! only for its exits and a rule of `sandbox.allow` covers every one of them
//! ([`Engine::always`](efr_permissions::Engine::always)), the call runs with the same
//! widening that a "yes" gives. It records `exit_requested` and `exit_judged` with the
//! judge `always`, the rule's place (`sandbox.allow[N]`) as the category and the rule in
//! words as the rationale. It needs no judge and no person: it runs while the
//! classifier is off or paused, and while the user is away. It counts as a judged
//! allow, but it does not reset the classifier's denials in a row, because no
//! reviewer or person looked at the call.
//!
//! When no rule covers the call but one may, the question offers that rule for an `a`
//! answer, and only when the daemon gave a writer ([`AllowWriter`](crate::AllowWriter)).
//! An `a` answer runs the call as a "yes"; then [`Turn::save_always`] hands the rule to
//! the writer and records `always_rule_saved` with what came of it.

use efr_permissions::{AllowRule, AlwaysVerdict};
use efr_protocol::{AlwaysSaved, Event, ExitRecord, JudgeKind, Scope, Verdict};

use super::{Authorization, Ruling, Turn};
use crate::exit;
use crate::{ConversationError, ToolCall};

impl Turn {
    /// What the always-allow rules say about the exits of `ruling`: a rule that covers
    /// them, a rule that an `a` answer would save, or neither. Only a call of an `auto`
    /// turn that the sandbox's launcher runs asks the engine.
    pub(super) fn always(&self, call: &ToolCall, ruling: &Ruling) -> AlwaysVerdict {
        if !call.context.auto || !ruling.launch.uses_launcher() {
            return AlwaysVerdict::None;
        }
        let project = match &call.context.scope {
            Scope::Project(id) => ruling.engine.locations().project_root(id),
            _ => None,
        };
        ruling.engine.always(&ruling.decision, project)
    }

    /// Runs `call` by the rule `sandbox.allow[index]`, which covers every exit of
    /// `ruling`, whose record is `record`: see the module doc.
    pub(super) async fn covered(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
        record: ExitRecord,
        index: usize,
    ) -> Result<Authorization, ConversationError> {
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        let kinds = exit::kinds(&ruling.decision);
        let rule = ruling.engine.allow_rules().get(index).map(|(rule, _)| rule.describe());
        self.record(vec![
            Event::ExitRequested {
                turn_id,
                call_id,
                kinds: kinds.clone(),
                grants: ruling.launch.grants().to_vec(),
                source: exit::source(&ruling.decision),
                record: Box::new(record),
            },
            Event::ExitJudged {
                turn_id,
                call_id,
                judge: JudgeKind::Always,
                verdict: Verdict::Allow,
                model: None,
                latency_ms: None,
                risk: None,
                user_authorization: None,
                category: Some(format!("sandbox.allow[{index}]")),
                rationale: rule,
                record_sha256: None,
                cached: false,
                chain_step: None,
                skipped: Vec::new(),
            },
        ])
        .await?;
        // NOTE: a judged allow for the record of the next exits, but no reset of the
        // classifier's denials in a row: nobody looked at this call.
        self.exits.judged(&kinds, Verdict::Allow);
        Ok(Authorization::Allowed { approved_interactive: false, launch: ruling.launch.clone() })
    }

    /// Hands `rule`, which the user saved with an `a` answer to the question of `call`,
    /// to the daemon's writer, and records what came of it. Without a writer nothing is
    /// saved; a question offers no rule then.
    pub(super) async fn save_always(
        &mut self,
        call: &ToolCall,
        rule: AllowRule,
    ) -> Result<(), ConversationError> {
        let Some(writer) = self.shared.deps.allow_writer.clone() else {
            return Ok(());
        };
        let outcome = writer.add(rule).await;
        match &outcome {
            AlwaysSaved::Saved { index, rule } => {
                tracing::info!(index, rule = %rule, "an always rule was saved");
            }
            AlwaysSaved::NotSaved { reason } => {
                tracing::warn!(reason = ?reason, "an always rule was not saved; the call runs once");
            }
            _ => {}
        }
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        self.record(vec![Event::AlwaysRuleSaved { turn_id, call_id, outcome }]).await?;
        Ok(())
    }
}
