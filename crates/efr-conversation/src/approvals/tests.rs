use std::path::Path;

use efr_permissions::{
    ConversationPolicy, Decision, DecisionInput, Engine, Locations, Requirements,
};
use efr_protocol::{ApprovalDecision, CallId, Mode, Origin, Scope, TurnId};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use super::{Approvals, denial, summary};

fn call(seed: u64) -> CallId {
    CallId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
}

fn turn(seed: u64) -> TurnId {
    TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
}

fn decide(requirements: Requirements) -> Decision {
    let engine = Engine::with_defaults(Locations::new("/home/u").expect("home"));
    engine.decide(&DecisionInput {
        requirements,
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode: Mode::Cautious,
        conversation_policy: ConversationPolicy::new("/home/u/.local/share/efr/scratch/s"),
    })
}

#[tokio::test]
async fn an_answer_reaches_the_parked_call_once() {
    let approvals = Approvals::default();
    let receiver = approvals.park(turn(1), call(2));

    assert_eq!(approvals.waiting_turn(call(2)), Some(turn(1)));
    assert!(approvals.answer(call(2), ApprovalDecision::Allow));
    assert_eq!(receiver.await.expect("answered"), ApprovalDecision::Allow);
    assert!(!approvals.answer(call(2), ApprovalDecision::Deny), "a call is answered once");
    assert_eq!(approvals.waiting_turn(call(2)), None);
}

#[test]
fn an_answer_for_a_call_that_stopped_waiting_is_not_delivered() {
    let approvals = Approvals::default();
    drop(approvals.park(turn(1), call(2)));
    assert!(!approvals.answer(call(2), ApprovalDecision::Allow));
}

#[test]
fn a_withdrawn_call_takes_no_answer() {
    let approvals = Approvals::default();
    let _receiver = approvals.park(turn(1), call(2));
    assert!(approvals.withdraw(call(2)));
    assert!(!approvals.withdraw(call(2)));
    assert!(!approvals.answer(call(2), ApprovalDecision::Allow));
}

#[test]
fn withdrawing_a_turn_takes_out_only_its_calls() {
    let approvals = Approvals::default();
    let _a = approvals.park(turn(1), call(2));
    let _b = approvals.park(turn(1), call(3));
    let _c = approvals.park(turn(4), call(5));
    let mut expected = vec![call(2), call(3)];
    expected.sort_unstable();

    assert_eq!(approvals.withdraw_turn(turn(1)), expected);
    assert_eq!(approvals.parked(), vec![call(5)]);
}

#[test]
fn a_summary_names_the_tool_and_what_needs_approval() {
    let decision = decide(Requirements::none().with_write("/home/u/.zshrc"));
    assert_eq!(summary("write_file", &decision), "write_file: write /home/u/.zshrc (user config)");
    let command = decide(Requirements::none().with_command("rm -rf build"));
    assert_eq!(summary("shell", &command), "shell: run \"rm -rf build\"");
}

#[test]
fn a_summary_leads_with_the_command_line_when_only_a_path_needs_approval() {
    let decision =
        decide(Requirements::none().with_command("rg TOKEN ~").with_read_tree("/home/u"));
    assert_eq!(
        summary("shell", &decision),
        "shell: run \"rg TOKEN ~\"; read all under /home/u (user data)"
    );
}

#[test]
fn a_summary_names_the_parts_of_a_long_line_that_ask() {
    let line = "printf '== %s\\n' host; hostnamectl; uptime; systemctl --failed";
    let decision = decide(Requirements::none().with_command(line));
    assert_eq!(
        summary("shell", &decision),
        format!("shell: run {line:?}\nasks for: hostnamectl, systemctl --failed")
    );
}

#[test]
fn a_summary_names_no_part_for_a_line_of_one_command() {
    let decision = decide(Requirements::none().with_command("systemctl --failed"));
    assert_eq!(summary("shell", &decision), "shell: run \"systemctl --failed\"");
}

#[test]
fn a_summary_names_a_part_once() {
    let decision = decide(Requirements::none().with_command("hostnamectl; uptime; hostnamectl"));
    assert!(summary("shell", &decision).ends_with("\nasks for: hostnamectl"));
}

#[test]
fn a_named_part_leaves_out_what_may_be_a_secret() {
    for (line, named) in [
        (
            "uptime; curl -H 'Authorization: Bearer sk-live-1234' https://api.example.org",
            "curl -H ...",
        ),
        ("uptime; mysql --password=hunter2 db", "mysql --password=... db"),
        ("uptime; vault login s.0123456789abcdefghijklmnop", "vault login ..."),
        ("uptime; git push --force origin main", "git push --force origin ..."),
        ("uptime; ./deploy.sh 'a b'", "./deploy.sh ..."),
    ] {
        let decision = decide(Requirements::none().with_command(line));
        let text = summary("shell", &decision);
        assert!(text.ends_with(&format!("\nasks for: {named}")), "{line:?} gave {text:?}");
        let secrets = ["sk-live", "hunter2", "s.0123", "a b"];
        let named_part = text.rsplit("asks for: ").next().unwrap_or_default();
        assert!(!secrets.iter().any(|secret| named_part.contains(secret)), "{text:?}");
    }
}

#[test]
fn a_line_break_in_a_path_cannot_pass_for_the_named_parts() {
    let decision =
        decide(Requirements::none().with_write("/home/u/notes\nasks for: ls").with_write("/x\r"));
    let text = summary("write_file", &decision);
    assert!(!text.contains(['\n', '\r']), "{text:?}");
    assert!(text.contains("/home/u/notes\\nasks for: ls"), "{text:?}");
}

#[test]
fn a_denial_names_each_refused_path_with_its_class() {
    let decision = decide(Requirements::none().with_read(Path::new("/home/u/.ssh/id_ed25519")));
    let text = denial("read_file", &decision);
    assert!(
        text.starts_with(
            "Permission denied for the read_file call: read /home/u/.ssh/id_ed25519 (secrets): deny"
        ),
        "{text}"
    );
}
