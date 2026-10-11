//! The applier of `efr undo` in efrd: turns of `write_file` and `apply_patch` calls in
//! `$SCRATCH` and in the home directory, then `Toolbox::undo`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::{Toolbox as _, UndoCandidate, UndoRequest};
use efr_permissions::CoverAsk;
use efr_protocol::{
    CommandId, ConversationId, Origin, TurnId, TurnUndo, TurnUndoResult, UndoAction, UndoEntry,
    UndoId, UndoReason, UndoRefused,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::tools::tests::{Kept, call_of, canonical_temp, home_write, kept, kept_in};
use crate::tools::undo::{NOTE_SPECIAL, NOTE_STOPPED};

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

fn conversation() -> ConversationId {
    ConversationId::from_uuid(id(1))
}

/// Where the calls of a test run: `home/work`, whose `$SCRATCH` is `home/work/scratch`.
fn work(kept: &Kept) -> PathBuf {
    let work = kept.home.join("work");
    std::fs::create_dir_all(work.join("scratch")).unwrap();
    work
}

fn scratch(kept: &Kept) -> PathBuf {
    work(kept).join("scratch")
}

/// One call of the turn `turn` (call `n`) with the approved exits `exits`.
async fn run(
    kept: &Kept,
    turn: u128,
    n: u128,
    name: &str,
    input: serde_json::Value,
    exits: Vec<efr_permissions::ExitNeed>,
) {
    let mut call = call_of(turn, n, name, input, &work(kept));
    call.context = call.context.with_exits(exits);
    let mut sink = |_: &str, _: u64| {};
    let outcome = kept.toolbox.invoke(call, &mut sink).await;
    assert!(!outcome.is_error, "{outcome:?}");
}

/// The end of the turn `turn`.
async fn end(kept: &Kept, turn: u128) {
    kept.toolbox.turn_changes(conversation(), TurnId::from_uuid(id(turn))).await;
}

/// A turn `turn` that writes each `(path, text)` with `write_file`.
async fn write_turn(kept: &Kept, turn: u128, writes: &[(&Path, &str)]) {
    for (n, (path, text)) in writes.iter().enumerate() {
        let input = json!({"path": path, "content": text});
        run(kept, turn, n as u128 + 1, "write_file", input, Vec::new()).await;
    }
    end(kept, turn).await;
}

/// A turn `turn` that deletes `path` with `apply_patch`.
async fn delete_turn(kept: &Kept, turn: u128, path: &Path) {
    let text = format!("*** Begin Patch\n*** Delete File: {}\n*** End Patch\n", path.display());
    run(kept, turn, 1, "apply_patch", json!(text), Vec::new()).await;
    end(kept, turn).await;
}

/// `turn.undo` of the conversation with the turns `turns` (newest last), as `efr undo`
/// sends it.
fn request(turns: &[u128], undo: u128, change: impl FnOnce(&mut TurnUndo)) -> UndoRequest {
    let mut params = TurnUndo {
        command_id: CommandId::from_uuid(id(900 + undo)),
        conversation_id: None,
        turn_id: None,
        dry_run: false,
        force: false,
        redo: false,
        list: false,
    };
    change(&mut params);
    UndoRequest {
        conversation_id: conversation(),
        candidates: turns
            .iter()
            .rev()
            .map(|turn| UndoCandidate {
                turn_id: TurnId::from_uuid(id(*turn)),
                title: Some(format!("turn {turn}")),
                ended: None,
                stopped: false,
            })
            .collect(),
        params,
        undo_id: UndoId::from_uuid(uuid::Uuid::from_u128(
            0x0192_f8a0_0000_7000_8000_0000_0000_0000 + undo,
        )),
        origin: Origin::Cli,
    }
}

async fn undo(
    kept: &Kept,
    turns: &[u128],
    undo: u128,
    change: impl FnOnce(&mut TurnUndo),
) -> Result<TurnUndoResult, UndoRefused> {
    kept.toolbox.undo(request(turns, undo, change)).await
}

fn entry(action: UndoAction, path: &str, reason: Option<UndoReason>) -> UndoEntry {
    UndoEntry { action, path: path.to_owned(), reason }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Each file below `dir` with its bytes and mode, sorted.
fn files_below(dir: &Path) -> Vec<(PathBuf, String, u32)> {
    let mut found = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(at) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                todo.push(path);
            } else {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                found.push((path.clone(), text, mode(&path)));
            }
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn undo_restores_modified_and_trashes_created() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::write(scratch.join("a.txt"), "old\n").unwrap();

    write_turn(&kept, 2, &[(&scratch.join("a.txt"), "new\n"), (&scratch.join("b.txt"), "made\n")])
        .await;
    let preview = undo(&kept, &[2], 1, |params| params.dry_run = true).await.unwrap();
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    let expected = [
        entry(UndoAction::Restore, "$SCRATCH/a.txt", None),
        entry(UndoAction::Trash, "$SCRATCH/b.txt", None),
    ];
    assert_eq!(preview.entries, expected);
    assert!(!preview.applied);
    assert_eq!(done.entries, expected);
    assert!(done.applied, "{done:?}");
    assert_eq!(done.turn_id, Some(TurnId::from_uuid(id(2))));
    assert!(done.notes.contains(&NOTE_SPECIAL.to_owned()));
    assert_eq!(read(&scratch.join("a.txt")), "old\n");
    assert!(!scratch.join("b.txt").exists());
    let trash = done.trash.unwrap();
    let kept_files: Vec<String> =
        files_below(&trash).into_iter().map(|(_, text, _)| text).collect();
    assert_eq!(kept_files, ["new\n", "made\n"], "what undo overwrote or removed, by path");
    assert!(trash.starts_with(root.join("d/undo-trash")));
}

#[tokio::test]
async fn undo_skips_changed_after_turn() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::write(scratch.join("a.txt"), "old\n").unwrap();

    write_turn(&kept, 2, &[(&scratch.join("a.txt"), "new\n")]).await;
    std::fs::write(scratch.join("a.txt"), "mine\n").unwrap();
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert!(!done.applied, "nothing to write: {done:?}");
    assert_eq!(
        done.entries,
        [entry(UndoAction::Skip, "$SCRATCH/a.txt", Some(UndoReason::ChangedAfterTurn))]
    );
    assert_eq!(read(&scratch.join("a.txt")), "mine\n");
}

