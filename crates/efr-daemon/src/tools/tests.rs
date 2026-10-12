use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_config::{Settings, SudoCache};
use efr_conversation::{CallContext, ToolCall, Toolbox as _};
use efr_holder::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyId, PtyInfo, Signal, SignalTarget, Size,
    SpawnSpec,
};
use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, Locations,
    Requirements, Resource, Rule,
};
use efr_protocol::{CallId, ConversationId, Launch, Mode, Origin, Scope, TurnId};
use efr_provider::EditTool;
use efr_scope::Home;
use efr_shell::{ShellConfig, ShellDeps, ShellSessions};
use efr_test_support::{TestClock, TestRng};
use efr_tools::{ToolError, ToolRequirements, ToolResult};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::connections::Connections;
use crate::reload::Reloads;
use crate::screens::ScreenBackend;
use crate::tools::{
    DaemonToolbox, SettingsTool, definition, for_model, outcome, permission_requirements, registry,
};

/// A holder that never starts a shell; these tests never run a command.
#[derive(Debug)]
struct NoHolder;

#[async_trait]
impl PtyHolder for NoHolder {
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError> {
        Err(HolderError::NotFound { pty_id: spec.pty_id })
    }
    async fn resize(&self, pty_id: PtyId, _size: Size) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn signal(
        &self,
        pty_id: PtyId,
        _signal: Signal,
        _target: SignalTarget,
    ) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError> {
        Ok(Vec::new())
    }
    async fn foreground(&self, pty_id: PtyId) -> Result<Option<u32>, HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
}

pub(super) fn toolbox(home: &Path) -> DaemonToolbox {
    toolbox_with(home, Settings::default()).0
}

pub(super) fn toolbox_with(
    home: &Path,
    settings: Settings,
) -> (DaemonToolbox, tokio::sync::watch::Sender<Arc<Settings>>) {
    let (sender, receiver) = tokio::sync::watch::channel(Arc::new(settings));
    let clock = TestClock::new();
    let mut config = ShellConfig::new(home.join("zsh"), BTreeMap::new());
    config.program = Some(PathBuf::from("/bin/sh"));
    let deps = ShellDeps::new(
        Arc::new(NoHolder),
        ScreenBackend::Vt100.factory(),
        clock.shared(),
        Arc::new(TestRng::new(1)),
    );
    let shells = ShellSessions::new(config, deps).unwrap();
    let registry = registry(&shells).unwrap();
    let engine = Engine::with_defaults(Locations::new(home).unwrap());
    let (_, engine) = tokio::sync::watch::channel(Arc::new(engine));
    let (reloads, _) = Reloads::new();
    let settings_tool = SettingsTool::new(
        &home.join(".config").join("efr"),
        receiver.clone(),
        engine,
        reloads,
        crate::testing::builtin_models(),
    );
    let toolbox = DaemonToolbox::new(
        registry,
        shells,
        Home::new(home).unwrap(),
        clock.shared(),
        Arc::new(Connections::default()),
        receiver,
        settings_tool,
    );
    (toolbox, sender)
}

#[test]
fn each_call_reads_the_sudo_cache_from_the_latest_settings() {
    let home = tempfile::tempdir().unwrap();
    let (toolbox, settings) = toolbox_with(home.path(), Settings::default());
    let call = call("shell", json!({"command": "sudo true"}), home.path());

    let kept = toolbox.context(&call.context).forget_credentials;
    let mut per_call = Settings::default();
    per_call.shell.sudo_cache = SudoCache::PerCall;
    settings.send_replace(Arc::new(per_call));
    let forgotten = toolbox.context(&call.context).forget_credentials;

    assert!(!kept, "keep leaves the cache to sudo");
    assert!(forgotten, "per_call reaches the next call");
}

#[test]
fn only_an_approved_interactive_call_gets_the_interactive_limit_of_the_latest_settings() {
    let home = tempfile::tempdir().unwrap();
    let (toolbox, settings) = toolbox_with(home.path(), Settings::default());
    let call = call("shell", json!({"command": "sudo pacman -Syu"}), home.path());
    let approved = call.context.clone().with_approved_interactive(true);

    assert_eq!(toolbox.context(&call.context).interactive_limit, None);
    assert_eq!(toolbox.context(&approved).interactive_limit, Some(Duration::from_secs(3600)));
    let mut shorter = Settings::default();
    shorter.shell.interactive_timeout_minutes = 5;
    settings.send_replace(Arc::new(shorter));
    assert_eq!(
        toolbox.context(&approved).interactive_limit,
        Some(Duration::from_secs(300)),
        "a change reaches the next call"
    );
}

pub(super) fn call(name: &str, input: serde_json::Value, cwd: &Path) -> ToolCall {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let context = CallContext::new(
        ConversationId::from_uuid(id(1)),
        TurnId::from_uuid(id(2)),
        CallId::from_uuid(id(3)),
        cwd,
        cwd.join("scratch"),
    );
    ToolCall::new(name, input, context)
}

#[test]
fn a_freeform_spec_becomes_a_freeform_definition_with_the_same_function_form() {
    // NOTE: the two crates name the member of the function form each on their own,
    // because neither depends on the other.
    assert_eq!(efr_tools::FREEFORM_INPUT, efr_provider::FREEFORM_INPUT);
    let grammar = efr_tools::ToolGrammar::Lark("start: LF".to_owned());
    let spec = efr_tools::ToolSpec::freeform("apply_patch", "Edits files.", grammar);
    let defined = definition(spec.clone());
    assert_eq!(defined.grammar, Some(efr_provider::ToolGrammar::lark("start: LF")));
    assert_eq!(defined.input_schema, spec.input_schema);
    assert_eq!(
        defined,
        efr_provider::ToolDefinition::freeform(
            "apply_patch",
            "Edits files.",
            efr_provider::ToolGrammar::lark("start: LF"),
        )
    );
}

#[test]
fn the_model_is_offered_the_shell_the_file_tools_and_the_settings() {
    let home = tempfile::tempdir().unwrap();
    let definitions = toolbox(home.path()).definitions(EditTool::ApplyPatch);

    let names: Vec<&str> = definitions.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(
        names,
        ["shell", "read_file", "write_file", "apply_patch", "settings", "jobs", "job_stop"]
    );
    assert!(definitions.iter().all(|tool| tool.input_schema["type"] == "object"));
    let patch = &definitions[3];
    assert_eq!(patch.grammar, Some(efr_provider::ToolGrammar::lark(efr_patch_grammar())));
}

#[test]
fn a_model_that_replaces_text_is_offered_edit_in_place_of_apply_patch() {
    let home = tempfile::tempdir().unwrap();
    let definitions = toolbox(home.path()).definitions(EditTool::Replace);

    let names: Vec<&str> = definitions.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["shell", "read_file", "write_file", "edit", "settings", "jobs", "job_stop"]);
    let edit = &definitions[3];
    assert_eq!(edit.grammar, None, "edit is a function tool");
    assert_eq!(edit.input_schema["required"], json!(["path", "old_string", "new_string"]));
}

#[tokio::test]
async fn a_call_of_the_edit_tool_that_the_request_did_not_offer_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    let patch = "*** Begin Patch\n*** Delete File: old.rs\n*** End Patch\n";
    let mut patched = call("apply_patch", json!(patch), home.path());
    patched.context = patched.context.with_edit_tool(EditTool::Replace);
    let edited =
        call("edit", json!({"path": "a.rs", "old_string": "a", "new_string": "b"}), home.path());

    let refused = toolbox.requirements(&patched).await;
    let other = toolbox.requirements(&edited).await;

    assert_eq!(
        refused,
        Err("This model has no apply_patch tool, and the call did not run. Use the edit tool \
             to change a file."
            .to_owned())
    );
    assert_eq!(
        other,
        Err("This model has no edit tool, and the call did not run. Use the apply_patch tool \
             to change a file."
            .to_owned())
    );
}

#[tokio::test]
async fn an_edit_declares_its_path_as_a_write_and_is_never_destructive() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    let mut edit =
        call("edit", json!({"path": "a.rs", "old_string": "a", "new_string": "b"}), home.path());
    edit.context = edit.context.with_edit_tool(EditTool::Replace);

    let requirements = toolbox.requirements(&edit).await;

    assert_eq!(requirements, Ok(Requirements::none().with_write(home.path().join("a.rs"))));
}

