use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::EditTool;
use crate::testing::Fixture;
use crate::{
    AccessMode, JournalEntry, NoOutput, Original, PathAccess, Tool as _, ToolError, WriteJournal,
    WrittenKind,
};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn edit(path: &str, old: &str, new: &str) -> Value {
    json!({"path": path, "old_string": old, "new_string": new})
}

/// `a.rs` in the working directory, with `fn old()` twice.
fn file(fixture: &Fixture) -> std::path::PathBuf {
    let path = fixture.cwd().join("a.rs");
    std::fs::write(&path, "fn old() {}\nfn main() {\n    old();\n}\nfn old() {}\n").unwrap();
    path
}

#[test]
fn the_spec_takes_a_path_two_strings_and_replace_all() {
    let spec = EditTool.spec();
    assert_eq!(spec.name, "edit");
    assert_eq!(spec.grammar, None);
    assert_eq!(spec.input_schema["required"], json!(["path", "old_string", "new_string"]));
    assert_eq!(spec.input_schema["additionalProperties"], json!(false));
    assert_eq!(spec.input_schema["properties"]["replace_all"]["type"], "boolean");
    assert_eq!(spec.input_schema["properties"]["replace_all"]["default"], json!(false));
    assert!(spec.description.contains("sed -i"), "{}", spec.description);
    assert!(!spec.description.contains("must read"), "it promises no read check");
}

#[test]
fn it_declares_the_resolved_path_as_a_write_and_is_never_destructive() {
    let fixture = Fixture::new();
    let requirements =
        EditTool.requirements(&fixture.context(), &edit("src/../a.rs", "a", "b")).unwrap();
    assert_eq!(
        requirements.paths,
        [PathAccess { path: fixture.cwd().join("a.rs"), mode: AccessMode::Write }]
    );
    assert!(!requirements.destructive);
    assert!(requirements.command.is_none() && !requirements.network);
}

#[test]
fn an_input_that_does_not_match_the_schema_is_refused() {
    let fixture = Fixture::new();
    let unknown = json!({"path": "a.rs", "old_string": "a", "new_string": "b", "all": true});
    let refused = EditTool.requirements(&fixture.context(), &unknown);
    assert!(matches!(refused, Err(ToolError::InvalidInput { .. })), "{refused:?}");
}