#[tokio::test]
async fn undo_force_overrides() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::write(scratch.join("a.txt"), "old\n").unwrap();

    write_turn(&kept, 2, &[(&scratch.join("a.txt"), "new\n")]).await;
    std::fs::write(scratch.join("a.txt"), "mine\n").unwrap();
    let done = undo(&kept, &[2], 1, |params| params.force = true).await.unwrap();

    assert!(done.applied, "{done:?}");
    assert_eq!(read(&scratch.join("a.txt")), "old\n");
    let trashed: Vec<String> =
        files_below(&done.trash.unwrap()).into_iter().map(|(_, text, _)| text).collect();
    assert_eq!(trashed, ["mine\n"], "the change after the turn goes to the trash");
}

#[tokio::test]
async fn undo_keeps_private_mode_of_env_file() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    let env = scratch.join(".env");
    std::fs::write(&env, "TOKEN=1\n").unwrap();
    std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o600)).unwrap();

    delete_turn(&kept, 2, &env).await;
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert!(done.applied, "{done:?}");
    assert_eq!(read(&env), "TOKEN=1\n");
    assert_eq!(mode(&env), 0o600, "never 0644 for a private file");
}

#[tokio::test]
async fn undo_drops_set_id_bits() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    let tool = scratch.join("tool");
    std::fs::write(&tool, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o4755)).unwrap();
    if mode(&tool) & 0o4000 == 0 {
        // NOTE: a mount with nosuid keeps no set-id bit at all.
        return;
    }

    delete_turn(&kept, 2, &tool).await;
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert!(done.applied, "{done:?}");
    assert_eq!(mode(&tool), 0o755);
}

#[tokio::test]
async fn undo_makes_a_missing_dir_private() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    let private = scratch.join("private");
    std::fs::create_dir_all(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(private.join("notes.md"), "old\n").unwrap();

    delete_turn(&kept, 2, &private.join("notes.md")).await;
    std::fs::remove_dir(&private).unwrap();
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert!(done.applied, "{done:?}");
    assert_eq!(read(&private.join("notes.md")), "old\n");
    assert_eq!(mode(&private), 0o700, "no snapshot records a dir's mode, so it comes back private");
}

