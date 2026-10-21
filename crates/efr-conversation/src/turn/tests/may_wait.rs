//! Which calls may run past the model's timeout while a person follows (DS3), what the
//! question says about a password (DS1, DS50), and who approved each call (DS52).
//!
//! NOTE: the fake toolbox runs no command: each line here is only read by the engine,
//! and `may_prompt` in the input stands for the soft flag that the shell tool declares.

use efr_permissions::{Action, CommandPattern, Effect, Engine, Locations, Policy, Resource, Rule};
use efr_protocol::{
    ApprovalDecision, ApprovedBy, Event, FullModeStatus, Launch, Mode, SandboxStatus,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::review::{reviewed, run, start};
use crate::testing::{FakeJudge, Harness, Setup, allows};

/// A shell call of `line` whose line may ask for a password (the soft flag).
fn prompting(line: &str) -> (&'static str, Value) {
    ("shell", json!({ "command": line, "may_prompt": true }))
}

/// A shell call of `line` that may ask for a password and reaches the network.
fn networked(line: &str) -> (&'static str, Value) {
    ("shell", json!({ "command": line, "may_prompt": true, "network": true }))
}

/// The line of the incident: an upload to another host over ssh with a password.
const EVIDENCE: &str = "rsync -a -e 'ssh -o PreferredAuthentications=password' \
                        eggfriedrice@t:.config/Slack /home/u/scratch/slack";

/// The first question of the log.
async fn question(h: &Harness) -> Event {
    h.events()
        .await
        .into_iter()
        .find(|event| matches!(event, Event::ApprovalRequested { .. }))
        .expect("a question")
}

/// Answers the first question of the turn `turn_id` with `decision` and waits for the
/// end.
async fn answer_and_end(
    h: &mut Harness,
    turn_id: efr_protocol::TurnId,
    decision: ApprovalDecision,
) {
    let call_id = h.wait_approval().await;
    h.answer(call_id, decision).await;
    h.wait_end(turn_id).await;
}

#[tokio::test]
async fn reviewed_exit_with_may_prompt_may_wait() {
    let judge = FakeJudge::answering(vec![allows()]);
    let setup = reviewed(Some(judge));
    let (h, _) = run(setup, vec![networked("git push origin main")], "push this branch").await;

    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::Unsandboxed);
    assert!(ran[0].may_prompt);
    assert!(ran[0].may_wait, "the limit holds only while a person follows, so the reviewer may");
    assert_eq!(ran[0].approved_by, Some(ApprovedBy::Reviewer));
}

#[tokio::test]
async fn full_call_with_may_prompt_may_wait() {
    let mut setup = reviewed(None);
    setup.config.mode = Mode::Full;
    setup.config.full_mode = FullModeStatus { allowed: true, refused: None };
    setup.sandbox = SandboxStatus::unavailable("bubblewrap is not installed");
    let (h, _) = run(setup, vec![prompting("cargo build")], "build").await;

    let ran = h.toolbox.ran();
    assert_eq!(ran[0].launch, Launch::Direct);
    assert!(ran[0].may_wait, "full mode keeps the long limit of a call that may ask (OQS1)");
    assert_eq!(ran[0].approved_by, Some(ApprovedBy::FullMode));
    assert!(!h.events().await.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })));
}

#[tokio::test]
async fn contained_call_never_may_wait() {
    let (h, _) = run(reviewed(None), vec![prompting("cargo test")], "test").await;

    let ran = h.toolbox.ran();
    assert_eq!(ran[0].launch, Launch::contained());
    assert!(ran[0].may_prompt);
    assert!(!ran[0].may_wait, "efr types no secret into the sandbox");
    assert_eq!(ran[0].approved_by, None, "a contained call needs no approval");
}

#[tokio::test]
async fn question_says_password_for_may_prompt_outside() {
    let (mut h, _, turn_id) = start(reviewed(None), vec![networked(EVIDENCE)], "copy it").await;
    answer_and_end(&mut h, turn_id, ApprovalDecision::Allow).await;

    let Event::ApprovalRequested { interactive, exit: Some(exit), .. } = question(&h).await else {
        panic!("the question shows the exit");
    };
    assert_eq!(exit.launch, Launch::Unsandboxed);
    assert!(interactive, "the evidence line may ask for a password outside the sandbox");
    let ran = h.toolbox.ran();
    assert!(ran[0].may_wait);
    assert_eq!(ran[0].approved_by, Some(ApprovedBy::You));
}

#[tokio::test]
async fn question_has_no_password_for_contained_may_prompt() {
    let (mut h, _, turn_id) =
        start(reviewed(None), vec![networked("git fetch origin")], "fetch").await;
    answer_and_end(&mut h, turn_id, ApprovalDecision::Allow).await;

    let Event::ApprovalRequested { interactive, exit: Some(exit), .. } = question(&h).await else {
        panic!("the question shows the exit");
    };
    assert!(matches!(exit.launch, Launch::Contained { .. }), "{:?}", exit.launch);
    assert!(!interactive, "a contained command never gets a password");
    let ran = h.toolbox.ran();
    assert!(!ran[0].may_wait);
    assert_eq!(ran[0].approved_by, Some(ApprovedBy::You));
}

/// A `cautious` setup whose user config allows `git pull`.
fn git_pull_allowed() -> Setup {
    let setup = Setup::new();
    let pattern = CommandPattern::new("git").with_args(["pull"]);
    let rule = Rule::new(Action::Execute, Resource::Command(pattern), Effect::Allow);
    let rules = Policy::new(vec![rule]).expect("policy");
    let locations = Locations::new(setup.home()).expect("locations");
    let mut setup = setup;
    setup.engine = Some(Engine::with_rules(locations, rules));
    setup
}

#[tokio::test]
async fn cautious_allow_rule_for_git_pull_still_allows() {
    let input = ("shell", json!({ "command": "git pull", "may_prompt": true }));
    let (h, _) = run(git_pull_allowed(), vec![input], "pull").await;

    assert!(
        !h.events().await.iter().any(|e| matches!(e, Event::ApprovalRequested { .. })),
        "the soft flag adds no reason"
    );
    let ran = h.toolbox.ran();
    assert_eq!(ran[0].launch, Launch::Direct);
    assert!(ran[0].may_wait);
}

#[tokio::test]
async fn approved_by_names_each_path() {
    // A rule of the user's config, a built-in rule, and a person's answer.
    let pull = ("shell", json!({ "command": "git pull", "may_prompt": true }));
    let listing = ("shell", json!({ "command": "ls" }));
    let (h, _) = run(git_pull_allowed(), vec![pull, listing], "pull and look").await;
    let ran = h.toolbox.ran();
    assert_eq!(ran[0].approved_by, Some(ApprovedBy::YourRule));
    assert_eq!(ran[1].approved_by, None, "a built-in rule is no person's approval");
    assert!(!ran[1].may_wait, "a line with no flag keeps the model's timeout");

    let sudo = ("shell", json!({ "command": "sudo true" }));
    let (mut h, _, turn_id) = start(Setup::new(), vec![sudo], "update").await;
    answer_and_end(&mut h, turn_id, ApprovalDecision::Allow).await;
    assert_eq!(h.toolbox.ran()[0].approved_by, Some(ApprovedBy::You));
}
