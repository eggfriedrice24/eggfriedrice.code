use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use efr_conversation::{CallContext, ToolCall, Toolbox as _};
use efr_holder::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyId, PtyInfo, Signal, SignalTarget, Size,
    SpawnSpec,
};
use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, Locations,
    Requirements, Resource, Rule,
};
use efr_protocol::{CallId, ConversationId, Origin, Scope, TurnId};
use efr_scope::Home;
use efr_shell::{ShellConfig, ShellDeps, ShellSessions};
use efr_test_support::{TestClock, TestRng};
use efr_tools::{ToolError, ToolRequirements, ToolResult};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::config::Config;
use crate::screens::ScreenBackend;
use crate::tools::{DaemonToolbox, for_model, outcome, permission_requirements, registry};

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
    DaemonToolbox::new(registry, shells, Home::new(home).unwrap(), clock.shared())
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
fn the_model_is_offered_the_shell_and_the_two_file_tools() {
    let home = tempfile::tempdir().unwrap();
    let definitions = toolbox(home.path()).definitions();

    let names: Vec<&str> = definitions.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["shell", "read_file", "write_file"]);
    assert!(definitions.iter().all(|tool| tool.input_schema["type"] == "object"));
}

#[test]
fn a_read_declares_its_resolved_path_for_reading() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());

    let requirements =
        toolbox.requirements(&call("read_file", json!({"path": "notes.txt"}), home.path()));

    assert_eq!(requirements, Ok(Requirements::none().with_read(home.path().join("notes.txt"))));
}

#[test]
fn a_shell_call_declares_its_command_line() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());

    let requirements =
        toolbox.requirements(&call("shell", json!({"command": "ls -la"}), home.path())).unwrap();

    assert_eq!(requirements.command.as_deref(), Some("ls -la"));
}

#[test]
fn an_unknown_tool_is_text_for_the_model() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());

    let requirements = toolbox.requirements(&call("rm_rf", json!({}), home.path()));

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
        .with_network(true)
        .with_interactive(true);

    assert_eq!(
        permission_requirements(declared),
        Requirements::none()
            .with_read("/etc/hosts")
            .with_read_tree("/var/log")
            .with_write("/tmp/out")
            .with_command("make")
            .with_network()
            .with_interactive()
    );
}

#[test]
fn a_shell_call_resolves_relative_paths_where_the_hidden_shell_is() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    let mut shell_call = call("shell", json!({"command": "cat notes.txt"}), home.path());
    shell_call.context = shell_call.context.with_shell_cwd(Some(PathBuf::from("/var/log")));

    let requirements = toolbox.requirements(&shell_call).unwrap();

    assert_eq!(requirements.paths, Requirements::none().with_read("/var/log/notes.txt").paths);
}

/// What the check point decides for a shell call with `command`, from the shell in
/// `~/p/app` (or the hidden shell's `shell_cwd` below the home directory), with the
/// built-in rules followed by `rules`, as the daemon composes them.
fn shell_decision(
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
    let requirements = toolbox.requirements(&shell_call).unwrap();
    let mut permissions = Config::default().permissions;
    permissions.rules = efr_permissions::Policy::new(rules).unwrap();
    let engine = Engine::new(Locations::new(&home).unwrap(), permissions.policy());
    let input = DecisionInput {
        requirements,
        scope: Scope::Machine,
        origin,
        conversation_policy: ConversationPolicy::new(home.join(".local/share/efr/scratch/x")),
    };
    engine.decide(&input).effect()
}

#[test]
fn shell_calls_decide_by_the_command_and_the_paths_it_names() {
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
        // Relative paths run from where the hidden shell is.
        ("cat id_ed25519", Some(".ssh"), Effect::Deny),
        ("ls", Some(".ssh"), Effect::Deny),
        ("cat notes.txt", Some("p/app"), Effect::Allow),
    ];
    for (command, shell_cwd, expected) in cases {
        let effect = shell_decision(command, shell_cwd, Vec::new(), Origin::Shell);
        assert_eq!(effect, expected, "{command:?} from {shell_cwd:?}");
    }
}

#[test]
fn the_users_rules_come_after_the_built_in_ones() {
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
    let decide = |command: &str| shell_decision(command, None, rules.clone(), Origin::Shell);

    assert_eq!(decide("cargo test --workspace"), Effect::Allow);
    assert_eq!(decide("cargo build"), Effect::Ask);
    assert_eq!(decide("cat README.md"), Effect::Deny);
    assert_eq!(decide("ls ~/.ssh/config"), Effect::Allow);
    assert_eq!(decide("ls ~/.ssh/id_ed25519"), Effect::Deny);
    assert_eq!(
        shell_decision("cargo test", None, rules.clone(), Origin::Phone),
        Effect::Ask,
        "the phone asks even when a rule allows"
    );
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