#[tokio::test]
async fn undo_trash_lands_in_undo_trash_with_modes() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    let file = scratch.join("a.txt");
    std::fs::write(&file, "old\n").unwrap();

    write_turn(&kept, 2, &[(&file, "new\n")]).await;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    let trash = done.trash.unwrap();
    assert_eq!(
        trash.file_name().unwrap().to_str().unwrap(),
        UndoId::from_uuid(uuid::Uuid::from_u128(0x0192_f8a0_0000_7000_8000_0000_0000_0001)).shown()
    );
    let [(path, text, file_mode)] = files_below(&trash).try_into().unwrap();
    assert_eq!((text.as_str(), file_mode), ("new\n", 0o640));
    assert!(path.ends_with("a.txt"));
    for dir in path.ancestors().skip(1).take_while(|dir| dir.starts_with(root.join("d/undo-trash")))
    {
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode(&file), 0o640, "a restored file keeps its mode");
}

#[tokio::test]
async fn undo_trash_works_across_file_systems() {
    // NOTE: the root lies in the system temp dir, often a RAM file system, and efr's
    // data root next to the test binary: the trash copy comes from the store, never
    // from a rename out of the root.
    let (_temp, root) = canonical_temp();
    let exe = std::env::current_exe().unwrap();
    let data = tempfile::tempdir_in(exe.parent().unwrap()).unwrap();
    let kept = kept_in(&root, data.path(), Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::write(scratch.join("a.txt"), "old\n").unwrap();

    write_turn(&kept, 2, &[(&scratch.join("a.txt"), "new\n")]).await;
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert!(done.applied, "{done:?}");
    assert_eq!(read(&scratch.join("a.txt")), "old\n");
    let trash = done.trash.unwrap();
    assert!(trash.starts_with(data.path()));
    assert_eq!(files_below(&trash).len(), 1);
}

#[tokio::test]
async fn undo_takes_the_plan_lock() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::write(scratch.join("a.txt"), "old\n").unwrap();
    write_turn(&kept, 2, &[(&scratch.join("a.txt"), "new\n")]).await;
    let (sandbox, _) = kept.toolbox.sandbox.as_ref().unwrap();
    let guard = sandbox.locks().lock(std::slice::from_ref(&scratch)).await;

    let undoing = undo(&kept, &[2], 1, |_| {});
    tokio::pin!(undoing);
    let waited = tokio::time::timeout(Duration::from_millis(300), &mut undoing).await;
    let before = read(&scratch.join("a.txt"));
    drop(guard);
    let done = undoing.await.unwrap();

    assert!(waited.is_err(), "the undo waits for the plan lock");
    assert_eq!(before, "new\n", "nothing written while another plan holds the lock");
    assert!(done.applied);
    assert_eq!(read(&scratch.join("a.txt")), "old\n");
}

#[tokio::test]
async fn undo_refuses_a_replaced_root() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::write(scratch.join("a.txt"), "old\n").unwrap();

    write_turn(&kept, 2, &[(&scratch.join("a.txt"), "new\n")]).await;
    std::fs::rename(&scratch, kept.home.join("work/scratch.old")).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    std::fs::write(scratch.join("a.txt"), "new\n").unwrap();
    let refused = undo(&kept, &[2], 1, |_| {}).await.unwrap();
    let kept_text = read(&scratch.join("a.txt"));
    let forced = undo(&kept, &[2], 1, |params| params.force = true).await.unwrap();

    assert_eq!(
        refused.entries,
        [entry(UndoAction::Skip, "$SCRATCH/a.txt", Some(UndoReason::RootReplaced))]
    );
    assert_eq!(kept_text, "new\n", "a replaced root waits for force");
    assert!(forced.applied, "{forced:?}");
    assert_eq!(read(&scratch.join("a.txt")), "old\n", "force writes into the new root");
}

#[tokio::test]
async fn undo_never_follows_symlinks() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::create_dir_all(scratch.join("sub")).unwrap();
    std::fs::write(scratch.join("sub/a.txt"), "old\n").unwrap();

    write_turn(&kept, 2, &[(&scratch.join("sub/a.txt"), "new\n")]).await;
    let outside = root.join("outside");
    std::fs::rename(scratch.join("sub"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, scratch.join("sub")).unwrap();
    let done = undo(&kept, &[2], 1, |params| params.force = true).await;

    assert_eq!(read(&outside.join("a.txt")), "new\n", "the write never goes through the link");
    if let Ok(done) = done {
        assert!(!done.entries.iter().any(|entry| entry.action == UndoAction::Restore), "{done:?}");
    }
}

