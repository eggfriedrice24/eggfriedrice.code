use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_permissions::{ConversationPolicy, DecisionInput, Effect, Requirements};
use efr_protocol::{
    AdminConfigReload, AdminConfigReloadResult, AdminStatus, AdminStatusResult, CommandId,
    ConfigFileError, Method, Mode, Origin, PromptSend, PromptSendResult, RootSource, Scope,
    ShellContext, TurnSettings,
};
use efr_test_support::{TestClock, TestDirs};
use pretty_assertions::assert_eq;

use super::{Outcome, notice};
use crate::testing::{RawClient, Running, deps, serve_with};
use crate::{ScreenChoice, Settings};

const TTY: &str = "/dev/pts/9";

fn config_file(dirs: &TestDirs) -> PathBuf {
    dirs.dirs().config().join("config.toml")
}

async fn start(dirs: &TestDirs, clock: &TestClock) -> Running {
    serve_with(Settings::default(), deps(dirs, clock)).await
}

async fn reload(socket: &Path) -> AdminConfigReloadResult {
    let (mut client, _) = RawClient::hello(socket, None).await;
    client.call(Method::AdminConfigReload(AdminConfigReload {})).await.unwrap()
}

async fn status(socket: &Path) -> AdminStatusResult {
    let (mut client, _) = RawClient::hello(socket, None).await;
    client.call(Method::AdminStatus(AdminStatus {})).await.unwrap()
}

fn error(message: &str) -> ConfigFileError {
    ConfigFileError { message: message.to_owned(), line: Some(2), column: Some(1), key: None }
}

#[test]
fn a_new_error_is_told_once() {
    let none = Outcome::default();
    let broken = Outcome { error: Some(error("bad")), restart_needed: Vec::new() };
    let keyed = Outcome {
        error: Some(ConfigFileError { key: Some("shell.login".to_owned()), ..error("worse") }),
        restart_needed: Vec::new(),
    };

    assert_eq!(
        notice(&none, &broken).as_deref(),
        Some("efr: config.toml has an error; the old settings stay: bad (line 2)")
    );
    assert_eq!(notice(&broken, &broken), None);
    assert_eq!(
        notice(&broken, &keyed).as_deref(),
        Some("efr: config.toml has an error; the old settings stay: worse (shell.login, line 2)")
    );
    assert_eq!(notice(&broken, &none), None, "a fixed file needs no notice");
}

#[test]
fn new_keys_that_wait_for_a_restart_are_told_once() {
    let none = Outcome::default();
    let screen = Outcome { error: None, restart_needed: vec!["screen".to_owned()] };
    let both = Outcome {
        error: None,
        restart_needed: vec!["screen".to_owned(), "model.provider".to_owned()],
    };

    assert_eq!(notice(&none, &screen).as_deref(), Some("efr: restart efrd to apply: screen"));
    assert_eq!(notice(&screen, &screen), None);
    assert_eq!(
        notice(&screen, &both).as_deref(),
        Some("efr: restart efrd to apply: screen, model.provider")
    );
    assert_eq!(notice(&both, &none), None);
}

#[tokio::test]
async fn a_valid_file_applies_to_the_settings_and_the_engine_follows_the_rules() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    let engine_before = Arc::clone(&daemon.engine.borrow());

    std::fs::write(config_file(&dirs), "[shell]\nidle_minutes = 5\n[model]\nname = \"gpt-5.4\"\n")
        .unwrap();
    let result = reload(&daemon.socket).await;

    assert_eq!(
        result,
        AdminConfigReloadResult { applied: true, error: None, restart_needed: vec![] }
    );
    let settings = Arc::clone(&daemon.settings.borrow());
    assert_eq!(settings.shell.idle_minutes, 5);
    assert_eq!(settings.model.name.as_deref(), Some("gpt-5.4"));
    assert!(Arc::ptr_eq(&engine_before, &daemon.engine.borrow()), "no rule changed");

    let rule = "[[permissions.rules]]\naction = \"execute\"\nresource = { command = { program = \"cargo\" } }\neffect = \"allow\"\n";
    std::fs::write(config_file(&dirs), rule).unwrap();
    let result = reload(&daemon.socket).await;

    assert!(result.applied);
    assert!(!Arc::ptr_eq(&engine_before, &daemon.engine.borrow()), "a new engine was sent");
    assert_eq!(daemon.settings.borrow().permissions.rules.rules().len(), 1);
    assert_eq!(daemon.settings.borrow().shell.idle_minutes, 60, "a removed key is its default");
    daemon.stop().await;
}

