use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_permissions::{
    AllowFloor, AllowRule, AutoSupport, ConversationPolicy, DecisionInput, Effect, Engine,
    Locations, Requirements,
};
use efr_protocol::{
    AdminConfigReload, AdminConfigReloadResult, AdminStatus, AdminStatusResult, CacheMode,
    CommandId, ConfigFileError, Method, Mode, NetworkMode, Origin, PromptSend, PromptSendResult,
    RootSource, SandboxStatus, Scope, ShellContext, TurnSettings,
};
use efr_scope::Registry;
use efr_test_support::{TestClock, TestDirs};
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::{Outcome, Reloads, Request, follow_probes, notice, probes_again};
use crate::testing::{RawClient, Running, deps, logged, serve_with};
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
async fn a_reloaded_model_name_of_another_company_is_warned_about() {
    let warning = "model.name is a model of another company than model.provider";
    assert!(!logged().contains(warning));
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;

    std::fs::write(config_file(&dirs), "[model]\nname = \"gpt-5.4\"\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    assert!(!logged().contains(warning), "gpt-5.4 is a model of the running provider");

    std::fs::write(config_file(&dirs), "[model]\nname = \"claude-opus-5-5\"\n").unwrap();
    let result = reload(&daemon.socket).await;

    assert!(result.applied, "the file is valid; only its turns fail");
    let log = logged();
    let line = log.lines().find(|line| line.contains(warning)).unwrap_or_else(|| panic!("{log}"));
    assert!(line.contains("model=claude-opus-5-5"), "{line}");
    assert!(line.contains("provider=openai-subscription"), "{line}");
    assert!(line.contains("set [model] name to a model of openai-subscription"), "{line}");
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
async fn a_reload_of_undo_or_ask_destructive_sends_an_engine_with_the_new_support() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    assert!(daemon.engine.borrow().support().undo, "undo is on by default");

    std::fs::write(config_file(&dirs), "[undo]\nenabled = false\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let off = *daemon.engine.borrow().support();
    std::fs::write(config_file(&dirs), "[sandbox]\nask_destructive = \"always\"\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let always = *daemon.engine.borrow().support();

    assert!(!off.undo && off.floor_backup, "{off:?}");
    assert!(!always.undo && always.floor_backup, "{always:?}");
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
fn decide(engine: &Engine, dirs: &TestDirs, mode: Mode, requirements: Requirements) -> Effect {
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

    // NOTE: the test daemon finds no sandbox launcher, so `auto` runs as `cautious` and
    // the fallback keeps the mode that the reload set.
    let asked = reloaded.fallback.as_ref().map(|fallback| fallback.asked);
    assert_eq!(
        (reloaded.mode, asked, reloaded.overridden.mode),
        (Mode::Cautious, Some(Mode::Auto), false)
    );
    assert_eq!((own.mode, own.overridden.mode), (Mode::Manual, true));
    drop(terminal);
    daemon.stop().await;
}

/// A probe's status of a ready sandbox whose network view self-test gave `netview`.
fn probed(netview: bool) -> SandboxStatus {
    SandboxStatus {
        available: true,
        reason: None,
        fix: None,
        landlock_abi: Some(10),
        errata: Some(0xf),
        bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        bwrap_version: Some("0.13.0".to_owned()),
        cache_mode: CacheMode::Tmp,
        network_mode: NetworkMode::None,
        warnings: Vec::new(),
        netview,
        netview_off: false,
        proxy: false,
        bus_proxy: false,
        bus_proxy_off: None,
    }
}

#[tokio::test]
async fn reload_of_another_key_keeps_netview() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let deps = deps(&dirs, &clock).with_probe_override(probed(true));
    let daemon = serve_with(Settings::default(), deps).await;
    assert!(daemon.engine.borrow().support().netview, "the start follows the first probe");

    std::fs::write(config_file(&dirs), "[shell]\nidle_minutes = 5\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    assert!(daemon.engine.borrow().support().netview, "a reload keeps the probe's view");

    let rule = "[[permissions.rules]]\naction = \"execute\"\nresource = { command = { program = \"cargo\" } }\neffect = \"allow\"\n";
    std::fs::write(config_file(&dirs), rule).unwrap();
    let before = Arc::clone(&daemon.engine.borrow());
    assert!(reload(&daemon.socket).await.applied);
    assert!(!Arc::ptr_eq(&before, &daemon.engine.borrow()), "a new engine was sent");
    assert!(daemon.engine.borrow().support().netview, "the new engine keeps the view");

    let sandbox = status(&daemon.socket).await.sandbox.unwrap();
    assert!(sandbox.netview && !sandbox.netview_off);

    std::fs::write(config_file(&dirs), "[network]\nnetview = false\n").unwrap();
    assert!(reload(&daemon.socket).await.applied);
    assert!(!daemon.engine.borrow().support().netview, "the key turns it off");
    // `efr status` shows the key, not only the probe's result.
    let sandbox = status(&daemon.socket).await.sandbox.unwrap();
    assert!(sandbox.netview, "the probe's result stays");
    assert!(sandbox.netview_off, "the status says the key turned the view off");
    daemon.stop().await;
}

#[tokio::test]
async fn rebuild_request_never_reprobes() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let deps = deps(&dirs, &clock).with_probe_override(probed(true));
    let daemon = crate::start(Settings::default(), deps).await.unwrap();
    let (reloads, engine) = (daemon.reloads(), daemon.engine());
    let status = daemon.sandbox_status();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let served = tokio::spawn(daemon.serve(shutdown.clone()));
    assert!(engine.borrow().support().netview);

    // NOTE: a file that turns the view off; a rebuild must not read it.
    std::fs::write(config_file(&dirs), "[network]\nnetview = false\n").unwrap();
    assert!(!reloads.rebuild().await.unwrap(), "nothing changed, so nothing is sent");
    let before = Arc::clone(&engine.borrow());
    let without = (*before).clone();
    let support = AutoSupport { netview: false, ..*without.support() };
    engine.send_replace(Arc::new(without.with_support(support)));
    assert!(reloads.rebuild().await.unwrap(), "the engine follows the probe again");

    assert!(engine.borrow().support().netview, "the running settings, not the file");
    assert!(!status.has_changed().unwrap(), "no rebuild started a probe");
    shutdown.cancel();
    served.await.unwrap().unwrap();
}

#[tokio::test]
async fn status_watch_task_asks_a_rebuild_on_change() {
    let (reloads, mut requests) = Reloads::new();
    let (status, status_receiver) = watch::channel(probed(false));
    let (_settings, settings) = watch::channel(Arc::new(Settings::default()));
    let locations = Locations::new("/home/u").unwrap();
    let engine_of = |netview: bool| {
        let support = AutoSupport { netview, ..AutoSupport::default() };
        Arc::new(Engine::with_defaults(locations.clone()).with_support(support))
    };
    let (engine, engine_receiver) = watch::channel(engine_of(false));
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(follow_probes(
        reloads,
        status_receiver,
        settings,
        engine_receiver,
        stop.clone(),
    ));
    let mut rebuilds = 0;
    // The reload task's part: each rebuild sends the engine of the last status.
    let mut answer = async |rebuilds: &mut usize| {
        let Some(Request::Rebuild { reply }) = requests.recv().await else {
            panic!("a rebuild request")
        };
        *rebuilds += 1;
        engine.send_replace(engine_of(status.borrow().netview));
        reply.send(true).unwrap();
    };

    status.send_replace(probed(true));
    answer(&mut rebuilds).await;
    // The same view again, as a reprobe gives it: the engine already follows it.
    status.send_replace(SandboxStatus { warnings: vec!["slow".to_owned()], ..probed(true) });
    status.send_replace(probed(false));
    answer(&mut rebuilds).await;
    status.send_replace(probed(false));
    stop.cancel();
    task.await.unwrap();

    assert_eq!(rebuilds, 2);
    assert!(requests.try_recv().is_err(), "no request for a status that changed nothing");
}

fn write_rule(under: &str) -> AllowRule {
    AllowRule::Write { under: under.to_owned(), project: None }
}

/// A `[[sandbox.allow]]` table of a write rule under `under`.
fn allow_table(under: &str) -> String {
    format!("[[sandbox.allow]]\nkind = \"write\"\nunder = \"{under}\"\n")
}

#[tokio::test]
async fn allow_change_sends_the_engine_and_starts_no_probe() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    let before = Arc::clone(&daemon.engine.borrow());

    std::fs::write(config_file(&dirs), allow_table("~/notes")).unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let after = Arc::clone(&daemon.engine.borrow());
    assert!(!Arc::ptr_eq(&before, &after), "a new engine was sent for the rule alone");
    assert_eq!(after.allow_rules(), [(write_rule("~/notes"), None)]);

    // Only the rules changed: no probe. The other keys of `[sandbox]`, the projects and
    // what the sandbox supports still run one.
    let running = Settings::default();
    let mut ruled = running.clone();
    ruled.sandbox.allow = vec![write_rule("~/notes")];
    assert!(!probes_again(&running, &ruled, &before, Some(&after)));
    assert!(!probes_again(&running, &ruled, &before, None));
    let mut rebuildable = ruled.clone();
    rebuildable.sandbox.rebuildable.push("out".to_owned());
    assert!(probes_again(&ruled, &rebuildable, &after, None));
    let home = dirs.home();
    let project = "0192f0c1-7a00-7000-8000-000000000009".parse().unwrap();
    let moved = Engine::with_defaults(
        Locations::new(home).unwrap().with_project(project, home.join("p")).unwrap(),
    );
    assert!(probes_again(&running, &running, &before, Some(&moved)));
    let undo = (*before).clone();
    let undo = undo.with_support(AutoSupport { undo: !before.support().undo, ..*before.support() });
    assert!(probes_again(&running, &running, &before, Some(&undo)));
    daemon.stop().await;
}

#[tokio::test]
async fn bad_allow_rule_shape_refused_on_reload() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    let before = Arc::clone(&daemon.engine.borrow());

    // A relative dir is a shape error: the file is refused, and nothing changes.
    let text = format!("{}{}", allow_table("~/notes"), allow_table("notes"));
    std::fs::write(config_file(&dirs), text).unwrap();
    let result = reload(&daemon.socket).await;
    assert!(!result.applied, "{result:?}");
    let error = result.error.unwrap();
    assert!(error.message.contains("sandbox.allow[1]"), "{error:?}");
    assert!(Arc::ptr_eq(&before, &daemon.engine.borrow()), "the old engine stays");
    assert!(daemon.settings.borrow().sandbox.allow.is_empty());
    assert!(status(&daemon.socket).await.config.unwrap().reload_error.is_some());
    daemon.stop().await;
}

#[tokio::test]
async fn always_allow_rejects_floor_path() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    // efrd starts with a rule that names a floor: the rule is disabled, never fatal.
    let mut settings = Settings::default();
    settings.sandbox.allow = vec![write_rule("~/.zshrc"), write_rule("~/notes")];
    let daemon = serve_with(settings, deps(&dirs, &clock)).await;
    let engine = Arc::clone(&daemon.engine.borrow());
    let marks: Vec<bool> = engine.allow_rules().iter().map(|(_, off)| off.is_some()).collect();
    assert_eq!(marks, [true, false]);
    let sandbox = status(&daemon.socket).await.sandbox.unwrap();
    let disabled: Vec<&String> =
        sandbox.warnings.iter().filter(|warning| warning.contains("sandbox.allow")).collect();
    assert_eq!(disabled.len(), 1, "{:?}", sandbox.warnings);
    assert!(disabled[0].starts_with("sandbox.allow[0] is disabled: "), "{}", disabled[0]);

    // A reload that adds a floor rule applies, with the rule disabled.
    let text = format!("{}{}", allow_table("~/notes"), allow_table("~/.config/systemd/user"));
    std::fs::write(config_file(&dirs), text).unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let engine = Arc::clone(&daemon.engine.borrow());
    assert_eq!(
        engine.allow_rules()[1].1,
        Some(AllowFloor::UnderFloor { floor: dirs.home().join(".config/systemd") })
    );
    let sandbox = status(&daemon.socket).await.sandbox.unwrap();
    assert!(
        sandbox.warnings.iter().any(|warning| warning.starts_with("sandbox.allow[1] is disabled")),
        "{:?}",
        sandbox.warnings
    );
    daemon.stop().await;
}