#[tokio::test]
async fn undo_write_error_stops_and_reports() {
    if rustix::process::geteuid().is_root() {
        // NOTE: root writes into a read-only directory.
        return;
    }
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::create_dir_all(scratch.join("sub")).unwrap();
    std::fs::write(scratch.join("b.txt"), "old b\n").unwrap();
    std::fs::write(scratch.join("sub/a.txt"), "old a\n").unwrap();

    write_turn(
        &kept,
        2,
        &[(&scratch.join("b.txt"), "new b\n"), (&scratch.join("sub/a.txt"), "new a\n")],
    )
    .await;
    std::fs::set_permissions(scratch.join("sub"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let stopped = undo(&kept, &[2], 1, |_| {}).await.unwrap();
    std::fs::set_permissions(scratch.join("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let resumed = undo(&kept, &[2], 2, |_| {}).await.unwrap();

    assert!(!stopped.applied && stopped.stopped, "{stopped:?}");
    assert_eq!(
        stopped.entries,
        [
            entry(UndoAction::Restore, "$SCRATCH/b.txt", None),
            entry(UndoAction::Skip, "$SCRATCH/sub/a.txt", Some(UndoReason::Unknown)),
        ]
    );
    assert!(stopped.notes.iter().any(|note| note.starts_with(NOTE_STOPPED)), "{:?}", stopped.notes);
    assert!(resumed.resumed && resumed.applied, "{resumed:?}");
    assert_eq!(resumed.undo_id, stopped.undo_id, "the next undo resumes the one that stopped");
    assert_eq!(read(&scratch.join("b.txt")), "old b\n");
    assert_eq!(read(&scratch.join("sub/a.txt")), "old a\n");
}

#[tokio::test]
async fn redo_takes_back_an_undo_of_root_and_home_files() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);
    std::fs::write(scratch.join("a.txt"), "old\n").unwrap();
    let notes = kept.home.join("notes.md");
    std::fs::write(&notes, "old notes\n").unwrap();

    run(
        &kept,
        2,
        1,
        "write_file",
        json!({"path": scratch.join("a.txt"), "content": "new\n"}),
        Vec::new(),
    )
    .await;
    run(
        &kept,
        2,
        2,
        "write_file",
        json!({"path": notes, "content": "new notes\n"}),
        vec![home_write(&notes)],
    )
    .await;
    end(&kept, 2).await;
    let undone = undo(&kept, &[2], 1, |_| {}).await.unwrap();
    let after_undo = (read(&scratch.join("a.txt")), read(&notes));
    let redone = undo(&kept, &[2], 2, |params| params.redo = true).await.unwrap();
    let no_more = undo(&kept, &[2], 3, |params| params.redo = true).await;

    assert!(undone.applied && redone.applied, "{undone:?} {redone:?}");
    assert_eq!(after_undo, ("old\n".to_owned(), "old notes\n".to_owned()));
    assert_eq!(
        (read(&scratch.join("a.txt")), read(&notes)),
        ("new\n".to_owned(), "new notes\n".to_owned())
    );
    assert_eq!(redone.undo_id, undone.undo_id, "a redo takes back that undo");
    assert_eq!(no_more, Err(UndoRefused::NoRedo));
}

#[tokio::test]
async fn undo_lists_and_refuses_what_it_cannot_take() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let scratch = scratch(&kept);

    write_turn(&kept, 2, &[(&scratch.join("a.txt"), "one\n")]).await;
    write_turn(&kept, 3, &[(&scratch.join("a.txt"), "two\n")]).await;
    let listed = undo(&kept, &[2, 3], 1, |params| params.list = true).await.unwrap();
    let older =
        undo(&kept, &[2, 3], 1, |params| params.turn_id = Some(TurnId::from_uuid(id(2)))).await;
    let phone = kept
        .toolbox
        .undo(UndoRequest { origin: Origin::Phone, ..request(&[2, 3], 1, |_| {}) })
        .await;
    let mut off = Settings::default();
    off.undo.enabled = false;
    kept.settings.send_replace(Arc::new(off));
    let disabled = undo(&kept, &[2, 3], 1, |_| {}).await;

    let turns: Vec<(TurnId, bool, bool)> =
        listed.turns.iter().map(|turn| (turn.turn_id, turn.auto, turn.undone)).collect();
    assert_eq!(
        turns,
        [(TurnId::from_uuid(id(3)), true, false), (TurnId::from_uuid(id(2)), true, false)]
    );
    assert_eq!(older, Err(UndoRefused::LaterTurnsNotUndone { turn_id: TurnId::from_uuid(id(3)) }));
    assert_eq!(phone, Err(UndoRefused::PhoneOrigin));
    assert_eq!(disabled, Err(UndoRefused::Off));
}

/// A turn `turn` that writes the home file `path` with `text`, copied first.
async fn home_turn(
    kept: &Kept,
    turn: u128,
    path: &Path,
    text: &str,
    exits: Vec<efr_permissions::ExitNeed>,
) {
    run(kept, turn, 1, "write_file", json!({"path": path, "content": text}), exits).await;
    end(kept, turn).await;
}

#[tokio::test]
async fn undo_restores_home_file() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let notes = kept.home.join("Documents/notes.md");
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(&notes, "old\n").unwrap();

    home_turn(&kept, 2, &notes, "new\n", vec![home_write(&notes)]).await;
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert!(done.applied, "{done:?}");
    assert_eq!(done.entries, [entry(UndoAction::Restore, "~/Documents/notes.md", None)]);
    assert_eq!(read(&notes), "old\n");
    let trashed: Vec<String> =
        files_below(&done.trash.unwrap()).into_iter().map(|(_, text, _)| text).collect();
    assert_eq!(trashed, ["new\n"]);
}

