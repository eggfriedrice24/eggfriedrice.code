//! The reviewer from the prompt to the real files (efr's auto spec, section 16.3, the
//! tests of the phase A plan for P7): a floor file edit through each file tool and
//! through a dotfiles link, with the record that the reviewer reads, the backup and
//! `home_file_changed`; and deletes in a private cache layer that reach the user's real
//! cache after the reviewer allows them, and stay private after it refuses. The cache
//! tests run a real zsh with a fake launcher that reports the deletes, or under `just
//! test-sandbox` the real `efr-sbx`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    Event, EventEnvelope, ExitKind, ExitRecord, ExitSource, JudgeKind, Method, Mode, PathClassName,
    Verdict,
};
use efr_provider::{
    ContentBlock, EditTool, Provider, ProviderError, ProviderId, ProviderStream, Request,
};
use efr_test_daemon::TestDaemon;
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    FAKE_LAUNCHER, REVIEWER_ALLOWS, ScriptedModel, completed, fake_launcher, judgements, kinds,
    ready, real_launcher, reviewed_daemon, run_turn, skip, with_zsh,
};
use crate::home::home_copies;
use crate::support::zsh_enabled;

/// The reviewer's answer that refuses an action.
const REVIEWER_REFUSES: &str = r#"{"verdict":"deny","risk":"high","user_authorization":"none","category":"unrelated","rationale":"nothing the user said relates to it"}"#;

/// The text of `~/.zshrc` before the edit.
const OLD_RC: &str = "alias ll='ls -l'\n";

/// The text of `~/.zshrc` after the edit.
const NEW_RC: &str = "alias ll='ls -l'\nalias k=kubectl\n";

/// The prompt that names `~/.zshrc`.
const NAMED: &str = "add alias k=kubectl to my zshrc";

/// The patch of `apply_patch` that adds the alias to `~/.zshrc`.
const ADD_ALIAS: &str = "*** Begin Patch\n*** Update File: ~/.zshrc\n@@\n alias ll='ls -l'\n+alias k=kubectl\n*** End Patch\n";

/// The records of a turn's exits.
fn records(events: &[EventEnvelope]) -> Vec<ExitRecord> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ExitRequested { record, .. } => Some((**record).clone()),
            _ => None,
        })
        .collect()
}

