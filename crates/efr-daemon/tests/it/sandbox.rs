//! The `auto` sandbox end to end through a `TestDaemon` and a scripted model: the
//! probe's status and the fallback, the exit questions and their launches, the
//! one-command rule, and the `shell_` tests that run a real zsh with a fake launcher
//! (a script that runs the child shell directly, as the fake bwrap of efr's auto spec,
//! section 16.1) or, under `just test-sandbox`, the real `efr-sbx`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminSandboxCheck, AdminSandboxCheckResult,
    AdminStatus, AdminStatusResult, ApprovalDecision, ApprovalRespond, ApprovalRespondResult,
    CacheMode, ErrorCode, Event, EventEnvelope, Grant, Launch, Method, Mode, ModeFallback,
    NetworkMode, PromptSendResult, ReportedFile, SandboxExplain, SandboxExplainResult,
    SandboxPathRole, SandboxStatus, SandboxSurfaceRespond, SandboxSurfaceRespondResult,
    TurnSettings,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_sandbox::{ProbeFailure, ProbeReport, SandboxSpec};
use efr_test_daemon::{ClientError, TTY, TestDaemon, TestDaemonBuilder, command_id, events_until};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::support::zsh_enabled;

/// A sandbox that the probe found ready.
fn ready() -> SandboxStatus {
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
    }
}

/// A model that calls `shell` with each input in turn, then says `done`.
#[derive(Debug)]
struct ScriptedModel {
    id: ProviderId,
    calls: Vec<Value>,
    asked: AtomicUsize,
}

