use clap::error::ErrorKind;
use clap::{CommandFactory as _, Parser as _};
use pretty_assertions::assert_eq;

use super::{Cli, Command, LastCommand};
use crate::testing::{CONVERSATION, command, conversation};

fn parse_error(args: &[&str]) -> ErrorKind {
    let mut argv = vec!["efr"];
    argv.extend_from_slice(args);
    Cli::try_parse_from(argv).unwrap_err().kind()
}

#[test]
fn the_command_line_is_consistent() {
    Cli::command().debug_assert();
}

#[test]
fn send_takes_the_plugin_context_and_the_prompt_words() {
    let Command::Send(args) =
        command(&["send", "--context-json", r#"{"pwd":"/etc"}"#, "--", "why", "is", "it", "slow"])
    else {
        panic!("not send");
    };
    assert!(!args.steer);
    assert_eq!(args.context_json.as_deref(), Some(r#"{"pwd":"/etc"}"#));
    assert_eq!(args.prompt, ["why", "is", "it", "slow"]);
    assert_eq!(args.last_command, None);
    assert_eq!(args.conversation, None);
}

#[test]
fn send_takes_the_last_command_as_its_own_flag() {
    let Command::Send(args) =
        command(&["send", "--last-command", "make -j8 test", "--", "fix", "it"])
    else {
        panic!("not send");
    };
    assert_eq!(args.last_command.as_ref().map(LastCommand::as_str), Some("make -j8 test"));
    assert_eq!(args.prompt, ["fix", "it"]);
}

#[test]
fn prompt_words_after_the_separator_may_look_like_flags() {
    let Command::Send(args) = command(&["send", "--", "what", "does", "--steer", "do", "-x"])
    else {
        panic!("not send");
    };
    assert!(!args.steer);
    assert_eq!(args.prompt, ["what", "does", "--steer", "do", "-x"]);
}

#[test]
fn steer_is_a_flag_of_send() {
    let Command::Send(args) =
        command(&["send", "--steer", "--context-json", "{}", "--", "use", "the", "other", "file"])
    else {
        panic!("not send");
    };
    assert!(args.steer);
    assert_eq!(args.prompt, ["use", "the", "other", "file"]);
}

#[test]
fn steer_and_a_last_command_do_not_go_together() {
    assert_eq!(
        parse_error(&["send", "--steer", "--last-command", "ls", "--", "x"]),
        ErrorKind::ArgumentConflict
    );
}

#[test]
fn send_takes_a_conversation_id() {
    let Command::Send(args) = command(&["send", "--conversation", CONVERSATION, "--", "hi"]) else {
        panic!("not send");
    };
    assert_eq!(args.conversation, Some(conversation()));
}

#[test]
fn a_conversation_that_is_not_an_id_is_a_usage_error() {
    assert_eq!(
        parse_error(&["send", "--conversation", "yesterday", "--", "hi"]),
        ErrorKind::ValueValidation
    );
}

#[test]
fn new_takes_an_optional_prompt() {
    let Command::New(args) = command(&["new", "--context-json", "{}", "--"]) else {
        panic!("not new");
    };
    assert!(args.prompt.is_empty());
    let Command::New(args) = command(&["new", "--last-command", "cd /etc", "--", "start", "over"])
    else {
        panic!("not new");
    };
    assert_eq!(args.prompt, ["start", "over"]);
    assert_eq!(args.last_command.as_ref().map(LastCommand::as_str), Some("cd /etc"));
}

#[test]
fn an_unknown_or_missing_command_is_a_usage_error() {
    assert_eq!(parse_error(&["frobnicate"]), ErrorKind::InvalidSubcommand);
    assert_eq!(parse_error(&[]), ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand);
}

#[test]
fn help_is_a_snapshot() {
    insta::assert_snapshot!(Cli::command().render_help().to_string());
}

#[test]
fn send_help_is_a_snapshot() {
    let mut cli = Cli::command();
    cli.build();
    let send = cli.find_subcommand_mut("send").unwrap();
    insta::assert_snapshot!(send.render_help().to_string());
}

#[test]
fn the_last_command_never_shows_in_debug_output() {
    let line = command(&["send", "--last-command", "export TOKEN=s3cret", "--", "hi"]);
    let debug = format!("{line:?}");
    assert!(!debug.contains("s3cret"), "{debug}");
    assert!(debug.contains("LastCommand(<19 bytes>)"), "{debug}");
}