/// The grammar that `apply_patch` sends, as its spec gives it.
fn efr_patch_grammar() -> String {
    use efr_tools::Tool as _;
    match efr_tools::ApplyPatchTool.spec().grammar {
        Some(efr_tools::ToolGrammar::Lark(grammar)) => grammar,
        None => panic!("apply_patch is a freeform tool"),
    }
}

#[tokio::test]
async fn a_patch_declares_every_path_as_a_write_and_a_delete_as_destructive() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    let text = "*** Begin Patch\n*** Delete File: old.rs\n*** End Patch\n";

    let requirements = toolbox.requirements(&call("apply_patch", json!(text), home.path())).await;

    assert_eq!(
        requirements,
        Ok(Requirements::none().with_write(home.path().join("old.rs")).with_destructive())
    );
}

#[tokio::test]
async fn a_read_declares_its_resolved_path_for_reading() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());

    let requirements =
        toolbox.requirements(&call("read_file", json!({"path": "notes.txt"}), home.path())).await;

    assert_eq!(requirements, Ok(Requirements::none().with_read(home.path().join("notes.txt"))));
}

#[tokio::test]
async fn a_shell_call_declares_its_command_line() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());

    let requirements = toolbox
        .requirements(&call("shell", json!({"command": "ls -la"}), home.path()))
        .await
        .unwrap();

    assert_eq!(requirements.command.as_deref(), Some("ls -la"));
}

#[tokio::test]
async fn an_unknown_tool_is_text_for_the_model() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());

    let requirements = toolbox.requirements(&call("rm_rf", json!({}), home.path())).await;

    assert!(requirements.unwrap_err().contains("rm_rf"));
}

#[tokio::test]
async fn a_call_for_the_sandbox_never_runs_in_the_hidden_shell() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    for launch in [Launch::contained(), Launch::Unsandboxed] {
        let mut call = call("shell", json!({"command": "touch made"}), home.path());
        call.context = call.context.with_launch(launch);
        let mut sink = |_: &str, _: u64| {};

        let outcome = toolbox.invoke(call, &mut sink).await;

        assert!(outcome.is_error, "{outcome:?}");
        assert_eq!(outcome.output, crate::tools::SANDBOX_NOT_STARTED);
        assert!(!home.path().join("made").exists());
    }
}

#[tokio::test]
async fn a_read_runs_and_answers_with_the_file() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("notes.txt"), "hello\n").unwrap();
    let toolbox = toolbox(home.path());
    let mut updates = Vec::new();
    let mut sink = |tail: &str, bytes: u64| updates.push((tail.to_owned(), bytes));

    let outcome = toolbox
        .invoke(call("read_file", json!({"path": "notes.txt"}), home.path()), &mut sink)
        .await;

    assert!(!outcome.is_error, "{outcome:?}");
    assert!(outcome.output.contains("hello"), "{outcome:?}");
}

#[tokio::test]
async fn a_write_is_previewed_as_a_diff_of_the_file() {
    let home = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    std::fs::write(home.join(".zshrc"), "alias ll='ls -l'\n").unwrap();
    let toolbox = toolbox(&home);
    let write = call(
        "write_file",
        json!({"path": "~/.zshrc", "content": "alias ll='ls -l'\nalias la='ls -a'\n"}),
        &home,
    );

    let preview = toolbox.preview(&write).await.unwrap();

    let path = home.join(".zshrc");
    assert_eq!(
        preview,
        format!(
            "--- a{0}\n+++ b{0}\n@@ -1,1 +1,2 @@\n alias ll='ls -l'\n+alias la='ls -a'\n",
            path.display()
        )
    );
    let read = call("read_file", json!({"path": "~/.zshrc"}), &home);
    assert_eq!(toolbox.preview(&read).await, None, "only a write has a preview");
}

#[test]
fn every_declared_requirement_is_copied() {
    let declared = ToolRequirements::none()
        .with_read("/etc/hosts")
        .with_read_tree("/var/log")
        .with_write("/tmp/out")
        .with_command("make")
        .with_command_dir("/srv/app")
        .with_network(true)
        .with_interactive(true)
        .with_destructive(true)
        .with_background(true)
        .with_max_minutes(Some(90))
        .with_watch(true);

    assert_eq!(
        permission_requirements(declared),
        Requirements::none()
            .with_read("/etc/hosts")
            .with_read_tree("/var/log")
            .with_write("/tmp/out")
            .with_command("make")
            .with_command_dir("/srv/app")
            .with_network()
            .with_interactive()
            .with_destructive()
            .with_background()
            .with_max_minutes(90)
            .with_watch()
    );
}

#[tokio::test]
async fn background_not_ready_before_the_runtime() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    let job = json!({"command": "npm run dev", "background": true, "max_minutes": 30});

    let refused = toolbox.requirements(&call("shell", job, home.path())).await;

    assert_eq!(refused, Err(super::job_tools::NOT_READY.to_owned()));
    // NOTE: the rules of the job members refuse first, as they will once jobs run.
    let early = json!({"command": "make", "background": true, "timeout_seconds": 5});
    let rule = toolbox.requirements(&call("shell", early, home.path())).await.unwrap_err();
    assert!(rule.contains("timeout_seconds"), "{rule}");
    let without = json!({"command": "make", "max_minutes": 5});
    let rule = toolbox.requirements(&call("shell", without, home.path())).await.unwrap_err();
    assert!(rule.contains("needs background: true"), "{rule}");
    let foreground = json!({"command": "ls", "background": false});
    assert!(toolbox.requirements(&call("shell", foreground, home.path())).await.is_ok());
}

#[tokio::test]
async fn a_shell_call_resolves_relative_paths_where_the_hidden_shell_is() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    let mut shell_call = call("shell", json!({"command": "cat notes.txt"}), home.path());
    shell_call.context = shell_call.context.with_shell_cwd(Some(PathBuf::from("/var/log")));

    let requirements = toolbox.requirements(&shell_call).await.unwrap();

    assert_eq!(requirements.paths, Requirements::none().with_read("/var/log/notes.txt").paths);
}

#[tokio::test]
async fn a_shell_call_through_a_link_into_the_secrets_declares_and_meets_the_secret() {
    let home = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    let cwd = home.join("p/app");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "key").unwrap();
    std::os::unix::fs::symlink(home.join(".ssh/id_ed25519"), cwd.join("notes")).unwrap();
    std::os::unix::fs::symlink(home.join(".ssh"), cwd.join("keys")).unwrap();
    let toolbox = toolbox(&home);
    let engine =
        Engine::with_rules(Locations::new(&home).unwrap(), Settings::default().permissions.rules);

    let cat = call("shell", json!({"command": "cat notes"}), &cwd);
    let requirements = toolbox.requirements(&cat).await.unwrap();
    assert_eq!(
        requirements.paths,
        Requirements::none()
            .with_read(cwd.join("notes"))
            .with_read(home.join(".ssh/id_ed25519"))
            .paths
    );

    for (command, expected) in [
        ("cat notes", Effect::Deny),
        ("rg TOKEN keys", Effect::Deny),
        ("ls keys/", Effect::Deny),
        ("cat README.md", Effect::Allow),
    ] {
        let shell_call = call("shell", json!({ "command": command }), &cwd);
        let input = DecisionInput {
            requirements: toolbox.requirements(&shell_call).await.unwrap(),
            scope: Scope::Machine,
            origin: Origin::Shell,
            mode: Mode::Cautious,
            conversation_policy: ConversationPolicy::new(home.join(".local/share/efr/scratch/x")),
        };
        assert_eq!(engine.decide(&input).effect(), expected, "{command:?}");
    }
}

