use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use efr_config::Settings;
use efr_conversation::{CallContext, ToolCall};
use efr_permissions::{
    Cause, ConversationPolicy, DecisionInput, Effect, Engine, Requirements, SettingsChange,
};
use efr_protocol::{AdminConfigReloadResult, CallId, ConversationId, Mode, Origin, Scope, TurnId};
use efr_scope::{Home, Registry};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::watch;

use super::{CHANGED, SettingsTool};
use crate::engine::{build, protected_config};
use crate::reload::Reloads;

const COMMENTED: &str = "\
# My efr settings, kept in dotfiles.
[model]
# Pinned while the new one is tested.
name = \"gpt-5.5\" # the old default

[permissions]
mode = \"cautious\"

# Never touch the downloads.
[[permissions.rules]]
action = \"write\"
resource = { under = \"~/Downloads\" }
effect = \"deny\"

[shell]
idle_minutes = 30
";

/// A settings tool over a temporary home, with a real engine and a reload task that
/// answers every request and counts them.
struct Fixture {
    home: TempDir,
    tool: SettingsTool,
    settings: watch::Sender<Arc<Settings>>,
    reloads: Arc<Mutex<Vec<&'static str>>>,
}

impl Fixture {
    fn new() -> Self {
        Fixture::with(Settings::default())
    }

    fn with(settings: Settings) -> Self {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".config").join("efr");
        std::fs::create_dir_all(&config).unwrap();
        let engine = engine_for(home.path(), &config, &settings);
        let (settings, receiver) = watch::channel(Arc::new(settings));
        let (_engine_sender, engine) = watch::channel(Arc::new(engine));
        let (reloads, mut requests) = Reloads::new();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let counted = Arc::clone(&asked);
        tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                counted.lock().unwrap().push("reload");
                request.answer(AdminConfigReloadResult {
                    applied: true,
                    error: None,
                    restart_needed: Vec::new(),
                });
            }
        });
        let tool = SettingsTool::new(&config, receiver, engine, reloads);
        Fixture { home, tool, settings, reloads: asked }
    }

    fn config(&self) -> PathBuf {
        self.home.path().join(".config").join("efr").join("config.toml")
    }

    fn write(&self, text: &str) {
        std::fs::write(self.config(), text).unwrap();
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap()
    }
}

fn engine_for(home: &Path, config: &Path, settings: &Settings) -> Engine {
    let home = Home::new(home).unwrap();
    let secrets = home.path().join(".local/share/efr/secrets");
    build(&home, &secrets, &protected_config(config), settings, &Registry::empty()).unwrap()
}

fn call(n: u128, input: Value, origin: Origin) -> ToolCall {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let context = CallContext::new(
        ConversationId::from_uuid(id(1)),
        TurnId::from_uuid(id(2)),
        CallId::from_uuid(id(n)),
        "/tmp",
        "/tmp/scratch",
    )
    .with_origin(origin);
    ToolCall::new("settings", input, context)
}

fn local(n: u128, input: Value) -> ToolCall {
    call(n, input, Origin::Shell)
}

fn change(summary: &str, loosens: bool) -> Requirements {
    Requirements::none().with_settings_change(SettingsChange::new(summary, loosens))
}

/// Plans, shows and runs `input` as the turn would after the user approved it: the
/// diff the user saw and the outcome.
async fn approve(
    fixture: &Fixture,
    n: u128,
    input: Value,
) -> (String, efr_conversation::ToolOutcome) {
    let call = local(n, input);
    fixture.tool.requirements(&call).await.unwrap();
    let diff = fixture.tool.preview(&call).await.unwrap();
    let outcome = fixture.tool.invoke(call).await;
    (diff, outcome)
}

#[test]
fn the_description_says_the_tool_never_switches_the_current_settings() {
    let description = SettingsTool::definition().description;
    assert!(description.contains("tell the user to type ,model <id>"), "{description}");
    assert!(
        description.contains("never say that it switched the current mode, model or effort"),
        "{description}"
    );
}

