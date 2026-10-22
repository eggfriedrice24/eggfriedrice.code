//! Background jobs at the check point (phase C): what starts at once, what the
//! classifier or the user decides, what no job may hold, and the question that each
//! job asks outside `auto`.

use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, CallId, Event, ExitInfo, JudgeKind, Launch, Mode, Origin,
    SandboxStatus, TurnId, Verdict,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::review::{judgements, reviewed, run, shell, start};
use crate::ConversationError;
use crate::exit::AWAY_EXPIRED;
use crate::testing::{FakeJudge, Harness, Script, Setup, allows};
use crate::turn::user_denied;

/// One shell call of `line` that starts a background job.
fn job(line: &str) -> (&'static str, Value) {
    ("shell", json!({ "command": line, "background": true }))
}

#[tokio::test]
async fn privilege_job_refused_as_job() {
    let judge = FakeJudge::answering(Vec::new());
    let (h, script) =
        run(reviewed(Some(Arc::clone(&judge))), vec![job("docker run --rm alpine")], "go").await;

    let results = script.results();
    assert_eq!(results.len(), 1);
    let (output, is_error) = &results[0];
    assert!(*is_error);
    assert_eq!(
        output,
        "The shell call did not start a job: \"docker run --rm alpine\" cannot run as a job. \
         Run that part in the foreground, and the rest as a job."
    );
    assert_eq!(judge.calls(), 0, "no review");
    assert!(h.toolbox.invoked().is_empty(), "no job started");
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })), "no question");
}

#[tokio::test]
async fn job_refused_kind_refused_in_cautious() {
    let script = Script::new(vec![job("xdotool type hello")]);
    let mut h =
        Setup::new().start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("type hello for me later").await;
    h.wait_end(sent.turn_id).await;

    let (output, is_error) = &script.results()[0];
    assert!(*is_error && output.contains("cannot run as a job"), "{output}");
    assert!(h.toolbox.invoked().is_empty());
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })), "no question");
    let refusal = events.iter().find_map(|event| match event {
        Event::ToolCallCompleted { refusal, .. } => refusal.clone(),
        _ => None,
    });
    assert_eq!(
        refusal.as_deref(),
        Some("run \"xdotool type hello\" as a job (desktop_ipc): it cannot run as a job")
    );
}

/// The question of the first approval request: its summary and its exit.
async fn asked(h: &mut Harness) -> (CallId, String, Option<ExitInfo>) {
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, summary, exit, .. } = asked else {
        unreachable!("wait_for returns an event it accepted");
    };
    (call_id, summary, exit)
}

/// Starts a turn of `setup` whose model starts `call`, and waits for its question.
async fn job_question(
    setup: Setup,
    call: (&'static str, Value),
) -> (Harness, Arc<Script>, TurnId, CallId, String) {
    let script = Script::new(vec![call]);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("start the dev server").await;
    let (call_id, summary, exit) = asked(&mut h).await;
    assert_eq!(exit, None, "outside auto a job has no exits");
    (h, script, sent.turn_id, call_id, summary)
}

#[tokio::test]
async fn contained_job_starts_without_question_or_classifier() {
    let judge = FakeJudge::answering(Vec::new());
    let (h, script) =
        run(reviewed(Some(Arc::clone(&judge))), vec![job("npm run dev")], "run the server").await;

    assert_eq!(judge.calls(), 0);
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::Contained { grants: Vec::new() });
    assert_eq!(script.results(), vec![("job j1 started: npm run dev".to_owned(), false)]);
}

#[tokio::test]
async fn job_exit_goes_to_classifier_with_background_fact() {
    let judge = FakeJudge::answering(vec![allows()]);
    let (h, _) = run(
        reviewed(Some(Arc::clone(&judge))),
        vec![job("git push origin main")],
        "push this branch when the tests pass",
    )
    .await;

    assert_eq!(judge.calls(), 1);
    let action = &judge.seen()[0].record.action;
    assert!(action.background);
    assert_eq!(action.max_minutes, Some(60), "the default limit, so the judge sees the lifetime");
    assert!(!action.watch && !action.wake_turn);
    assert_eq!(h.toolbox.ran().len(), 1);
}