#[tokio::test]
async fn shell_writes_into_efrs_config_are_denied_through_links_too() {
    let home = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    let cwd = home.join("p/app");
    let config = home.join(".config/efr");
    let dotfiles = home.join("dotfiles/efr");
    for dir in [&cwd, &config, &dotfiles] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(dotfiles.join("config.toml"), "").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("config.toml"), config.join("config.toml")).unwrap();
    std::os::unix::fs::symlink(&config, cwd.join("cfg")).unwrap();
    let toolbox = toolbox(&home);
    let engine = crate::engine::build(
        &Home::new(&home).unwrap(),
        &home.join(".local/share/efr/secrets"),
        &crate::engine::protected_config(&config),
        &Settings::default(),
        &efr_scope::Registry::empty(),
        &crate::engine::SandboxFacts::default(),
    )
    .unwrap();

    for (command, expected) in [
        ("cp notes ~/.config/efr/config.toml", Effect::Deny),
        ("ln -sf ~/p/app/x ~/.config/efr/config.toml", Effect::Deny),
        // An option after the destination does not make its value the destination.
        ("cp notes ~/.config/efr/config.toml -S x", Effect::Deny),
        ("cp notes ~/.config/efr/config.toml --suffix x", Effect::Deny),
        ("ln -sf ~/p/app/x ~/.config/efr/config.toml --suffix x", Effect::Deny),
        ("rm ~/dotfiles/efr/config.toml", Effect::Deny),
        ("mv x ~/.config/efr/projects.toml", Effect::Deny),
        ("echo x > ~/.config/efr/config.toml", Effect::Deny),
        // Through the link in the project, and through the link in the config.
        ("echo x > cfg/config.toml", Effect::Deny),
        ("tee cfg/new.toml", Effect::Deny),
        // A hard link writes the same file under a new name.
        ("ln ~/.config/efr/config.toml x", Effect::Deny),
        ("cp -l ~/.config/efr/config.toml x", Effect::Deny),
        // A link that the same line makes cannot be resolved before the line runs.
        ("ln -s ~/.config/efr c && cp notes c/config.toml", Effect::Ask),
        // Reading stays free.
        ("cat ~/.config/efr/config.toml", Effect::Allow),
        ("cat cfg/config.toml", Effect::Allow),
    ] {
        let shell_call = call("shell", json!({ "command": command }), &cwd);
        for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
            let input = DecisionInput {
                requirements: toolbox.requirements(&shell_call).await.unwrap(),
                scope: Scope::Machine,
                origin: Origin::Shell,
                mode,
                conversation_policy: ConversationPolicy::new(home.join("scratch")),
            };
            let effect = engine.decide(&input).effect();
            let expected = match (expected, mode) {
                (Effect::Allow, Mode::Manual) => Effect::Ask,
                // NOTE: auto runs every line in its sandbox.
                (Effect::Allow, Mode::Auto) => Effect::Contain,
                _ => expected,
            };
            assert_eq!(effect, expected, "{command:?} in {mode}");
        }
    }
}

#[tokio::test]
async fn the_settings_tool_asks_while_write_file_and_the_shell_stay_denied_on_the_same_file() {
    let home = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    let config = home.join(".config/efr");
    let dotfiles = home.join("dotfiles/efr");
    for dir in [&config, &dotfiles] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(dotfiles.join("config.toml"), "[model]\nname = \"gpt-5.5\"\n").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("config.toml"), config.join("config.toml")).unwrap();
    let toolbox = toolbox(&home);
    // NOTE: a user rule that allows every write changes none of the three answers.
    let mut settings = Settings::default();
    settings.permissions.rules =
        efr_permissions::Policy::new(vec![Rule::new(Action::Write, Resource::Any, Effect::Allow)])
            .unwrap();
    let engine = crate::engine::build(
        &Home::new(&home).unwrap(),
        &home.join(".local/share/efr/secrets"),
        &crate::engine::protected_config(&config),
        &settings,
        &efr_scope::Registry::empty(),
        &crate::engine::SandboxFacts::default(),
    )
    .unwrap();
    let calls = [
        (
            call("write_file", json!({"path": "~/.config/efr/config.toml", "content": "x"}), &home),
            Effect::Deny,
        ),
        (
            call(
                "write_file",
                json!({"path": "~/dotfiles/efr/config.toml", "content": "x"}),
                &home,
            ),
            Effect::Deny,
        ),
        (
            call("shell", json!({"command": "echo x > ~/.config/efr/config.toml"}), &home),
            Effect::Deny,
        ),
        (call("shell", json!({"command": "cp x ~/dotfiles/efr/config.toml"}), &home), Effect::Deny),
        (
            call(
                "settings",
                json!({"operation": "set", "key": "model.name", "value": "gpt-6-sol"}),
                &home,
            ),
            Effect::Ask,
        ),
    ];

    for (tool_call, expected) in &calls {
        let requirements = toolbox.requirements(tool_call).await.unwrap();
        for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
            let input = DecisionInput {
                requirements: requirements.clone(),
                scope: Scope::Machine,
                origin: Origin::Shell,
                mode,
                conversation_policy: ConversationPolicy::new(home.join("scratch")),
            };
            let effect = engine.decide(&input).effect();
            assert_eq!(effect, *expected, "{} {} in {mode}", tool_call.name, tool_call.input);
        }
    }
}

#[tokio::test]
async fn in_auto_a_copy_or_a_link_out_of_the_project_asks_wherever_its_options_stand() {
    let home = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    let app = home.join("p/app");
    std::fs::create_dir_all(&app).unwrap();
    let id = efr_protocol::ProjectId::from_uuid(uuid::Uuid::from_u128(7));
    let mut projects = efr_scope::Registry::empty();
    projects.register(id, &app, None).unwrap();
    let toolbox = toolbox(&home);
    let engine = crate::engine::build(
        &Home::new(&home).unwrap(),
        &home.join(".local/share/efr/secrets"),
        &[],
        &Settings::default(),
        &projects,
        &crate::engine::SandboxFacts::default(),
    )
    .unwrap();

    for (command, expected) in [
        ("cp x y", Effect::Contain),
        ("cp x y -v", Effect::Contain),
        ("cp x ~/.bashrc", Effect::Ask),
        ("cp x ~/.bashrc --suffix y", Effect::Ask),
        ("cp x ~/.bashrc -S y", Effect::Ask),
        ("ln -sf x ~/.zshrc -S y", Effect::Ask),
        ("ln -sf x ~/.zshrc --suffix y", Effect::Ask),
    ] {
        let shell_call = call("shell", json!({ "command": command }), &app);
        let input = DecisionInput {
            requirements: toolbox.requirements(&shell_call).await.unwrap(),
            scope: Scope::Project(id),
            origin: Origin::Shell,
            mode: Mode::Auto,
            conversation_policy: ConversationPolicy::new(home.join("scratch")),
        };
        assert_eq!(engine.decide(&input).effect(), expected, "{command:?}");
    }
}

/// What the check point decides for a shell call with `command`, from the shell in
/// `~/p/app` (or the hidden shell's `shell_cwd` below the home directory), with the
/// built-in rules followed by `rules`, as the daemon composes them.
async fn shell_decision(
    command: &str,
    shell_cwd: Option<&str>,
    rules: Vec<Rule>,
    origin: Origin,
) -> Effect {
    let home = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    let cwd = home.join("p/app");
    let toolbox = toolbox(&home);
    let mut shell_call = call("shell", json!({ "command": command }), &cwd);
    shell_call.context = shell_call
        .context
        .with_shell_cwd(shell_cwd.map(|below| home.join(below)))
        .with_origin(origin);
    let requirements = toolbox.requirements(&shell_call).await.unwrap();
    let mut permissions = Settings::default().permissions;
    permissions.rules = efr_permissions::Policy::new(rules).unwrap();
    let engine = Engine::with_rules(Locations::new(&home).unwrap(), permissions.rules);
    let input = DecisionInput {
        requirements,
        scope: Scope::Machine,
        origin,
        mode: Mode::Cautious,
        conversation_policy: ConversationPolicy::new(home.join(".local/share/efr/scratch/x")),
    };
    engine.decide(&input).effect()
}