#[tokio::test]
async fn read_needs_no_approval_and_lists_settings_rules_and_models() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let mut running = Settings::parse(&fixture.config(), Some(COMMENTED)).unwrap();
    running.set_source("model.effort", efr_config::Source::Default);
    fixture.settings.send_replace(Arc::new(running));
    let read = local(3, json!({"operation": "read"}));

    let requirements = fixture.tool.requirements(&read).await;
    let preview = fixture.tool.preview(&read).await;
    let outcome = fixture.tool.invoke(read).await;

    assert_eq!(requirements, Ok(Requirements::none()), "nothing to judge, so nothing asks");
    assert_eq!(preview, None);
    assert!(!outcome.is_error, "{}", outcome.output);
    let text = outcome.output;
    assert!(text.contains("(a file)"), "{text}");
    assert!(text.contains("The last reload succeeded."), "{text}");
    assert!(text.contains("model.name = \"gpt-5.5\"  # file, live"), "{text}");
    assert!(text.contains("screen = \"auto\"  # default, restart"), "{text}");
    assert!(text.contains("render.theme = (unset)  # default, client"), "{text}");
    assert!(
        text.contains(
            "permissions.rules[0] = { action = \"write\", resource = { under = \"~/Downloads\" }, effect = \"deny\" }"
        ),
        "{text}"
    );
    assert!(text.contains("gpt-5.5 (the default): efforts low, medium, high, xhigh"), "{text}");
    assert!(text.contains(",model"), "{text}");
    assert!(fixture.reloads.lock().unwrap().is_empty(), "a read reloads nothing");
}

#[tokio::test]
async fn a_change_asks_with_a_diff_that_keeps_the_comments_and_applies_after_a_reload() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let set = json!({"operation": "set", "key": "model.name", "value": "gpt-6-sol"});

    let requirements = fixture.tool.requirements(&local(3, set.clone())).await;
    let (diff, outcome) = approve(&fixture, 4, set).await;

    assert_eq!(requirements, Ok(change("set model.name = \"gpt-6-sol\"", false)));
    let target = std::fs::canonicalize(fixture.config()).unwrap();
    assert_eq!(
        diff,
        format!(
            "--- a{0}\n+++ b{0}\n@@ -1,7 +1,7 @@\n # My efr settings, kept in dotfiles.\n \
             [model]\n # Pinned while the new one is tested.\n-name = \"gpt-5.5\" # the old \
             default\n+name = \"gpt-6-sol\" # the old default\n \n [permissions]\n mode = \
             \"cautious\"\n",
            target.display()
        )
    );
    assert!(!outcome.is_error, "{}", outcome.output);
    assert_eq!(
        fixture.text(),
        COMMENTED.replace("name = \"gpt-5.5\" # the old", "name = \"gpt-6-sol\" # the old")
    );
    assert!(outcome.output.contains("set model.name = \"gpt-6-sol\""), "{}", outcome.output);
    assert!(outcome.output.contains("from the next turn"), "{}", outcome.output);
    assert!(outcome.output.contains("No restart is needed"), "{}", outcome.output);
    assert_eq!(fixture.reloads.lock().unwrap().len(), 1, "the daemon reloads at once");
}

#[tokio::test]
async fn a_restart_key_says_so_and_a_render_key_belongs_to_efr() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);

    let (_, screen) =
        approve(&fixture, 3, json!({"operation": "set", "key": "screen", "value": "vt100"})).await;
    let (_, theme) = approve(
        &fixture,
        4,
        json!({"operation": "set", "key": "render.theme", "value": "catppuccin-mocha"}),
    )
    .await;

    assert!(screen.output.contains("only after efrd restarts"), "{}", screen.output);
    assert!(theme.output.contains("efr reads it at its next run"), "{}", theme.output);
}

