use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{
    ActionFacts, CallId, ChainStep, ConversationId, ExitFacts, ExitKind, ExitRecord, ExitSource,
    Judgement, Risk, Scope, TurnId, UserAuthorization, Verdict,
};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use super::{ExitJudge, JudgeInput, JudgeOutcome};

fn send<T: Send>(_: &T) {}

fn record() -> ExitRecord {
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: vec!["copy the report to ~/Documents".to_owned()],
        action: ActionFacts {
            tool: "shell".to_owned(),
            line: "cp report.pdf ~/Documents/".to_owned(),
            cwd: "/home/u/p/app".into(),
            scope: Scope::Machine,
            exits: vec![ExitKind::Write],
            grants: Vec::new(),
            source: ExitSource::Predicted,
            edit: None,
            background: false,
            max_minutes: None,
            watch: false,
            wake_turn: false,
        },
        facts: ExitFacts::default(),
    }
}

/// Allows every exit with the turn's model, as the second step of the chain would.
#[derive(Debug)]
struct TurnModelAllows;

#[async_trait]
impl ExitJudge for TurnModelAllows {
    async fn judge(&self, input: JudgeInput<'_>) -> JudgeOutcome {
        JudgeOutcome {
            judgement: Some(Judgement {
                verdict: Verdict::Allow,
                risk: Risk::Low,
                user_authorization: UserAuthorization::High,
                category: "fits the request".to_owned(),
                rationale: input.record.action.line.clone(),
            }),
            model: Some(input.turn_model.to_owned()),
            chain_step: ChainStep::TurnModel,
            skipped: vec!["gpt-6-luna: missing".to_owned()],
            latency_ms: 0,
            record_sha256: None,
            cached: false,
        }
    }
}

#[tokio::test]
async fn judge_input_and_outcome_are_send() {
    let record = record();
    let clock = TestClock::new();
    let id = |seed: u64| uuid_v7(&clock, &TestRng::new(seed));
    let input = JudgeInput {
        conversation_id: ConversationId::from_uuid(id(1)),
        turn_id: TurnId::from_uuid(id(2)),
        call_id: CallId::from_uuid(id(3)),
        turn_model: "gpt-5.5",
        turn_effort: Some("low"),
        record: &record,
    };
    send(&input);
    let judge: Arc<dyn ExitJudge> = Arc::new(TurnModelAllows);
    // The turn awaits the judge in a task of its own, so the future must be Send too.
    let future = judge.judge(input);
    send(&future);
    let outcome = future.await;
    send(&outcome);

    assert_eq!(outcome.chain_step, ChainStep::TurnModel);
    assert_eq!(outcome.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(
        outcome.judgement.map(|judgement| judgement.rationale),
        Some("cp report.pdf ~/Documents/".to_owned())
    );
}