/// Each `home_file_changed` of a turn: the path, floor, covered and the backup.
fn changed(events: &[EventEnvelope]) -> Vec<(PathBuf, bool, bool, Option<PathBuf>)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::HomeFileChanged { path, floor, covered, backup, .. } => {
                Some((path.clone(), *floor, *covered, backup.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Checks the flow of a named floor file edit that the reviewer allowed, through a file
/// tool, of the file `path` (the real file), and returns its one record: no question,
/// one review before the call ran, the file's new text, a backup of the old text, and
/// `home_file_changed` with the backup before the turn's end.
async fn assert_reviewed_edit(
    dirs: &TestDirs,
    model: &ScriptedModel,
    events: &[EventEnvelope],
    path: &Path,
) -> ExitRecord {
    let kinds = kinds(events);
    assert!(!kinds.contains(&"approval_requested"), "{kinds:?}");
    let at = |kind: &str| kinds.iter().position(|known| *known == kind).unwrap();
    assert!(at("exit_review_started") < at("exit_judged"), "{kinds:?}");
    assert!(at("exit_judged") < at("tool_call_completed"), "{kinds:?}");
    assert!(at("home_file_changed") < at("turn_completed"), "{kinds:?}");
    assert_eq!(judgements(events), [(JudgeKind::Classifier, Verdict::Allow)]);
    assert_eq!(model.reviews.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read_to_string(path).unwrap(), NEW_RC);
    // NOTE: phase B copies the real file into the home store (plan PB12).
    let copies = home_copies(dirs).await;
    assert!(copies.contains(&(path.to_path_buf(), OLD_RC.to_owned())), "{copies:?}");
    let [(changed_path, floor, covered, backup)] = changed(events).try_into().unwrap();
    assert!(floor);
    assert!(covered && backup.is_none(), "the home store covers it: {covered} {backup:?}");
    assert!(changed_path == path || changed_path == dirs.home().join(".zshrc"));
    let [record] = records(events).try_into().unwrap();
    assert_eq!(record.action.exits, [ExitKind::Persistence]);
    assert_eq!(record.user_messages, [NAMED]);
    // The facts come from efrd's own look at the file (`DaemonToolbox::facts`).
    let [target] = record.facts.targets.clone().try_into().unwrap();
    assert_eq!(target.path, path);
    assert!(target.floor && target.exists, "{target:?}");
    assert!(target.named_in_user_messages && target.backup_before_write, "{target:?}");
    record
}

#[tokio::test]
async fn named_floor_file_edit_end_to_end() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let zshrc = dirs.home().join(".zshrc");
    std::fs::write(&zshrc, OLD_RC).unwrap();
    let patch = json!({ "input": ADD_ALIAS });
    let model = ScriptedModel::reviewed(vec![("apply_patch", patch)], REVIEWER_ALLOWS);
    let daemon = reviewed_daemon(&dirs, Arc::clone(&model)).await;

    let (_, events) = run_turn(&daemon, NAMED, Mode::Auto, ApprovalDecision::Deny).await;

    let record = assert_reviewed_edit(&dirs, &model, &events, &zshrc).await;
    // The reviewer reads the hunk that efr will write, never only the path.
    let edit = record.action.edit.unwrap();
    assert!(edit.contains("+alias k=kubectl"), "{edit}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn unnamed_floor_file_edit_end_to_end_asks_user() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let zshrc = dirs.home().join(".zshrc");
    std::fs::write(&zshrc, OLD_RC).unwrap();
    let patch = json!({ "input": ADD_ALIAS });
    let model = ScriptedModel::reviewed(vec![("apply_patch", patch)], REVIEWER_ALLOWS);
    let daemon = reviewed_daemon(&dirs, Arc::clone(&model)).await;

    let (_, events) =
        run_turn(&daemon, "set up kubectl for me", Mode::Auto, ApprovalDecision::Deny).await;

    let exit = events.iter().find_map(|envelope| match &envelope.event {
        Event::ApprovalRequested { exit, .. } => exit.clone(),
        _ => None,
    });
    let exit = exit.unwrap();
    assert_eq!(exit.kinds, [ExitKind::Persistence]);
    assert!(exit.user_only, "no message names the file, so only the user decides");
    assert_eq!(model.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(judgements(&events), [(JudgeKind::User, Verdict::Deny)]);
    assert_eq!(std::fs::read_to_string(&zshrc).unwrap(), OLD_RC);
    assert!(home_copies(&dirs).await.is_empty());
    assert!(changed(&events).is_empty());
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn named_floor_file_edit_through_write_file_goes_to_classifier() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let zshrc = dirs.home().join(".zshrc");
    std::fs::write(&zshrc, OLD_RC).unwrap();
    let write = json!({ "path": "~/.zshrc", "content": NEW_RC });
    let model = ScriptedModel::reviewed(vec![("write_file", write)], REVIEWER_ALLOWS);
    let daemon = reviewed_daemon(&dirs, Arc::clone(&model)).await;

    let (_, events) = run_turn(&daemon, NAMED, Mode::Auto, ApprovalDecision::Deny).await;

    let record = assert_reviewed_edit(&dirs, &model, &events, &zshrc).await;
    assert_eq!(record.action.edit.as_deref(), Some(NEW_RC));
    daemon.stop().await.unwrap();
}

/// A scripted model that keeps the requests of each turn, and that offers `edit_tool`.
#[derive(Debug)]
struct Recording {
    model: Arc<ScriptedModel>,
    edit_tool: EditTool,
    requests: Mutex<Vec<Request>>,
}

impl Recording {
    fn new(model: &Arc<ScriptedModel>, edit_tool: EditTool) -> Arc<Self> {
        Arc::new(Recording {
            model: Arc::clone(model),
            edit_tool,
            requests: Mutex::new(Vec::new()),
        })
    }

    /// The tool results that the model read last.
    fn tool_results(&self) -> Vec<String> {
        let requests = self.requests.lock().unwrap();
        let last = requests.iter().rfind(|request| !request.one_shot).unwrap();
        last.messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult { output, .. } => Some(output.clone()),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
impl Provider for Recording {
    fn id(&self) -> &ProviderId {
        self.model.id()
    }

    fn default_edit_tool(&self) -> EditTool {
        self.edit_tool
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        self.model.stream(request).await
    }
}

#[tokio::test]
async fn named_floor_file_edit_through_edit_goes_to_classifier() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let zshrc = dirs.home().join(".zshrc");
    std::fs::write(&zshrc, OLD_RC).unwrap();
    let edit = json!({
        "path": "~/.zshrc",
        "old_string": "alias ll='ls -l'\n",
        "new_string": "alias ll='ls -l'\nalias k=kubectl\n",
    });
    let model = ScriptedModel::reviewed(vec![("edit", edit)], REVIEWER_ALLOWS);
    let (launcher, _) = fake_launcher(&dirs);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(Recording::new(&model, EditTool::Replace))
        .start()
        .await
        .unwrap();

    let (_, events) = run_turn(&daemon, NAMED, Mode::Auto, ApprovalDecision::Deny).await;

    let record = assert_reviewed_edit(&dirs, &model, &events, &zshrc).await;
    let edit = record.action.edit.unwrap();
    assert!(edit.contains("alias k=kubectl") && edit.contains("alias ll='ls -l'"), "{edit}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn named_edit_of_dotfiles_link_target_goes_to_classifier() {
    // A stow layout: ~/.zshrc links to a file in ~/dotfiles, a registered project. The
    // model writes the real path, as a file tool must; the floor of the link target
    // takes the group and the names of its link, so "my zshrc" names it.
    let dirs = Arc::new(TestDirs::new().unwrap());
    let dotfiles = dirs.create_dir("home/dotfiles").unwrap();
    let real = dotfiles.join("zsh/.zshrc");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, OLD_RC).unwrap();
    let link = dirs.home().join(".zshrc");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let write = json!({ "path": real, "content": NEW_RC });
    let model = ScriptedModel::reviewed(vec![("write_file", write)], REVIEWER_ALLOWS);
    let daemon = reviewed_daemon(&dirs, Arc::clone(&model)).await;
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: dotfiles.clone(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();

    let (_, events) = run_turn(&daemon, NAMED, Mode::Auto, ApprovalDecision::Deny).await;

    assert_reviewed_edit(&dirs, &model, &events, &real).await;
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    drop(client);
    daemon.stop().await.unwrap();
}

/// The fake launcher of the sandbox tests, which also reports the paths of
/// `<fake>/cache_deletes`, a JSON array, as the deletes of the call in its private
/// cache layers.
fn cache_launcher(dirs: &TestDirs) -> (PathBuf, PathBuf) {
    let (launcher, fake) = fake_launcher(dirs);
    let result = "printf '{\"started\":true";
    let report = "if [ -e \"$fake/cache_deletes\" ]; then \
                  changes=\"$changes,\\\"cache_deletes\\\":$(cat \"$fake/cache_deletes\")\"; fi\n";
    assert!(FAKE_LAUNCHER.contains(result), "the fake launcher writes its result with printf");
    let script = std::fs::read_to_string(&launcher).unwrap();
    std::fs::write(&launcher, script.replacen(result, &format!("{report}{result}"), 1)).unwrap();
    (launcher, fake)
}

/// A file of 4 KiB in a dir of the user's go build cache, which a contained call
/// deletes: the dir.
fn go_build_entry(dirs: &TestDirs) -> PathBuf {
    let entry = dirs.create_dir("home/.cache/go-build/aa").unwrap();
    std::fs::write(entry.join("aa0123-d"), vec![b'x'; 4096]).unwrap();
    entry
}

/// The record of the cache delete question of a turn.
fn cache_record(events: &[EventEnvelope]) -> ExitRecord {
    let [record] = records(events).try_into().unwrap();
    assert_eq!(record.action.source, ExitSource::CacheDelete);
    assert_eq!(record.action.exits, [ExitKind::Write]);
    let [target] = record.facts.targets.clone().try_into().unwrap();
    assert_eq!(target.class, Some(PathClassName::Cache));
    record
}

/// The `cache_delete_applied` events of a turn: the paths and the freed bytes.
fn applied(events: &[EventEnvelope]) -> Vec<(Vec<PathBuf>, u64)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::CacheDeleteApplied { paths, freed_bytes, .. } => {
                Some((paths.clone(), *freed_bytes))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn shell_cache_delete_reaches_real_cache_after_allow() {
    if !zsh_enabled("shell_cache_delete_reaches_real_cache_after_allow") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, fake) = cache_launcher(&dirs);
    let entry = go_build_entry(&dirs);
    std::fs::write(fake.join("cache_deletes"), json!([entry]).to_string()).unwrap();
    // NOTE: the fake launcher runs the line on the real files, so the line deletes
    // nothing itself; the launcher reports the delete as the real sandbox would.
    let line = json!({ "command": "echo cleaned" });
    let model = ScriptedModel::reviewed(vec![("shell", line)], REVIEWER_ALLOWS);
    let recording = Recording::new(&model, EditTool::default());
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(Arc::clone(&recording) as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();

    let (_, events) = run_turn(
        &daemon,
        "free the space in my go build cache",
        Mode::Auto,
        ApprovalDecision::Deny,
    )
    .await;

    assert!(!kinds(&events).contains(&"approval_requested"), "{:?}", kinds(&events));
    assert_eq!(judgements(&events), [(JudgeKind::Classifier, Verdict::Allow)]);
    assert_eq!(model.reviews.load(Ordering::SeqCst), 1);
    let record = cache_record(&events);
    assert_eq!(record.facts.targets[0].path, entry);
    let [(paths, freed)] = applied(&events).try_into().unwrap();
    assert_eq!(paths, std::slice::from_ref(&entry));
    assert!(freed >= 4096, "{freed}");
    assert!(!entry.exists(), "efrd deleted the dir in the real cache");
    // The model reads what efr deleted after the call's output.
    let [result] = recording.tool_results().try_into().unwrap();
    assert!(result.contains("efr deleted 1 path in the real tool caches"), "{result}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_cache_delete_kept_private_after_deny() {
    if !zsh_enabled("shell_cache_delete_kept_private_after_deny") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (launcher, fake) = cache_launcher(&dirs);
    let entry = go_build_entry(&dirs);
    std::fs::write(fake.join("cache_deletes"), json!([entry]).to_string()).unwrap();
    let line = json!({ "command": "echo cleaned" });
    let model = ScriptedModel::reviewed(vec![("shell", line)], REVIEWER_REFUSES);
    let recording = Recording::new(&model, EditTool::default());
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .sandbox_launcher(&launcher)
        .probe_override(ready())
        .custom_provider(Arc::clone(&recording) as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();

    let (_, events) =
        run_turn(&daemon, "show what is in my go build cache", Mode::Auto, ApprovalDecision::Allow)
            .await;

    assert!(!kinds(&events).contains(&"approval_requested"), "{:?}", kinds(&events));
    assert_eq!(judgements(&events), [(JudgeKind::Classifier, Verdict::Deny)]);
    cache_record(&events);
    assert!(applied(&events).is_empty());
    assert!(entry.join("aa0123-d").is_file(), "the real cache keeps the dir");
    let [result] = recording.tool_results().try_into().unwrap();
    assert!(result.contains("stayed private: the deletes were refused"), "{result}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_cache_delete_reaches_real_cache_in_the_real_sandbox() {
    let test = "shell_cache_delete_reaches_real_cache_in_the_real_sandbox";
    if !zsh_enabled(test) {
        return;
    }
    let Some(launcher) = real_launcher(test) else { return };
    // NOTE: below the target dir, not /tmp, which the sandbox replaces with its own.
    let dirs = Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap());
    let project = dirs.create_dir("home/project").unwrap();
    let entry = go_build_entry(&dirs);
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = std::collections::BTreeMap::from([
        ("HOME".to_owned(), dirs.home().to_string_lossy().into_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.to_string_lossy().into_owned()),
    ]);
    // NOTE: the delete hides in `sh -c`, so efr predicts no exit: the line runs
    // contained, and the delete lands in the private layer of ~/.cache.
    let line = json!({ "command": "sh -c 'rm -rf \"$HOME/.cache/go-build/aa\"'; echo rm=$?" });
    let model = ScriptedModel::reviewed(vec![("shell", line)], REVIEWER_ALLOWS);
    let recording = Recording::new(&model, EditTool::default());
    let daemon = with_zsh(TestDaemon::builder())
        .dirs(Arc::clone(&dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .custom_provider(Arc::clone(&recording) as Arc<dyn Provider>)
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

    let (_, events) = run_turn(
        &daemon,
        "free the space in my go build cache",
        Mode::Auto,
        ApprovalDecision::Deny,
    )
    .await;

    let [(output, _)] = completed(&events).try_into().unwrap();
    assert!(output.contains("rm=0"), "{output}");
    assert!(!kinds(&events).contains(&"approval_requested"), "{:?}", kinds(&events));
    assert_eq!(judgements(&events), [(JudgeKind::Classifier, Verdict::Allow)]);
    let record = cache_record(&events);
    assert_eq!(record.facts.targets[0].path, entry);
    let [(paths, freed)] = applied(&events).try_into().unwrap();
    assert_eq!(paths, std::slice::from_ref(&entry));
    assert!(freed >= 4096, "{freed}");
    assert!(!entry.exists(), "efrd deleted the dir in the real cache");
    let [result] = recording.tool_results().try_into().unwrap();
    assert!(result.contains("efr deleted 1 path in the real tool caches"), "{result}");
    drop(client);
    daemon.stop().await.unwrap();
}