#[tokio::test]
async fn an_invalid_change_goes_back_to_the_model_and_asks_nothing() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let cases = [
        (json!({"operation": "set", "key": "conversation.max_queued", "value": 0}), "max_queued"),
        (json!({"operation": "set", "key": "shell.idle_minutes", "value": "soon"}), "whole number"),
        (json!({"operation": "set", "key": "permissions.mode", "value": "yolo"}), "manual"),
        (json!({"operation": "set", "key": "shell.colour", "value": "red"}), "shell.colour"),
        (json!({"operation": "set", "key": "permissions.rules", "value": "x"}), "add_rule"),
        (json!({"operation": "set", "key": "model.name", "value": "gpt-9"}), "openai.models"),
        (json!({"operation": "set", "key": "model.effort", "value": "ultra"}), "xhigh"),
        (json!({"operation": "set", "key": "model.name", "value": "gpt-5.5"}), "nothing changes"),
        (json!({"operation": "set", "key": "model.name"}), "set needs value"),
        (json!({"operation": "unset", "key": "model.effort"}), "default applies already"),
        (json!({"operation": "remove_rule", "index": 7}), "has 1 rules"),
        (json!({"operation": "add_rule", "rule": {"action": "write"}}), "not a rule"),
        (
            json!({"operation": "add_rule", "rule": {"action": "write", "resource": {"under": "src"}, "effect": "allow"}}),
            "would not be valid",
        ),
        (json!({"operation": "rewrite"}), "schema"),
    ];

    for (n, (input, expected)) in (10..).zip(cases) {
        let call = local(n, input.clone());
        let refused = fixture.tool.requirements(&call).await.unwrap_err();
        assert!(refused.contains(expected), "{input}: {refused}");
        assert_eq!(fixture.tool.preview(&call).await, None, "{input}");
    }
    assert_eq!(fixture.text(), COMMENTED);
}

#[tokio::test]
async fn a_new_model_id_becomes_valid_through_openai_models() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);

    let (_, added) = approve(
        &fixture,
        3,
        json!({"operation": "set", "key": "openai.models", "value": ["gpt-9"]}),
    )
    .await;
    let (_, named) =
        approve(&fixture, 4, json!({"operation": "set", "key": "model.name", "value": "gpt-9"}))
            .await;

    assert!(!added.is_error, "{}", added.output);
    assert!(!named.is_error, "{}", named.output);
    assert!(fixture.text().contains("models = [\"gpt-9\"]"), "{}", fixture.text());
}

#[tokio::test]
async fn a_rule_that_names_secrets_is_refused() {
    let mut settings = Settings::default();
    settings.permissions.secret_paths = vec![PathBuf::from("~/.config/rclone/rclone.conf")];
    let fixture = Fixture::with(settings);
    fixture.write(COMMENTED);
    let rules = [
        json!({"action": "read", "resource": {"class": "secrets"}, "effect": "allow"}),
        json!({"action": "read", "resource": {"under": "~/.ssh"}, "effect": "allow"}),
        json!({"action": "any", "resource": {"under": "~/.ssh/id_ed25519"}, "effect": "deny"}),
        json!({"action": "read", "resource": {"under": "~/.config/rclone/rclone.conf"}, "effect": "allow"}),
    ];

    for (n, rule) in (10..).zip(rules) {
        let refused = fixture
            .tool
            .requirements(&local(n, json!({"operation": "add_rule", "rule": rule})))
            .await
            .unwrap_err();
        assert!(refused.contains("names secrets"), "{rule}: {refused}");
    }
    assert_eq!(fixture.text(), COMMENTED);
}

#[tokio::test]
async fn a_secrets_rule_of_the_file_is_not_removed_by_the_tool() {
    let fixture = Fixture::new();
    let with_secret = format!(
        "{COMMENTED}\n[[permissions.rules]]\naction = \"read\"\nresource = {{ under = \
         \"~/.ssh/config\" }}\neffect = \"allow\"\n"
    );
    fixture.write(&with_secret);

    let refused = fixture
        .tool
        .requirements(&local(3, json!({"operation": "remove_rule", "index": 1})))
        .await
        .unwrap_err();

    assert!(refused.contains("names secrets"), "{refused}");
    assert_eq!(fixture.text(), with_secret);
}

