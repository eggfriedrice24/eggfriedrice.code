//! Always-allow rules in a turn (phase E): a covered exit runs before the classifier
//! with no question, a question offers the rule that an `a` answer saves, and only the
//! user's own terminal may give that answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_permissions::{
    Action, AllowRule, CallFacts, Effect, Engine, Policy, Requirements, Resource, Rule, TargetKind,
};
use efr_protocol::{
    AlwaysSaved, ApprovalDecision, ApprovalRespond, CallId, Event, ExitKind, Grant, JudgeKind,
    Launch, Needs, NotSavedReason, Origin, PauseReason, Verdict,
};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::review::{judgements, reviewed, shell};
use crate::ConversationError;
use crate::testing::{FakeAllowWriter, FakeJudge, Harness, Script, Setup, allows, denies};

/// A rule that covers writes at or below `under`.
fn write(under: &str) -> AllowRule {
    AllowRule::Write { under: under.to_owned(), project: None }
}

/// The reviewed setup of the review tests (an `auto` turn in the project at its working
/// directory) with the always rules `allow`, the user's own `rules`, `judge` before the
/// user and `writer` for an `a` answer.
fn ruled(
    allow: Vec<AllowRule>,
    rules: Vec<Rule>,
    judge: Option<Arc<FakeJudge>>,
    writer: Option<Arc<FakeAllowWriter>>,
) -> Setup {
    let mut setup = reviewed(judge);
    let engine = setup.engine.take().unwrap();
    let engine = Engine::with_rules(engine.locations().clone(), Policy::new(rules).unwrap())
        .with_support(*engine.support())
        .with_allow(allow);
    setup.engine = Some(engine);
    setup.allow_writer = writer;
    setup
}

/// A copy to the new file `~/notes/<name>` in the existing dir `~/notes`, with the facts
/// that efrd gives: the kinds and the real paths. Its "yes" binds `~/notes`.
fn notes_copy(home: &Path, name: &str) -> (String, Requirements) {
    let notes = home.join("notes");
    let file = notes.join(name);
    let line = format!("cp a.txt {}", file.display());
    let facts = CallFacts {
        targets: vec![(file.clone(), None), (notes.clone(), Some(TargetKind::Dir))],
        real_paths: vec![(file.clone(), file.clone()), (notes.clone(), notes)],
        ..CallFacts::default()
    };
    let requirements =
        Requirements::none().with_command(line.clone()).with_write(file).with_facts(facts);
    (line, requirements)
}

/// Starts `setup` with a model that makes `calls`, with `declared` as the requirements
/// of each line, and sends `said`.
async fn start(
    setup: Setup,
    calls: Vec<(&str, Value)>,
    declared: Vec<(String, Requirements)>,
    said: &str,
) -> (Harness, efr_protocol::TurnId) {
    let script = Script::new(calls);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    for (line, requirements) in declared {
        h.toolbox.declared.lock().unwrap().insert(line, requirements);
    }
    let sent = h.prompt(said).await;
    (h, sent.turn_id)
}

/// The category and the rationale of every `exit_judged` of the judge `always`.
async fn always_judged(h: &Harness) -> Vec<(Option<String>, Option<String>)> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ExitJudged { judge: JudgeKind::Always, category, rationale, .. } => {
                Some((category, rationale))
            }
            _ => None,
        })
        .collect()
}

/// The outcome of every `always_rule_saved`.
async fn saved(h: &Harness) -> Vec<AlwaysSaved> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::AlwaysRuleSaved { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect()
}

/// The kinds of every `exit_requested`.
async fn requested(h: &Harness) -> Vec<Vec<ExitKind>> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ExitRequested { kinds, .. } => Some(kinds),
            _ => None,
        })
        .collect()
}

/// The question of the next `approval_requested`: its call and the rule it offers.
async fn question(h: &mut Harness) -> (CallId, Option<String>) {
    let asked = h.wait_for(|e| matches!(e, Event::ApprovalRequested { .. })).await;
    let Event::ApprovalRequested { call_id, exit: Some(exit), .. } = asked else {
        panic!("the question shows the exit: {asked:?}");
    };
    (call_id, exit.always)
}