impl ScriptedModel {
    fn new(calls: Vec<Value>) -> Arc<Self> {
        Arc::new(ScriptedModel {
            id: ProviderId::new("test").unwrap(),
            calls,
            asked: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Provider for ScriptedModel {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        let events = match self.calls.get(n) {
            Some(input) => {
                let call_id = format!("call_{n}");
                vec![
                    ProviderEvent::ToolCallStart {
                        call_id: call_id.clone(),
                        name: "shell".to_owned(),
                    },
                    ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
                    ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
                ]
            }
            None => vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ],
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// A turn's events, with each approval answered by `decision`.
async fn run_turn(
    daemon: &TestDaemon,
    text: &str,
    mode: Mode,
    decision: ApprovalDecision,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    run_turn_on(daemon, TTY, text, mode, decision).await
}

/// [`run_turn`] from the shell of `tty`, which has a conversation of its own.
async fn run_turn_on(
    daemon: &TestDaemon,
    tty: &str,
    text: &str,
    mode: Mode,
    decision: ApprovalDecision,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(tty).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(1, text, tty) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(mode), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = 10;
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(
                event,
                Event::ApprovalRequested { .. }
                    | Event::SurfaceQuestionRequested { .. }
                    | Event::TurnCompleted { .. }
                    | Event::TurnFailed { .. }
                    | Event::TurnInterrupted { .. }
            )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events);
        match last {
            Event::SurfaceQuestionRequested { question_id, .. } => {
                answers += 1;
                let answer = Method::SandboxSurfaceRespond(SandboxSurfaceRespond {
                    command_id: command_id(answers),
                    conversation_id: sent.conversation_id,
                    question_id,
                    keep: decision == ApprovalDecision::Allow,
                });
                let _: SandboxSurfaceRespondResult = client.call(answer).await.unwrap();
            }
            Event::ApprovalRequested { call_id, .. } => {
                answers += 1;
                let answer = Method::ApprovalRespond(ApprovalRespond {
                    command_id: command_id(answers),
                    conversation_id: sent.conversation_id,
                    call_id,
                    decision,
                });
                let _: ApprovalRespondResult = client.call(answer).await.unwrap();
            }
            _ => break,
        }
    }
    (sent, seen)
}

fn started_settings(events: &[EventEnvelope]) -> efr_protocol::EffectiveSettings {
    events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::TurnStarted { settings, .. } => settings.clone(),
            _ => None,
        })
        .unwrap()
}

fn kinds(events: &[EventEnvelope]) -> Vec<&str> {
    events.iter().map(|envelope| envelope.event.kind()).collect()
}

fn completed(events: &[EventEnvelope]) -> Vec<(String, Option<i32>)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, exit_code, .. } => {
                Some((output.clone(), *exit_code))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_probe_failure_falls_back_to_cautious_with_the_reason() {
    let failure = ProbeFailure::AbiLow { found: 8 };
    let mut status = SandboxStatus::unavailable(failure.reason());
    status.fix = Some(failure.fix());
    let daemon = TestDaemon::builder()
        .probe_override(status)
        .custom_provider(ScriptedModel::new(Vec::new()))
        .start()
        .await
        .unwrap();
    let (_, events) = run_turn(&daemon, "hello", Mode::Auto, ApprovalDecision::Allow).await;
    let settings = started_settings(&events);
    assert_eq!(settings.mode, Mode::Cautious);
    assert_eq!(
        settings.fallback,
        Some(ModeFallback {
            asked: Mode::Auto,
            reason: "Landlock ABI 8 found; auto needs 9 (Linux 7.1)".to_owned()
        })
    );
    // admin.status and admin.sandbox_check say the same.
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    assert!(!sandbox.available);
    assert_eq!(sandbox.fix.as_deref(), Some("use a newer kernel"));
    assert!(status.sandbox_paths.unwrap().runtime.ends_with("runtime/sbx"));
    let check: AdminSandboxCheckResult =
        client.call(Method::AdminSandboxCheck(AdminSandboxCheck {})).await.unwrap();
    assert!(!check.status.available);
    assert_eq!(check.checks[0].detail.as_deref(), sandbox.reason.as_deref());
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn without_a_launcher_auto_says_why() {
    // A test daemon finds no efr-sbx next to its own binary.
    let daemon = TestDaemon::builder()
        .custom_provider(ScriptedModel::new(Vec::new()))
        .start()
        .await
        .unwrap();
    let (_, events) = run_turn(&daemon, "hello", Mode::Auto, ApprovalDecision::Allow).await;
    let fallback = started_settings(&events).fallback.unwrap();
    assert_eq!(fallback.reason, ProbeFailure::NoLauncher.reason());
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn escape_launcher_in_write_root_refuses_auto() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    // The launcher that efrd would copy lies in a registered project, where a
    // contained call could rewrite it, as with `just run` in the project's target/.
    let launcher = project.join("target/debug/efr-sbx");
    std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    std::fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .custom_provider(ScriptedModel::new(Vec::new()))
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    // The reload after the registration runs the probe again in the background.
    let mut reason = None;
    for _ in 0..500 {
        let status: AdminStatusResult =
            client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
        reason = status.sandbox.and_then(|sandbox| sandbox.reason);
        if reason.as_deref().is_some_and(|text| text.contains("writable project")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        reason,
        Some(format!("efr-sbx lies in a writable project ({})", launcher.display()))
    );
    let (_, events) = run_turn(&daemon, "hello", Mode::Auto, ApprovalDecision::Allow).await;
    assert_eq!(started_settings(&events).mode, Mode::Cautious);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn home_project_falls_back_to_cautious() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .probe_override(ready())
        .custom_provider(ScriptedModel::new(Vec::new()))
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: dirs.home().to_path_buf(),
        name: Some("home".to_owned()),
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    let (_, events) = run_turn(&daemon, "hello", Mode::Auto, ApprovalDecision::Allow).await;
    let settings = started_settings(&events);
    assert_eq!(settings.mode, Mode::Cautious);
    assert_eq!(
        settings.fallback.map(|fallback| fallback.reason),
        Some(ModeFallback::HOME_PROJECT_REASON.to_owned())
    );
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn escape_exit_cannot_carry_helper() {
    let model = ScriptedModel::new(vec![json!({ "command": "sudo -v && ./helper" })]);
    let daemon =
        TestDaemon::builder().probe_override(ready()).custom_provider(model).start().await.unwrap();
    let (_, events) = run_turn(&daemon, "update", Mode::Auto, ApprovalDecision::Allow).await;
    assert!(!kinds(&events).contains(&"approval_requested"), "{:?}", kinds(&events));
    let [(output, _)] = completed(&events).try_into().unwrap();
    assert!(output.starts_with(efr_permissions_one_command()), "{output}");
    // Nothing ran: no shell started.
    assert!(!kinds(&events).contains(&"shell_started"));
    daemon.stop().await.unwrap();
}

/// The first words of the one-command refusal.
fn efr_permissions_one_command() -> &'static str {
    "an approved command outside the sandbox must run alone"
}

#[tokio::test]
async fn sandbox_explain_answers_from_the_plan() {
    // NOTE: below the target dir, not /tmp, which the sandbox replaces with its own.
    let dirs = Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .probe_override(ready())
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    let explain =
        |path: PathBuf| Method::SandboxExplain(SandboxExplain { path, cwd: Some(project.clone()) });
    let zshrc = dirs.home().join(".zshrc");
    std::fs::write(&zshrc, "").unwrap();
    let answer: SandboxExplainResult = client.call(explain(zshrc.clone())).await.unwrap();
    assert_eq!(answer.role, SandboxPathRole::Floor);
    assert_eq!(answer.reason, "a shell startup file (floor)");
    assert_eq!((answer.read, answer.write), (true, false));
    assert_eq!(answer.write_exit, Some(efr_protocol::ExitKind::Persistence));
    assert_eq!(answer.project.as_deref(), Some(project.as_path()));
    let inside: SandboxExplainResult =
        client.call(explain(PathBuf::from("src/main.rs"))).await.unwrap();
    assert_eq!(inside.path, project.join("src/main.rs"));
    assert_eq!(inside.role, SandboxPathRole::WriteRoot);
    assert!(inside.write);
    std::fs::create_dir_all(dirs.home().join(".ssh")).unwrap();
    let secret: SandboxExplainResult =
        client.call(explain(dirs.home().join(".ssh/id_ed25519"))).await.unwrap();
    assert_eq!(secret.role, SandboxPathRole::Masked);
    assert!(!secret.read);
    let relative = client
        .call::<SandboxExplainResult>(Method::SandboxExplain(SandboxExplain {
            path: PathBuf::from("x"),
            cwd: None,
        }))
        .await;
    let Err(ClientError::Server { body }) = relative else { panic!("{relative:?}") };
    assert_eq!(body.code, ErrorCode::Invalid);
    drop(client);
    daemon.stop().await.unwrap();
}

/// The fake launcher of these tests. `probe` prints `<fake>/report.json` and counts in
/// `<fake>/probes`; `run` keeps a copy of the call's spec, fails before it starts when
/// `<fake>/fail` exists, and otherwise runs the child script directly, with no sandbox,
/// and writes the launcher's files. With `<fake>/survivors` the result names one survivor,
/// once.
const FAKE_LAUNCHER: &str = r#"#!/bin/sh
fake='@FAKE@'
case "$1" in
probe) echo probe >> "$fake/probes"; cat "$fake/report.json"; exit 0 ;;
run) ;;
*) exit 2 ;;
esac
dir=$3
cp "$dir/spec.json" "$fake/spec-${dir##*/}.json"
if [ -e "$fake/fail" ]; then echo "efr-sbx: a fake setup failure" >&2; exit 125; fi
: > "$dir/started"
mkdir -p "$dir/fake-child"
cp "$dir/line" "$dir/fake-child/"
if [ -e "${dir%/*}/snapshot.zsh" ]; then cp "${dir%/*}/snapshot.zsh" "$dir/fake-child/"; fi
zsh -f "${0%/bin/*}/zsh/efr-child.zsh" "$dir/fake-child" 3> "$dir/records"
status=$?
changes=''
if [ -e "$fake/quarantine" ] && [ -e "$PWD/planted" ]; then
    sbx=$(sed -n 's/.*"sandbox_dir": "\(.*\)",/\1/p' "$dir/spec.json")
    q="$sbx/quarantine/${dir##*/}"
    mkdir -p "$q"
    mv "$PWD/planted" "$q/0-planted"
    printf '[{"from":"%s","to":"0-planted"}]' "$PWD/planted" > "$q/entries.json"
    changes=$(printf ',"surface_changes":[{"path":"%s","rule":"code_key","key":"core.fsmonitor","quarantined":true}]' "$PWD/planted")