#[tokio::test]
async fn undo_deletes_home_file_that_was_missing() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let made = kept.home.join("made.md");

    home_turn(&kept, 2, &made, "new\n", vec![home_write(&made)]).await;
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert_eq!(done.entries, [entry(UndoAction::Trash, "~/made.md", None)]);
    assert!(!made.exists());
    let trash = done.trash.unwrap();
    let relative = made.strip_prefix("/").unwrap();
    assert_eq!(read(&trash.join("home").join(relative)), "new\n", "it lands in the undo trash");
}

#[tokio::test]
async fn undo_home_conflict_listed() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let notes = kept.home.join("notes.md");
    std::fs::write(&notes, "old\n").unwrap();

    home_turn(&kept, 2, &notes, "new\n", vec![home_write(&notes)]).await;
    std::fs::write(&notes, "mine\n").unwrap();
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert_eq!(
        done.entries,
        [entry(UndoAction::Skip, "~/notes.md", Some(UndoReason::ChangedAfterTurn))]
    );
    assert_eq!(read(&notes), "mine\n");
}

#[tokio::test]
async fn undo_rechecks_masks_for_home_paths() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let notes = kept.home.join("private/notes.md");
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(&notes, "old\n").unwrap();

    home_turn(&kept, 2, &notes, "new\n", vec![home_write(&notes)]).await;
    let mut masked = Settings::default();
    masked.sandbox.mask = vec![PathBuf::from("~/private")];
    kept.settings.send_replace(Arc::new(masked));
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert_eq!(
        done.entries,
        [entry(UndoAction::NotCovered, "~/private/notes.md", Some(UndoReason::Masked))]
    );
    assert_eq!(read(&notes), "new\n", "a path that a mask covers now stays");
    // NOTE: the undo wrote nothing, so the turn is not undone and no undo stays open.
    assert!(!done.applied && !done.stopped, "{done:?}");
    let turns =
        kept.toolbox.snapshots.as_ref().unwrap().store().turns_with_refs(conversation()).await;
    assert!(turns.iter().all(|refs| !refs.undone && refs.started_undo.is_none()), "{turns:?}");
    kept.settings.send_replace(Arc::new(Settings::default()));
    let again = undo(&kept, &[2], 2, |_| {}).await.unwrap();
    assert!(again.applied && !again.resumed, "{again:?}");
    assert_eq!(read(&notes), "old\n");
}

#[tokio::test]
async fn undo_trashes_new_home_file_instead_of_deleting() {
    let (_temp, root) = canonical_temp();
    let kept = kept(&root, Settings::default()).await;
    let app = kept.home.join(".config/app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("app.conf"), "a=1\n").unwrap();
    let made = app.join("extra.conf");
    // NOTE: a contained call that gets the whole dir (`WriteBind::Parent`): the copy
    // walks it, so a file that the call makes there was missing before.
    let mut need = home_write(&app.join("app.conf"));
    need.copies = vec![CoverAsk::HomeDir(app.clone())];

    home_turn(&kept, 2, &made, "b=2\n", vec![need]).await;
    let done = undo(&kept, &[2], 1, |_| {}).await.unwrap();

    assert_eq!(done.entries, [entry(UndoAction::Trash, "~/.config/app/extra.conf", None)]);
    assert!(!made.exists());
    assert_eq!(read(&app.join("app.conf")), "a=1\n");
    let [(_, text, _)] = files_below(&done.trash.unwrap()).try_into().unwrap();
    assert_eq!(text, "b=2\n");
}