/// Answers `call_id` with `decision` from `origin`.
async fn answer_from(
    h: &mut Harness,
    call_id: CallId,
    decision: ApprovalDecision,
    origin: Origin,
) -> Result<efr_protocol::ApprovalRespondResult, ConversationError> {
    let params = ApprovalRespond {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        call_id,
        decision,
    };
    h.handle.respond_approval(params, origin).await
}

#[tokio::test]
async fn always_allow_beats_classifier() {
    let judge = FakeJudge::answering(vec![denies()]);
    let setup = ruled(vec![write("~/notes")], Vec::new(), Some(Arc::clone(&judge)), None);
    let notes = setup.home().join("notes");
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let (mut h, turn_id) = start(setup, vec![shell(&line)], vec![(line, declared)], "copy").await;
    h.wait_end(turn_id).await;

    assert_eq!(judge.calls(), 0, "the classifier never sees a covered exit");
    assert_eq!(judgements(&h).await, vec![(JudgeKind::Always, Verdict::Allow)]);
    assert_eq!(
        always_judged(&h).await,
        vec![(Some("sandbox.allow[0]".to_owned()), Some("write under ~/notes".to_owned()))]
    );
    let kinds = h.kinds().await;
    assert!(kinds.contains(&"exit_requested".to_owned()), "{kinds:?}");
    assert!(!kinds.contains(&"approval_requested".to_owned()), "{kinds:?}");
    assert!(!kinds.contains(&"exit_review_started".to_owned()), "{kinds:?}");
    // The covered call runs with the widening of a "yes", and nobody follows it.
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].launch, Launch::Contained { grants: vec![Grant::Write { path: notes }] });
    assert!(!ran[0].approved_interactive);
    assert_eq!(ran[0].exits.len(), 1);
}

#[tokio::test]
async fn always_offer_absent_without_writer() {
    let setup = ruled(Vec::new(), Vec::new(), None, None);
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let (mut h, turn_id) = start(setup, vec![shell(&line)], vec![(line, declared)], "copy").await;
    let (call_id, offered) = question(&mut h).await;
    assert_eq!(offered, None, "no writer, no offer");

    // An `a` to a question that offers no rule is refused, and the question stays open.
    let refused = answer_from(&mut h, call_id, ApprovalDecision::AllowAlways, Origin::Shell).await;
    assert!(
        matches!(refused, Err(ConversationError::NoAlwaysOffer { call_id: id }) if id == call_id),
        "{refused:?}"
    );
    assert_eq!(h.handle.state().await.unwrap().pending_approvals, vec![call_id]);
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(turn_id).await;
    assert_eq!(h.toolbox.ran().len(), 1);
    assert_eq!(saved(&h).await, Vec::new());
}

#[tokio::test]
async fn always_answer_saves_the_offered_rule_and_runs_the_call() {
    let writer = FakeAllowWriter::saving();
    let setup = ruled(Vec::new(), Vec::new(), None, Some(Arc::clone(&writer)));
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let (mut h, turn_id) = start(setup, vec![shell(&line)], vec![(line, declared)], "copy").await;
    let (call_id, offered) = question(&mut h).await;
    let rule =
        AllowRule::Write { under: "~/notes".to_owned(), project: Some("~/project".to_owned()) };
    assert_eq!(offered, Some(rule.describe()));

    answer_from(&mut h, call_id, ApprovalDecision::AllowAlways, Origin::Shell).await.unwrap();
    h.wait_end(turn_id).await;
    assert_eq!(writer.rules(), vec![rule.clone()]);
    assert_eq!(
        saved(&h).await,
        vec![AlwaysSaved::Saved { index: 0, rule: rule.describe() }],
        "the turn records what the writer said"
    );
    assert_eq!(judgements(&h).await, vec![(JudgeKind::User, Verdict::Allow)]);
    // The answer is the user's; the rule is saved before the call runs, as a "yes".
    let kinds = h.kinds().await;
    let at = |kind: &str| kinds.iter().position(|known| known == kind).unwrap();
    assert!(at("approval_resolved") < at("always_rule_saved"));
    assert!(at("always_rule_saved") < at("tool_call_completed"));
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    assert!(ran[0].launch.uses_launcher());
}