#[tokio::test]
async fn a_rules_change_reaches_the_trusted_programs_and_a_shell_change_how_shells_start() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = crate::start(Settings::default(), deps(&dirs, &clock)).await.unwrap();
    let shells = daemon.shells();
    let socket = daemon.socket_path().to_path_buf();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let served = tokio::spawn(daemon.serve(shutdown.clone()));
    assert!(!format!("{shells:?}").contains("\"frobnicate\""), "{shells:?}");
    // The read-only commands of cautious are trusted from the start, in every mode.
    assert!(format!("{shells:?}").contains("\"cat\""), "{shells:?}");

    let file = "[shell]\nprogram = \"/usr/bin/zsh-test\"\nlogin = false\n[[permissions.rules]]\naction = \"execute\"\nresource = { command = { program = \"frobnicate\" } }\neffect = \"allow\"\n";
    std::fs::write(config_file(&dirs), file).unwrap();
    assert!(reload(&socket).await.applied);

    let shown = format!("{shells:?}");
    assert!(shown.contains("\"frobnicate\""), "{shown}");
    assert!(shown.contains("program: \"/usr/bin/zsh-test\""), "{shown}");
    assert!(shown.contains("login: false"), "{shown}");
    // The drain waits for the last handle of the shells.
    drop(shells);
    shutdown.cancel();
    served.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_broken_file_keeps_the_old_settings_and_the_status_says_why_until_it_is_fixed() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    std::fs::write(config_file(&dirs), "[shell]\nidle_minutes = 5\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let before = Arc::clone(&daemon.settings.borrow());

    std::fs::write(config_file(&dirs), "[shell]\nidle_minuets = 6\n").unwrap();
    let result = reload(&daemon.socket).await;

    assert!(!result.applied);
    let error = result.error.unwrap();
    assert_eq!((error.line, error.column), (Some(2), Some(1)));
    assert_eq!(error.key.as_deref(), Some("shell.idle_minuets"));
    assert!(error.message.contains("unknown field `idle_minuets`"), "{}", error.message);
    assert!(Arc::ptr_eq(&before, &daemon.settings.borrow()), "the old settings stay");
    let config = status(&daemon.socket).await.config.unwrap();
    assert_eq!(config.reload_error, Some(error));
    assert!(config.exists);

    std::fs::write(config_file(&dirs), "[shell]\nidle_minutes = 7\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    assert_eq!(status(&daemon.socket).await.config.unwrap().reload_error, None);
    assert_eq!(daemon.settings.borrow().shell.idle_minutes, 7);
    daemon.stop().await;
}

#[tokio::test]
async fn a_restart_key_keeps_its_value_and_is_listed_until_it_is_set_back() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;

    std::fs::write(config_file(&dirs), "screen = \"ghostty\"\n[shell]\nlogin = false\n").unwrap();
    let result = reload(&daemon.socket).await;

    assert!(result.applied);
    assert_eq!(result.restart_needed, ["screen"]);
    assert_eq!(daemon.settings.borrow().screen, ScreenChoice::Auto);
    assert!(!daemon.settings.borrow().shell.login, "the live key applies");
    assert_eq!(status(&daemon.socket).await.config.unwrap().restart_needed, ["screen"]);

    std::fs::write(config_file(&dirs), "[shell]\nlogin = false\n").unwrap();
    assert_eq!(reload(&daemon.socket).await.restart_needed, Vec::<String>::new());
    daemon.stop().await;
}

