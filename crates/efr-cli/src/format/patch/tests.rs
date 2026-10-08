use efr_render::{ColourMode, RenderOptions, WidthMethod, display_width};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{DiffFile, Op, PatchFile, file_diffs, files, flow, rows, summary, text};
use crate::format::{Tone, call_text, tool_call};
use crate::testing::readable;

/// A patch that updates two files, adds one, deletes one and moves one.
const PATCH: &str = "*** Begin Patch
*** Update File: src/a.rs
@@ fn main() {
-    old();
+    new();
+    more();
     done();
*** Update File: src/b.rs
@@
+use std::fmt;
*** Add File: notes.md
+# Notes
+
*** Delete File: old.rs
*** Update File: src/expr.rs
*** Move to: src/expression.rs
@@ pub fn parse
-    let a = 1;
+    let a = 2;
*** End of File
*** End Patch
";

fn file(path: &str, op: Op, added: u32, removed: u32) -> PatchFile {
    PatchFile { path: path.to_owned(), op, added, removed }
}

#[test]
fn the_text_comes_from_either_form_of_the_call() {
    assert_eq!(text(&json!("*** Begin Patch\n")), Some("*** Begin Patch\n"));
    assert_eq!(text(&json!({ "input": "*** Begin Patch\n" })), Some("*** Begin Patch\n"));
    assert_eq!(text(&json!({ "patch": "x" })), None);
    assert_eq!(text(&json!(7)), None);
}

#[test]
fn the_files_of_a_patch_with_their_counts_in_order() {
    assert_eq!(
        files(PATCH),
        [
            file("src/a.rs", Op::Update, 2, 1),
            file("src/b.rs", Op::Update, 1, 0),
            file("notes.md", Op::Add, 2, 0),
            file("old.rs", Op::Delete, 0, 0),
            file("src/expr.rs", Op::Move("src/expression.rs".to_owned()), 1, 1),
        ]
    );
    assert_eq!(
        summary(&files(PATCH)),
        "src/a.rs +2 \u{2212}1, src/b.rs +1, new notes.md +2, delete old.rs, \
         move src/expr.rs \u{2192} src/expression.rs +1 \u{2212}1"
    );
}

#[test]
fn a_patch_read_leniently_and_safely() {
    // Spaces around the markers, two updates of one file, and nothing after the end.
    let patch = "  *** Begin Patch\n *** Update File:  src/a.rs \n-a\n+b\n*** Update File: src/a.rs\n+c\n*** End Patch \n+ignored\n";
    assert_eq!(files(patch), [file("src/a.rs", Op::Update, 2, 1)]);
    // A patch without a file names none, and a path cannot drive the terminal.
    assert_eq!(summary(&files("not a patch")), "");
    let patch = "*** Begin Patch\n*** Add File: a\u{1b}[2Jb\n+x\n*** End Patch\n";
    assert_eq!(summary(&files(patch)), "new a\u{241b}[2Jb +1");
}

#[test]
fn the_entries_flow_into_rows_cut_only_between_two_of_them() {
    let entries = super::entries(&files(PATCH));
    let rows = flow(&entries, 26, WidthMethod::CodePoint);
    assert_eq!(
        rows,
        [
            "src/a.rs +2 \u{2212}1,",
            "src/b.rs +1,",
            "new notes.md +2,",
            "delete old.rs,",
            "move src/expr.rs \u{2192}",
            "src/expression.rs +1 \u{2212}1",
        ]
    );
    let wide = flow(&entries, 40, WidthMethod::CodePoint);
    assert_eq!(wide[0], "src/a.rs +2 \u{2212}1, src/b.rs +1,");
    for row in &rows {
        assert!(display_width(row, WidthMethod::CodePoint) <= 26, "{rows:?}");
    }
    // Nothing is lost: the rows joined by a space are the line.
    assert_eq!(rows.join(" "), summary(&files(PATCH)));
    // An entry wider than a row is cut at its spaces.
    let long = vec!["move src/parse/expression.rs \u{2192} src/parse/expr.rs +1".to_owned()];
    let rows = flow(&long, 20, WidthMethod::CodePoint);
    assert_eq!(rows, ["move", "src/parse/expression", ".rs \u{2192}", "src/parse/expr.rs +1"]);
}

#[test]
fn an_apply_patch_call_is_named_by_its_files() {
    let freeform = json!(PATCH);
    let text = call_text("apply_patch", &freeform);
    assert_eq!(text.name, "apply_patch");
    assert_eq!(text.lines, [summary(&files(PATCH))]);
    // The function form reads the same.
    assert_eq!(call_text("apply_patch", &json!({ "input": PATCH })), text);
    let short = "*** Begin Patch\n*** Update File: src/a.rs\n-a\n+b\n+c\n*** Update File: src/b.rs\n+d\n*** End Patch\n";
    assert_eq!(
        tool_call("apply_patch", &json!(short), None),
        "apply_patch src/a.rs +2 \u{2212}1, src/b.rs +1"
    );
    // A patch without a file shows the tool alone.
    assert_eq!(tool_call("apply_patch", &json!("*** Begin Patch\n"), None), "apply_patch");
}

/// A diff of a patch as the daemon sends it: the diffs of its files one after the
/// other, the second with a removed line `-- note` that looks like a header.
const DIFF: &str = "--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn main() {
-    old();
+    new();
+    more();
     done();
--- a/schema.sql
+++ b/schema.sql
@@ -1,2 +1,2 @@
--- note
++++ b
 select 1;
--- /dev/null
+++ b/notes.md
@@ -0,0 +1,2 @@
+# Notes
+
--- a/old.rs
+++ /dev/null
@@ -1,1 +0,0 @@
-fn old() {}
--- a/src/expr.rs
+++ b/src/expression.rs
@@ -1,1 +1,1 @@
-    let a = 1;
+    let a = 2;
... 12 more lines
";

#[test]
fn a_diff_of_several_files_is_cut_into_one_per_file() {
    let diffs = file_diffs(DIFF);
    let kinds: Vec<DiffFile> = diffs.iter().map(|diff| diff.file.clone()).collect();
    assert_eq!(
        kinds,
        [
            DiffFile::Update("src/a.rs".to_owned()),
            DiffFile::Update("schema.sql".to_owned()),
            DiffFile::Add("notes.md".to_owned()),
            DiffFile::Delete("old.rs".to_owned()),
            DiffFile::Move { from: "src/expr.rs".to_owned(), to: "src/expression.rs".to_owned() },
        ]
    );
    // The lines that look like headers stay in their hunk.
    assert_eq!(diffs[1].parts.body, ["@@ -1,2 +1,2 @@", "--- note", "++++ b", " select 1;"]);
    // Each file keeps its own headers, and the daemon's cut belongs to the last.
    assert_eq!(diffs[0].parts.head, ["--- a/src/a.rs", "+++ b/src/a.rs"]);
    assert_eq!(diffs[4].parts.cut, 12);
    assert!(diffs[..4].iter().all(|diff| diff.parts.cut == 0));
    // The parts give the text back.
    let joined: String = diffs.iter().map(super::FileDiff::text).collect();
    assert_eq!(joined, DIFF);
}

#[test]
fn a_preview_marks_a_delete_and_a_move_before_their_diff() {
    let preview = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,1 +1,1 @@\n-a\n+b\ndelete old.rs\n--- a/old.rs\n+++ b/old.rs\n@@ -1,1 +0,0 @@\n-x\nmove a.rs -> b.rs\n--- a/a.rs\n+++ b/b.rs\ndelete empty.txt\n";
    let diffs = file_diffs(preview);
    let kinds: Vec<DiffFile> = diffs.iter().map(|diff| diff.file.clone()).collect();
    assert_eq!(
        kinds,
        [
            DiffFile::Update("src/a.rs".to_owned()),
            DiffFile::Delete("old.rs".to_owned()),
            DiffFile::Move { from: "a.rs".to_owned(), to: "b.rs".to_owned() },
            DiffFile::Delete("empty.txt".to_owned()),
        ]
    );
    // The mark is not a line of the diff.
    assert_eq!(diffs[1].parts.head, ["--- a/old.rs", "+++ b/old.rs"]);
    assert!(diffs[3].parts.body.is_empty() && diffs[3].parts.head.is_empty());
    // A move without a change has its headers and no lines.
    assert_eq!(diffs[2].parts.head, ["--- a/a.rs", "+++ b/b.rs"]);
    assert!(diffs[2].parts.body.is_empty());
    let renamed =
        file_diffs("--- a/x.rs\n+++ b/y.rs\n--- a/z.rs\n+++ b/z.rs\n@@ -1 +1 @@\n-a\n+b\n");
    assert_eq!(renamed[0].file, DiffFile::Move { from: "x.rs".to_owned(), to: "y.rs".to_owned() });
    assert_eq!(renamed[1].file, DiffFile::Update("z.rs".to_owned()));
}

#[test]
fn a_diff_of_one_file_is_one_item() {
    assert_eq!(file_diffs("@@ -1 +1 @@\n-a\n+b\n").len(), 1);
    assert_eq!(
        file_diffs("diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n").len(),
        1
    );
    assert!(file_diffs("").is_empty());
    // A hunk without counts ends at the headers of the next file.
    let loose = "--- a/x\n+++ b/x\n@@\n-a\n+b\n--- a/y\n+++ b/y\n@@\n+c\n";
    assert_eq!(file_diffs(loose).len(), 2);
}

#[test]
fn the_heading_of_a_file_marks_a_delete_and_a_move_in_a_question() {
    let delete = DiffFile::Delete("old.rs".to_owned());
    assert_eq!(delete.heading(true), [("delete old.rs".to_owned(), Tone::Attention)]);
    assert_eq!(
        delete.heading(false),
        [("deleted ".to_owned(), Tone::Dim), ("old.rs".to_owned(), Tone::Code)]
    );
    let moved = DiffFile::Move { from: "a.rs".to_owned(), to: "b.rs".to_owned() };
    assert_eq!(moved.heading(true), [("move a.rs \u{2192} b.rs".to_owned(), Tone::Attention)]);
    assert_eq!(moved.path(), Some("b.rs"));
    let added = DiffFile::Add("n.md".to_owned());
    assert_eq!(
        added.heading(true),
        [("new ".to_owned(), Tone::Dim), ("n.md".to_owned(), Tone::Plain)]
    );
    assert!(DiffFile::Unknown.heading(true).is_empty());
}

#[test]
fn each_file_keeps_its_limit_and_counts_what_it_leaves_out() {
    let diffs = file_diffs(DIFF);
    let options = RenderOptions::new(40).with_colour(ColourMode::None).with_terminal(false);
    let first = rows(&diffs[0].parts, None, 2, "  \u{2502} ", &options);
    assert_eq!(
        first,
        "  \u{2502} @@ -1,3 +1,4 @@\n  \u{2502}  fn main() {\n  \u{2502} \u{2026} 4 more lines\n"
    );
    let last = rows(&diffs[4].parts, None, 20, "  \u{2502} ", &options);
    assert!(last.ends_with("  \u{2502} \u{2026} 12 more lines\n"), "{}", readable(&last));
}