#[tokio::test]
async fn shell_calls_decide_by_the_command_and_the_paths_it_names() {
    let cases = [
        // Read-only commands run.
        ("ls -la", None, Effect::Allow),
        ("cat README.md", None, Effect::Allow),
        ("git status && git diff --stat", None, Effect::Allow),
        ("rg -n TODO src", None, Effect::Allow),
        ("rg TODO", None, Effect::Allow),
        ("find . -name '*.rs' | wc -l", None, Effect::Allow),
        ("pacman -Qi zsh", None, Effect::Allow),
        ("systemctl status nginx --no-pager", None, Effect::Allow),
        ("journalctl -u nginx -n 20 --no-pager", None, Effect::Allow),
        ("cat /etc/os-release", None, Effect::Allow),
        // Anything else asks.
        ("ls; rm -rf ~", None, Effect::Ask),
        ("cargo test", None, Effect::Ask),
        ("cat $(cat list.txt)", None, Effect::Ask),
        ("sudo cat /etc/hosts", None, Effect::Ask),
        ("pacman -Syu", None, Effect::Ask),
        ("du -sh ~", None, Effect::Ask),
        ("rg TOKEN ~/.aws", None, Effect::Ask),
        ("grep -r token ~/.config", None, Effect::Ask),
        // A secret that the line names is denied, whatever runs.
        ("cat ~/.ssh/id_ed25519", None, Effect::Deny),
        ("head -c 100 ~/.aws/credentials", None, Effect::Deny),
        ("wc -c < ~/.netrc", None, Effect::Deny),
        ("echo $(cat ~/.ssh/id_ed25519)", None, Effect::Deny),
        ("sudo cat /etc/shadow", None, Effect::Deny),
        ("cd ~/.ssh && cat id_ed25519", None, Effect::Deny),
        ("cat /proc/self/environ", None, Effect::Deny),
        ("echo key >> ~/.ssh/authorized_keys", None, Effect::Deny),
        ("cat $HOME/.ssh/id_ed25519", None, Effect::Deny),
        ("git show HEAD:.ssh/id_ed25519", Some(""), Effect::Deny),
        ("sort --files0-from=list.txt", None, Effect::Ask),
        ("systemctl --user show", None, Effect::Ask),
        // Relative paths run from where the hidden shell is.
        ("cat id_ed25519", Some(".ssh"), Effect::Deny),
        ("ls", Some(".ssh"), Effect::Deny),
        ("cat notes.txt", Some("p/app"), Effect::Allow),
    ];
    for (command, shell_cwd, expected) in cases {
        let effect = shell_decision(command, shell_cwd, Vec::new(), Origin::Shell).await;
        assert_eq!(effect, expected, "{command:?} from {shell_cwd:?}");
    }
}

#[tokio::test]
async fn the_users_rules_come_after_the_built_in_ones() {
    let cargo_test = Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("cargo").with_args(["test"])),
        Effect::Allow,
    );
    let deny_cat =
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("cat")), Effect::Deny);
    let open_ssh_config =
        Rule::new(Action::Read, Resource::Under("~/.ssh/config".into()), Effect::Allow);
    let rules = vec![cargo_test, deny_cat, open_ssh_config];
    let decide = async |line: &str| shell_decision(line, None, rules.clone(), Origin::Shell).await;

    assert_eq!(decide("cargo test --workspace").await, Effect::Allow);
    assert_eq!(decide("cargo build").await, Effect::Ask);
    assert_eq!(decide("cat README.md").await, Effect::Deny);
    assert_eq!(decide("ls ~/.ssh/config").await, Effect::Allow);
    assert_eq!(decide("ls ~/.ssh/id_ed25519").await, Effect::Deny);
    assert_eq!(
        shell_decision("cargo test", None, rules.clone(), Origin::Phone).await,
        Effect::Ask,
        "the phone asks even when a rule allows"
    );
}

#[tokio::test]
async fn a_rule_may_allow_a_command_in_one_project_only() {
    // `shell_decision` runs from `~/p/app` in a home of its own.
    let rule = Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("cargo").with_args(["test"]).with_under("~/p/app")),
        Effect::Allow,
    );
    let decide = async |line: &str, shell_cwd: Option<&str>| {
        shell_decision(line, shell_cwd, vec![rule.clone()], Origin::Shell).await
    };

    assert_eq!(decide("cargo test", None).await, Effect::Allow, "the shell starts in ~/p/app");
    assert_eq!(decide("cargo test", Some("p/app/crates/x")).await, Effect::Allow);
    assert_eq!(decide("cargo test", Some("p/other")).await, Effect::Ask);
    assert_eq!(decide("cd ../other && cargo test", None).await, Effect::Ask);
}

/// The rules of each `toml` example in `docs/permissions.md`, as the config reads them.
fn documented_rules() -> Vec<Vec<Rule>> {
    let doc = include_str!("../../../../docs/permissions.md");
    doc.split("```toml\n")
        .skip(1)
        .map(|block| {
            let text = block.split("```").next().unwrap_or_default();
            let config =
                Settings::parse(Path::new("/home/u/.config/efr/config.toml"), Some(text)).unwrap();
            config.permissions.rules.rules().to_vec()
        })
        .collect()
}

#[tokio::test]
async fn the_examples_in_the_permissions_doc_do_what_it_says() {
    let [cargo_test, restart_nginx] = documented_rules().try_into().unwrap();
    let cargo = async |line: &str, shell_cwd: Option<&str>| {
        shell_decision(line, shell_cwd, cargo_test.clone(), Origin::Shell).await
    };
    assert_eq!(cargo("cargo test", None).await, Effect::Allow);
    assert_eq!(cargo("cargo test --workspace", Some("p/app/crates/x")).await, Effect::Allow);
    assert_eq!(cargo("cargo test", Some("p/other")).await, Effect::Ask);
    assert_eq!(cargo("cd ../other && cargo test", None).await, Effect::Ask);

    let nginx =
        async |line: &str| shell_decision(line, None, restart_nginx.clone(), Origin::Shell).await;
    assert_eq!(nginx("systemctl restart nginx").await, Effect::Allow);
    assert_eq!(nginx("systemctl restart nginx sshd").await, Effect::Ask);
    assert_eq!(nginx("sudo systemctl restart nginx").await, Effect::Ask);
}

#[test]
fn a_tool_result_keeps_its_flags_and_exit_code() {
    let result =
        ToolResult::ok("partial").with_error(true).with_truncated(true).with_exit_code(Some(2));

    let outcome = outcome(result);

    assert_eq!(outcome.output, "partial");
    assert!(outcome.is_error);
    assert!(outcome.truncated);
    assert_eq!(outcome.exit_code, Some(2));
}

#[test]
fn the_model_reads_an_error_with_its_causes() {
    let error = ToolError::Read {
        path: PathBuf::from("/root/x"),
        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    };

    let text = for_model(&error);

    assert!(text.starts_with(&error.to_string()), "{text}");
    assert!(text.contains("permission denied"), "{text}");
}

#[test]
fn a_call_s_sink_passes_output_and_waits_on_and_asks_the_live_subscriptions() {
    use efr_protocol::InputWait;
    use efr_tools::ToolOutputSink as _;
    use efr_transport::ConnId;

    use crate::connections::HelloInfo;
    use crate::tools::CallSink;

    struct Seen {
        output: Vec<(String, u64)>,
        inputs: Vec<(InputWait, bool)>,
    }
    impl efr_conversation::OutputSink for Seen {
        fn update(&mut self, tail: &str, bytes: u64) {
            self.output.push((tail.to_owned(), bytes));
        }
        fn input_changed(&mut self, wait: InputWait, looks_secret: bool) {
            self.inputs.push((wait, looks_secret));
        }
    }

    let conversation_id = ConversationId::from_uuid(uuid::Uuid::from_u128(1));
    let connections = Arc::new(Connections::default());
    let mut seen = Seen { output: Vec::new(), inputs: Vec::new() };
    {
        let mut sink = CallSink { out: &mut seen, connections: &connections, conversation_id };
        sink.update("pw: ", 4);
        sink.input_changed(InputWait::Hidden, false);
        sink.input_changed(InputWait::Visible, true);
        assert!(!sink.can_answer_hidden(), "nobody follows the conversation");
        assert!(!sink.can_answer());

        let hello = HelloInfo { surface: Origin::Shell, tty: None, client: None };
        connections.opened(ConnId::new(1), hello);
        let viewer = connections.subscribe(ConnId::new(1), conversation_id, false);
        assert!(!sink.can_answer_hidden(), "a viewer cannot type an answer");
        assert!(!sink.can_answer(), "a viewer keeps no call past its timeout");
        let answerer = connections.subscribe(ConnId::new(1), conversation_id, true);
        assert!(sink.can_answer_hidden());
        assert!(sink.can_answer());
        drop(answerer);
        assert!(!sink.can_answer_hidden(), "the answering client went away");
        assert!(!sink.can_answer());
        drop(viewer);
    }
    assert_eq!(seen.output, [("pw: ".to_owned(), 4)]);
    assert_eq!(seen.inputs, [(InputWait::Hidden, false), (InputWait::Visible, true)]);
}