#[tokio::test]
async fn a_change_that_loosens_permissions_is_marked() {
    // The file starts in manual mode, with per_call, a secret path and a deny rule.
    let manual = COMMENTED
        .replace("mode = \"cautious\"", "mode = \"manual\"")
        .replace("idle_minutes = 30\n", "idle_minutes = 30\nsudo_cache = \"per_call\"\n")
        .replace(
            "[permissions]\n",
            "[permissions]\nsecret_paths = [\"~/.config/rclone/rclone.conf\"]\n",
        );
    let auto = manual.replace("mode = \"manual\"", "mode = \"auto\"");
    let cases = [
        (
            &manual,
            json!({"operation": "add_rule", "rule": {"action": "execute", "resource": {"command": {"program": "cargo", "args": ["test"]}}, "effect": "allow"}}),
            true,
        ),
        (
            &manual,
            json!({"operation": "add_rule", "rule": {"action": "write", "resource": {"under": "~/Music"}, "effect": "deny"}}),
            false,
        ),
        (&manual, json!({"operation": "remove_rule", "index": 0}), true),
        (&manual, json!({"operation": "set", "key": "permissions.mode", "value": "auto"}), true),
        (&manual, json!({"operation": "unset", "key": "permissions.mode"}), true),
        (&auto, json!({"operation": "set", "key": "permissions.mode", "value": "manual"}), false),
        (&manual, json!({"operation": "set", "key": "shell.sudo_cache", "value": "keep"}), true),
        (
            &manual,
            json!({"operation": "set", "key": "permissions.secret_paths", "value": []}),
            true,
        ),
        (&manual, json!({"operation": "set", "key": "model.effort", "value": "high"}), false),
    ];

    for (n, (file, input, loosens)) in (10..).zip(cases) {
        let fixture = Fixture::new();
        fixture.write(file);
        let call = local(n, input.clone());

        let requirements = fixture.tool.requirements(&call).await.unwrap();
        let preview = fixture.tool.preview(&call).await.unwrap();

        let marked = requirements.settings.as_ref().map(|change| change.loosens);
        assert_eq!(marked, Some(loosens), "{input}");
        let flagged = preview.starts_with("This change loosens permissions.\n");
        assert_eq!(flagged, loosens, "{input}: {preview}");
    }
}

#[tokio::test]
async fn an_edit_after_the_user_saw_the_change_writes_nothing_and_the_next_call_asks_again() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let set = json!({"operation": "set", "key": "model.name", "value": "gpt-6-sol"});
    let first = local(3, set.clone());
    fixture.tool.requirements(&first).await.unwrap();
    let seen = fixture.tool.preview(&first).await.unwrap();

    let theirs = COMMENTED.replace("while the new one is tested", "until friday");
    fixture.write(&theirs);
    let outcome = fixture.tool.invoke(first).await;

    assert!(outcome.is_error);
    assert_eq!(outcome.output, CHANGED);
    assert_eq!(fixture.text(), theirs, "their edit stays and nothing else is written");
    assert!(fixture.reloads.lock().unwrap().is_empty());

    let (shown_again, written) = approve(&fixture, 4, set).await;
    assert_ne!(shown_again, seen, "the new plan shows the file as it is now");
    assert!(!written.is_error, "{}", written.output);
    assert_eq!(
        fixture.text(),
        theirs.replace("name = \"gpt-5.5\" # the old", "name = \"gpt-6-sol\" # the old")
    );
}

#[tokio::test]
async fn a_change_the_user_was_not_shown_is_not_written() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let call = local(3, json!({"operation": "set", "key": "model.name", "value": "gpt-6-sol"}));
    fixture.tool.requirements(&call).await.unwrap();

    let outcome = fixture.tool.invoke(call).await;

    assert!(outcome.is_error);
    assert!(outcome.output.contains("not shown"), "{}", outcome.output);
    assert_eq!(fixture.text(), COMMENTED);
}