#[tokio::test]
async fn always_rule_not_saved_still_runs_the_call() {
    let writer =
        FakeAllowWriter::answering(AlwaysSaved::NotSaved { reason: NotSavedReason::ReadOnly });
    let setup = ruled(Vec::new(), Vec::new(), None, Some(Arc::clone(&writer)));
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let (mut h, turn_id) = start(setup, vec![shell(&line)], vec![(line, declared)], "copy").await;
    let (call_id, offered) = question(&mut h).await;
    assert!(offered.is_some());
    answer_from(&mut h, call_id, ApprovalDecision::AllowAlways, Origin::Shell).await.unwrap();
    h.wait_end(turn_id).await;

    assert_eq!(writer.rules().len(), 1);
    assert_eq!(saved(&h).await, vec![AlwaysSaved::NotSaved { reason: NotSavedReason::ReadOnly }]);
    assert_eq!(h.toolbox.ran().len(), 1, "the call ran once as a yes");
}

#[tokio::test]
async fn always_allow_only_from_own_terminal() {
    let writer = FakeAllowWriter::saving();
    let setup = ruled(Vec::new(), Vec::new(), None, Some(Arc::clone(&writer)));
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let (mut h, turn_id) = start(setup, vec![shell(&line)], vec![(line, declared)], "copy").await;
    let (call_id, offered) = question(&mut h).await;
    assert!(offered.is_some());

    // A phone may answer this question, but never with `a`: the question stays open.
    let refused = answer_from(&mut h, call_id, ApprovalDecision::AllowAlways, Origin::Phone).await;
    assert!(
        matches!(
            refused,
            Err(ConversationError::RemoteAlwaysAnswer { origin: Origin::Phone, call_id: id })
                if id == call_id
        ),
        "{refused:?}"
    );
    assert_eq!(h.handle.state().await.unwrap().pending_approvals, vec![call_id], "still open");
    assert!(writer.rules().is_empty());
    assert!(!h.kinds().await.contains(&"approval_resolved".to_owned()));

    // The user's own terminal gives it.
    answer_from(&mut h, call_id, ApprovalDecision::AllowAlways, Origin::Cli).await.unwrap();
    h.wait_end(turn_id).await;
    assert_eq!(writer.rules().len(), 1);
}

#[tokio::test]
async fn deny_rule_beats_always_allow() {
    let under = |dir: &str, effect: Effect| {
        Rule::new(Action::Write, Resource::Under(PathBuf::from(dir)), effect)
    };
    // A user `deny` of a path below the bound dir that the line never names, and a user
    // `ask` of the dir: each beats the rule, so the call goes to the classifier or the
    // user, never to the rule.
    for rule in [under("~/notes/private", Effect::Deny), under("~/notes", Effect::Ask)] {
        let judge = FakeJudge::answering(vec![allows()]);
        let setup = ruled(vec![write("~/notes")], vec![rule], Some(Arc::clone(&judge)), None);
        let (line, declared) = notes_copy(setup.home(), "a.txt");
        let (mut h, turn_id) =
            start(setup, vec![shell(&line)], vec![(line, declared)], "copy").await;
        let ended = h
            .wait_for(|e| {
                matches!(e, Event::ApprovalRequested { .. })
                    || (e.turn_id() == Some(turn_id) && matches!(e, Event::TurnCompleted { .. }))
            })
            .await;
        if let Event::ApprovalRequested { call_id, .. } = ended {
            h.answer(call_id, ApprovalDecision::Deny).await;
            h.wait_end(turn_id).await;
        }
        assert_eq!(always_judged(&h).await, Vec::new(), "no rule covered the call");
    }
}