#[tokio::test]
async fn file_tool_write_has_target_kind_facts() {
    use efr_permissions::TargetKind;

    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let home = root.join("home");
    let app = home.join("p/app");
    std::fs::create_dir_all(app.join("sub")).unwrap();
    std::fs::write(app.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(home.join("dotfiles/zsh")).unwrap();
    std::fs::write(home.join("dotfiles/zsh/.zshrc"), "# zsh\n").unwrap();
    std::os::unix::fs::symlink(home.join("dotfiles/zsh/.zshrc"), home.join(".zshrc")).unwrap();
    let clock = TestClock::new();
    let store = efr_test_support::TestStore::open(clock.shared()).await.unwrap();
    let dirs =
        efr_stdx::paths::Dirs::new(root.join("c"), root.join("d"), root.join("s"), root.join("r"))
            .unwrap();
    let sandbox = crate::sandbox::SandboxService::new(crate::sandbox::ServiceParts {
        dirs: dirs.clone(),
        home: Home::new(&home).unwrap(),
        host: crate::sandbox::HostFacts::default(),
        source: None,
        seams: crate::sandbox::Seams::default(),
        registry: dirs.config().join("projects.toml"),
        writer: store.writer().clone(),
        clock: clock.shared(),
        rng: Arc::new(TestRng::new(3)),
        git: efr_scope::Git::new(clock.shared()).isolated(),
    })
    .await;
    let engine = Engine::with_defaults(Locations::new(&home).unwrap());
    let (_engine, engine) = tokio::sync::watch::channel(Arc::new(engine));
    let toolbox = toolbox(&home).with_sandbox(sandbox, engine);
    let facts = async |name: &str, input: serde_json::Value, auto: bool| {
        let mut tool_call = call(name, input, &app);
        tool_call.context = tool_call.context.with_auto(auto).with_edit_tool(if name == "edit" {
            EditTool::Replace
        } else {
            EditTool::default()
        });
        toolbox.requirements(&tool_call).await.unwrap().facts
    };

    let new_file = facts("write_file", json!({"path": "new.txt", "content": "x"}), true).await;
    let edited =
        facts("edit", json!({"path": "main.rs", "old_string": "main", "new_string": "run"}), true)
            .await;
    let dir = facts("write_file", json!({"path": "sub", "content": "x"}), true).await;
    let cautious = facts("write_file", json!({"path": "new.txt", "content": "x"}), false).await;
    let zshrc = home.join(".zshrc").display().to_string();
    let linked = facts("write_file", json!({"path": zshrc, "content": "x"}), true).await;

    let linked = linked.unwrap();
    let real = home.join("dotfiles/zsh/.zshrc");
    assert_eq!(linked.links, [(home.join(".zshrc"), real.clone())], "the real file of the link");
    assert_eq!(linked.target(&real), Some(Some(TargetKind::File)));
    let new_file = new_file.unwrap();
    assert_eq!(new_file.target(&app.join("new.txt")), Some(None), "a new file is missing");
    assert_eq!(new_file.target(&app), Some(Some(TargetKind::Dir)), "under a directory");
    assert_eq!(edited.unwrap().target(&app.join("main.rs")), Some(Some(TargetKind::File)));
    assert_eq!(dir.unwrap().target(&app.join("sub")), Some(Some(TargetKind::Dir)));
    assert_eq!(cautious, None, "only auto reads the facts");
}

/// A toolbox with a sandbox service in `root`, whose home is `root/home`, and the
/// default engine of that home.
async fn sandboxed(root: &Path, settings: Settings) -> DaemonToolbox {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let clock = TestClock::new();
    let store = efr_test_support::TestStore::open(clock.shared()).await.unwrap();
    let dirs =
        efr_stdx::paths::Dirs::new(root.join("c"), root.join("d"), root.join("s"), root.join("r"))
            .unwrap();
    let sandbox = crate::sandbox::SandboxService::new(crate::sandbox::ServiceParts {
        dirs: dirs.clone(),
        home: Home::new(&home).unwrap(),
        host: crate::sandbox::HostFacts::default(),
        source: None,
        seams: crate::sandbox::Seams::default(),
        registry: dirs.config().join("projects.toml"),
        writer: store.writer().clone(),
        clock: clock.shared(),
        rng: Arc::new(TestRng::new(3)),
        git: efr_scope::Git::new(clock.shared()).isolated(),
    })
    .await;
    let engine = Engine::with_defaults(Locations::new(&home).unwrap());
    let (_engine, engine) = tokio::sync::watch::channel(Arc::new(engine));
    let (toolbox, _settings) = toolbox_with(&home, settings);
    toolbox.with_sandbox(sandbox, engine)
}

/// A temp dir and its path with every link resolved.
pub(super) fn canonical_temp() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

#[tokio::test]
async fn delete_cache_paths_frees_real_space() {
    use std::os::unix::fs::PermissionsExt as _;

    let (_temp, root) = canonical_temp();
    let toolbox = sandboxed(&root, Settings::default()).await;
    let cache = root.join("home/.cache");
    let build = cache.join("go-build");
    std::fs::create_dir_all(build.join("aa")).unwrap();
    std::fs::write(build.join("aa/one"), vec![7_u8; 64 * 1024]).unwrap();
    std::fs::write(cache.join("pip-wheel"), vec![7_u8; 64 * 1024]).unwrap();
    // Go's module cache makes its dirs and files read-only.
    let module = root.join("home/go/pkg/mod/example.com/m@v1");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("go.mod"), "module m\n").unwrap();
    for dir in [module.as_path(), module.parent().unwrap()] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    std::fs::write(cache.join("kept"), "x").unwrap();
    let call = call("shell", json!({"command": "go clean -cache"}), &root);
    let paths = [
        build.clone(),
        cache.join("pip-wheel"),
        root.join("home/go/pkg/mod/example.com"),
        cache.join("already-gone"),
    ];

    let outcome = toolbox.delete_cache_paths(&call.context, &paths).await.unwrap();

    assert_eq!(outcome.deleted, paths[..3]);
    assert!(outcome.freed_bytes >= 128 * 1024, "{}", outcome.freed_bytes);
    assert!(!build.exists() && !cache.join("pip-wheel").exists());
    assert!(!root.join("home/go/pkg/mod/example.com").exists());
    assert!(cache.join("kept").exists());
}

#[tokio::test]
async fn cache_delete_never_below_pins() {
    let (_temp, root) = canonical_temp();
    let toolbox = sandboxed(&root, Settings::default()).await;
    let cargo = root.join("home/.cargo");
    std::fs::create_dir_all(cargo.join("bin")).unwrap();
    std::fs::create_dir_all(cargo.join("registry/cache")).unwrap();
    std::fs::write(cargo.join("bin/x"), "#!/bin/sh\n").unwrap();
    std::fs::write(cargo.join("registry/cache/a.crate"), "x").unwrap();
    std::fs::create_dir_all(root.join("home/Documents")).unwrap();
    std::fs::write(root.join("home/Documents/thesis.tex"), "x").unwrap();
    let call = call("shell", json!({"command": "cargo cache --autoclean"}), &root);

    for refused in [
        vec![cargo.join("registry/cache/a.crate"), cargo.join("bin/x")],
        vec![cargo.join("bin")],
        vec![root.join("home/Documents/thesis.tex")],
    ] {
        let result = toolbox.delete_cache_paths(&call.context, &refused).await;
        assert!(result.unwrap_err().contains("stayed private"), "{refused:?}");
    }

    assert!(cargo.join("bin/x").exists());
    assert!(cargo.join("registry/cache/a.crate").exists(), "one bad path refuses the list");
    assert!(root.join("home/Documents/thesis.tex").exists());
}