#[tokio::test]
async fn an_invalid_log_filter_is_refused_with_its_key() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;

    std::fs::write(config_file(&dirs), "log = \"info,efr_=[\"\n").unwrap();
    let result = reload(&daemon.socket).await;

    assert!(!result.applied);
    assert_eq!(result.error.unwrap().key.as_deref(), Some("log"));
    assert_eq!(daemon.settings.borrow().log, "info");
    daemon.stop().await;
}

#[tokio::test]
async fn a_removed_file_reloads_as_no_file() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    std::fs::write(config_file(&dirs), "[conversation]\nmax_queued = 3\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);

    std::fs::remove_file(config_file(&dirs)).unwrap();
    assert!(reload(&daemon.socket).await.applied);

    assert_eq!(daemon.settings.borrow().conversation.max_queued, 16);
    assert!(!status(&daemon.socket).await.config.unwrap().exists);
    daemon.stop().await;
}

#[tokio::test]
async fn a_terminal_with_a_recent_conversation_hears_of_a_broken_file() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    let (mut terminal, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
    let mut context = ShellContext::new(dirs.home().to_path_buf());
    context.tty = Some(TTY.to_owned());
    let _: PromptSendResult = terminal
        .call(Method::PromptSend(PromptSend {
            command_id: CommandId::from_uuid(uuid::Uuid::from_u128(1)),
            conversation_id: None,
            new_conversation: false,
            text: "hi".to_owned(),
            context: Some(context),
            last_command: None,
            settings: TurnSettings::default(),
        }))
        .await
        .unwrap();

    std::fs::write(config_file(&dirs), "[shell\n").unwrap();
    assert!(!reload(&daemon.socket).await.applied);

    let text = std::fs::read_to_string(dirs.dirs().runtime().join("notices/pts-9")).unwrap();
    assert!(
        text.lines().any(|line| line
            .starts_with("efr: config.toml has an error; the old settings stay: the config file")),
        "{text}"
    );
    let told = text.lines().filter(|line| line.contains("config.toml has an error")).count();
    assert!(!reload(&daemon.socket).await.applied);
    let again = std::fs::read_to_string(dirs.dirs().runtime().join("notices/pts-9")).unwrap();
    assert_eq!(
        again.lines().filter(|line| line.contains("config.toml has an error")).count(),
        told,
        "the same error is told once"
    );
    drop(terminal);
    daemon.stop().await;
}

#[tokio::test]
async fn the_status_reports_the_roots_and_a_symlinked_config_file() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let dotfiles = dirs.root().join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("efr.toml"), "").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("efr.toml"), config_file(&dirs)).unwrap();
    let daemon = start(&dirs, &clock).await;

    let status = status(&daemon.socket).await;

    let roots = status.roots.unwrap();
    assert_eq!(roots.data.path, dirs.dirs().data());
    assert_eq!(roots.runtime.path, dirs.dirs().runtime());
    assert_eq!(roots.config.source, RootSource::DirVariable);
    let config = status.config.unwrap();
    assert_eq!(config.path, config_file(&dirs));
    assert!(config.exists);
    assert_eq!(config.symlink_target, Some(dotfiles.join("efr.toml")));
    assert_eq!((config.reload_error, config.restart_needed), (None, vec![]));
    daemon.stop().await;
}

#[tokio::test]
async fn sighup_reloads() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = serve_with(Settings::default(), deps(&dirs, &clock).with_reload_on_hangup()).await;
    let mut settings = daemon.settings.subscribe();
    std::fs::write(config_file(&dirs), "[conversation]\ntty_idle_hours = 2\n").unwrap();

    nix::sys::signal::raise(nix::sys::signal::Signal::SIGHUP).unwrap();
    settings.changed().await.unwrap();

    assert_eq!(settings.borrow().conversation.tty_idle_hours, 2);
    daemon.stop().await;
}

/// What `engine` decides for `requirements` in `mode`, for a turn on this machine.
fn decide(
    engine: &efr_permissions::Engine,
    dirs: &TestDirs,
    mode: Mode,
    requirements: Requirements,
) -> Effect {
    let input = DecisionInput {
        requirements,
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode,
        conversation_policy: ConversationPolicy::new(dirs.home().join("scratch")),
    };
    engine.decide(&input).effect()
}