#[tokio::test]
async fn always_allow_ignored_for_phone_turn() {
    let setup = ruled(vec![write("~/notes")], Vec::new(), None, None);
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let script = Script::new(vec![shell(&line)]);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    h.toolbox.declared.lock().unwrap().insert(line, declared);
    let cwd = h.cwd.clone();
    let params = h.prompt_params(&cwd, "copy");
    let sent = h.handle.send_prompt(params, Origin::Phone).await.unwrap();
    // A phone turn runs as `cautious`: a write outside the project asks the user.
    let call_id = h.wait_approval().await;
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(sent.turn_id).await;
    assert_eq!(always_judged(&h).await, Vec::new());
    assert!(h.toolbox.ran().is_empty());
}

#[tokio::test]
async fn always_allow_never_unsandboxed() {
    let setup = ruled(vec![write("~/notes")], Vec::new(), None, Some(FakeAllowWriter::saving()));
    let notes = setup.home().join("notes");
    // An `outside` exit in the dir that the rule names: a question that offers no rule.
    let line = format!("make -C {}", notes.display());
    let input = json!({ "command": line, "needs": Needs { outside: true, ..Needs::default() } });
    let (mut h, turn_id) = start(setup, vec![("shell", input)], Vec::new(), "build it").await;
    let (call_id, offered) = question(&mut h).await;
    assert_eq!(offered, None, "no rule may cover an outside exit");
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(turn_id).await;
    assert_eq!(always_judged(&h).await, Vec::new());
    assert_eq!(requested(&h).await, vec![vec![ExitKind::Outside]]);
    assert!(h.toolbox.ran().is_empty());
}

#[tokio::test]
async fn always_allow_rejects_named_persistence_kind() {
    let allow = vec![write("~/.config/systemd/user"), write("~/notes")];
    let setup = ruled(allow, Vec::new(), None, Some(FakeAllowWriter::saving()));
    let line = "systemctl --user enable --now x.service";
    let (mut h, turn_id) =
        start(setup, vec![shell(line)], Vec::new(), "enable x.service for my user").await;
    let (call_id, offered) = question(&mut h).await;
    assert_eq!(offered, None, "no rule may cover a persistence exit");
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(turn_id).await;
    assert_eq!(always_judged(&h).await, Vec::new());
    assert_eq!(requested(&h).await, vec![vec![ExitKind::Persistence]]);
}

/// Runs a turn with two classifier denials, a covered copy, a third denial and a second
/// covered copy, and returns the harness, the judge's calls and each pause.
async fn denials_around_rules() -> (Harness, usize, Vec<(PauseReason, u8)>) {
    let judge = FakeJudge::answering(vec![denies(), denies(), denies()]);
    let setup = ruled(vec![write("~/notes")], Vec::new(), Some(Arc::clone(&judge)), None);
    let (a, a_declared) = notes_copy(setup.home(), "a.txt");
    let (b, b_declared) = notes_copy(setup.home(), "b.txt");
    let push = "git push origin main";
    let calls = vec![shell(push), shell(push), shell(&a), shell(push), shell(&b)];
    let declared = vec![(a, a_declared), (b, b_declared)];
    let said = "push this branch, then copy the notes";
    let (mut h, turn_id) = start(setup, calls, declared, said).await;
    h.wait_end(turn_id).await;
    let paused = h
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ClassifierPaused { reason, denials, .. } => Some((reason, denials)),
            _ => None,
        })
        .collect();
    (h, judge.calls(), paused)
}

#[tokio::test]
async fn always_counts_as_judged_but_keeps_denials_in_a_row() {
    // The covered copy between the second and the third denial resets nothing, so the
    // classifier stops after three denials in a row.
    let (h, judged, paused) = denials_around_rules().await;
    assert_eq!(judged, 3);
    assert_eq!(paused, vec![(PauseReason::InARow, 3)]);
    let rules = always_judged(&h).await;
    assert_eq!(rules.len(), 2);
}

