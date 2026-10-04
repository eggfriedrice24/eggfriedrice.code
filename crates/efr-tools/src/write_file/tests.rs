use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::sync::Arc;

use async_trait::async_trait;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::WriteFileTool;
use crate::testing::Fixture;
use crate::{
    AccessMode, JournalEntry, NoOutput, Original, PathAccess, Tool as _, ToolError, WriteJournal,
};

#[test]
fn it_declares_the_resolved_path_for_writing() {
    let fixture = Fixture::new();
    let requirements = WriteFileTool::new()
        .requirements(&fixture.context(), &json!({"path": "out/a.txt", "content": "x"}))
        .unwrap();
    assert_eq!(
        requirements.paths,
        [PathAccess { path: fixture.cwd().join("out/a.txt"), mode: AccessMode::Write }]
    );
}

#[tokio::test]
async fn a_new_file_is_created_and_journalled_as_missing() {
    let fixture = Fixture::new();
    let result = WriteFileTool::new()
        .invoke(
            fixture.context(),
            json!({"path": "new/dir/a.txt", "content": "hello"}),
            &mut NoOutput,
        )
        .await
        .unwrap();
    let path = fixture.cwd().join("new/dir/a.txt");
    assert_eq!(result.output, format!("created {} (5 bytes)", path.display()));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777, 0o644);
    let entries = fixture.journal.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].snapshot.path, path);
    assert_eq!(entries[0].snapshot.original, Original::Missing);
    assert_eq!(entries[0].ids, crate::testing::ids());
}

#[tokio::test]
async fn an_existing_file_is_snapshotted_and_keeps_its_mode() {
    let fixture = Fixture::new();
    let path = fixture.cwd().join("script.sh");
    std::fs::write(&path, "old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let result = WriteFileTool::new()
        .invoke(fixture.context(), json!({"path": "script.sh", "content": "new"}), &mut NoOutput)
        .await
        .unwrap();
    assert!(result.output.starts_with("replaced "));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(after.permissions().mode() & 0o7777, 0o750);
    assert_eq!((after.uid(), after.gid()), (metadata.uid(), metadata.gid()));
    let entries = fixture.journal.entries();
    assert_eq!(
        entries[0].snapshot.original,
        Original::File {
            mode: 0o750,
            uid: metadata.uid(),
            gid: metadata.gid(),
            content: b"old".to_vec()
        }
    );
}

#[derive(Debug)]
struct BrokenJournal;

#[async_trait]
impl WriteJournal for BrokenJournal {
    async fn record(&self, entry: JournalEntry) -> Result<(), ToolError> {
        Err(ToolError::journal(entry.snapshot.path, std::io::Error::other("store is down")))
    }
}

#[tokio::test]
async fn nothing_is_written_when_the_journal_fails() {
    let fixture = Fixture::new();
    let path = fixture.cwd().join("keep.txt");
    std::fs::write(&path, "original").unwrap();
    let mut ctx = fixture.context();
    ctx.journal = Arc::new(BrokenJournal);
    let result = WriteFileTool::new()
        .invoke(ctx, json!({"path": "keep.txt", "content": "changed"}), &mut NoOutput)
        .await;
    assert!(matches!(result, Err(ToolError::Journal { .. })), "{result:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
}

#[tokio::test]
async fn a_path_through_a_symlink_is_not_written() {
    let fixture = Fixture::new();
    let elsewhere = fixture.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, fixture.cwd().join("link")).unwrap();
    let result = WriteFileTool::new()
        .invoke(fixture.context(), json!({"path": "link/x", "content": "x"}), &mut NoOutput)
        .await;
    assert!(matches!(result, Err(ToolError::ThroughSymlink { .. })), "{result:?}");
    assert!(!elsewhere.join("x").exists());
    assert!(fixture.journal.entries().is_empty());
}

#[tokio::test]
async fn a_directory_or_a_dangling_link_is_not_a_file() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.cwd().join("dir")).unwrap();
    symlink(fixture.root().join("gone"), fixture.cwd().join("dangling")).unwrap();
    let tool = WriteFileTool::new();
    for path in ["dir", "dangling"] {
        let result = tool
            .invoke(fixture.context(), json!({"path": path, "content": "x"}), &mut NoOutput)
            .await;
        assert!(matches!(result, Err(ToolError::NotAFile { .. })), "{path}: {result:?}");
    }
}

#[tokio::test]
async fn the_preview_of_a_replacement_is_a_diff_and_writes_nothing() {
    let fixture = Fixture::new();
    let path = fixture.home().join(".zshrc");
    std::fs::write(&path, "export PATH\nalias ll='ls -l'\n").unwrap();
    let input = json!({"path": "~/.zshrc", "content": "export PATH\nalias ll='ls -la'\n"});

    let preview = WriteFileTool::new().preview(&fixture.context(), &input).await.unwrap();

    let shown = path.display();
    assert_eq!(
        preview,
        format!(
            "--- a{shown}\n+++ b{shown}\n@@ -1,2 +1,2 @@\n export PATH\n-alias ll='ls -l'\n+alias ll='ls -la'\n"
        )
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "export PATH\nalias ll='ls -l'\n");
    assert!(fixture.journal.entries().is_empty());
}

#[tokio::test]
async fn the_preview_of_a_new_file_shows_all_of_it() {
    let fixture = Fixture::new();
    let input = json!({"path": "notes/todo.txt", "content": "one\n"});

    let preview = WriteFileTool::new().preview(&fixture.context(), &input).await.unwrap();

    let shown = fixture.cwd().join("notes/todo.txt");
    assert_eq!(
        preview,
        format!("--- /dev/null\n+++ b{}\n@@ -0,0 +1,1 @@\n+one\n", shown.display())
    );
    assert!(!shown.exists());
}

#[tokio::test]
async fn there_is_no_preview_through_a_symbolic_link() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root().join("real.txt"), "x").unwrap();
    symlink(fixture.root().join("real.txt"), fixture.cwd().join("link.txt")).unwrap();
    let input = json!({"path": "link.txt", "content": "y"});

    assert_eq!(WriteFileTool::new().preview(&fixture.context(), &input).await, None);
}
