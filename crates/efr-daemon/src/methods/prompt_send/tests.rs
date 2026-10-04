use efr_protocol::{CommandId, ConversationId, PromptSend, ShellContext};
use pretty_assertions::assert_eq;

use crate::DaemonError;
use crate::methods::prompt_send::{Target, target};

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