#[tokio::test]
async fn always_runs_while_classifier_paused() {
    // The second covered copy comes after the pause and still runs by the rule.
    let (h, _, paused) = denials_around_rules().await;
    assert_eq!(paused.len(), 1);
    let kinds = h.kinds().await;
    let pause = kinds.iter().position(|kind| kind == "classifier_paused").unwrap();
    let last_completed = kinds.iter().rposition(|kind| kind == "tool_call_completed").unwrap();
    assert!(pause < last_completed);
    assert_eq!(always_judged(&h).await.len(), 2, "both covered calls ran by the rule");
    assert_eq!(h.toolbox.ran().len(), 2);
}

#[tokio::test]
async fn always_runs_while_away() {
    let setup = ruled(vec![write("~/notes")], Vec::new(), None, None);
    setup.presence.set_away(true);
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let (mut h, turn_id) = start(setup, vec![shell(&line)], vec![(line, declared)], "copy").await;
    h.wait_end(turn_id).await;
    assert_eq!(always_judged(&h).await.len(), 1);
    assert_eq!(h.toolbox.ran().len(), 1);
}

#[tokio::test]
async fn always_allow_under_dir_keeps_floor_binds() {
    // `~/.zshrc` is a link into `~/dotfiles`. A rule under `~/dotfiles` covers a new file
    // there, and the call runs with exactly the grant that a "yes" gives; the plan of
    // that grant keeps the floor read-only (`efr-sandbox`, the test of this name). The
    // engine never covers a dir bind that holds a floor (`efr-permissions`).
    let mut setup = ruled(vec![write("~/dotfiles")], Vec::new(), None, None);
    let home = setup.home().to_path_buf();
    let dotfiles = home.join("dotfiles");
    let engine = setup.engine.take().unwrap();
    let locations = engine
        .locations()
        .clone()
        .with_link_floor_root(dotfiles.join("zsh/.zshrc"), home.join(".zshrc"))
        .unwrap();
    let engine = Engine::with_rules(locations, Policy::empty())
        .with_support(*engine.support())
        .with_allow(vec![write("~/dotfiles")]);
    assert_eq!(engine.allow_rules()[0].1, None, "a rule above a floor may load");
    setup.engine = Some(engine);
    let file = dotfiles.join("notes.txt");
    let line = format!("touch {}", file.display());
    let facts = CallFacts {
        targets: vec![(file.clone(), None), (dotfiles.clone(), Some(TargetKind::Dir))],
        real_paths: vec![(file.clone(), file.clone()), (dotfiles.clone(), dotfiles.clone())],
        ..CallFacts::default()
    };
    let declared =
        Requirements::none().with_command(line.clone()).with_write(file.clone()).with_facts(facts);
    let (mut h, turn_id) =
        start(setup, vec![shell(&line)], vec![(line, declared)], "touch a note").await;
    h.wait_end(turn_id).await;
    assert_eq!(always_judged(&h).await.len(), 1);
    let ran = h.toolbox.ran();
    assert_eq!(ran.len(), 1);
    let Launch::Contained { grants } = &ran[0].launch else { panic!("{:?}", ran[0].launch) };
    assert_eq!(grants.len(), 1, "{grants:?}");
    let Grant::Write { path } = &grants[0] else { panic!("{grants:?}") };
    assert!(path.starts_with(&dotfiles), "{path:?}");
    assert!(!home.join(".zshrc").starts_with(path), "the grant never holds the floor's link");
}

#[tokio::test]
async fn always_applies_to_jobs() {
    let setup = ruled(vec![write("~/notes")], Vec::new(), None, None);
    let (line, declared) = notes_copy(setup.home(), "a.txt");
    let declared = declared.with_background();
    let input = json!({ "command": line, "background": true });
    let (mut h, turn_id) =
        start(setup, vec![("shell", input)], vec![(line, declared)], "copy").await;
    h.wait_end(turn_id).await;
    assert_eq!(always_judged(&h).await.len(), 1);
    assert_eq!(h.toolbox.ran().len(), 1);
}