fi
if [ -e "$fake/survivors" ]; then rm "$fake/survivors"; changes="$changes"',"survivors":["sudo"]'; fi
printf '{"started":true,"exit_code":%s,"cwd":"%s","state_kept":true,"summary":{"confined":true%s}}' "$status" "$(pwd)" "$changes" > "$dir/result.tmp"
mv "$dir/result.tmp" "$dir/result.json"
exit "$status"
"#;

/// The fake launcher and its files in `<dirs>/fake`, answering its probe as ready.
fn fake_launcher(dirs: &TestDirs) -> (PathBuf, PathBuf) {
    let fake = dirs.create_dir("fake").unwrap();
    let launcher = fake.join("efr-sbx");
    std::fs::write(&launcher, FAKE_LAUNCHER.replace("@FAKE@", &fake.to_string_lossy())).unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    let report = ProbeReport { bwrap: Some("/usr/bin/bwrap".into()), ..ProbeReport::default() };
    std::fs::write(fake.join("report.json"), report.to_json().unwrap()).unwrap();
    (launcher, fake)
}

/// The specs that the fake launcher kept, one per call.
fn specs(fake: &Path) -> Vec<SandboxSpec> {
    let mut specs: Vec<SandboxSpec> = std::fs::read_dir(fake)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("spec-"))
        .map(|entry| SandboxSpec::from_json(&std::fs::read(entry.path()).unwrap()).unwrap())
        .collect();
    specs.sort_by_key(|spec| spec.call);
    specs
}

fn with_zsh(builder: TestDaemonBuilder) -> TestDaemonBuilder {
    builder.local_pty()
}