#[tokio::test]
async fn a_missing_file_is_created_from_the_example_and_the_diff_shows_only_the_change() {
    let fixture = Fixture::new();

    let (diff, outcome) = approve(
        &fixture,
        3,
        json!({"operation": "set", "key": "permissions.mode", "value": "auto"}),
    )
    .await;

    assert!(diff.starts_with("This change loosens permissions.\n"), "{diff}");
    assert!(diff.contains("does not exist yet; it is created from the example"), "{diff}");
    assert!(diff.contains("+mode = \"auto\""), "{diff}");
    assert!(diff.lines().count() < 20, "{diff}");
    assert!(!outcome.is_error, "{}", outcome.output);
    let text = fixture.text();
    assert!(text.starts_with("#:schema "), "{text}");
    assert!(text.contains("[permissions]\nmode = \"auto\"\n"), "{text}");
}

#[tokio::test]
async fn the_file_behind_a_link_is_written_and_the_link_stays() {
    let fixture = Fixture::new();
    let dotfiles = fixture.home.path().join("dotfiles").join("efr");
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("config.toml"), COMMENTED).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("config.toml"), fixture.config()).unwrap();

    let (diff, outcome) =
        approve(&fixture, 3, json!({"operation": "set", "key": "model.effort", "value": "high"}))
            .await;

    assert!(!outcome.is_error, "{}", outcome.output);
    let target = std::fs::canonicalize(dotfiles.join("config.toml")).unwrap();
    assert!(diff.starts_with(&format!("--- a{}", target.display())), "{diff}");
    assert!(std::fs::symlink_metadata(fixture.config()).unwrap().is_symlink());
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("effort = \"high\""), "{text}");
    assert!(text.contains("# Pinned while the new one is tested."), "{text}");
}

#[tokio::test]
async fn a_rule_is_added_after_the_rules_of_the_file_and_removed_by_its_number() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let rule = json!({"action": "execute", "resource": {"command": {"program": "cargo", "args": ["test"], "under": "project"}}, "effect": "allow"});

    let added = local(3, json!({"operation": "add_rule", "rule": rule}));
    let requirements = fixture.tool.requirements(&added).await.unwrap();
    let (_, outcome) = approve(&fixture, 4, json!({"operation": "add_rule", "rule": rule})).await;

    assert_eq!(
        requirements,
        change(
            "add rule permissions.rules[1] = { action = \"execute\", resource = { command = { \
             program = \"cargo\", args = [\"test\"], under = \"project\" } }, effect = \"allow\" }",
            true
        )
    );
    assert!(outcome.output.contains("from the next tool call"), "{}", outcome.output);
    assert!(fixture.text().contains("[[permissions.rules]]\naction = \"execute\""));

    let (_, removed) = approve(&fixture, 5, json!({"operation": "remove_rule", "index": 0})).await;
    assert!(!removed.is_error, "{}", removed.output);
    let text = fixture.text();
    assert!(!text.contains("~/Downloads"), "{text}");
    assert!(text.contains("program = \"cargo\""), "{text}");
}

#[tokio::test]
async fn every_valid_change_asks_in_every_mode_and_a_remote_turn_is_denied() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let config = fixture.home.path().join(".config").join("efr");
    // NOTE: a user rule that allows every write, the file included, changes nothing.
    let mut settings = Settings::default();
    settings.permissions.rules = efr_permissions::Policy::new(vec![efr_permissions::Rule::new(
        efr_permissions::Action::Any,
        efr_permissions::Resource::Any,
        Effect::Allow,
    )])
    .unwrap();
    let engine = engine_for(fixture.home.path(), &config, &settings);
    let set = json!({"operation": "set", "key": "permissions.mode", "value": "auto"});

    for origin in [Origin::Shell, Origin::Cli, Origin::Proxy, Origin::Phone] {
        let requirements = fixture.tool.requirements(&call(3, set.clone(), origin)).await.unwrap();
        for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
            let decision = engine.decide(&DecisionInput {
                requirements: requirements.clone(),
                scope: Scope::Machine,
                origin,
                mode,
                conversation_policy: ConversationPolicy::new("/tmp/scratch"),
            });
            let expected = if origin == Origin::Phone {
                (Effect::Deny, Cause::RemoteSettings { origin })
            } else {
                (Effect::Ask, Cause::SettingsChange)
            };
            let reason = decision.reasons()[0].cause.clone();
            assert_eq!((decision.effect(), reason), expected, "{mode} from {origin:?}");
        }
    }
    assert_eq!(fixture.text(), COMMENTED);
}