#[tokio::test]
async fn background_record_has_max_minutes() {
    let judge = FakeJudge::answering(vec![allows()]);
    let input = json!({ "command": "git push origin main", "background": true,
                        "max_minutes": 180, "watch": { "pattern": "error" } });
    let (_, _) = run(reviewed(Some(Arc::clone(&judge))), vec![("shell", input)], "push it").await;

    let action = &judge.seen()[0].record.action;
    assert_eq!((action.background, action.max_minutes, action.watch), (true, Some(180), true));
    // A call in the foreground names no job.
    let judge = FakeJudge::answering(vec![allows()]);
    let (_, _) =
        run(reviewed(Some(Arc::clone(&judge))), vec![shell("git push origin main")], "push it")
            .await;
    let action = &judge.seen()[0].record.action;
    assert_eq!((action.background, action.max_minutes, action.watch), (false, None, false));
}

#[tokio::test]
async fn outside_job_goes_to_classifier() {
    let judge = FakeJudge::answering(vec![allows()]);
    let (h, _) = run(
        reviewed(Some(Arc::clone(&judge))),
        vec![job("git push origin main")],
        "push this branch",
    )
    .await;

    assert_eq!(judgements(&h).await, vec![(JudgeKind::Classifier, Verdict::Allow)]);
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::Unsandboxed, "the exit child as a job");
    assert_eq!(h.toolbox.invoked()[0].1["background"], json!(true));
}

#[tokio::test]
async fn job_grant_lasts_for_job() {
    let input = json!({ "command": "curl -sS https://example.com/feed", "background": true,
                        "network": true });
    let (mut h, _, turn_id) = start(reviewed(None), vec![("shell", input)], "watch the feed").await;
    let (call_id, _, exit) = asked(&mut h).await;
    let exit = exit.expect("the question shows the exit");
    assert!(
        exit.facts.contains(
            &"runs as a job for up to 60 minutes; these grants stay open until it ends".to_owned()
        ),
        "{:?}",
        exit.facts
    );
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(turn_id).await;

    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert!(!ran[0].launch.grants().is_empty(), "the grant reaches the job's launch");
    assert_eq!(ran[0].launch.grants(), exit.grants.as_slice());
}

#[tokio::test]
async fn job_question_names_lifetime() {
    let input = json!({ "command": "curl -sS https://example.com/feed", "background": true,
                        "network": true, "max_minutes": 90 });
    let (mut h, _, _) = start(reviewed(None), vec![("shell", input)], "watch the feed").await;
    let (_, _, exit) = asked(&mut h).await;
    let facts = exit.expect("the question shows the exit").facts;
    assert_eq!(
        facts[0],
        "runs as a job for up to 90 minutes; these grants stay open until it ends"
    );
}

#[tokio::test]
async fn phone_cannot_answer_job_question() {
    let (mut h, _, turn_id, call_id, _) = job_question(Setup::new(), job("npm run dev")).await;
    let params = ApprovalRespond {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        call_id,
        decision: ApprovalDecision::Allow,
    };
    let refused = h.handle.respond_approval(params, Origin::Phone).await;
    assert!(
        matches!(
            refused,
            Err(ConversationError::RemotePrivilegeAnswer { origin: Origin::Phone, .. })
        ),
        "{refused:?}"
    );
    assert_eq!(h.handle.state().await.unwrap().pending_approvals, vec![call_id], "still open");
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(turn_id).await;
    assert_eq!(h.toolbox.invoked().len(), 1);
}

#[tokio::test]
async fn job_in_cautious_asks_user() {
    let (mut h, script, turn_id, call_id, summary) =
        job_question(Setup::new(), job("npm run dev")).await;
    assert_eq!(
        summary,
        "shell: run \"npm run dev\"; start a background job\nruns as a job for up to 60 \
         minutes; these grants stay open until it ends"
    );
    assert!(h.toolbox.invoked().is_empty(), "nothing starts before the answer");
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(turn_id).await;

    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::Direct);
    assert_eq!(script.results(), vec![("job j1 started: npm run dev".to_owned(), false)]);
}