#[tokio::test]
async fn shell_a_routine_command_in_auto_runs_contained_without_a_question() {
    if !zsh_enabled("shell_a_routine_command_in_auto_runs_contained_without_a_question") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, fake) = fake_launcher(&dirs);
    let model = ScriptedModel::new(vec![json!({ "command": "echo efr-$((40+2))" })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let (_, events) = run_turn(&daemon, "say it", Mode::Auto, ApprovalDecision::Deny).await;
    assert_eq!(started_settings(&events).mode, Mode::Auto);
    assert!(!kinds(&events).contains(&"approval_requested"), "{:?}", kinds(&events));
    let launch = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallStarted { launch, .. } => launch.clone(),
        _ => None,
    });
    assert_eq!(launch, Some(Launch::contained()));
    let [(output, exit_code)] = completed(&events).try_into().unwrap();
    assert!(output.contains("efr-42"), "{output}");
    assert_eq!(exit_code, Some(0));
    let summary = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { sandbox, .. } => sandbox.clone(),
        _ => None,
    });
    assert!(summary.is_some_and(|summary| summary.confined));
    let [spec] = specs(&fake).try_into().unwrap();
    assert!(spec.grants.is_empty());
    assert_eq!(spec.runtime.launcher, dirs.dirs().runtime().join("bin/efr-sbx"));
    assert!(!spec.runtime.call_dir.exists(), "the dir of a call that ended is removed");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_predicted_exit_asks_and_a_yes_runs_with_its_grant() {
    if !zsh_enabled("shell_a_predicted_exit_asks_and_a_yes_runs_with_its_grant") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, fake) = fake_launcher(&dirs);
    let model = ScriptedModel::new(vec![json!({ "command": "echo hi > ~/notes.txt" })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let (_, events) = run_turn(&daemon, "write it", Mode::Auto, ApprovalDecision::Allow).await;
    let kinds = kinds(&events);
    let asked = kinds.iter().position(|kind| *kind == "approval_requested").unwrap();
    assert_eq!(kinds[asked - 1], "exit_requested", "{kinds:?}");
    let notes = dirs.home().join("notes.txt");
    let [spec] = specs(&fake).try_into().unwrap();
    assert_eq!(spec.grants, [Grant::Write { path: notes.clone() }]);
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hi\n");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_needs_write_of_a_missing_home_file_runs_in_the_exit_child() {
    if !zsh_enabled("shell_a_needs_write_of_a_missing_home_file_runs_in_the_exit_child") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, fake) = fake_launcher(&dirs);
    // The line does not name the file, so only `needs` tells the daemon to look it up.
    let model = ScriptedModel::new(vec![json!({
        "command": "sh -c 'echo made > \"$HOME/gen.txt\"'",
        "needs": { "write": ["~/gen.txt"], "reason": "the script writes ~/gen.txt" },
    })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let (_, events) = run_turn(&daemon, "make it", Mode::Auto, ApprovalDecision::Allow).await;
    let exit = events.iter().find_map(|envelope| match &envelope.event {
        Event::ApprovalRequested { exit, .. } => exit.clone(),
        _ => None,
    });
    let exit = exit.unwrap();
    // A bind of the missing file alone cannot start, and a bind of `~` opens too much.
    assert_eq!(exit.launch, Launch::Unsandboxed);
    assert!(exit.user_only);
    assert!(exit.facts.contains(&"~/gen.txt does not exist yet".to_owned()), "{:?}", exit.facts);
    let [spec] = specs(&fake).try_into().unwrap();
    assert_eq!(spec.launch, efr_sandbox::SpecLaunch::Unsandboxed);
    assert_eq!(std::fs::read_to_string(dirs.home().join("gen.txt")).unwrap(), "made\n");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_an_exit_question_resolves_programs_with_the_hidden_shells_path() {
    if !zsh_enabled("shell_an_exit_question_resolves_programs_with_the_hidden_shells_path") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, _) = fake_launcher(&dirs);
    // The user's startup file puts a sudo of their own first, as ~/.local/bin often is.
    let bin = dirs.create_dir("home/bin").unwrap();
    let sudo = bin.join("sudo");
    std::fs::write(&sudo, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dirs.home().join(".zshenv"), format!("path=({} $path)\n", bin.display()))
        .unwrap();
    let model =
        ScriptedModel::new(vec![json!({ "command": "true" }), json!({ "command": "sudo true" })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let (_, events) = run_turn(&daemon, "as root", Mode::Auto, ApprovalDecision::Deny).await;
    let record = events.iter().find_map(|envelope| match &envelope.event {
        Event::ExitRequested { record, .. } => Some(record.clone()),
        _ => None,
    });
    let programs = record.unwrap().facts.programs;
    let resolved = programs.iter().find(|program| program.word == "sudo").unwrap();
    assert_eq!(resolved.resolved.as_deref(), Some(sudo.as_path()), "{programs:?}");
    daemon.stop().await.unwrap();
}

// NOTE: two workers, so the probe runs while the test waits for its count.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_reprobe_after_sandbox_failed() {
    if !zsh_enabled("shell_reprobe_after_sandbox_failed") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, fake) = fake_launcher(&dirs);
    std::fs::write(fake.join("fail"), "").unwrap();
    let model = ScriptedModel::new(vec![json!({ "command": "true" })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let probes =
        || std::fs::read_to_string(fake.join("probes")).unwrap_or_default().lines().count();
    assert_eq!(probes(), 1, "the probe ran at start and said ready");
    let (_, events) = run_turn(&daemon, "try", Mode::Auto, ApprovalDecision::Deny).await;
    let [(output, _)] = completed(&events).try_into().unwrap();
    assert!(output.contains("the sandbox could not start"), "{output}");
    for _ in 0..500 {
        if probes() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(probes(), 2, "the failed start ran the probe again");
    daemon.stop().await.unwrap();
}

/// Says on stderr that `test` skips, and why.
#[expect(clippy::print_stderr, reason = "a skipped test says why")]
fn skip(test: &str, reason: &str) {
    eprintln!("skipped: {test}: {reason}");
}

/// The real launcher from `EFR_TEST_SBX_BIN`, which `just test-sandbox` sets, or
/// `None` with a message.
fn real_launcher(test: &str) -> Option<PathBuf> {
    match efr_stdx::env::path(efr_stdx::env::Var::TestSbxBin) {
        Ok(Some(bin)) => Some(bin),
        _ => {
            skip(test, "EFR_TEST_SBX_BIN is not set; just test-sandbox sets it");
            None
        }
    }
}

#[tokio::test]
async fn shell_a_contained_call_runs_in_the_real_sandbox() {
    let test = "shell_a_contained_call_runs_in_the_real_sandbox";
    if !zsh_enabled(test) {
        return;
    }
    let Some(launcher) = real_launcher(test) else { return };
    // NOTE: below the target dir, not /tmp, which the sandbox replaces with its own.
    let dirs = Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = std::collections::BTreeMap::from([
        ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.to_string_lossy().into_owned()),
    ]);
    // NOTE: the write to the home dir hides in `sh -c`, so efr predicts no exit and the
    // sandbox alone must stop it.
    let line = "touch inside.txt && echo private > /tmp/x && cat /tmp/x; \
                sh -c 'echo no > $HOME/outside.txt'; echo status=$?";
    let model = ScriptedModel::new(vec![json!({ "command": line })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        return;
    }
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    let (_, events) = run_turn(&daemon, "try it", Mode::Auto, ApprovalDecision::Deny).await;
    assert_eq!(started_settings(&events).mode, Mode::Auto);
    let [(output, _)] = completed(&events).try_into().unwrap();
    assert!(output.contains("private"), "{output}");
    assert!(output.contains("Read-only file system"), "{output}");
    // A failed redirection ends bash's `sh` with 1 and dash's (Debian, Ubuntu) with 2.
    assert!(output.contains("status=1") || output.contains("status=2"), "{output}");
    assert!(project.join("inside.txt").is_file(), "the project is a write root");
    assert!(!dirs.home().join("outside.txt").exists(), "the home dir is not");
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_contained_call_cannot_reach_the_snapshot_store() {
    let test = "shell_a_contained_call_cannot_reach_the_snapshot_store";
    if !zsh_enabled(test) {
        return;
    }
    let Some(launcher) = real_launcher(test) else { return };
    // NOTE: below the target dir, not /tmp, which the sandbox replaces with its own.
    let dirs = Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    std::fs::write(project.join("a.txt"), "old\n").unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = std::collections::BTreeMap::from([
        ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.to_string_lossy().into_owned()),
    ]);
    let store = dirs.dirs().data().join("snapshots");
    // NOTE: the snapshot before the call made the store, so it exists while the line
    // runs; the line reads it, plants a file and deletes what it finds. The store's
    // path hides in `sh -c`, so efr predicts no exit and the sandbox alone must stop it.
    let line = format!(
        "echo new > a.txt; S={} sh -c 'ls -A \"$S\"; echo listed=$?; touch \"$S/planted\"; \
         echo touched=$?; rm -rf \"$S\"/*; echo removed=$?'",
        store.display()
    );
    let model = ScriptedModel::new(vec![json!({ "command": line })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        return;
    }
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    let before: Vec<_> = std::fs::read_dir(&store).map(|dir| dir.count()).into_iter().collect();
    assert!(before.is_empty() || before == [0], "no store before the turn");

    let (_, events) = run_turn(&daemon, "try it", Mode::Auto, ApprovalDecision::Deny).await;
    assert_eq!(started_settings(&events).mode, Mode::Auto);
    let [(output, _)] = completed(&events).try_into().unwrap();
    assert!(!output.contains(".git"), "the masked store lists nothing: {output}");
    assert!(output.contains("touched=1"), "{output}");

    let mut names: Vec<String> = std::fs::read_dir(&store)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    // NOTE: one store for the project and one for $SCRATCH, each with its repository,
    // its index and its root file.
    let stores = names.iter().filter(|name| name.ends_with(".git")).count();
    assert_eq!((stores, names.len()), (2, 6), "{names:?}");
    assert!(!store.join("planted").exists());
    let changes = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { changes, .. } => changes.clone(),
        _ => None,
    });
    let changes = changes.unwrap();
    assert_eq!(changes.files[0].path, "a.txt", "the call's own change is listed");
    drop(client);
    daemon.stop().await.unwrap();
}

/// A peer that the model's command leaves behind: it sends the frames in `frames/` to
/// the daemon's socket and writes every byte of the answers to `out`.
const PEER: &str = r#"
import os, socket, sys, time
sock, frames, out = sys.argv[1:4]
s = socket.socket(socket.AF_UNIX)
s.connect(sock)
for name in sorted(os.listdir(frames)):
    s.sendall(open(os.path.join(frames, name), "rb").read())
s.settimeout(0.5)
data = b""
wanted = len(os.listdir(frames))
end = time.time() + 10
def answered(data):
    count, at = 0, 0
    while at + 4 <= len(data):
        size = int.from_bytes(data[at:at + 4], "big")
        payload = data[at + 4:at + 4 + size]
        if len(payload) < size:
            break
        count += b'"end":true' in payload or b'"error":' in payload
        at += 4 + size
    return count
while time.time() < end and answered(data) < wanted:
    try:
        chunk = s.recv(65536)
    except socket.timeout:
        continue
    if not chunk:
        break
    data += chunk
open(out + ".tmp", "wb").write(data)
os.rename(out + ".tmp", out)
"#;

// NOTE: two workers, so the daemon in this process answers the peer while the test
// waits for the peer's file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn escape_model_side_peer_read_scope() {
    use efr_protocol::framing::{Decoder, encode};
    use efr_protocol::{ClientFrame, Hello, Origin, PROTOCOL_VERSION, ProjectsList, RequestId};

    let test = "escape_model_side_peer_read_scope";
    if !zsh_enabled(test) {
        return;
    }
    if !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|dir| dir.join("python3").is_file())
    {
        skip(test, "python3 is not installed");
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let work = dirs.create_dir("peer").unwrap();
    let frames = dirs.create_dir("peer/frames").unwrap();
    std::fs::write(work.join("peer.py"), PEER).unwrap();
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        origin: Origin::Shell,
        client: Some("peer".to_owned()),
        capabilities: efr_protocol::Capabilities::default(),
        tty: None,
        pid: None,
        device_id: None,
    };
    let requests = [
        Method::Hello(hello),
        Method::AdminStatus(AdminStatus {}),
        Method::ProjectsList(ProjectsList {}),
        Method::AdminSandboxCheck(AdminSandboxCheck {}),
    ];
    for (n, method) in requests.into_iter().enumerate() {
        let frame = ClientFrame::Request { id: RequestId::new(n as u64 + 1), method };
        std::fs::write(frames.join(format!("{n}")), encode(&frame).unwrap()).unwrap();
    }
    let out = work.join("answers");
    // NOTE: the subshell exits at once, so python3 is an orphan that the hidden zsh,
    // a child subreaper, adopts: a double-forked process of the model's command.
    let command = format!(
        "(python3 {} {} {} {} >/dev/null 2>&1 &)",
        work.join("peer.py").display(),
        dirs.dirs().socket_path().display(),
        frames.display(),
        out.display()
    );
    let model = ScriptedModel::new(vec![json!({ "command": command })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let (_, events) =
        run_turn(&daemon, "leave a peer", Mode::Cautious, ApprovalDecision::Allow).await;
    assert!(kinds(&events).contains(&"tool_call_completed"), "{:?}", kinds(&events));
    let mut answers = None;
    for _ in 0..1000 {
        if let Ok(bytes) = std::fs::read(&out) {
            answers = Some(bytes);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let answers = answers.expect("the peer wrote what the daemon answered");
    let mut decoder = Decoder::new();
    let mut ended: Vec<(u64, Option<ErrorCode>)> = Vec::new();
    for payload in decoder.push(&answers).unwrap() {
        match efr_protocol::ServerFrame::from_json(&payload).unwrap() {
            efr_protocol::ServerFrame::End { id } => ended.push((id.get(), None)),
            efr_protocol::ServerFrame::Error(frame) => {
                ended.push((frame.id.map_or(0, RequestId::get), Some(frame.error.code)));
            }
            efr_protocol::ServerFrame::Item { id, .. } if id.get() == 1 => {
                ended.push((1, None));
            }
            _ => {}
        }
    }
    ended.sort_by_key(|(id, _)| *id);
    ended.dedup_by_key(|(id, _)| *id);
    assert_eq!(
        ended,
        [(1, None), (2, Some(ErrorCode::Forbidden)), (3, None), (4, Some(ErrorCode::Forbidden)),],
        "a process of the model's command may read, never administer"
    );
    daemon.stop().await.unwrap();
}

/// Runs git in `dir` without the machine's configuration.
async fn git(dir: &Path, args: &[&str]) {
    let status = efr_stdx::process::command("git", dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .status()
        .await
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test]
async fn shell_a_turn_reports_its_surface_files_and_keeps_a_quarantined_change_on_yes() {
    let test = "shell_a_turn_reports_its_surface_files_and_keeps_a_quarantined_change_on_yes";
    if !zsh_enabled(test) {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    git(&project, &["init", "-q"]).await;
    std::fs::write(project.join("README.md"), "x\n").unwrap();
    git(&project, &["add", "README.md"]).await;
    git(&project, &["commit", "-q", "-m", "one"]).await;
    let (launcher, fake) = fake_launcher(&dirs);
    std::fs::write(fake.join("quarantine"), "").unwrap();
    let model = ScriptedModel::new(vec![json!({
        "command": "printf 'all:\\n' > Makefile && echo hook > planted"
    })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    let (_, events) = run_turn(&daemon, "build", Mode::Auto, ApprovalDecision::Allow).await;
    let kinds = kinds(&events);
    assert!(kinds.contains(&"surface_question_requested"), "{kinds:?}");
    assert!(kinds.contains(&"surface_question_answered"), "{kinds:?}");
    assert_eq!(std::fs::read_to_string(project.join("planted")).unwrap(), "hook\n", "kept");
    let report = events.iter().find_map(|envelope| match &envelope.event {
        Event::TurnSurfaceReport { files, .. } => Some(files.clone()),
        _ => None,
    });
    let file = |path: &str, detail: Option<&str>| ReportedFile {
        path: PathBuf::from(path),
        detail: detail.map(str::to_owned),
    };
    assert_eq!(
        report,
        Some(vec![file("Makefile", None), file("planted", Some("core.fsmonitor"))]),
        "{kinds:?}"
    );
    let at = |kind: &str| kinds.iter().position(|known| *known == kind).unwrap();
    assert_eq!(at("turn_surface_report") + 1, at("turn_completed"));
    drop(client);
    daemon.stop().await.unwrap();
}

// NOTE: two workers, so the daemon runs while the test waits for A's second call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_a_call_queued_behind_a_running_one_holds_no_plan_lock() {
    let test = "shell_a_call_queued_behind_a_running_one_holds_no_plan_lock";
    if !zsh_enabled(test) {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    let (launcher, _) = fake_launcher(&dirs);
    // Conversation A leaves a command running past its timeout, as a dev server does,
    // then its next call queues behind it. Conversation B calls in the same project.
    let model = ScriptedModel::new(vec![
        json!({ "command": "sleep 3; : > orphan-done", "timeout_seconds": 1 }),
        json!({ "command": "echo second" }),
        json!({ "command": "echo from-b" }),
    ]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();

    let Method::PromptSend(mut params) = daemon.prompt(1, "serve", TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    // The test clock moves until A's first call times out and its second one starts.
    let mut a_started = 0;
    for _ in 0..200 {
        let events = daemon.events(&client, sent.conversation_id).await.unwrap();
        a_started = kinds(&events).iter().filter(|kind| **kind == "tool_call_started").count();
        if a_started == 2 {
            break;
        }
        daemon.clock().advance(Duration::from_millis(500));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(a_started, 2, "A's second call never started");

    // A's second call waits for its shell; B's call in the same project runs now.
    let b = daemon.client_for_tty("/dev/pts/efr-test-b").await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(2, "go", "/dev/pts/efr-test-b") else {
        unreachable!()
    };
    params.new_conversation = true;
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let b_sent: PromptSendResult = b.call(Method::PromptSend(params)).await.unwrap();
    assert_ne!(b_sent.conversation_id, sent.conversation_id);
    let mut b_follow = daemon.follow(&b, b_sent.conversation_id).await.unwrap();
    let b_events =
        events_until(&mut b_follow, |event| matches!(event, Event::TurnCompleted { .. }))
            .await
            .unwrap();
    let [(output, _)] = completed(&b_events).try_into().unwrap();
    assert!(output.contains("from-b"), "{output}");
    assert!(!project.join("orphan-done").exists(), "B waited for A's command to end");

    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| matches!(event, Event::TurnCompleted { .. }))
        .await
        .unwrap();
    let outputs = completed(&events);
    assert_eq!(outputs.len(), 2, "{:?}", kinds(&events));
    assert!(outputs[1].0.contains("second"), "{outputs:?}");
    drop((client, b));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_survivor_of_an_exit_closes_the_hidden_shell() {
    if !zsh_enabled("shell_a_survivor_of_an_exit_closes_the_hidden_shell") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, fake) = fake_launcher(&dirs);
    std::fs::write(fake.join("survivors"), "").unwrap();
    let model =
        ScriptedModel::new(vec![json!({ "command": "true" }), json!({ "command": "true" })]);
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let (_, events) = run_turn(&daemon, "go", Mode::Auto, ApprovalDecision::Deny).await;
    let kinds = kinds(&events);
    let [(first, _), _] = completed(&events).try_into().unwrap();
    assert!(first.contains("could not be ended: sudo"), "{first}");
    // The shell that a survivor may still read ends, and the second call gets a new one.
    let started: Vec<usize> = (0..kinds.len()).filter(|at| kinds[*at] == "shell_started").collect();
    let exited = kinds.iter().position(|kind| *kind == "shell_exited").unwrap();
    let second_call = kinds.iter().rposition(|kind| *kind == "tool_call_started").unwrap();
    assert_eq!(started.len(), 2, "{kinds:?}");
    assert!(started[0] < exited && exited < second_call && second_call < started[1], "{kinds:?}");
    daemon.stop().await.unwrap();
}

/// A model that calls `shell` with `line` `calls` times in each turn, then says `done`.
#[derive(Debug)]
struct RepeatModel {
    id: ProviderId,
    line: String,
    calls: usize,
    asked: AtomicUsize,
}

#[async_trait]
impl Provider for RepeatModel {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        let events = if n % (self.calls + 1) < self.calls {
            let call_id = format!("call_{n}");
            let input = json!({ "command": self.line });
            vec![
                ProviderEvent::ToolCallStart { call_id: call_id.clone(), name: "shell".to_owned() },
                ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
                ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
            ]
        } else {
            vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// An rc like a big one of a person: functions, aliases, options and compinit.
fn big_rc() -> String {
    let mut rc = String::from("setopt extended_glob hist_ignore_dups no_beep\n");
    for n in 0..96 {
        rc.push_str(&format!(
            "userfn_{n}() {{\n  local a=$1 b=${{2:-x}}\n  if [[ -n $a ]]; then print -r -- \"$a\" | \
             sed -e 's/x/y/g'; fi\n  for f in *(N); do [[ -d $f ]] && print -r -- $f; done\n}}\n"
        ));
    }
    for n in 0..350 {
        rc.push_str(&format!("alias al{n}='ls -la --color=auto /srv/{n}'\n"));
    }
    rc.push_str("autoload -Uz compinit && compinit -u\n");
    rc
}

/// What a routine call in `auto` may cost more than in `cautious`, in a release build.
const AUTO_ADDS_AT_MOST: Duration = Duration::from_millis(40);

/// The debug lines of efr's crates in this process, for the bench's `phase` lines.
fn debug_log() -> Arc<std::sync::Mutex<Vec<u8>>> {
    static LOG: std::sync::OnceLock<Arc<std::sync::Mutex<Vec<u8>>>> = std::sync::OnceLock::new();
    Arc::clone(LOG.get_or_init(|| {
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("efr_=debug"))
            .with_ansi(false)
            .with_writer(move || LogSink(Arc::clone(&sink)))
            .finish();
        // NOTE: nextest runs each test in a process of its own.
        let _ = tracing::subscriber::set_global_default(subscriber);
        log
    }))
}

struct LogSink(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The log so far, which then starts empty.
fn take_log() -> String {
    String::from_utf8_lossy(&std::mem::take(&mut *debug_log().lock().unwrap())).into_owned()
}

/// The total of each `phase` line of `log` in milliseconds, in the order in which each
/// phase first shows.
fn phase_totals(log: &str) -> Vec<(String, f64)> {
    let field = |line: &str, name: &str| {
        let start = line.find(&format!(" {name}="))? + name.len() + 2;
        let value = line.get(start..)?.split(' ').next()?;
        Some(value.trim_matches('"').to_owned())
    };
    let mut totals: Vec<(String, f64)> = Vec::new();
    for line in log.lines() {
        let (Some(phase), Some(ms)) = (field(line, "phase"), field(line, "elapsed_ms")) else {
            continue;
        };
        let Ok(ms) = ms.parse::<f64>() else { continue };
        match totals.iter_mut().find(|(known, _)| *known == phase) {
            Some((_, total)) => *total += ms,
            None => totals.push((phase, ms)),
        }
    }
    totals
}

/// One turn of `calls` calls from `cwd` in `mode`, answered with yes: the time per call.
async fn turn_cost(daemon: &TestDaemon, n: u128, cwd: &Path, mode: Mode, calls: u32) -> Duration {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, "go", TTY) else { unreachable!() };
    let mut context = efr_protocol::ShellContext::new(cwd);
    context.tty = Some(TTY.to_owned());
    params.context = Some(context);
    params.settings = TurnSettings { mode: Some(mode), ..TurnSettings::default() };
    let start = std::time::Instant::now();
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    // From this turn's prompt on: the log holds the earlier turns too.
    let params = efr_protocol::ConversationSubscribe {
        conversation_id: sent.conversation_id,
        after_seq: Some(sent.seq),
        answers_input: false,
        drafts: false,
    };
    let mut follow = client.stream(Method::ConversationSubscribe(params)).await.unwrap();
    let mut answers = n * 100;
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(
                event,
                Event::ApprovalRequested { .. }
                    | Event::TurnCompleted { .. }
                    | Event::TurnFailed { .. }
                    | Event::TurnInterrupted { .. }
            )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        let Event::ApprovalRequested { call_id, .. } = last else {
            assert!(matches!(last, Event::TurnCompleted { .. }), "{last:?}");
            let outputs = completed(&events);
            assert!(outputs.iter().all(|(_, code)| *code == Some(0)), "{outputs:?}");
            break;
        };
        answers += 1;
        let answer = Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(answers),
            conversation_id: sent.conversation_id,
            call_id,
            decision: ApprovalDecision::Allow,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    start.elapsed() / calls
}

/// What a routine call costs in `auto` against `cautious`, from a project and from the
/// home dir, with a big rc and every default cache: it prints the cost of each. A
/// release build fails when a call in `auto` costs 40 ms more than one in `cautious`;
/// a debug build only when a call costs more than a quarter of a second. Run the
/// release gate with `cargo nextest run --release`, as docs/sandbox.md says.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_routine_calls_in_auto_cost_close_to_cautious() {
    let test = "shell_routine_calls_in_auto_cost_close_to_cautious";
    if !zsh_enabled(test) {
        return;
    }
    let Some(launcher) = real_launcher(test) else { return };
    let dirs = Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    std::fs::write(dirs.home().join(".zshrc"), big_rc()).unwrap();
    for cache in [
        ".cargo/registry",
        ".rustup/toolchains",
        ".cache/go-build",
        "go/pkg/mod",
        ".npm/_cacache",
        ".bun/install/cache",
        ".local/share/pnpm/store",
        ".m2/repository",
        ".gradle/caches",
    ] {
        dirs.create_dir(format!("home/{cache}")).unwrap();
    }
    for n in 0..200 {
        std::fs::write(project.join(format!("file{n}.rs")), "fn main() {}\n").unwrap();
    }
    let env = std::collections::BTreeMap::from([
        ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.to_string_lossy().into_owned()),
    ]);
    let calls = 10;
    let model = Arc::new(RepeatModel {
        id: ProviderId::new("test").unwrap(),
        line: "true".to_owned(),
        calls: calls as usize,
        asked: AtomicUsize::new(0),
    });
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        return;
    }
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: project.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    // The first turn starts the hidden shell and writes the snapshot; it does not count.
    debug_log();
    turn_cost(&daemon, 1, &project, Mode::Auto, calls).await;
    take_log();
    let auto = turn_cost(&daemon, 2, &project, Mode::Auto, calls).await;
    let auto_phases = phase_totals(&take_log());
    let home = turn_cost(&daemon, 3, dirs.home(), Mode::Auto, calls).await;
    take_log();
    let cautious = turn_cost(&daemon, 4, &project, Mode::Cautious, calls).await;
    let cautious_phases = phase_totals(&take_log());
    let ms = |cost: Duration| cost.as_secs_f64() * 1000.0;
    #[expect(clippy::print_stdout, reason = "the bench prints its numbers")]
    {
        println!(
            "a routine call ({calls} per turn, a model that answers at once): auto from the \
             project {:.1} ms, auto from the home dir {:.1} ms, cautious {:.1} ms",
            ms(auto),
            ms(home),
            ms(cautious)
        );
        // The `phase` lines of efrd (docs/sandbox.md), per call: where the time goes.
        println!("{:<28} {:>9} {:>9}", "phase (ms per call)", "auto", "cautious");
        let per_call = |phases: &[(String, f64)], name: &str| {
            phases
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, total)| total / f64::from(calls))
        };
        let shown =
            |cost: Option<f64>| cost.map_or_else(|| "-".to_owned(), |ms| format!("{ms:.2}"));
        for (name, _) in &auto_phases {
            let cautious = per_call(&cautious_phases, name);
            println!(
                "{name:<28} {:>9} {:>9}",
                shown(per_call(&auto_phases, name)),
                shown(cautious)
            );
        }
    }
    // Every step of a contained call has its line (docs/sandbox.md).
    for phase in [
        "model_open",
        "model_first_event",
        "model_first_tool_call",
        "model_answer",
        "requirements",
        "real_paths",
        "facts",
        "engine",
        "exit_prediction",
        "record_started",
        "shell_free_wait",
        "plan_lock",
        "plan_build",
        "call_dir_write",
        "sandbox_prepare",
        "surface_before",
        "shell_typing",
        "wrapper_snapshot",
        "launcher_plan",
        "launcher_guard_before",
        "launcher_bwrap_setup",
        "launcher_layers",
        "launcher_child",
        "launcher_guard_after",
        "wrapper_launcher",
        "wrapper_apply",
        "shell_command",
        "shell_check",
        "shell_total",
        "sandboxed_run",
        "call_dir_remove",
        "tool_run",
        "record_completed",
        "tool_call",
    ] {
        assert!(
            auto_phases.iter().any(|(known, _)| known == phase),
            "no {phase} line in auto: {auto_phases:?}"
        );
    }
    for cost in [auto, home, cautious] {
        assert!(cost < Duration::from_millis(250), "a call cost {:.1} ms", ms(cost));
    }
    // NOTE: a debug build runs the launcher and the daemon several times slower, so
    // only a release build holds the bound of what auto may add to a call.
    if !cfg!(debug_assertions) {
        for cost in [auto, home] {
            let added = cost.saturating_sub(cautious);
            assert!(
                added < AUTO_ADDS_AT_MOST,
                "a call in auto cost {:.1} ms more than in cautious; at most {:.1} ms",
                ms(added),
                ms(AUTO_ADDS_AT_MOST)
            );
        }
    }
    drop(client);
    daemon.stop().await.unwrap();
}