#[tokio::test]
async fn a_reload_reaches_the_rules_of_every_mode_at_the_next_tool_call() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    let rm = || Requirements::none().with_command("rm build.log");
    let effects = |daemon: &Running| {
        let engine = Arc::clone(&daemon.engine.borrow());
        Mode::ALL.map(|mode| decide(&engine, &dirs, mode, rm()))
    };
    // Manual asks for everything; cautious asks for a writer program; auto runs it in
    // its sandbox.
    assert_eq!(effects(&daemon), [Effect::Ask, Effect::Ask, Effect::Contain]);

    let rule = "[[permissions.rules]]\naction = \"execute\"\nresource = { command = { program = \"rm\" } }\neffect = \"deny\"\n";
    std::fs::write(config_file(&dirs), rule).unwrap();
    assert!(reload(&daemon.socket).await.applied);

    assert_eq!(effects(&daemon), [Effect::Deny; 3], "the user's rule follows every mode");
    daemon.stop().await;
}

#[tokio::test]
async fn a_retargeted_config_link_is_write_sealed_after_the_reload() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let dotfiles = dirs.home().join("dotfiles");
    let (old, new) = (dotfiles.join("a/config.toml"), dotfiles.join("b/config.toml"));
    for file in [&old, &new] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "[shell]\nidle_minutes = 5\n").unwrap();
    }
    std::fs::create_dir_all(dirs.dirs().config()).unwrap();
    std::os::unix::fs::symlink(&old, config_file(&dirs)).unwrap();
    let daemon = start(&dirs, &clock).await;
    let write = |daemon: &Running, path: &Path| {
        let engine = Arc::clone(&daemon.engine.borrow());
        decide(&engine, &dirs, Mode::Auto, Requirements::none().with_write(path))
    };
    assert_eq!(write(&daemon, &old), Effect::Deny);
    assert_ne!(write(&daemon, &new), Effect::Deny);
    let engine_before = Arc::clone(&daemon.engine.borrow());

    std::fs::remove_file(config_file(&dirs)).unwrap();
    std::os::unix::fs::symlink(&new, config_file(&dirs)).unwrap();
    assert!(reload(&daemon.socket).await.applied);

    assert!(!Arc::ptr_eq(&engine_before, &daemon.engine.borrow()), "a new engine was sent");
    assert_eq!(write(&daemon, &new), Effect::Deny, "the new target is sealed");
    assert_ne!(write(&daemon, &old), Effect::Deny, "the old target is a file like any other");
    daemon.stop().await;
}

#[tokio::test]
async fn a_prompt_gets_the_reloaded_mode_unless_it_names_its_own() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    let (mut terminal, _) = RawClient::hello(&daemon.socket, Some(TTY)).await;
    let mut context = ShellContext::new(dirs.home().to_path_buf());
    context.tty = Some(TTY.to_owned());
    let mut next = 0;
    let mut send = async |mode: Option<Mode>| -> PromptSendResult {
        next += 1;
        let settings = TurnSettings { mode, ..TurnSettings::default() };
        terminal
            .call(Method::PromptSend(PromptSend {
                command_id: CommandId::from_uuid(uuid::Uuid::from_u128(next)),
                conversation_id: None,
                new_conversation: false,
                text: "hi".to_owned(),
                context: Some(context.clone()),
                last_command: None,
                settings,
            }))
            .await
            .unwrap()
    };

    std::fs::write(config_file(&dirs), "[permissions]\nmode = \"auto\"\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let reloaded = send(None).await.settings.unwrap();
    let own = send(Some(Mode::Manual)).await.settings.unwrap();

    // NOTE: efrd does not run the sandbox yet, so `auto` runs as `cautious` and the
    // fallback keeps the mode that the reload set.
    let asked = reloaded.fallback.as_ref().map(|fallback| fallback.asked);
    assert_eq!(
        (reloaded.mode, asked, reloaded.overridden.mode),
        (Mode::Cautious, Some(Mode::Auto), false)
    );
    assert_eq!((own.mode, own.overridden.mode), (Mode::Manual, true));
    drop(terminal);
    daemon.stop().await;
}
