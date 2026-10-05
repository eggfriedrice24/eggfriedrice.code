use clap::error::ErrorKind;
use clap::{CommandFactory as _, Parser as _};
use pretty_assertions::assert_eq;

use efr_protocol::Mode;

use super::{Cli, Command, ConfigCommand, LastCommand, LoginCommand, PathsArgs, ProjectCommand};
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
fn the_plugins_calls_need_no_arguments() {
    // The plugin hands everything over in the environment, so its calls are bare.
    let Command::Send(args) = command(&["send"]) else { panic!("not send") };
    assert_eq!((args.context_json, args.last_command, args.prompt.len()), (None, None, 0));
    let Command::Send(args) = command(&["send", "--steer"]) else { panic!("not send") };
    assert!(args.steer);
    let Command::New(args) = command(&["new"]) else { panic!("not new") };
    assert!(args.prompt.is_empty());
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
fn history_takes_a_conversation_a_limit_and_a_cursor() {
    let Command::History(args) = command(&["history"]) else { panic!("not history") };
    assert_eq!((args.conversation, args.limit, args.cursor), (None, None, None));
    let Command::History(args) =
        command(&["history", "019a9b1c", "--limit", "5", "--cursor", "c1"])
    else {
        panic!("not history");
    };
    assert_eq!(args.conversation.as_deref(), Some("019a9b1c"));
    assert_eq!(args.limit, Some(5));
    assert_eq!(args.cursor.as_deref(), Some("c1"));
}

#[test]
fn login_takes_a_known_provider() {
    assert!(matches!(command(&["login", "openai"]), Command::Login(LoginCommand::Openai)));
    assert_eq!(parse_error(&["login", "anthropic"]), ErrorKind::InvalidSubcommand);
}

#[test]
fn status_and_config_show_parse() {
    assert!(matches!(command(&["status"]), Command::Status));
    assert!(matches!(command(&["config", "show"]), Command::Config(ConfigCommand::Show)));
}

#[test]
fn the_config_commands_and_paths_parse() {
    assert!(matches!(
        command(&["config", "check"]),
        Command::Config(ConfigCommand::Check { path: None })
    ));
    assert!(matches!(
        command(&["config", "check", "/tmp/x.toml"]),
        Command::Config(ConfigCommand::Check { path: Some(_) })
    ));
    assert!(matches!(command(&["config", "edit"]), Command::Config(ConfigCommand::Edit)));
    assert!(matches!(command(&["config", "schema"]), Command::Config(ConfigCommand::Schema)));
    assert!(matches!(command(&["config", "reload"]), Command::Config(ConfigCommand::Reload)));
    match command(&["config", "set", "conversation.max_queued", "-1"]) {
        Command::Config(ConfigCommand::Set { key, value }) => {
            assert_eq!((key.as_str(), value.as_str()), ("conversation.max_queued", "-1"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        command(&["config", "unset", "model.name"]),
        Command::Config(ConfigCommand::Unset { .. })
    ));
    assert!(matches!(command(&["paths", "--json"]), Command::Paths(PathsArgs { json: true })));
    assert_eq!(parse_error(&["config", "set", "model.name"]), ErrorKind::MissingRequiredArgument);
}

#[test]
fn send_and_new_take_the_turn_settings() {
    let Command::Send(args) =
        command(&["send", "--mode", "auto", "--model", "gpt-5.4", "--effort", "high", "--", "x"])
    else {
        panic!("not send");
    };
    assert_eq!(args.settings.mode, Some(Mode::Auto));
    assert_eq!(args.settings.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(args.settings.effort.as_deref(), Some("high"));
    let Command::New(args) = command(&["new", "--mode", "manual", "--", "x"]) else {
        panic!("not new");
    };
    assert_eq!(args.settings.mode, Some(Mode::Manual));
}

#[test]
fn every_mode_parses_and_an_unknown_one_is_a_usage_error() {
    for mode in Mode::ALL {
        let Command::Settings(args) = command(&["settings", "--mode", mode.as_str()]) else {
            panic!("not settings");
        };
        assert_eq!(args.mode, Some(mode));
    }
    assert_eq!(parse_error(&["settings", "--mode", "fast"]), ErrorKind::InvalidValue);
    assert_eq!(parse_error(&["send", "--mode", "default", "--", "x"]), ErrorKind::InvalidValue);
}

#[test]
fn a_value_after_an_equals_sign_may_start_with_a_dash() {
    // The plugin checks a typed value as one `--model=<value>` word, so that a value
    // such as `--help` reaches the check instead of being read as a flag.
    let Command::Settings(args) = command(&["settings", "--model=--help", "--effort=-x"]) else {
        panic!("not settings");
    };
    assert_eq!(args.model.as_deref(), Some("--help"));
    assert_eq!(args.effort.as_deref(), Some("-x"));
}

#[test]
fn models_takes_names() {
    let Command::Models(args) = command(&["models"]) else { panic!("not models") };
    assert!(!args.names);
    let Command::Models(args) = command(&["models", "--names"]) else { panic!("not models") };
    assert!(args.names);
}

#[test]
fn settings_help_is_a_snapshot() {
    let mut cli = Cli::command();
    cli.build();
    let settings = cli.find_subcommand_mut("settings").unwrap();
    insta::assert_snapshot!(settings.render_help().to_string());
}

#[test]
fn project_add_help_is_a_snapshot() {
    let mut cli = Cli::command();
    cli.build();
    let project = cli.find_subcommand_mut("project").unwrap();
    project.build();
    let add = project.find_subcommand_mut("add").unwrap();
    insta::assert_snapshot!(add.render_help().to_string());
}

#[test]
fn project_paths_parse_and_remove_needs_one() {
    let parse = |args: &[&str]| {
        let mut argv = vec!["efr", "project"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).map(|cli| cli.command)
    };
    assert!(matches!(
        parse(&["add"]),
        Ok(Command::Project(ProjectCommand::Add { path: None, name: None }))
    ));
    assert!(matches!(
        parse(&["add", "../x", "--name", "x"]),
        Ok(Command::Project(ProjectCommand::Add { path: Some(_), name: Some(_) }))
    ));
    assert!(matches!(parse(&["list"]), Ok(Command::Project(ProjectCommand::List))));
    assert!(parse(&["remove"]).is_err());
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
