use std::time::Duration;

use efr_config::Settings;
use efr_conversation::CallContext;
use efr_permissions::{Engine, Locations, Requirements};
use efr_protocol::{CallId, ConversationId, ExitKind, Launch, Mode, TurnId};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::{
    CACHE, CHANGED, DENIED, EXPIRED, INTERACTIVE, JOBS_OFF, Moves, NESTED, NOT_AS_JOB,
    PREVIEW_LIFE, RunningCall, check,
};

const HOME: &str = "/home/u";

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

fn engine() -> Engine {
    let locations =
        Locations::new(HOME).unwrap().with_sealed_root("/home/u/.local/share/efr/secrets").unwrap();
    Engine::with_defaults(locations)
}

/// A call of conversation `conversation` that runs `requirements` in `mode` with
/// `launch`.
fn call(conversation: u128, mode: Mode, launch: Launch, requirements: Requirements) -> RunningCall {
    let context = CallContext::new(
        ConversationId::from_uuid(id(conversation)),
        TurnId::from_uuid(id(10)),
        CallId::from_uuid(id(100)),
        "/home/u/p/app",
        "/home/u/.local/share/efr/scratch/x",
    )
    .with_mode(mode);
    let mut context = context;
    context.launch = launch;
    RunningCall {
        command: requirements.command.clone().unwrap_or_default(),
        context,
        requirements: Some(requirements),
        started: Timestamp::UNIX_EPOCH,
        contained: false,
        call_dir: None,
        holds_cache: false,
    }
}

fn line(command: &str) -> Requirements {
    Requirements::none().with_command(command).with_command_dir("/home/u/p/app")
}

#[test]
fn move_refused_when_jobs_off() {
    let mut settings = Settings::default();
    settings.jobs.enabled = false;
    let running = call(1, Mode::Cautious, Launch::Direct, line("sleep 600"));
    assert_eq!(check(&engine(), &running, &settings), Err(JOBS_OFF));
}

#[test]
fn move_refused_for_interactive_line() {
    let running = call(1, Mode::Cautious, Launch::Direct, line("ssh h uptime").with_interactive());
    assert_eq!(check(&engine(), &running, &Settings::default()), Err(INTERACTIVE));
}

#[test]
fn move_refused_for_nested_shell_and_repl() {
    // NOTE: efr-shell refuses a line that starts a shell or a REPL at the hand-over.
    let running = call(1, Mode::Cautious, Launch::Direct, line("bash").with_nested());
    assert_eq!(check(&engine(), &running, &Settings::default()), Err(NESTED));
}

#[test]
fn move_refused_with_layer_lock() {
    let mut running = call(1, Mode::Auto, Launch::contained(), line("cargo build"));
    running.contained = true;
    running.holds_cache = true;
    assert_eq!(check(&engine(), &running, &Settings::default()), Err(CACHE));
}

#[test]
fn move_refused_for_not_as_job() {
    // NOTE: the hard flag stops `sudo` before the decision; without it the job decision
    // still refuses the privilege exit, in every mode.
    let running = call(1, Mode::Auto, Launch::Unsandboxed, line("sudo pacman -Syu"));
    assert_eq!(check(&engine(), &running, &Settings::default()), Err(NOT_AS_JOB));
}

#[test]
fn move_refused_for_any_deny() {
    let requirements = line("cp a b").with_write("/home/u/.local/share/efr/secrets/token");
    let running = call(1, Mode::Cautious, Launch::Direct, requirements);
    assert_eq!(check(&engine(), &running, &Settings::default()), Err(DENIED));
}

#[test]
fn move_in_cautious_records_the_answer_and_exits() {
    let running = call(1, Mode::Cautious, Launch::Direct, line("sleep 600"));
    let found = check(&engine(), &running, &Settings::default()).unwrap();
    assert_eq!(found.launch, Launch::Unsandboxed, "a typed line counts as the exit child");
    assert_eq!(found.minutes, 60);
    let [exit] = found.exits.as_slice() else { panic!("{found:?}") };
    assert!(exit.user_only);
    assert!(
        exit.facts.iter().any(|fact| fact.contains("approves each background job")),
        "{exit:?}"
    );
}

#[test]
fn preview_lists_the_job_exits() {
    // NOTE: in `auto` an uncovered delete of a job is an exit of the job decision.
    let running = call(1, Mode::Auto, Launch::Unsandboxed, line("rm -rf build"));
    let found = check(&engine(), &running, &Settings::default()).unwrap();
    let kinds: Vec<ExitKind> = found.exits.iter().flat_map(|exit| exit.kinds.clone()).collect();
    assert!(kinds.contains(&ExitKind::Destructive), "{found:?}");
}

#[test]
fn a_contained_call_with_no_exit_moves_with_no_exit_listed() {
    let mut running = call(1, Mode::Auto, Launch::contained(), line("sleep 600"));
    running.contained = true;
    let found = check(&engine(), &running, &Settings::default()).unwrap();
    assert_eq!(found.exits, []);
    assert_eq!(found.launch, Launch::contained());
}