#[tokio::test]
async fn a_cache_delete_never_follows_a_link_out_of_its_cache() {
    let (_temp, root) = canonical_temp();
    let toolbox = sandboxed(&root, Settings::default()).await;
    let cache = root.join("home/.cache");
    std::fs::create_dir_all(&cache).unwrap();
    let outside = root.join("home/work");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("notes"), "mine").unwrap();
    std::os::unix::fs::symlink(&outside, cache.join("work")).unwrap();
    let call = call("shell", json!({"command": "rm -rf ~/.cache/work/notes"}), &root);

    let through = toolbox.delete_cache_paths(&call.context, &[cache.join("work/notes")]).await;
    let link = toolbox.delete_cache_paths(&call.context, &[cache.join("work")]).await.unwrap();

    assert!(through.is_err(), "{through:?}");
    assert_eq!(link.deleted, [cache.join("work")], "the link itself goes");
    assert_eq!(std::fs::read_to_string(outside.join("notes")).unwrap(), "mine");
}

/// An approved `persistence` exit for `target` whose file efrd copies first.
pub(super) fn floor_edit(target: &Path) -> efr_permissions::ExitNeed {
    let mut need = home_write(target);
    need.kind = efr_protocol::ExitKind::Persistence;
    need.approver = efr_permissions::Approver::ClassifierWhenNamed;
    need
}

/// An approved `write` exit for the home file `target` whose file efrd copies first.
pub(super) fn home_write(target: &Path) -> efr_permissions::ExitNeed {
    efr_permissions::ExitNeed {
        kind: efr_protocol::ExitKind::Write,
        grants: Vec::new(),
        part: format!("write_file {}", target.display()),
        approver: efr_permissions::Approver::Classifier,
        names: Vec::new(),
        hosts: Vec::new(),
        backup_before_write: true,
        package_fetch: None,
        source: efr_protocol::ExitSource::Predicted,
        target: Some(target.to_path_buf()),
        bind: None,
        link: None,
        copies: vec![efr_permissions::CoverAsk::Home(target.to_path_buf())],
        covered: Some(true),
        installs: Vec::new(),
    }
}

/// A toolbox with the sandbox service, the snapshot store and the undo trash, all below
/// `root`, whose home is `root/home`.
pub(super) struct Kept {
    pub(super) toolbox: DaemonToolbox,
    pub(super) home: PathBuf,
    pub(super) store: efr_snapshot::Snapshots,
    pub(super) settings: tokio::sync::watch::Sender<Arc<Settings>>,
}

pub(super) async fn kept(root: &Path, settings: Settings) -> Kept {
    kept_in(root, &root.join("d"), settings).await
}

/// [`kept`] with efr's data root at `data`.
pub(super) async fn kept_in(root: &Path, data: &Path, settings: Settings) -> Kept {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let clock = TestClock::new();
    let store = efr_test_support::TestStore::open(clock.shared()).await.unwrap();
    let dirs =
        efr_stdx::paths::Dirs::new(root.join("c"), data, root.join("s"), root.join("r")).unwrap();
    let sandbox = crate::sandbox::SandboxService::new(crate::sandbox::ServiceParts {
        dirs: dirs.clone(),
        home: Home::new(&home).unwrap(),
        host: crate::sandbox::HostFacts::default(),
        source: None,
        seams: crate::sandbox::Seams::default(),
        registry: dirs.config().join("projects.toml"),
        writer: store.writer().clone(),
        clock: clock.shared(),
        rng: Arc::new(TestRng::new(3)),
        git: efr_scope::Git::new(clock.shared()).isolated(),
    })
    .await;
    let engine = Engine::with_defaults(Locations::new(&home).unwrap());
    let (_engine, engine) = tokio::sync::watch::channel(Arc::new(engine));
    let (toolbox, sender) = toolbox_with(&home, settings);
    let snapshots = efr_snapshot::Snapshots::new(efr_snapshot::SnapshotParts {
        dir: data.join("snapshots"),
        git: efr_scope::Git::new(clock.shared()).isolated(),
        home: Home::new(&home).unwrap(),
        clock: clock.shared(),
        excludes_file: None,
        timeout: efr_snapshot::DEFAULT_SNAPSHOT_TIMEOUT,
    });
    let call_snapshots = crate::tools::CallSnapshots::new(
        snapshots.clone(),
        engine.clone(),
        sender.subscribe(),
        Home::new(&home).unwrap(),
    )
    .with_state(root.join("s"));
    let toolbox = toolbox
        .with_sandbox(sandbox, engine)
        .with_snapshots(call_snapshots)
        .with_undo_trash(data.join(crate::home::UNDO_TRASH_DIR))
        .with_efr_roots(vec![root.join("c"), data.to_path_buf(), root.join("s"), root.join("r")]);
    Kept { toolbox, home, store: snapshots, settings: sender }
}

/// A call of the turn `turn` (call `n`) of conversation 1, in `auto`.
pub(super) fn call_of(
    turn: u128,
    n: u128,
    name: &str,
    input: serde_json::Value,
    cwd: &Path,
) -> ToolCall {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let context = CallContext::new(
        ConversationId::from_uuid(id(1)),
        TurnId::from_uuid(id(turn)),
        CallId::from_uuid(id(turn * 100 + n)),
        cwd,
        cwd.join("scratch"),
    )
    .with_auto(true);
    ToolCall::new(name, input, context)
}

/// Runs `write_file` of `content` to `path` with the approved exits `exits`.
async fn write_with(
    toolbox: &DaemonToolbox,
    home: &Path,
    path: &Path,
    content: &str,
    exits: Vec<efr_permissions::ExitNeed>,
) -> efr_conversation::ToolOutcome {
    let mut write = call("write_file", json!({"path": path, "content": content}), home);
    write.context = write.context.with_exits(exits);
    let mut sink = |_: &str, _: u64| {};
    toolbox.invoke(write, &mut sink).await
}

/// The bytes of each home path in the home store from before the turn 2 of
/// conversation 1, as the undo of the turn would write them.
async fn copied(kept: &Kept) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let plan = kept
        .store
        .plan_undo(
            ConversationId::from_uuid(id(1)),
            TurnId::from_uuid(id(2)),
            &[],
            false,
            "u-0192f8a0-0000-7000-8000-000000000001",
            efr_snapshot::Limits::default(),
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    for step in plan.home.steps {
        let bytes = match &step.blob {
            Some(blob) => Some(kept.store.home_blob(blob).await.unwrap()),
            None => None,
        };
        out.push((step.path, bytes));
    }
    out
}

#[tokio::test]
async fn home_copy_before_write_file_outside_roots() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let notes = kept.home.join("Documents/notes.md");
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(&notes, "old notes\n").unwrap();

    let outcome =
        write_with(&kept.toolbox, &kept.home, &notes, "new notes\n", vec![home_write(&notes)])
            .await;

    assert!(!outcome.is_error, "{outcome:?}");
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "new notes\n");
    assert_eq!(copied(&kept).await, [(notes.clone(), Some(b"old notes\n".to_vec()))]);
    assert_eq!(
        outcome.backups,
        [efr_conversation::FileBackup {
            path: notes,
            backup: PathBuf::new(),
            floor: false,
            real: None,
            reason: None
        }]
    );
    assert!(!root.join("d/file-backups").exists(), "the phase A store gets nothing");
}

#[tokio::test]
async fn home_copy_names_the_link_of_a_floor_file() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let home = &kept.home;
    let real = home.join("dotfiles/zsh/.zshrc");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "old\n").unwrap();
    let zshrc = home.join(".zshrc");
    std::os::unix::fs::symlink(&real, &zshrc).unwrap();
    let mut need = floor_edit(&real);
    need.link = Some(zshrc.clone());

    // write_file refuses a path through a link, so the model writes the real file.
    let outcome = write_with(&kept.toolbox, home, &real, "new\n", vec![need]).await;

    assert!(!outcome.is_error, "{outcome:?}");
    let [backup] = outcome.backups.try_into().unwrap();
    assert_eq!((backup.path, backup.real, backup.floor), (zshrc.clone(), Some(real.clone()), true));
    assert_eq!(copied(&kept).await, [(real, Some(b"old\n".to_vec()))], "the link did not change");
    assert!(std::fs::symlink_metadata(&zshrc).unwrap().file_type().is_symlink());
}