/// The tool in a running daemon: a turn asks, the answer writes, the reload applies.
mod daemon {
    use std::sync::Arc;

    use efr_config::Settings;
    use efr_protocol::{
        ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CommandId, ConversationSubscribe,
        ConversationSubscribeItem, Event, Method, Mode, Origin, PromptSend, PromptSendResult,
        ShellContext, TurnSettings,
    };
    use efr_test_support::{TestClock, TestDirs};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::COMMENTED;
    use crate::testing::{CallsOneToolFactory, RawClient, Running, deps, serve_with};

    const TTY: &str = "/dev/pts/7";

    fn command(n: u128) -> CommandId {
        CommandId::from_uuid(uuid::Uuid::from_u128(n))
    }

    /// A daemon whose model sets `model.name` to `gpt-6-sol` with the settings tool,
    /// over a commented config file.
    async fn daemon(dirs: &TestDirs, clock: &TestClock) -> Running {
        let path = dirs.dirs().config().join("config.toml");
        std::fs::create_dir_all(dirs.dirs().config()).unwrap();
        std::fs::write(&path, COMMENTED).unwrap();
        let settings = Settings::parse(&path, Some(COMMENTED)).unwrap();
        let arguments = json!({"operation": "set", "key": "model.name", "value": "gpt-6-sol"});
        let factory = CallsOneToolFactory("settings".to_owned(), arguments);
        serve_with(settings, deps(dirs, clock).with_providers(Arc::new(factory))).await
    }

    fn prompt(n: u128, cwd: &std::path::Path, mode: Mode) -> PromptSend {
        let mut context = ShellContext::new(cwd.to_path_buf());
        context.tty = Some(TTY.to_owned());
        PromptSend {
            command_id: command(n),
            conversation_id: None,
            new_conversation: false,
            text: "make gpt-6-sol my default model".to_owned(),
            context: Some(context),
            last_command: None,
            settings: TurnSettings { mode: Some(mode), ..TurnSettings::default() },
        }
    }