#[test]
fn confirm_with_a_changed_decision_is_refused() {
    let moves = Moves::default();
    let running = call(1, Mode::Cautious, Launch::Direct, line("sleep 600"));
    let found = check(&engine(), &running, &Settings::default()).unwrap();
    let call_id = running.context.call_id;
    let _registration = moves.register(running);
    let now = Timestamp::UNIX_EPOCH;

    let token = moves.preview(call_id, found.exits.clone(), now).unwrap();
    assert_eq!(moves.answered(call_id, token + 1, &found.exits, now), Err(EXPIRED));
    let token = moves.preview(call_id, found.exits.clone(), now).unwrap();
    assert_eq!(moves.answered(call_id, token, &[], now), Err(CHANGED));
    let token = moves.preview(call_id, found.exits.clone(), now).unwrap();
    let late = now.checked_add(PREVIEW_LIFE + Duration::from_secs(1)).unwrap();
    assert_eq!(moves.answered(call_id, token, &found.exits, late), Err(EXPIRED));
    let token = moves.preview(call_id, found.exits.clone(), now).unwrap();
    assert_eq!(moves.answered(call_id, token, &found.exits, now), Ok(()));
    assert_eq!(moves.answered(call_id, token, &found.exits, now), Err(EXPIRED), "used once");
}

#[test]
fn a_call_registers_only_while_it_runs_and_for_its_conversation() {
    let moves = Moves::default();
    let running = call(1, Mode::Cautious, Launch::Direct, line("sleep 600"));
    let (conversation, call_id) = (running.context.conversation_id, running.context.call_id);
    let registration = moves.register(running);
    assert!(moves.running(conversation, call_id).is_some());
    assert!(moves.running(ConversationId::from_uuid(id(2)), call_id).is_none());
    assert!(registration.finish(None).is_none(), "no move was under way");
    assert!(moves.running(conversation, call_id).is_none());
    let running = call(1, Mode::Cautious, Launch::Direct, line("sleep 600"));
    drop(moves.register(running));
    assert!(moves.running(conversation, call_id).is_none(), "a dropped call goes too");
    assert_eq!(moves.preview(call_id, Vec::new(), Timestamp::UNIX_EPOCH), None);
}

/// A call whose command was left running stays for a move of that command (OQS16):
/// one for each conversation, previewed and answered as a running call, taken once, and
/// put back when efr-shell did not hand it over.
#[test]
fn a_left_call_stays_for_a_move_of_its_command() {
    let moves = Moves::default();
    let running = call(1, Mode::Cautious, Launch::Direct, line("make all"));
    let (conversation, call_id) = (running.context.conversation_id, running.context.call_id);
    let parts = super::LeftParts { guard_roots: Vec::new(), write_roots: vec!["/home/u/p".into()] };
    assert!(moves.register(running).finish(Some(parts)).is_none(), "no move was under way");
    assert!(moves.running(conversation, call_id).is_none(), "it no longer runs as a call");
    assert!(moves.left_call(conversation, call_id).is_some());
    assert!(moves.left_call(ConversationId::from_uuid(id(2)), call_id).is_none());

    let now = Timestamp::UNIX_EPOCH;
    let token = moves.preview_left(call_id, Vec::new(), now).unwrap();
    assert_eq!(moves.answered_left(call_id, token + 1, &[], now), Err(EXPIRED));
    let token = moves.preview_left(call_id, Vec::new(), now).unwrap();
    assert_eq!(moves.answered_left(call_id, token, &[], now), Ok(()));

    let (taken, parts) = moves.take_left(call_id).unwrap();
    assert_eq!(parts.write_roots, [std::path::PathBuf::from("/home/u/p")]);
    assert!(moves.take_left(call_id).is_none(), "taken once");
    moves.restore_left(taken, parts);
    assert!(moves.left_call(conversation, call_id).is_some(), "put back after a refusal");

    // NOTE: the hidden shell takes no other call while one runs, so a later left call of
    // the conversation replaces it.
    let mut later = call(1, Mode::Cautious, Launch::Direct, line("make check"));
    later.context.call_id = CallId::from_uuid(id(101));
    moves.register(later).finish(Some(super::LeftParts::default()));
    assert!(moves.left_call(conversation, call_id).is_none());
    assert!(moves.left_call(conversation, CallId::from_uuid(id(101))).is_some());
    moves.forget_left(CallId::from_uuid(id(101)));
    assert!(moves.left_call(conversation, CallId::from_uuid(id(101))).is_none());
    // A call that ended keeps nothing.
    let running = call(3, Mode::Cautious, Launch::Direct, line("true"));
    moves.register(running).finish(None);
    assert!(moves.left_call(ConversationId::from_uuid(id(3)), call_id).is_none());
}
