use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_patch::{ChangeKind, FileChange, NearLine, Patch, PatchError};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{ApplyPatchTool, Originals, failure, restore_all, written};
use crate::testing::Fixture;
use crate::{
    AccessMode, FileSnapshot, JournalEntry, NoOutput, Original, PathAccess, Tool as _, ToolError,
    ToolGrammar, WriteJournal, WrittenKind,
};

fn tool() -> ApplyPatchTool {
    ApplyPatchTool
}

fn patch(body: &str) -> Value {
    Value::String(format!("*** Begin Patch\n{body}*** End Patch\n"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// `a.txt`, `d.txt` and `e.txt` in the working directory.
fn files(fixture: &Fixture) {
    let cwd = fixture.cwd();
    std::fs::write(cwd.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(cwd.join("d.txt"), "gone\n").unwrap();
    std::fs::write(cwd.join("e.txt"), "import os\nrun()\n").unwrap();
}

/// A patch that updates `a.txt`, adds `new/c.txt`, deletes `d.txt` and moves `e.txt`
/// to `f.txt` with a change.
fn every_kind() -> Value {
    patch(
        "*** Update File: a.txt\n@@\n one\n-two\n+2\n three\n\
         *** Add File: new/c.txt\n+hello\n\
         *** Delete File: d.txt\n\
         *** Update File: e.txt\n*** Move to: f.txt\n@@\n-import os\n+import sys\n run()\n",
    )
}

#[test]
fn the_spec_is_a_freeform_tool_with_the_patch_grammar() {
    let spec = ApplyPatchTool.spec();
    assert_eq!(spec.name, "apply_patch");
    assert_eq!(spec.grammar, Some(ToolGrammar::Lark(efr_patch::GRAMMAR.to_owned())));
    assert_eq!(spec.input_schema["required"], json!(["input"]));
    assert!(spec.description.contains("*** Begin Patch"), "{}", spec.description);
    assert!(spec.description.contains("sed -i"), "{}", spec.description);
}

#[test]
fn every_path_is_a_write_and_a_delete_or_a_move_is_destructive() {
    let fixture = Fixture::new();
    let ctx = fixture.context();
    let write = |path: PathBuf| PathAccess { path, mode: AccessMode::Write };

    let routine = tool()
        .requirements(
            &ctx,
            &patch("*** Update File: a.txt\n@@\n-x\n+y\n*** Add File: ~/n.md\n+n\n"),
        )
        .unwrap();
    assert_eq!(
        routine.paths,
        [write(fixture.cwd().join("a.txt")), write(fixture.home().join("n.md"))]
    );
    assert!(!routine.destructive);

    let delete = tool().requirements(&ctx, &patch("*** Delete File: sub/../d.txt\n")).unwrap();
    assert_eq!(delete.paths, [write(fixture.cwd().join("d.txt"))]);
    assert!(delete.destructive);

    let moved = tool()
        .requirements(&ctx, &patch("*** Update File: e.txt\n*** Move to: /srv/f.txt\n@@\n-a\n+b\n"))
        .unwrap();
    assert_eq!(
        moved.paths,
        [write(fixture.cwd().join("e.txt")), write(PathBuf::from("/srv/f.txt"))]
    );
    assert!(moved.destructive);
    assert!(moved.command.is_none() && !moved.network && !moved.interactive);
}

#[test]
fn the_function_form_reads_the_same_patch() {
    let fixture = Fixture::new();
    let text = patch("*** Delete File: d.txt\n");
    let function = json!({ "input": text });
    let ctx = fixture.context();
    assert_eq!(
        tool().requirements(&ctx, &text).unwrap(),
        tool().requirements(&ctx, &function).unwrap()
    );
}

#[test]
fn an_input_that_is_no_patch_is_refused_before_it_runs() {
    let fixture = Fixture::new();
    let ctx = fixture.context();
    let not_text = tool().requirements(&ctx, &json!({ "path": "a.txt" }));
    assert!(matches!(not_text, Err(ToolError::InvalidInput { .. })), "{not_text:?}");
    let not_a_patch = tool().requirements(&ctx, &json!("sed -i s/a/b/ a.txt"));
    assert!(
        matches!(&not_a_patch, Err(ToolError::Patch { source: PatchError::Parse { line: 1, .. } })),
        "{not_a_patch:?}"
    );
}

#[tokio::test]
async fn a_patch_of_every_kind_changes_each_file_and_says_what_it_did() {
    let fixture = Fixture::new();
    files(&fixture);
    let cwd = fixture.cwd();

    let result = tool().invoke(fixture.context(), every_kind(), &mut NoOutput).await.unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert_eq!(
        result.output,
        "Success. Updated: a.txt; Added: new/c.txt; Deleted: d.txt; Moved: e.txt -> f.txt"
    );
    assert_eq!(read(&cwd.join("a.txt")), "one\n2\nthree\n");
    assert_eq!(read(&cwd.join("new/c.txt")), "hello\n");
    assert!(!cwd.join("d.txt").exists());
    assert!(!cwd.join("e.txt").exists());
    assert_eq!(read(&cwd.join("f.txt")), "import sys\nrun()\n");

    let kinds: Vec<(PathBuf, WrittenKind)> =
        result.written.iter().map(|file| (file.path.clone(), file.kind.clone())).collect();
    assert_eq!(
        kinds,
        [
            (cwd.join("a.txt"), WrittenKind::Changed),
            (cwd.join("new/c.txt"), WrittenKind::Created),
            (cwd.join("d.txt"), WrittenKind::Deleted),
            (cwd.join("f.txt"), WrittenKind::Moved { from: cwd.join("e.txt") }),
        ]
    );
    let diffs: Vec<String> =
        result.written.iter().map(|file| file.diff.clone().unwrap().text).collect();
    let (a, c, d, e, f) = (
        cwd.join("a.txt").display().to_string(),
        cwd.join("new/c.txt").display().to_string(),
        cwd.join("d.txt").display().to_string(),
        cwd.join("e.txt").display().to_string(),
        cwd.join("f.txt").display().to_string(),
    );
    assert_eq!(
        diffs,
        [
            format!("--- a{a}\n+++ b{a}\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n"),
            format!("--- /dev/null\n+++ b{c}\n@@ -0,0 +1,1 @@\n+hello\n"),
            format!("--- a{d}\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-gone\n"),
            format!("--- a{e}\n+++ b{f}\n@@ -1,2 +1,2 @@\n-import os\n+import sys\n run()\n"),
        ]
    );

    let journalled: Vec<(PathBuf, bool)> = fixture
        .journal
        .entries()
        .into_iter()
        .map(|entry| (entry.snapshot.path, entry.snapshot.original == Original::Missing))
        .collect();
    assert_eq!(
        journalled,
        [
            (cwd.join("a.txt"), false),
            (cwd.join("new/c.txt"), true),
            (cwd.join("d.txt"), false),
            (cwd.join("e.txt"), false),
            (cwd.join("f.txt"), true),
        ]
    );
}

#[tokio::test]
async fn a_file_keeps_its_mode_and_owner_and_a_new_one_gets_0644() {
    let fixture = Fixture::new();
    let cwd = fixture.cwd();
    std::fs::write(cwd.join("run.sh"), "echo a\n").unwrap();
    std::fs::set_permissions(cwd.join("run.sh"), std::fs::Permissions::from_mode(0o750)).unwrap();
    std::fs::write(cwd.join("key.txt"), "a\n").unwrap();
    std::fs::set_permissions(cwd.join("key.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let before = std::fs::metadata(cwd.join("run.sh")).unwrap();
    let input = patch(
        "*** Update File: run.sh\n@@\n-echo a\n+echo b\n\
         *** Update File: key.txt\n*** Move to: moved.txt\n@@\n-a\n+b\n\
         *** Add File: fresh.txt\n+x\n",
    );

    let result = tool().invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(!result.is_error, "{}", result.output);
    let mode =
        |name: &str| std::fs::metadata(cwd.join(name)).unwrap().permissions().mode() & 0o7777;
    assert_eq!((mode("run.sh"), mode("moved.txt"), mode("fresh.txt")), (0o750, 0o600, 0o644));
    let after = std::fs::metadata(cwd.join("run.sh")).unwrap();
    assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
}

#[tokio::test]
async fn a_hunk_that_does_not_match_changes_no_file() {
    let fixture = Fixture::new();
    files(&fixture);
    let cwd = fixture.cwd();
    let input = patch(
        "*** Update File: a.txt\n@@\n-two\n+2\n\
         *** Delete File: d.txt\n\
         *** Update File: e.txt\n@@\n-not in the file\n+x\n",
    );

    let result = tool().invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert!(result.output.starts_with("hunk 1 does not match the lines of "), "{}", result.output);
    assert!(result.output.ends_with("No file was changed."), "{}", result.output);
    assert_eq!(read(&cwd.join("a.txt")), "one\ntwo\nthree\n");
    assert!(cwd.join("d.txt").exists());
    assert!(result.written.is_empty());
    assert!(fixture.journal.entries().is_empty(), "nothing is journalled before a match");
}

#[tokio::test]
async fn an_update_of_a_missing_file_changes_nothing() {
    let fixture = Fixture::new();
    files(&fixture);
    let input = patch("*** Delete File: d.txt\n*** Update File: nope.txt\n@@\n-a\n+b\n");

    let result = tool().invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    let missing = fixture.cwd().join("nope.txt");
    assert_eq!(
        result.output,
        format!(
            "{} does not exist.\nAdd the file with *** Add File, or check the path.\n\
             No file was changed.",
            missing.display()
        )
    );
    assert!(fixture.cwd().join("d.txt").exists());
}

/// A journal that records `ok` entries, then fails.
#[derive(Debug)]
struct FailingJournal {
    ok: usize,
    seen: AtomicUsize,
    entries: Mutex<Vec<JournalEntry>>,
}

#[async_trait]
impl WriteJournal for FailingJournal {
    async fn record(&self, entry: JournalEntry) -> Result<(), ToolError> {
        if self.seen.fetch_add(1, Ordering::SeqCst) >= self.ok {
            return Err(ToolError::journal(
                entry.snapshot.path,
                std::io::Error::other("store is down"),
            ));
        }
        self.entries.lock().unwrap().push(entry);
        Ok(())
    }
}

#[tokio::test]
async fn a_failure_after_some_writes_puts_every_file_back() {
    let fixture = Fixture::new();
    files(&fixture);
    let cwd = fixture.cwd();
    std::fs::set_permissions(cwd.join("a.txt"), std::fs::Permissions::from_mode(0o640)).unwrap();
    // a.txt, then new/c.txt, then d.txt, then e.txt and f.txt: the journal fails at the
    // entry of f.txt, after the first three changes were made.
    let journal = Arc::new(FailingJournal {
        ok: 4,
        seen: AtomicUsize::new(0),
        entries: Mutex::new(Vec::new()),
    });
    let mut ctx = fixture.context();
    ctx.journal = Arc::clone(&journal) as Arc<dyn WriteJournal>;

    let result = tool().invoke(ctx, every_kind(), &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert_eq!(
        result.output,
        format!(
            "could not record the original of {} before writing it: store is down. No file \
             was changed.",
            cwd.join("f.txt").display()
        )
    );
    assert_eq!(read(&cwd.join("a.txt")), "one\ntwo\nthree\n");
    assert_eq!(std::fs::metadata(cwd.join("a.txt")).unwrap().permissions().mode() & 0o7777, 0o640);
    assert!(!cwd.join("new").exists(), "the directory that the call made is gone too");
    assert_eq!(read(&cwd.join("d.txt")), "gone\n");
    assert_eq!(read(&cwd.join("e.txt")), "import os\nrun()\n");
    assert!(!cwd.join("f.txt").exists());
    assert!(result.written.is_empty());
}

#[tokio::test]
async fn nothing_is_written_when_the_first_journal_entry_fails() {
    let fixture = Fixture::new();
    files(&fixture);
    let journal = Arc::new(FailingJournal {
        ok: 0,
        seen: AtomicUsize::new(0),
        entries: Mutex::new(Vec::new()),
    });
    let mut ctx = fixture.context();
    ctx.journal = journal;

    let result = tool().invoke(ctx, every_kind(), &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert_eq!(read(&fixture.cwd().join("a.txt")), "one\ntwo\nthree\n");
}

#[test]
fn putting_back_restores_content_and_mode_and_removes_a_new_file() {
    let fixture = Fixture::new();
    let cwd = fixture.cwd();
    std::fs::write(cwd.join("kept.txt"), "changed\n").unwrap();
    std::fs::write(cwd.join("new.txt"), "made by the call\n").unwrap();
    let uid = std::fs::metadata(cwd.join("kept.txt")).unwrap().uid();
    let gid = std::fs::metadata(cwd.join("kept.txt")).unwrap().gid();
    let snapshots = [
        FileSnapshot::new(cwd.join("new.txt"), Original::Missing),
        FileSnapshot::new(
            cwd.join("kept.txt"),
            Original::File { mode: 0o600, uid, gid, content: b"original\n".to_vec() },
        ),
        FileSnapshot::new(cwd.join("never-made.txt"), Original::Missing),
    ];

    let failed = restore_all(&snapshots, &[]);

    assert!(failed.is_empty(), "{failed:?}");
    assert!(!cwd.join("new.txt").exists());
    assert_eq!(read(&cwd.join("kept.txt")), "original\n");
    assert_eq!(
        std::fs::metadata(cwd.join("kept.txt")).unwrap().permissions().mode() & 0o7777,
        0o600
    );
}

#[test]
fn putting_back_removes_the_empty_directories_that_the_call_made() {
    let fixture = Fixture::new();
    let cwd = fixture.cwd();
    std::fs::create_dir_all(cwd.join("made/deep")).unwrap();
    std::fs::write(cwd.join("made/deep/new.txt"), "x\n").unwrap();
    std::fs::create_dir_all(cwd.join("kept")).unwrap();
    std::fs::write(cwd.join("kept/other.txt"), "written by another program\n").unwrap();
    let snapshots = [FileSnapshot::new(cwd.join("made/deep/new.txt"), Original::Missing)];
    let dirs = [cwd.join("made"), cwd.join("made/deep"), cwd.join("kept")];

    let failed = restore_all(&snapshots, &dirs);

    assert!(failed.is_empty(), "{failed:?}");
    assert!(!cwd.join("made").exists());
    assert_eq!(read(&cwd.join("kept/other.txt")), "written by another program\n");
}

/// A journal that holds the entry `stop_at` (from 1) until `release` is notified,
/// and notifies `reached` when it gets there.
#[derive(Debug, Default)]
struct GateJournal {
    stop_at: usize,
    seen: AtomicUsize,
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl WriteJournal for GateJournal {
    async fn record(&self, _entry: JournalEntry) -> Result<(), ToolError> {
        if self.seen.fetch_add(1, Ordering::SeqCst) + 1 == self.stop_at {
            self.reached.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn an_interrupt_during_the_writes_puts_every_file_back() {
    let fixture = Fixture::new();
    let cwd = fixture.cwd();
    std::fs::write(cwd.join("a.txt"), "one\n").unwrap();
    std::fs::write(cwd.join("b.txt"), "two\n").unwrap();
    let journal = Arc::new(GateJournal { stop_at: 2, ..GateJournal::default() });
    let mut ctx = fixture.context();
    ctx.journal = Arc::clone(&journal) as Arc<dyn WriteJournal>;
    let input = patch(
        "*** Update File: a.txt\n@@\n-one\n+ONE\n\
         *** Add File: new/deep/c.txt\n+c\n\
         *** Update File: b.txt\n@@\n-two\n+TWO\n",
    );

    // The turn drops the call's future when the user interrupts it, as here: the
    // journal holds the entry of the second file, after the first one was written.
    let tool = tool();
    let mut out = NoOutput;
    tokio::select! {
        result = tool.invoke(ctx, input, &mut out) => panic!("the call ended: {result:?}"),
        () = journal.reached.notified() => {}
    }
    assert_eq!(read(&cwd.join("a.txt")), "ONE\n", "the first file was written");
    journal.release.notify_one();

    // NOTE: the task of the writes ends on its own once the call's future is gone, so
    // nothing can await it.
    Wait::new("a.txt put back").until(|| read(&cwd.join("a.txt")) == "one\n").await.unwrap();
    Wait::new("new/ removed").until(|| !cwd.join("new").exists()).await.unwrap();
    assert_eq!(read(&cwd.join("b.txt")), "two\n");
}

#[tokio::test]
async fn a_moved_file_keeps_its_mode_when_moves_are_chained() {
    let fixture = Fixture::new();
    let cwd = fixture.cwd();
    std::fs::write(cwd.join("a.sh"), "script\n").unwrap();
    std::fs::set_permissions(cwd.join("a.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(cwd.join("c.sh"), "other\n").unwrap();
    std::fs::set_permissions(cwd.join("c.sh"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let input = patch(
        "*** Update File: a.sh\n*** Move to: b.sh\n\
         *** Update File: c.sh\n*** Move to: a.sh\n",
    );

    let result = tool().invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert_eq!(
        result.output,
        "Success. Updated: a.sh (from c.sh); Added: b.sh (from a.sh); Deleted: c.sh"
    );
    let mode =
        |name: &str| std::fs::metadata(cwd.join(name)).unwrap().permissions().mode() & 0o7777;
    assert_eq!((read(&cwd.join("b.sh")), mode("b.sh")), ("script\n".to_owned(), 0o755));
    assert_eq!((read(&cwd.join("a.sh")), mode("a.sh")), ("other\n".to_owned(), 0o644));
    assert!(!cwd.join("c.sh").exists());
}

#[tokio::test]
async fn a_path_through_a_symlink_is_refused_and_nothing_changes() {
    let fixture = Fixture::new();
    files(&fixture);
    let elsewhere = fixture.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("x.txt"), "x\n").unwrap();
    symlink(&elsewhere, fixture.cwd().join("link")).unwrap();
    let input = patch("*** Delete File: d.txt\n*** Update File: link/x.txt\n@@\n-x\n+y\n");

    let result = tool().invoke(fixture.context(), input, &mut NoOutput).await;

    match result {
        Err(ToolError::ThroughSymlink { path, real }) => {
            assert_eq!(path, fixture.cwd().join("link/x.txt"));
            assert_eq!(real, elsewhere.join("x.txt"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(read(&elsewhere.join("x.txt")), "x\n");
    assert!(fixture.cwd().join("d.txt").exists());
    assert!(fixture.journal.entries().is_empty());
}

#[tokio::test]
async fn binary_large_and_non_text_files_and_directories_are_refused() {
    let fixture = Fixture::new();
    let cwd = fixture.cwd();
    std::fs::write(cwd.join("blob"), b"a\0b\n").unwrap();
    std::fs::write(cwd.join("latin1"), b"na\xefve\n").unwrap();
    std::fs::File::create(cwd.join("huge")).unwrap().set_len(16 * 1024 * 1024 + 1).unwrap();
    std::fs::create_dir(cwd.join("dir")).unwrap();
    for name in ["blob", "latin1", "huge", "dir"] {
        let input = patch(&format!("*** Delete File: {name}\n"));
        let result = tool().invoke(fixture.context(), input, &mut NoOutput).await;
        let refused = matches!(
            (name, &result),
            ("blob" | "latin1", Err(ToolError::NotText { .. }))
                | ("huge", Err(ToolError::TooLarge { .. }))
                | ("dir", Err(ToolError::NotAFile { .. }))
        );
        assert!(refused, "{name}: {result:?}");
        assert!(cwd.join(name).exists(), "{name}");
    }
}

#[tokio::test]
async fn the_preview_shows_the_diff_of_every_file_and_marks_a_delete_and_a_move() {
    let fixture = Fixture::new();
    files(&fixture);
    let cwd = fixture.cwd();

    let preview = tool().preview(&fixture.context(), &every_kind()).await.unwrap();

    let (a, c, d, e, f) = (
        cwd.join("a.txt").display().to_string(),
        cwd.join("new/c.txt").display().to_string(),
        cwd.join("d.txt").display().to_string(),
        cwd.join("e.txt").display().to_string(),
        cwd.join("f.txt").display().to_string(),
    );
    assert_eq!(
        preview,
        format!(
            "--- a{a}\n+++ b{a}\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n\
             --- /dev/null\n+++ b{c}\n@@ -0,0 +1,1 @@\n+hello\n\
             delete {d}\n--- a{d}\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-gone\n\
             move {e} -> {f}\n--- a{e}\n+++ b{f}\n@@ -1,2 +1,2 @@\n-import os\n+import sys\n run()\n"
        )
    );
    assert_eq!(read(&cwd.join("a.txt")), "one\ntwo\nthree\n", "a preview writes nothing");
    assert!(fixture.journal.entries().is_empty());
    let failing = patch("*** Update File: a.txt\n@@\n-missing\n+x\n");
    assert_eq!(tool().preview(&fixture.context(), &failing).await, None);
}

#[test]
fn a_hunk_that_matches_nowhere_shows_its_anchors_and_the_nearest_lines() {
    let path = PathBuf::from("/w/src/lib.rs");
    let parsed = Patch {
        operations: vec![efr_patch::Operation::Update {
            path: path.clone(),
            move_to: None,
            hunks: vec![
                efr_patch::Hunk::default(),
                efr_patch::Hunk { anchors: vec!["impl Engine".to_owned()], ..Default::default() },
            ],
        }],
    };
    let error = PatchError::NoMatch {
        path,
        hunk: 2,
        nearest: vec![
            NearLine { number: 41, text: "impl Engine {".to_owned() },
            NearLine { number: 42, text: "    fn run(&self) {".to_owned() },
        ],
    };

    assert_eq!(
        failure(&error, &parsed),
        "hunk 2 does not match the lines of /w/src/lib.rs.\n\
         Its @@ lines: impl Engine.\n\
         The nearest lines of the file:\n\
         \x20   41 | impl Engine {\n\
         \x20   42 |     fn run(&self) {\n\
         Read the file again and send a corrected patch.\n\
         No file was changed."
    );
}

#[test]
fn the_diffs_of_all_files_together_stay_within_the_line_limit() {
    let limit = efr_protocol::MAX_CALL_DIFF_LINES;
    let long: String = (0..limit).map(|n| format!("line {n}\n")).collect();
    let changes: Vec<FileChange> = ["/w/a", "/w/b"]
        .into_iter()
        .map(|path| FileChange {
            path: PathBuf::from(path),
            kind: ChangeKind::Added { content: long.clone() },
            from: None,
        })
        .collect();

    let files = written(&changes, &Originals::default());

    let first = files[0].diff.as_ref().unwrap();
    let second = files[1].diff.as_ref().unwrap();
    assert_eq!((first.added, second.added), (limit, limit), "the counts stay whole");
    assert_eq!(first.text.lines().count(), 2 + limit + 1, "header, the limit, the cut line");
    assert_eq!(
        second.text.lines().collect::<Vec<_>>(),
        ["--- /dev/null", "+++ b/w/b", &format!("... {} more lines", limit + 1)]
    );
}

#[tokio::test]
async fn a_hunk_matches_across_trailing_whitespace_and_unicode_punctuation() {
    let fixture = Fixture::new();
    let path = fixture.cwd().join("quote.txt");
    std::fs::write(&path, "say \u{201c}hi\u{201d}   \nend\n").unwrap();
    let input = patch("*** Update File: quote.txt\n@@\n-say \"hi\"\n+say \"bye\"\n end\n");

    let result = ApplyPatchTool.invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert_eq!(read(&path), "say \"bye\"\nend\n");
}

#[tokio::test]
async fn a_miss_of_the_real_engine_names_the_nearest_lines() {
    let fixture = Fixture::new();
    files(&fixture);
    let input = patch("*** Update File: a.txt\n@@\n one\n-too\n+2\n three\n");

    let result = ApplyPatchTool.invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert!(result.output.contains("The nearest lines of the file:\n"), "{}", result.output);
    assert!(result.output.ends_with("No file was changed."), "{}", result.output);
}

#[tokio::test]
async fn an_ambiguous_hunk_names_its_lines_and_asks_for_more_context() {
    let fixture = Fixture::new();
    let path = fixture.cwd().join("twice.txt");
    std::fs::write(&path, "a\nx\nb\nx\n").unwrap();
    let input = patch("*** Update File: twice.txt\n@@\n-x\n+y\n");

    let result = ApplyPatchTool.invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert_eq!(
        result.output,
        format!(
            "hunk 1 matches 2 places in {}.\nIt matches at lines 2, 4.\n\
             Add more context lines, or an @@ line that names the function or class, so the \
             hunk matches one place.\nNo file was changed.",
            path.display()
        )
    );
    assert_eq!(read(&path), "a\nx\nb\nx\n");
}

#[tokio::test]
async fn an_anchor_that_matches_nowhere_shows_the_nearest_lines() {
    let fixture = Fixture::new();
    files(&fixture);
    let input = patch("*** Update File: a.txt\n@@ fn nowhere()\n-two\n+2\n");

    let result = ApplyPatchTool.invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert!(result.output.contains("matches no line of"), "{}", result.output);
    assert!(result.output.contains("Use a line of the file as it is"), "{}", result.output);
    assert!(result.output.ends_with("No file was changed."), "{}", result.output);
}

#[tokio::test]
async fn an_add_of_a_file_that_exists_says_to_update_it() {
    let fixture = Fixture::new();
    files(&fixture);
    let input = patch("*** Add File: a.txt\n+new\n");

    let result = ApplyPatchTool.invoke(fixture.context(), input, &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert_eq!(
        result.output,
        format!(
            "{} already exists.\nUpdate the file, or delete it first in the same patch.\n\
             No file was changed.",
            fixture.cwd().join("a.txt").display()
        )
    );
    assert_eq!(read(&fixture.cwd().join("a.txt")), "one\ntwo\nthree\n");
}