#[tokio::test]
async fn home_copy_of_a_missing_file_and_an_unchanged_one() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let conf = kept.home.join(".config/hypr/monitors.conf");
    std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
    let gitconfig = kept.home.join(".gitconfig");
    std::fs::write(&gitconfig, "[user]\n").unwrap();

    let made =
        write_with(&kept.toolbox, &kept.home, &conf, "monitor=\n", vec![floor_edit(&conf)]).await;
    let same =
        write_with(&kept.toolbox, &kept.home, &gitconfig, "[user]\n", vec![floor_edit(&gitconfig)])
            .await;

    assert_eq!(made.backups.len(), 1, "{made:?}");
    assert_eq!(made.backups[0].reason, None);
    assert_eq!(same.backups, [], "an unchanged file is no change");
    assert_eq!(copied(&kept).await, [(conf, None)], "missing before, so undo trashes it");
}

#[tokio::test]
async fn home_copy_failure_stops_the_call() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let zshrc = kept.home.join(".zshrc");
    std::fs::write(&zshrc, "old\n").unwrap();
    // Something that is not a directory stands where the store goes.
    std::fs::create_dir_all(root.join("d")).unwrap();
    std::fs::write(root.join("d/snapshots"), "not a dir").unwrap();

    let outcome =
        write_with(&kept.toolbox, &kept.home, &zshrc, "new\n", vec![floor_edit(&zshrc)]).await;
    let without = toolbox(&kept.home);
    let no_store =
        write_with(&without, &kept.home, &zshrc, "new\n", vec![floor_edit(&zshrc)]).await;

    for outcome in [outcome, no_store] {
        assert!(outcome.is_error, "{outcome:?}");
        assert!(outcome.output.contains("did not run"), "{}", outcome.output);
    }
    assert_eq!(std::fs::read_to_string(&zshrc).unwrap(), "old\n", "the edit did not run");
}

#[tokio::test]
async fn home_copy_of_a_unit_or_cron_change_stops_without_its_copy() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let notes = kept.home.join("notes.md");
    let named = |ask: efr_permissions::CoverAsk| {
        let mut need = floor_edit(&notes);
        need.copies = vec![ask];
        need
    };

    // efrd finds no file of the unit, so it cannot tell what enabling it writes.
    let unit = named(efr_permissions::CoverAsk::UnitLinks("absent.service".to_owned()));
    let unit = write_with(&kept.toolbox, &kept.home, &notes, "x\n", vec![unit]).await;
    // This efrd found no trusted crontab.
    let cron = named(efr_permissions::CoverAsk::CronTable);
    let cron = write_with(&kept.toolbox, &kept.home, &notes, "x\n", vec![cron]).await;

    assert!(unit.is_error && unit.output.contains("absent.service"), "{unit:?}");
    assert!(cron.is_error && cron.output.contains("@crontab"), "{cron:?}");
    assert!(!notes.exists(), "neither call ran");
}

#[tokio::test]
async fn home_copy_of_a_named_unit_change_holds_the_unit_dir() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let dir = kept.home.join(".config/systemd/user");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("agent.service"), "[Install]\nWantedBy=default.target\n").unwrap();
    let notes = kept.home.join("notes.md");
    let mut need = floor_edit(&notes);
    need.copies = vec![efr_permissions::CoverAsk::UnitLinks("agent.service".to_owned())];

    let outcome = write_with(&kept.toolbox, &kept.home, &notes, "x\n", vec![need]).await;

    assert!(!outcome.is_error, "{outcome:?}");
    let paths: Vec<PathBuf> = copied(&kept).await.into_iter().map(|(path, _)| path).collect();
    assert!(paths.is_empty(), "nothing in the unit dir changed: {paths:?}");
}

#[tokio::test]
async fn home_copy_never_copies_a_masked_floor() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let home = &kept.home;
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::create_dir_all(home.join(".aws")).unwrap();
    std::fs::write(home.join(".ssh/config"), "Host *\n").unwrap();
    std::fs::write(home.join(".aws/config"), "[default]\n").unwrap();

    let secret = home.join(".ssh/config");
    let floor = write_with(&kept.toolbox, home, &secret, "x\n", vec![floor_edit(&secret)]).await;
    let masked = home.join(".aws/config");
    let write = write_with(&kept.toolbox, home, &masked, "y\n", vec![home_write(&masked)]).await;

    assert!(floor.is_error && floor.output.contains("secret or a masked file"), "{floor:?}");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "Host *\n");
    assert!(!write.is_error, "a masked write runs with no copy: {write:?}");
    let [backup] = write.backups.try_into().unwrap();
    assert_eq!(backup.reason, Some(efr_protocol::UndoReason::Masked));
}

#[tokio::test]
async fn facts_answer_cover_asks_for_root_and_home() {
    use efr_permissions::{CoverAsk, Coverage};

    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let home = &kept.home;
    let work = home.join("work");
    let scratch = work.join("scratch");
    std::fs::create_dir_all(scratch.join("build")).unwrap();
    std::fs::write(scratch.join("build/out.txt"), "out\n").unwrap();
    std::fs::write(home.join("notes.md"), "notes\n").unwrap();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id"), "KEY\n").unwrap();
    let call = call_of(2, 1, "shell", json!({"command": "rm -r build"}), &work);
    let asks = [
        CoverAsk::Path(scratch.join("build")),
        CoverAsk::Path(scratch.join("never-made")),
        CoverAsk::Path(scratch.clone()),
        CoverAsk::Path(home.join("elsewhere")),
        CoverAsk::Home(home.join("notes.md")),
        CoverAsk::Home(home.join(".ssh/id")),
        CoverAsk::Home(scratch.join("build/out.txt")),
    ];

    let mut answers = kept.toolbox.cover_facts(&call.context, &asks, Vec::new()).await;
    answers.sort_by_key(|(ask, _)| format!("{ask:?}"));

    let mut expected = vec![
        (asks[0].clone(), Coverage::Covered),
        (asks[1].clone(), Coverage::Covered),
        (asks[2].clone(), Coverage::NotAllowed),
        (asks[3].clone(), Coverage::NotAllowed),
        (asks[4].clone(), Coverage::Covered),
        (asks[5].clone(), Coverage::Masked),
    ];
    expected.sort_by_key(|(ask, _)| format!("{ask:?}"));
    assert_eq!(answers, expected, "a home ask inside a root gets no answer");
}

#[tokio::test]
async fn covered_line_stops_when_coverage_changes_under_the_lock() {
    use efr_permissions::CoverAsk;

    let (_temp, root) = canonical_temp();
    let mut settings = Settings::default();
    settings.snapshot.max_file_mib = 1;
    let kept = kept(&root, settings).await;
    let work = kept.home.join("work");
    let scratch = work.join("scratch");
    std::fs::create_dir_all(scratch.join("build")).unwrap();
    std::fs::write(scratch.join("build/out.txt"), "out\n").unwrap();
    let call = call_of(2, 1, "shell", json!({"command": "rm -r build"}), &work);
    let ask = CoverAsk::Path(scratch.join("build"));

    let answers =
        kept.toolbox.cover_facts(&call.context, std::slice::from_ref(&ask), Vec::new()).await;
    assert!(answers[0].1.is_covered(), "{answers:?}");
    std::fs::write(scratch.join("build/big.bin"), vec![1_u8; 2 * 1024 * 1024]).unwrap();
    let stop = kept.toolbox.check_covered(&call.context, &[]).await;

    assert_eq!(stop.as_deref(), Some(crate::tools::COVERAGE_CHANGED));
    let again = kept.toolbox.check_covered(&call.context, &[]).await;
    assert_eq!(again, None, "the answers go with the check");
}

