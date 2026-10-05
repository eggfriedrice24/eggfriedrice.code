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
use efr_protocol::{CallId, ConversationId, Mode, Origin, Scope, TurnId};
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
    DaemonToolbox, SettingsTool, for_model, outcome, permission_requirements, registry,
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
    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
}

fn toolbox(home: &Path) -> DaemonToolbox {
    toolbox_with(home, Settings::default()).0
}

fn toolbox_with(
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
    let settings_tool =
        SettingsTool::new(&home.join(".config").join("efr"), receiver.clone(), engine, reloads);
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

fn call(name: &str, input: serde_json::Value, cwd: &Path) -> ToolCall {
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
fn the_model_is_offered_the_shell_the_two_file_tools_and_the_settings() {
    let home = tempfile::tempdir().unwrap();
    let definitions = toolbox(home.path()).definitions();

    let names: Vec<&str> = definitions.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["shell", "read_file", "write_file", "settings"]);
    assert!(definitions.iter().all(|tool| tool.input_schema["type"] == "object"));
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
        .with_interactive(true);

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
    );
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
    )
    .unwrap();

    for (command, expected) in [
        ("cp x y", Effect::Allow),
        ("cp x y -v", Effect::Allow),
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