#[tokio::test]
async fn stale_allow_rule_is_disabled_not_fatal() {
    let dirs = TestDirs::new().unwrap();
    let clock = TestClock::new();
    let daemon = start(&dirs, &clock).await;
    let text = format!("{}{}", allow_table("~/code"), allow_table("~/notes"));
    std::fs::write(config_file(&dirs), text).unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let marks = |engine: &Engine| -> Vec<Option<AllowFloor>> {
        engine.allow_rules().iter().map(|(_, off)| off.clone()).collect()
    };
    assert_eq!(marks(&daemon.engine.borrow()), [None, None]);

    // Projects appear below the rule's dir after it was saved, each a write root of
    // every call: the next reload disables the rule, sends the engine, and the other
    // rule still works.
    let home = dirs.home();
    let mut registry = Registry::empty();
    for (n, name) in [(1_u128, "site"), (2, "blog")] {
        std::fs::create_dir_all(home.join("code").join(name)).unwrap();
        let id = efr_protocol::ProjectId::from_uuid(uuid::Uuid::from_u128(n));
        registry.register(id, home.join("code").join(name), None).unwrap();
    }
    registry.save(&Registry::path_in(dirs.dirs().config())).unwrap();
    assert!(reload(&daemon.socket).await.applied);
    let stale = Some(AllowFloor::WriteRoot { root: home.join("code/site") });
    assert_eq!(marks(&daemon.engine.borrow()), [stale, None]);
    let sandbox = status(&daemon.socket).await.sandbox.unwrap();
    assert!(
        sandbox.warnings.iter().any(|warning| warning.starts_with("sandbox.allow[0] is disabled")),
        "{:?}",
        sandbox.warnings
    );
    daemon.stop().await;

    // efrd starts again with the disabled rule in the file.
    let daemon = start(&dirs, &clock).await;
    assert!(reload(&daemon.socket).await.applied);
    let stale = Some(AllowFloor::WriteRoot { root: home.join("code/site") });
    assert_eq!(marks(&daemon.engine.borrow()), [stale, None]);
    daemon.stop().await;
}
