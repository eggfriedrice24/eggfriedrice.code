use std::time::Duration;

use efr_protocol::{CommandId, ConversationId, PromptSend, ShellContext};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use crate::DaemonError;
use crate::conversations::ActiveTty;
use crate::methods::prompt_send::{Target, continues, target};

const TTY: &str = "/dev/pts/3";

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn prompt(tty: Option<&str>) -> PromptSend {
    let context = tty.map(|tty| {
        let mut context = ShellContext::new("/home/u");
        context.tty = Some(tty.to_owned());
        context
    });
    PromptSend {
        command_id: CommandId::from_uuid(uuid::Uuid::from_u128(99)),
        conversation_id: None,
        new_conversation: false,
        text: "what is using port 8080".to_owned(),
        context,
        last_command: None,
    }
}

fn none(_: &str) -> Option<ConversationId> {
    None
}

#[test]
fn a_named_conversation_takes_the_prompt() {
    let params = PromptSend { conversation_id: Some(conversation(7)), ..prompt(Some(TTY)) };

    let routed = target(&params, None, |_| Some(conversation(1))).unwrap();

    assert_eq!(routed, Target::Existing(conversation(7)));
}

#[test]
fn a_terminal_line_goes_to_the_terminals_active_conversation() {
    let routed = target(&prompt(Some(TTY)), None, |tty| {
        assert_eq!(tty, TTY);
        Some(conversation(1))
    })
    .unwrap();

    assert_eq!(routed, Target::Existing(conversation(1)));
}

#[test]
fn a_terminal_without_a_conversation_starts_one_there() {
    let routed = target(&prompt(Some(TTY)), None, none).unwrap();

    assert_eq!(routed, Target::New { tty: Some(TTY.to_owned()) });
}

#[test]
fn new_conversation_starts_one_even_when_the_terminal_has_one() {
    let params = PromptSend { new_conversation: true, ..prompt(Some(TTY)) };

    let routed = target(&params, None, |_| Some(conversation(1))).unwrap();

    assert_eq!(routed, Target::New { tty: Some(TTY.to_owned()) });
}

#[test]
fn without_a_context_tty_the_hello_tty_counts() {
    let routed = target(&prompt(None), Some("/dev/pts/9".to_owned()), |tty| {
        (tty == "/dev/pts/9").then(|| conversation(9))
    })
    .unwrap();

    assert_eq!(routed, Target::Existing(conversation(9)));
}

#[test]
fn a_prompt_from_no_terminal_starts_a_conversation_of_its_own() {
    assert_eq!(target(&prompt(None), None, none).unwrap(), Target::New { tty: None });
}

#[test]
fn an_empty_prompt_or_a_contradiction_is_refused() {
    let empty = PromptSend { text: "  \n".to_owned(), ..prompt(Some(TTY)) };
    let both = PromptSend {
        conversation_id: Some(conversation(1)),
        new_conversation: true,
        ..prompt(Some(TTY))
    };

    assert!(matches!(target(&empty, None, none), Err(DaemonError::InvalidParams { .. })));
    assert!(matches!(target(&both, None, none), Err(DaemonError::InvalidParams { .. })));
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

const NOW: &str = "2026-10-04T12:00:00Z";
const TWELVE_HOURS: Option<Duration> = Some(Duration::from_secs(12 * 3600));

fn taken_by(shell_pid: Option<u32>) -> ActiveTty {
    ActiveTty { conversation_id: conversation(1), shell_pid }
}

#[test]
fn the_shell_that_took_the_terminal_continues_its_conversation() {
    let recent = Some(at("2026-10-04T11:00:00Z"));
    let never = |_| panic!("the same shell needs no liveness check");

    assert!(continues(taken_by(Some(41)), Some(41), recent, at(NOW), TWELVE_HOURS, never));
    // A shell or a record that does not say its pid continues as before.
    assert!(continues(taken_by(None), Some(41), recent, at(NOW), TWELVE_HOURS, never));
    assert!(continues(taken_by(Some(41)), None, recent, at(NOW), TWELVE_HOURS, never));
}

#[test]
fn a_new_shell_on_a_reused_terminal_starts_over_once_the_old_shell_is_gone() {
    let recent = Some(at("2026-10-04T11:00:00Z"));

    let gone = continues(taken_by(Some(41)), Some(77), recent, at(NOW), TWELVE_HOURS, |pid| {
        assert_eq!(pid, 41);
        false
    });
    // A nested shell in the same terminal: the first shell still runs.
    let nested = continues(taken_by(Some(41)), Some(77), recent, at(NOW), TWELVE_HOURS, |_| true);

    assert!(!gone);
    assert!(nested);
}

#[test]
fn an_idle_terminal_conversation_ends_after_the_configured_hours() {
    let alive = |_| true;
    let old = Some(at("2026-10-03T23:59:59Z"));
    let young = Some(at("2026-10-04T00:00:01Z"));

    assert!(!continues(taken_by(Some(41)), Some(41), old, at(NOW), TWELVE_HOURS, alive));
    assert!(continues(taken_by(Some(41)), Some(41), young, at(NOW), TWELVE_HOURS, alive));
    // 0 hours in the config: never idle.
    assert!(continues(taken_by(Some(41)), Some(41), old, at(NOW), None, alive));
}