#[tokio::test]
async fn a_unique_match_is_replaced_and_journalled() {
    let fixture = Fixture::new();
    let path = file(&fixture);

    let result = EditTool
        .invoke(fixture.context(), edit("a.rs", "    old();", "    new();"), &mut NoOutput)
        .await
        .unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert_eq!(result.output, "Success. Replaced 1 match in a.rs.");
    assert_eq!(read(&path), "fn old() {}\nfn main() {\n    new();\n}\nfn old() {}\n");
    let written = &result.written[0];
    assert_eq!((written.path.as_path(), &written.kind), (path.as_path(), &WrittenKind::Changed));
    let diff = written.diff.as_ref().unwrap();
    assert_eq!((diff.added, diff.removed), (1, 1));
    let entries = fixture.journal.entries();
    assert_eq!(entries.len(), 1);
    match &entries[0].snapshot.original {
        Original::File { content, .. } => assert!(content.starts_with(b"fn old() {}\n")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn several_matches_are_refused_unless_replace_all() {
    let fixture = Fixture::new();
    let path = file(&fixture);
    let before = read(&path);

    let refused = EditTool
        .invoke(fixture.context(), edit("a.rs", "fn old() {}", "fn new() {}"), &mut NoOutput)
        .await
        .unwrap();

    assert!(refused.is_error);
    assert_eq!(
        refused.output,
        "Found 2 matches of old_string, but replace_all is false. Give more context to make \
         one match, or set replace_all to true."
    );
    assert_eq!(read(&path), before);
    assert!(fixture.journal.entries().is_empty());

    let mut all = edit("a.rs", "old", "new");
    all["replace_all"] = json!(true);
    let result = EditTool.invoke(fixture.context(), all, &mut NoOutput).await.unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert_eq!(result.output, "Success. Replaced 3 matches in a.rs.");
    assert_eq!(read(&path), "fn new() {}\nfn main() {\n    new();\n}\nfn new() {}\n");
}

#[tokio::test]
async fn text_that_is_not_found_shows_the_nearest_lines() {
    let fixture = Fixture::new();
    let path = file(&fixture);

    let result = EditTool
        .invoke(fixture.context(), edit("a.rs", "fn main() {\n    olde();", "x"), &mut NoOutput)
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(
        result.output,
        "old_string was not found in the file.\n\
         The nearest lines are:\n\
         \x20    1 | fn old() {}\n\
         \x20    2 | fn main() {\n\
         \x20    3 |     old();\n\
         \x20    4 | }\n\
         Read the file again and copy old_string exactly as it is."
    );
    assert!(read(&path).contains("    old();"));
    assert!(fixture.journal.entries().is_empty());
}

#[tokio::test]
async fn an_empty_old_string_and_no_change_are_refused_before_the_file_is_read() {
    let fixture = Fixture::new();
    let path = file(&fixture);
    let before = read(&path);
    let cases = [
        (edit("a.rs", "", "x"), "old_string is empty. Use write_file to make a new file."),
        (edit("a.rs", "old", "old"), "No change: old_string and new_string are the same."),
        (edit("missing.rs", "", "x"), "old_string is empty. Use write_file to make a new file."),
    ];
    for (input, expected) in cases {
        let result = EditTool.invoke(fixture.context(), input, &mut NoOutput).await.unwrap();
        assert!(result.is_error);
        assert_eq!(result.output, expected);
    }
    assert_eq!(read(&path), before);
    assert!(fixture.journal.entries().is_empty());
}

#[tokio::test]
async fn a_missing_file_says_to_use_write_file() {
    let fixture = Fixture::new();

    let result =
        EditTool.invoke(fixture.context(), edit("new.rs", "a", "b"), &mut NoOutput).await.unwrap();

    assert!(result.is_error);
    assert_eq!(
        result.output,
        format!(
            "{} does not exist. Use write_file to make a new file.",
            fixture.cwd().join("new.rs").display()
        )
    );
    assert!(!fixture.cwd().join("new.rs").exists());
}

#[tokio::test]
async fn a_crlf_file_matches_lf_strings_and_keeps_its_line_ends() {
    let fixture = Fixture::new();
    let path = fixture.cwd().join("win.txt");
    std::fs::write(&path, "one\r\ntwo\r\nthree\r\n").unwrap();

    let result = EditTool
        .invoke(fixture.context(), edit("win.txt", "one\ntwo\n", "one\n2\nmore\n"), &mut NoOutput)
        .await
        .unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert_eq!(read(&path), "one\r\n2\r\nmore\r\nthree\r\n");
}

#[tokio::test]
async fn a_file_with_mixed_line_ends_takes_the_strings_as_written() {
    let fixture = Fixture::new();
    let path = fixture.cwd().join("mixed.txt");
    std::fs::write(&path, "one\r\ntwo\nthree\r\n").unwrap();

    let result = EditTool
        .invoke(fixture.context(), edit("mixed.txt", "two\nthree", "2\n3"), &mut NoOutput)
        .await
        .unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert_eq!(read(&path), "one\r\n2\n3\r\n");
}

#[tokio::test]
async fn a_file_keeps_its_mode_and_owner() {
    let fixture = Fixture::new();
    let path = fixture.cwd().join("run.sh");
    std::fs::write(&path, "echo a\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    let before = std::fs::metadata(&path).unwrap();

    let result = EditTool
        .invoke(fixture.context(), edit("run.sh", "echo a", "echo b"), &mut NoOutput)
        .await
        .unwrap();

    assert!(!result.is_error, "{}", result.output);
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(after.permissions().mode() & 0o7777, 0o750);
    assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
}

#[tokio::test]
async fn a_path_through_a_symlink_and_a_binary_file_are_refused() {
    let fixture = Fixture::new();
    let elsewhere = fixture.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("x.txt"), "x\n").unwrap();
    symlink(&elsewhere, fixture.cwd().join("link")).unwrap();
    std::fs::write(fixture.cwd().join("blob"), b"a\0b\n").unwrap();

    let linked =
        EditTool.invoke(fixture.context(), edit("link/x.txt", "x", "y"), &mut NoOutput).await;
    assert!(matches!(linked, Err(ToolError::ThroughSymlink { .. })), "{linked:?}");
    let binary = EditTool.invoke(fixture.context(), edit("blob", "a", "b"), &mut NoOutput).await;
    assert!(matches!(binary, Err(ToolError::NotText { .. })), "{binary:?}");
    assert_eq!(read(&elsewhere.join("x.txt")), "x\n");
    assert!(fixture.journal.entries().is_empty());
}

/// A journal that records no entry and fails.
#[derive(Debug)]
struct FailingJournal;

#[async_trait]
impl WriteJournal for FailingJournal {
    async fn record(&self, entry: JournalEntry) -> Result<(), ToolError> {
        Err(ToolError::journal(entry.snapshot.path, std::io::Error::other("store is down")))
    }
}

#[tokio::test]
async fn nothing_is_written_when_the_journal_fails() {
    let fixture = Fixture::new();
    let path = file(&fixture);
    let before = read(&path);
    let mut ctx = fixture.context();
    ctx.journal = Arc::new(FailingJournal);

    let result = EditTool.invoke(ctx, edit("a.rs", "main", "start"), &mut NoOutput).await;

    assert!(matches!(result, Err(ToolError::Journal { .. })), "{result:?}");
    assert_eq!(read(&path), before);
}

/// A journal that keeps each entry, then holds the call until `release` is notified,
/// and notifies `reached` when it gets there.
#[derive(Debug, Default)]
struct GateJournal {
    entries: Mutex<Vec<JournalEntry>>,
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl WriteJournal for GateJournal {
    async fn record(&self, entry: JournalEntry) -> Result<(), ToolError> {
        self.entries.lock().unwrap().push(entry);
        self.reached.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn an_interrupt_while_the_journal_records_leaves_the_file_as_it_was() {
    let fixture = Fixture::new();
    let path = file(&fixture);
    let before = read(&path);
    let journal = Arc::new(GateJournal::default());
    let mut ctx = fixture.context();
    ctx.journal = Arc::clone(&journal) as Arc<dyn WriteJournal>;

    // The turn drops the call's future when the user interrupts it, as here: the
    // journal holds the call after it kept the original.
    let mut out = NoOutput;
    tokio::select! {
        result = EditTool.invoke(ctx, edit("a.rs", "main", "start"), &mut out) => {
            panic!("the call ended: {result:?}")
        }
        () = journal.reached.notified() => {}
    }
    journal.release.notify_one();
    tokio::task::yield_now().await;

    assert_eq!(read(&path), before);
    let entries = journal.entries.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].snapshot.path, path);
}

#[tokio::test]
async fn the_preview_is_the_diff_of_the_file_and_writes_nothing() {
    let fixture = Fixture::new();
    let path = file(&fixture);
    let shown = path.display().to_string();

    let preview =
        EditTool.preview(&fixture.context(), &edit("a.rs", "    old();", "    new();")).await;

    assert_eq!(
        preview.unwrap(),
        format!(
            "--- a{shown}\n+++ b{shown}\n@@ -1,5 +1,5 @@\n fn old() {{}}\n fn main() {{\n\
             -    old();\n+    new();\n }}\n fn old() {{}}\n"
        )
    );
    assert!(read(&path).contains("    old();"), "a preview writes nothing");
    assert!(fixture.journal.entries().is_empty());
    let failing = edit("a.rs", "fn old() {}", "x");
    assert_eq!(EditTool.preview(&fixture.context(), &failing).await, None);
    assert_eq!(EditTool.preview(&fixture.context(), &edit("a.rs", "a", "a")).await, None);
}