#[tokio::test]
async fn job_in_manual_asks_user() {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Manual;
    let (mut h, _, turn_id, call_id, summary) = job_question(setup, job("ls -la")).await;
    assert!(summary.contains("start a background job"), "{summary}");
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(turn_id).await;
    assert_eq!(h.toolbox.invoked().len(), 1);
}

#[tokio::test]
async fn job_question_denied_starts_no_job() {
    let (mut h, script, turn_id, call_id, _) = job_question(Setup::new(), job("npm run dev")).await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    assert_eq!(script.results(), vec![(user_denied("shell"), true)]);
}

#[tokio::test]
async fn job_question_away_expires_starts_no_job() {
    let setup = Setup::new();
    setup.presence.set_away(true);
    let (mut h, script, turn_id, call_id, _) = job_question(setup, job("npm run dev")).await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(300));
    h.wait_end(turn_id).await;

    assert!(h.toolbox.invoked().is_empty());
    assert_eq!(script.results(), vec![(AWAY_EXPIRED.to_owned(), true)]);
    let events = h.events().await;
    assert!(events.contains(&Event::ApprovalExpired { turn_id, call_id, away: true }));
}

#[tokio::test]
async fn auto_fallback_cautious_job_asks_user() {
    let mut setup = reviewed(Some(FakeJudge::answering(Vec::new())));
    setup.sandbox = SandboxStatus::unavailable("bubblewrap is not installed");
    let (mut h, _, turn_id, call_id, summary) = job_question(setup, job("npm run dev")).await;
    assert!(summary.contains("start a background job"), "{summary}");
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(turn_id).await;
    assert_eq!(h.toolbox.ran()[0].launch, Launch::Direct);
}

/// How often `text` is in `request`, in the text blocks of its messages.
fn count_in(request: &efr_provider::Request, text: &str) -> usize {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            efr_provider::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .map(|block| block.matches(text).count())
        .sum()
}

#[tokio::test]
async fn efr_notes_reach_the_next_request_once() {
    let early = "efr: job j0 listens on 127.0.0.1:8765; forwarded";
    let late = "efr: job j1 listens on 127.0.0.1:8767; not forwarded";
    let script = Script::new(vec![job("python3 -m http.server 8767")]);
    let mut h =
        reviewed(None).start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    h.toolbox.notes.lock().unwrap().0.push(early.to_owned());
    h.toolbox.notes_after_job.lock().unwrap().push(late.to_owned());
    let sent = h.prompt("start a server on 8767").await;
    h.wait_end(sent.turn_id).await;

    let requests = script.requests();
    assert_eq!(requests.len(), 2, "the job, then the answer");
    assert_eq!(count_in(&requests[0], early), 1, "a note before the turn: its first request");
    assert_eq!(count_in(&requests[0], late), 0);
    // NOTE: the note that came while the turn ran reaches the model in the same turn.
    assert_eq!(count_in(&requests[1], late), 1, "the next request of the same turn");
    assert_eq!(count_in(&requests[1], early), 1, "once: only in the history");
    let message = requests[1].messages.last().unwrap();
    assert_eq!(message.role, efr_provider::Role::User);
    assert_eq!(
        message.content,
        [efr_provider::ContentBlock::Text { text: format!("{}\n{late}", crate::NOT_USER_NOTE) }],
        "a message of its own, which says that no person wrote it"
    );

    let sent = h.prompt("and now?").await;
    h.wait_end(sent.turn_id).await;
    let requests = script.requests();
    let last = requests.last().unwrap();
    assert_eq!((count_in(last, early), count_in(last, late)), (1, 1), "never given again");
    assert_eq!(count_in(last, crate::NOT_USER_NOTE), 2, "no new note message");
    assert_eq!(h.toolbox.notes.lock().unwrap().1, 3, "the turn asks before each model call");
}
