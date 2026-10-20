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
use crate::engine::{Probed, build, protected_config};
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
        let tool =
            SettingsTool::new(&config, receiver, engine, reloads, crate::testing::builtin_models());
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
    let facts = crate::engine::SandboxFacts::default();
    build(
        &home,
        &secrets,
        &protected_config(config),
        settings,
        &Registry::empty(),
        &facts,
        Probed::default(),
    )
    .unwrap()
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
    assert!(
        text.contains(
            "gpt-5.5 (the default): efforts low, medium, high, xhigh, default effort medium; \
             window 272000 tokens"
        ),
        "{text}"
    );
    assert!(
        text.contains("gpt-6.1-sol: efforts low, medium, high, xhigh, max, ultra, default effort low; window 272000 tokens, openai.models can raise it up to 872000"),
        "{text}"
    );
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
        for mode in Mode::ALL {
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
                drafts: false,
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
        // NOTE: the test daemon finds no sandbox launcher, so the turn that asked for
        // `auto` runs as `cautious`; the engine's own tests hold that a settings change
        // asks in `auto`.
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

#[test]
fn each_sandbox_key_that_opens_the_sandbox_loosens() {
    use std::path::PathBuf;

    use efr_config::{SandboxSettings, WriteProjects};

    let base = SandboxSettings::default();
    let changed = |change: fn(&mut SandboxSettings)| {
        let mut next = base.clone();
        change(&mut next);
        next
    };
    let opening: Vec<(&str, SandboxSettings)> = vec![
        ("sandbox.bwrap", changed(|s| s.bwrap = Some(PathBuf::from("/opt/bwrap")))),
        ("sandbox.write_projects all", changed(|s| s.write_projects = WriteProjects::All)),
        ("sandbox.write_roots added", changed(|s| s.write_roots.push("~/notes".into()))),
        ("sandbox.caches added", changed(|s| s.caches.push("~/.stack".into()))),
        ("sandbox.mask_globs removed", changed(|s| s.mask_globs.truncate(1))),
        ("sandbox.env_keep added", changed(|s| s.env_keep.push("GH_TOKEN".to_owned()))),
        ("sandbox.promote_env added", changed(|s| s.promote_env.push("PATH".to_owned()))),
        ("sandbox.synced_dirs removed", changed(|s| s.synced_dirs.clear())),
        ("sandbox.rebuildable added", changed(|s| s.rebuildable.push("src".to_owned()))),
        (
            "sandbox.allow added",
            changed(|s| {
                s.allow.push(efr_config::AllowRule::Write {
                    under: "~/notes".to_owned(),
                    project: None,
                });
            }),
        ),
    ];
    for (row, next) in &opening {
        assert!(crate::loosens::sandbox_loosens(&base, next), "{row} must loosen");
    }
    let closing: Vec<(&str, SandboxSettings)> = vec![
        ("sandbox.enabled", changed(|s| s.enabled = false)),
        ("sandbox.write_projects turn", changed(|s| s.write_projects = WriteProjects::Turn)),
        ("sandbox.mask added", changed(|s| s.mask.push("~/private".into()))),
        ("sandbox.mask_globs added", changed(|s| s.mask_globs.push("*.key".to_owned()))),
        ("sandbox.protect added", changed(|s| s.protect.push("~/bin".into()))),
        ("sandbox.env_deny added", changed(|s| s.env_deny.push("FOO".to_owned()))),
        ("sandbox.export_deny added", changed(|s| s.export_deny.push("FOO".to_owned()))),
        ("sandbox.cache_days", changed(|s| s.cache_days = 1)),
        ("sandbox.caches removed", changed(|s| s.caches.clear())),
        ("sandbox.offline_hints", changed(|s| s.offline_hints = false)),
    ];
    for (row, next) in &closing {
        assert!(!crate::loosens::sandbox_loosens(&base, next), "{row} must not loosen");
    }
}

#[test]
fn loosens_classifier_enabled() {
    let mut off = Settings::default();
    off.classifier.enabled = false;
    let on = Settings::default();
    assert!(crate::loosens::loosens(&off, &on), "turning the classifier on loosens");
    assert!(!crate::loosens::loosens(&on, &off), "turning it off asks more, so it does not loosen");

    let changed = |change: fn(&mut Settings)| {
        let mut next = Settings::default();
        change(&mut next);
        next
    };
    let others: Vec<(&str, Settings)> = vec![
        ("classifier.model", changed(|s| s.classifier.model = Some("gpt-6-luna".to_owned()))),
        (
            "classifier.fallback_to_turn_model",
            changed(|s| s.classifier.fallback_to_turn_model = false),
        ),
        ("classifier.effort", changed(|s| s.classifier.effort = "high".to_owned())),
        ("classifier.timeout_secs", changed(|s| s.classifier.timeout_secs = 60)),
        ("presence.away_after_minutes", changed(|s| s.presence.away_after_minutes = 30)),
        ("presence.away_wait_secs", changed(|s| s.presence.away_wait_secs = 60)),
        ("presence.push", changed(|s| s.presence.push = false)),
    ];
    for (row, next) in &others {
        assert!(!crate::loosens::loosens(&on, next), "{row} must not loosen");
    }
}

#[test]
fn keep_downloads_does_not_loosen() {
    // It keeps the private layer that the `overlay` mode keeps, and opens nothing of
    // the host, as `cache_mode` does not loosen.
    let mut off = Settings::default();
    off.sandbox.keep_downloads = false;
    let mut on = Settings::default();
    on.sandbox.keep_downloads = true;
    assert!(!crate::loosens::loosens(&off, &on), "false to true does not loosen");
    assert!(!crate::loosens::sandbox_loosens(&off.sandbox, &on.sandbox));
    assert!(!crate::loosens::loosens(&on, &off), "true to false does not loosen");
}

#[test]
fn loosens_ask_destructive_unless_undoable() {
    use efr_config::AskDestructive;

    let mut always = Settings::default();
    always.sandbox.ask_destructive = AskDestructive::Always;
    let unless = Settings::default();
    assert_eq!(unless.sandbox.ask_destructive, AskDestructive::UnlessUndoable);
    assert!(crate::loosens::loosens(&always, &unless), "always to unless_undoable asks less");
    assert!(crate::loosens::sandbox_loosens(&always.sandbox, &unless.sandbox));
    assert!(!crate::loosens::loosens(&unless, &always), "unless_undoable to always asks more");
}

#[test]
fn loosens_dbus_rows() {
    use efr_config::DbusSettings;
    use efr_permissions::BusRule;
    use efr_protocol::BusKind;

    let entry = |bus: BusKind, name: &str, calls: &[&str]| BusRule {
        bus,
        name: name.to_owned(),
        call: calls.iter().map(|call| (*call).to_owned()).collect(),
    };
    let get = "org.freedesktop.Notifications.GetCapabilities";
    let info = "org.freedesktop.Notifications.GetServerInformation";
    let mut base = DbusSettings::default();
    base.proxy = false;
    base.allow.push(entry(BusKind::Session, "org.freedesktop.Notifications", &[get]));
    let changed = |change: &dyn Fn(&mut DbusSettings)| {
        let mut next = base.clone();
        change(&mut next);
        next
    };
    let opening: Vec<(&str, DbusSettings)> = vec![
        ("dbus.proxy true", changed(&|d| d.proxy = true)),
        ("dbus.allow method added", changed(&|d| d.allow[0].call.push(info.to_owned()))),
        (
            "dbus.allow entry added",
            changed(&|d| d.allow.push(entry(BusKind::Session, "org.example.Player", &[get]))),
        ),
        ("dbus.allow other bus", changed(&|d| d.allow[0].bus = BusKind::System)),
    ];
    for (row, next) in &opening {
        assert!(crate::loosens::dbus_loosens(&base, next), "{row} must loosen");
        let mut before = Settings::default();
        before.dbus = base.clone();
        let mut after = Settings::default();
        after.dbus = next.clone();
        assert!(crate::loosens::loosens(&before, &after), "{row} must loosen the whole file");
    }
    let mut on = base.clone();
    on.proxy = true;
    let closing: Vec<(&str, DbusSettings)> = vec![
        ("dbus.proxy false", base.clone()),
        ("dbus.allow removed", {
            let mut next = on.clone();
            next.allow.clear();
            next
        }),
        ("dbus.allow split", {
            let mut next = on.clone();
            next.allow.push(next.allow[0].clone());
            next
        }),
    ];
    for (row, next) in &closing {
        assert!(!crate::loosens::dbus_loosens(&on, next), "{row} must not loosen");
    }
}

/// `COMMENTED` with the gate of full mode on.
fn gated() -> String {
    COMMENTED.replace("mode = \"cautious\"\n", "mode = \"cautious\"\nallow_full = true\n")
}

#[tokio::test]
async fn settings_tool_refuses_allow_full() {
    let cases = [
        (
            COMMENTED.to_owned(),
            json!({"operation": "set", "key": "permissions.allow_full", "value": true}),
        ),
        (
            COMMENTED.to_owned(),
            json!({"operation": "set", "key": "permissions.allow_full", "value": "true"}),
        ),
        (
            COMMENTED.to_owned(),
            json!({"operation": "set", "key": "permissions.allow_full", "value": false}),
        ),
        (gated(), json!({"operation": "set", "key": "permissions.allow_full", "value": false})),
        (gated(), json!({"operation": "unset", "key": "permissions.allow_full"})),
        (COMMENTED.to_owned(), json!({"operation": "unset", "key": "permissions.allow_full"})),
    ];
    for (n, (file, input)) in (10..).zip(cases) {
        let fixture = Fixture::new();
        fixture.write(&file);
        let call = local(n, input.clone());

        let refused = fixture.tool.requirements(&call).await.unwrap_err();

        assert!(refused.contains("never changes permissions.allow_full"), "{input}: {refused}");
        assert!(refused.contains("efr config edit"), "{input}: {refused}");
        assert_eq!(fixture.tool.preview(&call).await, None, "{input}");
        assert_eq!(fixture.text(), file, "{input}");
        assert!(fixture.reloads.lock().unwrap().is_empty(), "{input}");
    }
}

#[tokio::test]
async fn settings_tool_refuses_mode_full() {
    let set_full = json!({"operation": "set", "key": "permissions.mode", "value": "full"});
    for (n, file) in (10..).zip([COMMENTED.to_owned(), gated()]) {
        let fixture = Fixture::new();
        fixture.write(&file);
        let call = local(n, set_full.clone());

        let refused = fixture.tool.requirements(&call).await.unwrap_err();

        assert!(refused.contains("never sets permissions.mode to full"), "{file}: {refused}");
        assert!(refused.contains("efr config edit"), "{file}: {refused}");
        assert_eq!(fixture.tool.preview(&call).await, None, "{file}");
        assert_eq!(fixture.text(), file);
    }

    // The other modes stay a change that asks, also with the gate on, and so does a
    // move away from full that the user set by hand.
    let full = gated().replace("mode = \"cautious\"", "mode = \"full\"");
    let cases = [
        (gated(), json!({"operation": "set", "key": "permissions.mode", "value": "auto"}), true),
        (
            full.clone(),
            json!({"operation": "set", "key": "permissions.mode", "value": "auto"}),
            false,
        ),
        (full, json!({"operation": "unset", "key": "permissions.mode"}), false),
    ];
    for (n, (file, input, loosens)) in (20..).zip(cases) {
        let fixture = Fixture::new();
        fixture.write(&file);

        let requirements = fixture.tool.requirements(&local(n, input.clone())).await.unwrap();

        let marked = requirements.settings.as_ref().map(|change| change.loosens);
        assert_eq!(marked, Some(loosens), "{input}");
    }
}

#[test]
fn loosens_allow_full() {
    let off = Settings::default();
    assert!(!off.permissions.allow_full, "the gate is off by default");
    let mut on = Settings::default();
    on.permissions.allow_full = true;
    assert!(crate::loosens::loosens(&off, &on), "turning the gate on loosens");
    assert!(!crate::loosens::loosens(&on, &off), "turning it off asks more, so it does not loosen");

    // Full is above auto, so a move to full as the default loosens too.
    let mut auto = on.clone();
    auto.permissions.mode = Mode::Auto;
    let mut full = on.clone();
    full.permissions.mode = Mode::Full;
    assert!(crate::loosens::loosens(&auto, &full), "auto to full loosens");
    assert!(!crate::loosens::loosens(&full, &auto), "full to auto does not loosen");
}

#[tokio::test]
async fn settings_tool_cannot_write_sandbox_allow() {
    let fixture = Fixture::new();
    fixture.write(COMMENTED);
    let rule = json!([{"kind": "write", "under": "~/notes"}]);
    let bus = json!([{"bus": "system", "name": "org.freedesktop.timedate1", "call": ["org.freedesktop.DBus.Properties.GetAll"]}]);
    let cases = [
        json!({"operation": "set", "key": "sandbox.allow", "value": rule}),
        json!({"operation": "set", "key": "sandbox.allow", "value": "[{ kind = \"write\", under = \"~/notes\" }]"}),
        json!({"operation": "unset", "key": "sandbox.allow"}),
        json!({"operation": "set", "key": "dbus.allow", "value": bus}),
        json!({"operation": "unset", "key": "dbus.allow"}),
        json!({"operation": "set", "key": "network.hosts", "value": []}),
    ];
    for (n, input) in (10..).zip(cases) {
        let call = local(n, input.clone());
        let refused = fixture.tool.requirements(&call).await.unwrap_err();
        assert!(refused.contains("only the user changes"), "{input}: {refused}");
        assert_eq!(fixture.tool.preview(&call).await, None, "{input}");
    }
    // `add_rule` writes `permissions.rules` only: an always-allow rule is not a rule.
    let call =
        local(20, json!({"operation": "add_rule", "rule": {"kind": "write", "under": "~/notes"}}));
    let refused = fixture.tool.requirements(&call).await.unwrap_err();
    assert!(refused.contains("not a rule"), "{refused}");
    assert_eq!(fixture.text(), COMMENTED);
}

#[test]
fn loosens_network_rows() {
    let on = Settings::default();
    assert!(on.network.netview, "the network view is on by default");
    let mut off = Settings::default();
    off.network.netview = false;
    assert!(crate::loosens::loosens(&off, &on), "false to true runs more lines with no review");
    assert!(crate::loosens::network_loosens(&off.network, &on.network));
    assert!(!crate::loosens::loosens(&on, &off), "true to false sends them to the reviewer again");
    assert!(!crate::loosens::network_loosens(&on.network, &on.network));

    // The proxy keys of phase D2.
    use efr_config::{NetworkHost, NetworkSettings};
    use efr_protocol::NetworkMode;

    let mirror = |port: Option<u16>, http: bool, private: bool| NetworkHost {
        host: "mirror.example.org".to_owned(),
        port,
        http,
        private,
    };
    let mut base = NetworkSettings::default();
    // NOTE: the default is the proxy; the base has no network, so the mode can open.
    base.mode = NetworkMode::None;
    base.deny.push("tracker.example.com".to_owned());
    base.hosts.push(mirror(None, false, false));
    let changed = |change: &dyn Fn(&mut NetworkSettings)| {
        let mut next = base.clone();
        change(&mut next);
        next
    };
    let opening: Vec<(&str, NetworkSettings)> = vec![
        ("network.mode proxy", changed(&|n| n.mode = NetworkMode::Proxy)),
        ("network.allow added", changed(&|n| n.allow.push("gitlab.com".to_owned()))),
        ("network.allow added suffix", changed(&|n| n.allow.push("*.example.com".to_owned()))),
        ("network.deny removed", changed(&|n| n.deny.clear())),
        (
            "network.hosts added",
            changed(&|n| {
                n.hosts.push(NetworkHost {
                    host: "192.168.1.5".to_owned(),
                    port: Some(8080),
                    http: false,
                    private: true,
                });
            }),
        ),
        ("network.hosts http true", changed(&|n| n.hosts[0].http = true)),
        ("network.hosts private true", changed(&|n| n.hosts[0].private = true)),
        ("network.hosts new port", changed(&|n| n.hosts[0].port = Some(8443))),
        ("network.max_upload_bytes higher", changed(&|n| n.max_upload_bytes += 1)),
    ];
    for (row, next) in &opening {
        assert!(crate::loosens::network_loosens(&base, next), "{row} must loosen");
        let mut before = Settings::default();
        before.network = base.clone();
        let mut after = Settings::default();
        after.network = next.clone();
        assert!(crate::loosens::loosens(&before, &after), "{row} must loosen the whole file");
    }

    let mut wide = base.clone();
    wide.mode = NetworkMode::Proxy;
    wide.hosts[0] = mirror(Some(8443), true, true);
    let narrowing = |change: &dyn Fn(&mut NetworkSettings)| {
        let mut next = wide.clone();
        change(&mut next);
        next
    };
    let closing: Vec<(&str, NetworkSettings)> = vec![
        ("network.mode none", narrowing(&|n| n.mode = NetworkMode::None)),
        ("network.allow removed", narrowing(&|n| n.allow.clear())),
        ("network.deny added", narrowing(&|n| n.deny.push("github.com".to_owned()))),
        ("network.hosts removed", narrowing(&|n| n.hosts.clear())),
        ("network.hosts http false", narrowing(&|n| n.hosts[0].http = false)),
        ("network.hosts private false", narrowing(&|n| n.hosts[0].private = false)),
        ("network.max_upload_bytes lower", narrowing(&|n| n.max_upload_bytes = 65_536)),
    ];
    for (row, next) in &closing {
        assert!(!crate::loosens::network_loosens(&wide, next), "{row} must not loosen");
    }
}

#[test]
fn loosens_undo_and_snapshot_enabled() {
    let mut off = Settings::default();
    off.undo.enabled = false;
    let on = Settings::default();
    assert!(crate::loosens::loosens(&off, &on), "turning undo on loosens");
    assert!(!crate::loosens::loosens(&on, &off), "turning undo off asks more");

    let mut no_snapshots = Settings::default();
    no_snapshots.snapshot.enabled = false;
    assert!(crate::loosens::loosens(&no_snapshots, &on), "turning snapshots on loosens");
    assert!(!crate::loosens::loosens(&on, &no_snapshots), "turning snapshots off asks more");

    let mut no_ignored = Settings::default();
    no_ignored.snapshot.ignored = efr_config::IgnoredFiles::None;
    assert!(crate::loosens::loosens(&no_ignored, &on), "taking small ignored files loosens");
    assert!(!crate::loosens::loosens(&on, &no_ignored), "leaving them out asks more");
}

#[test]
fn loosens_more_coverage_limits() {
    let base = Settings::default();
    let changed = |change: fn(&mut Settings)| {
        let mut next = Settings::default();
        change(&mut next);
        next
    };
    let higher: Vec<(&str, Settings)> = vec![
        ("snapshot.max_file_mib", changed(|s| s.snapshot.max_file_mib += 1)),
        ("snapshot.mirror_max_mib", changed(|s| s.snapshot.mirror_max_mib += 1)),
        ("snapshot.home_max_files", changed(|s| s.snapshot.home_max_files += 1)),
        ("snapshot.home_max_mib", changed(|s| s.snapshot.home_max_mib += 1)),
        ("snapshot.max_files", changed(|s| s.snapshot.max_files += 1)),
    ];
    for (row, next) in &higher {
        assert!(crate::loosens::loosens(&base, next), "a higher {row} must loosen");
        assert!(!crate::loosens::loosens(next, &base), "a lower {row} must not loosen");
    }
    let others: Vec<(&str, Settings)> = vec![
        ("snapshot.keep_turns", changed(|s| s.snapshot.keep_turns += 1)),
        ("snapshot.max_age_days", changed(|s| s.snapshot.max_age_days += 1)),
    ];
    for (row, next) in &others {
        assert!(!crate::loosens::loosens(&base, next), "{row} must not loosen");
    }
}

#[test]
fn loosens_jobs_rows() {
    let changed = |change: fn(&mut Settings)| {
        let mut next = Settings::default();
        change(&mut next);
        next
    };
    let mut off = Settings::default();
    off.jobs.enabled = false;
    let on = Settings::default();
    assert!(crate::loosens::loosens(&off, &on), "turning jobs on loosens");
    assert!(!crate::loosens::loosens(&on, &off), "turning jobs off asks more");

    let base = changed(|s| {
        s.jobs.default_minutes = 30;
        s.jobs.max_minutes = 120;
        s.jobs.max_running = 2;
        s.jobs.max_running_daemon = 8;
        s.jobs.max_wakes_per_hour = 3;
        s.jobs.max_wakes_per_hour_daemon = 6;
        s.jobs.max_wakes_per_watch = 1;
        s.jobs.nice = 10;
    });
    type Change = fn(&mut Settings);
    let more: [(&str, Change); 8] = [
        ("jobs.default_minutes", |s| s.jobs.default_minutes = 31),
        ("jobs.max_minutes", |s| s.jobs.max_minutes = 121),
        ("jobs.max_running", |s| s.jobs.max_running = 3),
        ("jobs.max_running_daemon", |s| s.jobs.max_running_daemon = 9),
        ("jobs.max_wakes_per_hour", |s| s.jobs.max_wakes_per_hour = 4),
        ("jobs.max_wakes_per_hour_daemon", |s| s.jobs.max_wakes_per_hour_daemon = 7),
        ("jobs.max_wakes_per_watch", |s| s.jobs.max_wakes_per_watch = 2),
        ("a lower jobs.nice", |s| s.jobs.nice = 9),
    ];
    for (row, change) in more {
        let mut next = base.clone();
        change(&mut next);
        assert!(crate::loosens::loosens(&base, &next), "{row} must loosen");
        assert!(crate::loosens::jobs_loosens(&base.jobs, &next.jobs), "{row}");
        assert!(!crate::loosens::loosens(&next, &base), "the way back from {row} must not loosen");
    }
}