#[tokio::test]
async fn covered_line_names_the_root_of_its_target_and_keeps_its_answers() {
    use efr_permissions::CoverAsk;

    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let work = kept.home.join("work");
    let scratch = work.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("db.sqlite"), "data\n").unwrap();
    let call = call_of(2, 1, "shell", json!({"command": "dd of=scratch/db.sqlite"}), &work);
    let asks = [CoverAsk::Path(scratch.join("db.sqlite")), CoverAsk::Home(work.join("x"))];

    let answers = kept.toolbox.cover_facts(&call.context, &asks, Vec::new()).await;
    assert!(answers[0].1.is_covered(), "{answers:?}");

    // The plan names the root of a covered target, so it locks and writes it.
    assert_eq!(kept.toolbox.covered_root_paths(&call.context), [scratch.join("db.sqlite")]);
    let stop = kept.toolbox.check_covered(&call.context, &[]).await;
    assert_eq!(stop, None, "the answers stay for the check under the lock");
}

#[tokio::test]
async fn covered_line_stops_when_the_capture_fails() {
    use efr_permissions::CoverAsk;

    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let work = kept.home.join("work");
    let scratch = work.join("scratch");
    std::fs::create_dir_all(scratch.join("build")).unwrap();
    std::fs::write(scratch.join("build/out.txt"), "out\n").unwrap();
    let call = call_of(2, 1, "shell", json!({"command": "rm -r build"}), &work);
    let ask = CoverAsk::Path(scratch.join("build"));

    let answers =
        kept.toolbox.cover_facts(&call.context, std::slice::from_ref(&ask), Vec::new()).await;
    assert!(answers[0].1.is_covered(), "{answers:?}");
    std::fs::remove_dir_all(&scratch).unwrap();
    let stop = kept.toolbox.check_covered(&call.context, &[]).await;

    assert_eq!(stop.as_deref(), Some(crate::tools::COVERAGE_CHANGED));
}

#[tokio::test]
async fn turn_end_counts_home_files() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let notes = kept.home.join("notes.md");
    std::fs::write(&notes, "old\n").unwrap();
    let masked = kept.home.join(".aws/config");
    std::fs::create_dir_all(masked.parent().unwrap()).unwrap();
    let run = async |path: &Path, n: u128| {
        let mut write =
            call_of(2, n, "write_file", json!({"path": path, "content": "new\n"}), &kept.home);
        write.context = write.context.with_exits(vec![home_write(path)]);
        let mut sink = |_: &str, _: u64| {};
        kept.toolbox.invoke(write, &mut sink).await
    };

    run(&notes, 1).await;
    run(&masked, 2).await;
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let changes =
        kept.toolbox.turn_changes(ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2))).await;

    let undo = changes.undo.unwrap();
    assert_eq!(undo.home_files, 1);
    assert_eq!(undo.not_covered, ["~/.aws/config (masked)"]);
    assert!(
        changes.snapshots.iter().any(|record| record.root == "home"),
        "{:?}",
        changes.snapshots
    );
}

#[tokio::test]
async fn turn_end_not_undoable_while_a_call_runs() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let work = kept.home.join("work");
    std::fs::create_dir_all(work.join("scratch")).unwrap();
    let file = work.join("scratch/a.txt");
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let turn = async |turn: u128| {
        let write = call_of(turn, 1, "write_file", json!({"path": file, "content": "x\n"}), &work);
        let mut sink = |_: &str, _: u64| {};
        kept.toolbox.invoke(write, &mut sink).await;
        kept.toolbox
            .turn_changes(ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(turn)))
            .await
    };

    let idle = turn(2).await.undo.unwrap();
    let call_dir = root.join("r/sbx").join(ConversationId::from_uuid(id(1)).to_string()).join("c1");
    std::fs::create_dir_all(&call_dir).unwrap();
    std::fs::write(call_dir.join(efr_sandbox::STARTED_FILE), "").unwrap();
    let busy = turn(3).await.undo.unwrap();

    assert!(idle.undoable, "{idle:?}");
    assert_eq!(
        idle.roots,
        [efr_protocol::RootFiles { shown: "$SCRATCH".to_owned(), files: 1, refs: 0 }]
    );
    assert!(!busy.undoable, "{busy:?}");
    assert_eq!(busy.not_covered, ["$SCRATCH (a call still runs)"]);
    assert!(kept.toolbox.call_runs(ConversationId::from_uuid(id(1))));
}

#[tokio::test]
async fn cache_delete_is_copied_under_the_limits() {
    let (_temp, root) = canonical_temp();
    let mut settings = Settings::default();
    settings.snapshot.home_max_files = 3;
    let kept = kept(&root, settings).await;
    let cache = kept.home.join(".cache");
    std::fs::create_dir_all(cache.join("small")).unwrap();
    std::fs::write(cache.join("small/a"), "a\n").unwrap();
    std::fs::create_dir_all(cache.join("big")).unwrap();
    for n in 0..5 {
        std::fs::write(cache.join(format!("big/{n}")), "x\n").unwrap();
    }
    let call = call_of(2, 1, "shell", json!({"command": "rm -rf ~/.cache/small"}), &kept.home);

    let small =
        kept.toolbox.delete_cache_paths(&call.context, &[cache.join("small")]).await.unwrap();
    let big = kept.toolbox.delete_cache_paths(&call.context, &[cache.join("big")]).await.unwrap();

    assert_eq!(small.deleted, [cache.join("small")]);
    assert_eq!(big.deleted, [cache.join("big")]);
    let copies = copied(&kept).await;
    assert!(copies.contains(&(cache.join("small/a"), Some(b"a\n".to_vec()))), "{copies:?}");
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let changes =
        kept.toolbox.turn_changes(ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2))).await;
    let undo = changes.undo.unwrap();
    assert_eq!(undo.home_files, 1, "{undo:?}");
    assert!(
        undo.not_covered.contains(&"~/.cache/big (over the copy limits)".to_owned()),
        "{undo:?}"
    );
}

#[test]
fn cover_facts_read_the_projects_that_the_plan_writes() {
    use efr_config::WriteProjects;

    let home = Path::new("/home/u");
    let app = PathBuf::from("/home/u/p/app");
    let all = vec![app.clone(), PathBuf::from("/home/u")];
    // NOTE: with `turn` no other project is a write root, so a target there is never
    // covered: its line goes to the reviewer at once, not into a refusal loop.
    let turn = crate::tools::cover_projects(all.clone(), WriteProjects::Turn, home);
    assert_eq!(turn, Vec::<PathBuf>::new());
    let named = crate::tools::cover_projects(all.clone(), WriteProjects::Named, home);
    assert_eq!(
        named,
        std::slice::from_ref(&app),
        "a project at the home directory is never a root"
    );
    assert_eq!(crate::tools::cover_projects(all, WriteProjects::All, home), [app]);
}

#[tokio::test]
async fn turn_end_names_a_home_copy_whose_call_did_not_finish() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let notes = kept.home.join("notes.md");
    std::fs::write(&notes, "old\n").unwrap();
    let mut write =
        call_of(2, 1, "write_file", json!({"path": notes, "content": "new\n"}), &kept.home);
    write.context = write.context.with_exits(vec![home_write(&notes)]);

    // NOTE: the copy before the call, then an interrupt drops the call before its copy
    // after.
    let copying = kept.toolbox.copy_before(&write.context).await.unwrap();
    assert!(copying.is_some());
    drop(copying);
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let changes =
        kept.toolbox.turn_changes(ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2))).await;

    let undo = changes.undo.unwrap();
    assert_eq!(undo.not_covered, ["~/notes.md (the call did not finish)"]);
}

#[test]
fn home_copy_masks_the_env_files_of_a_project_it_reaches() {
    let (_temp, root) = canonical_temp();
    let app = root.join("p/app");
    let other = root.join("p/other");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(app.join(".env"), "TOKEN=1\n").unwrap();
    std::fs::write(other.join(".env"), "TOKEN=2\n").unwrap();
    let globs = Settings::default().sandbox.mask_globs;
    let projects = [app.clone(), other.clone()];

    // A write exit into the other project, as with `sandbox.write_projects = "turn"`.
    let into = crate::tools::env_masks(&projects, &[app.join("notes.md")], &globs);
    assert_eq!(into, [app.join(".env")]);
    // A walk of a dir above both projects reaches both.
    let above = crate::tools::env_masks(&projects, &[root.join("p")], &globs);
    assert_eq!(above, [app.join(".env"), other.join(".env")]);
    assert!(crate::tools::env_masks(&projects, &[root.join("elsewhere")], &globs).is_empty());
}
