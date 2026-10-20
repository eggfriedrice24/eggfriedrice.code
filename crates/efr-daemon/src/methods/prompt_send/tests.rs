use efr_protocol::{CommandId, ConversationId, PromptSend, ShellContext};
use pretty_assertions::assert_eq;

use crate::DaemonError;
use crate::methods::prompt_send::{prompt_tty, validate};

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
        recent_commands: Vec::new(),
        settings: efr_protocol::TurnSettings::default(),
    }
}

#[test]
fn the_context_tty_wins_and_without_one_the_hello_tty_counts() {
    assert_eq!(prompt_tty(&prompt(Some(TTY)), Some("/dev/pts/9".to_owned())).as_deref(), Some(TTY));
    assert_eq!(
        prompt_tty(&prompt(None), Some("/dev/pts/9".to_owned())).as_deref(),
        Some("/dev/pts/9")
    );
    assert_eq!(prompt_tty(&prompt(None), None), None);
}

#[test]
fn an_empty_prompt_or_a_contradiction_is_refused() {
    let empty = PromptSend { text: "  \n".to_owned(), ..prompt(Some(TTY)) };
    let both = PromptSend {
        conversation_id: Some(conversation(1)),
        new_conversation: true,
        ..prompt(Some(TTY))
    };

    assert!(matches!(validate(&empty), Err(DaemonError::InvalidParams { .. })));
    assert!(matches!(validate(&both), Err(DaemonError::InvalidParams { .. })));
    assert!(validate(&prompt(Some(TTY))).is_ok());
}