    /// Follows the turn of `sent` to its end and answers each approval with `answer`;
    /// every event of the turn.
    async fn follow(
        running: &Running,
        client: &mut RawClient,
        sent: &PromptSendResult,
        answer: ApprovalDecision,
    ) -> Vec<Event> {
        let stream = client
            .send(Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: sent.conversation_id,
                after_seq: Some(sent.seq),
                answers_input: false,
            }))
            .await;
        let mut events = Vec::new();
        loop {
            let item = client.next(stream).await.unwrap().unwrap();
            let ConversationSubscribeItem::Event(envelope) = serde_json::from_value(item).unwrap()
            else {
                panic!("a resume right after the prompt replays events");
            };
            if let Event::ApprovalRequested { call_id, .. } = &envelope.event {
                let (mut other, _) = RawClient::hello(&running.socket, None).await;
                let _: ApprovalRespondResult = other
                    .call(Method::ApprovalRespond(ApprovalRespond {
                        command_id: command(99),
                        conversation_id: sent.conversation_id,
                        call_id: *call_id,
                        decision: answer,
                    }))
                    .await
                    .unwrap();
            }
            let end =
                matches!(envelope.event, Event::TurnCompleted { .. } | Event::TurnFailed { .. });
            events.push(envelope.event);
            if end {
                return events;
            }
        }
    }

    fn tool_output(events: &[Event]) -> (bool, String) {
        events
            .iter()
            .find_map(|event| match event {
                Event::ToolCallCompleted { is_error, output, .. } => {
                    Some((*is_error, output.clone()))
                }
                _ => None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn an_auto_mode_turn_still_asks_and_the_reload_applies_the_approved_change() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let running = daemon(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello(&running.socket, Some(TTY)).await;

        let sent: PromptSendResult =
            client.call(Method::PromptSend(prompt(1, dirs.home(), Mode::Auto))).await.unwrap();
        let events = follow(&running, &mut client, &sent, ApprovalDecision::Allow).await;

        let started = events.iter().find_map(|event| match event {
            Event::TurnStarted { settings, .. } => settings.clone(),
            _ => None,
        });
        // NOTE: efrd does not run the sandbox yet, so the turn that asked for `auto` runs
        // as `cautious`; the engine's own tests hold that a settings change asks in `auto`.
        let asked = started.and_then(|settings| settings.fallback).map(|fallback| fallback.asked);
        assert_eq!(asked, Some(Mode::Auto));
        let asked = events.iter().find_map(|event| match event {
            Event::ApprovalRequested { summary, diff_preview, .. } => {
                Some((summary.clone(), diff_preview.clone()))
            }
            _ => None,
        });
        let (summary, diff) = asked.expect("the change asked, also in auto");
        assert_eq!(summary, "settings: change settings: set model.name = \"gpt-6-sol\"");
        let diff = diff.unwrap();
        assert!(diff.contains("-name = \"gpt-5.5\" # the old default\n"), "{diff}");
        assert!(diff.contains("+name = \"gpt-6-sol\" # the old default\n"), "{diff}");
        assert!(diff.contains(" # Pinned while the new one is tested.\n"), "{diff}");
        let (is_error, output) = tool_output(&events);
        assert!(!is_error, "{output}");
        assert!(output.contains("from the next turn"), "{output}");
        let path = dirs.dirs().config().join("config.toml");
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            COMMENTED.replace("name = \"gpt-5.5\" # the old", "name = \"gpt-6-sol\" # the old")
        );
        assert_eq!(
            running.settings.borrow().model.name.as_deref(),
            Some("gpt-6-sol"),
            "the reload applied the file for the next turn"
        );

        drop(client);
        running.stop().await;
    }

    #[tokio::test]
    async fn a_denied_change_writes_nothing() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let running = daemon(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello(&running.socket, Some(TTY)).await;

        let sent: PromptSendResult =
            client.call(Method::PromptSend(prompt(1, dirs.home(), Mode::Cautious))).await.unwrap();
        let events = follow(&running, &mut client, &sent, ApprovalDecision::Deny).await;

        let (is_error, output) = tool_output(&events);
        assert!(is_error);
        assert!(output.contains("denied"), "{output}");
        let path = dirs.dirs().config().join("config.toml");
        assert_eq!(std::fs::read_to_string(path).unwrap(), COMMENTED);
        assert_eq!(running.settings.borrow().model.name.as_deref(), Some("gpt-5.5"));

        drop(client);
        running.stop().await;
    }

    #[tokio::test]
    async fn a_turn_from_the_phone_cannot_change_the_settings() {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let running = daemon(&dirs, &clock).await;
        let (mut client, _) = RawClient::hello_as(&running.socket, None, Origin::Phone).await;

        let sent: PromptSendResult =
            client.call(Method::PromptSend(prompt(1, dirs.home(), Mode::Auto))).await.unwrap();
        let events = follow(&running, &mut client, &sent, ApprovalDecision::Allow).await;

        assert!(
            !events.iter().any(|event| matches!(event, Event::ApprovalRequested { .. })),
            "nothing is asked: {events:?}"
        );
        let (is_error, output) = tool_output(&events);
        assert!(is_error);
        assert!(output.contains("may read efr's settings but not change them"), "{output}");
        let path = dirs.dirs().config().join("config.toml");
        assert_eq!(std::fs::read_to_string(path).unwrap(), COMMENTED);

        drop(client);
        running.stop().await;
    }
}
