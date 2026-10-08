use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{ChangeKind, FileChange, Files, apply};
use crate::{NearLine, PatchError, parse};

fn files(entries: &[(&str, &str)]) -> BTreeMap<PathBuf, String> {
    entries.iter().map(|&(path, text)| (PathBuf::from(path), text.to_owned())).collect()
}

fn run(patch: &str, entries: &[(&str, &str)]) -> Result<Vec<FileChange>, PatchError> {
    let patch = parse(patch).unwrap_or_else(|error| panic!("the patch parses: {error}"));
    apply(&patch, &files(entries))
}

/// The new text of `a.txt` after an update of it with `hunks`.
fn update(text: &str, hunks: &str) -> Result<String, PatchError> {
    let changes = run(
        &format!("*** Begin Patch\n*** Update File: a.txt\n{hunks}*** End Patch\n"),
        &[("a.txt", text)],
    )?;
    match changes.as_slice() {
        [] => Ok(text.to_owned()),
        [FileChange { kind: ChangeKind::Updated { content }, .. }] => Ok(content.clone()),
        other => panic!("expected one update, got {other:?}"),
    }
}

fn change(path: &str, kind: ChangeKind) -> FileChange {
    FileChange { path: path.into(), kind }
}

fn near(lines: &[(usize, &str)]) -> Vec<NearLine> {
    lines.iter().map(|&(number, text)| NearLine { number, text: text.to_owned() }).collect()
}

#[test]
fn a_map_gives_the_text_of_a_path_and_none_for_a_missing_one() {
    let mut hashed: HashMap<PathBuf, String> = HashMap::new();
    hashed.insert(PathBuf::from("/w/a.txt"), "a\n".to_owned());
    let sorted: BTreeMap<PathBuf, String> = hashed.clone().into_iter().collect();
    for files in [&hashed as &dyn Files, &sorted as &dyn Files] {
        assert_eq!(files.text(Path::new("/w/a.txt")), Some("a\n"));
        assert_eq!(files.text(Path::new("/w/b.txt")), None);
    }
}

#[test]
fn one_hunk_replaces_a_line_between_context() {
    assert_eq!(update("a\nb\nc\n", "@@\n a\n-b\n+B\n c\n"), Ok("a\nB\nc\n".to_owned()));
}

#[test]
fn several_hunks_apply_from_the_top_down() {
    let text = "a\nb\nc\nd\ne\nf\n";
    let hunks = "@@\n a\n-b\n+B\n@@\n c\n d\n-e\n+E\n@@\n f\n+g\n*** End of File\n";
    assert_eq!(update(text, hunks), Ok("a\nB\nc\nd\nE\nf\ng\n".to_owned()));
}

#[test]
fn hunks_at_the_start_and_the_end_of_the_file() {
    let text = "first\nsecond\nthird\nlast\n";
    let hunks = "@@\n-first\n+FIRST\n second\n@@\n third\n-last\n+LAST\n";
    assert_eq!(update(text, hunks), Ok("FIRST\nsecond\nthird\nLAST\n".to_owned()));
}

#[test]
fn end_of_file_matches_only_at_the_end() {
    let text = "x\nend\nx\nend\n";
    assert_eq!(
        update(text, "@@\n x\n-end\n+END\n*** End of File\n"),
        Ok("x\nend\nx\nEND\n".to_owned())
    );
    // Without the marker the same hunk matches twice.
    assert_eq!(
        update(text, "@@\n x\n-end\n+END\n"),
        Err(PatchError::Ambiguous { path: "a.txt".into(), hunk: 1, lines: vec![1, 3] })
    );
}

#[test]
fn end_of_file_that_is_not_the_end_does_not_match() {
    assert!(matches!(
        update("a\nb\n", "@@\n-a\n+A\n*** End of File\n"),
        Err(PatchError::NoMatch { hunk: 1, .. })
    ));
}

#[test]
fn a_hunk_of_only_added_lines_appends_to_the_file() {
    assert_eq!(update("a\n", "@@\n+b\n+c\n"), Ok("a\nb\nc\n".to_owned()));
}

#[test]
fn an_appending_hunk_leaves_the_cursor_for_the_next_hunk() {
    let hunks = "@@\n+after\n@@\n line1\n-line2\n-line3\n+replaced\n";
    assert_eq!(update("line1\nline2\nline3\n", hunks), Ok("line1\nreplaced\nafter\n".to_owned()));
}

#[test]
fn a_hunk_of_only_added_lines_inserts_after_its_anchor() {
    let text = "fn a() {\n}\nfn b() {\n}\n";
    assert_eq!(
        update(text, "@@ fn a() {\n+    one();\n"),
        Ok("fn a() {\n    one();\n}\nfn b() {\n}\n".to_owned())
    );
}

#[test]
fn a_deletion_only_hunk_removes_lines() {
    assert_eq!(
        update("line1\nline2\nline3\n", "@@\n line1\n-line2\n line3\n"),
        Ok("line1\nline3\n".to_owned())
    );
}

#[test]
fn anchors_pick_the_place_among_repeated_lines() {
    let text = "impl A {\n    fn run() {\n        go();\n    }\n}\nimpl B {\n    fn run() {\n        go();\n    }\n}\n";
    let hunks = "@@ impl B {\n@@     fn run() {\n-        go();\n+        stop();\n";
    assert_eq!(
        update(text, hunks),
        Ok("impl A {\n    fn run() {\n        go();\n    }\n}\nimpl B {\n    fn run() {\n        stop();\n    }\n}\n"
            .to_owned())
    );
}

#[test]
fn an_anchor_may_be_the_first_context_line_of_its_hunk() {
    let text = "def f():\n    return 1\n";
    assert_eq!(
        update(text, "@@ def f():\n def f():\n-    return 1\n+    return 2\n"),
        Ok("def f():\n    return 2\n".to_owned())
    );
}

#[test]
fn an_anchor_matches_with_tolerance() {
    let text = "class A:\n    def f(self):\n        pass\n";
    assert_eq!(
        update(text, "@@ def f(self):\n-        pass\n+        return 1\n"),
        Ok("class A:\n    def f(self):\n        return 1\n".to_owned())
    );
}

#[test]
fn a_missing_anchor_names_it_and_shows_the_nearest_lines() {
    let text = "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n";
    assert_eq!(
        update(text, "@@ fn betas() {}\n-x\n"),
        Err(PatchError::NoAnchor {
            path: "a.txt".into(),
            hunk: 1,
            anchor: "fn betas() {}".to_owned(),
            nearest: near(&[(1, "fn alpha() {}"), (2, "fn beta() {}"), (3, "fn gamma() {}")]),
        })
    );
}

#[test]
fn a_context_that_appears_twice_is_ambiguous_without_an_anchor() {
    let text = "start\n    x = 1\nmiddle\n    x = 1\nend\n";
    assert_eq!(
        update(text, "@@\n-    x = 1\n+    x = 2\n"),
        Err(PatchError::Ambiguous { path: "a.txt".into(), hunk: 1, lines: vec![2, 4] })
    );
    // An anchor or more context makes it unique.
    assert_eq!(
        update(text, "@@ middle\n-    x = 1\n+    x = 2\n"),
        Ok("start\n    x = 1\nmiddle\n    x = 2\nend\n".to_owned())
    );
    assert_eq!(
        update(text, "@@\n start\n-    x = 1\n+    x = 2\n"),
        Ok("start\n    x = 2\nmiddle\n    x = 1\nend\n".to_owned())
    );
}

#[test]
fn a_repeat_before_the_previous_hunk_does_not_make_a_hunk_ambiguous() {
    let text = "x\na\nx\nb\n";
    assert_eq!(update(text, "@@\n a\n@@\n-x\n+X\n"), Ok("x\na\nX\nb\n".to_owned()));
}

#[test]
fn a_hunk_before_the_previous_one_does_not_match() {
    let text = "a\nb\nc\n";
    assert!(matches!(
        update(text, "@@\n-c\n+C\n@@\n-a\n+A\n"),
        Err(PatchError::NoMatch { hunk: 2, .. })
    ));
}

#[test]
fn a_miss_names_the_hunk_and_shows_the_nearest_lines() {
    let text = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
    assert_eq!(
        update(text, "@@\n fn main() {\n-    let x = 2;\n+    let x = 3;\n"),
        Err(PatchError::NoMatch {
            path: "a.txt".into(),
            hunk: 1,
            nearest: near(&[
                (1, "fn main() {"),
                (2, "    let x = 1;"),
                (3, "    println!(\"{x}\");")
            ]),
        })
    );
}

#[test]
fn trailing_whitespace_in_the_file_does_not_stop_a_match() {
    assert_eq!(update("a  \nb\t\nc\n", "@@\n a\n-b\n+B\n"), Ok("a  \nB\nc\n".to_owned()));
}

#[test]
fn indentation_drift_matches_and_keeps_the_file_s_context_lines() {
    let text = "\tif x {\n\t\ty();\n\t}\n";
    assert_eq!(
        update(text, "@@\n if x {\n-    y();\n+\t\tz();\n }\n"),
        Ok("\tif x {\n\t\tz();\n\t}\n".to_owned())
    );
}

#[test]
fn unicode_punctuation_in_the_file_matches_ascii_in_the_patch() {
    let text = "# local import \u{2013} avoids top\u{2011}level dep\nprint(\u{201C}hi\u{201D})\n";
    assert_eq!(
        update(text, "@@\n-# local import - avoids top-level dep\n+# HELLO\n print(\"hi\")\n"),
        Ok("# HELLO\nprint(\u{201C}hi\u{201D})\n".to_owned())
    );
}

#[test]
fn unicode_text_in_a_line_matches_exactly() {
    assert_eq!(
        update(
            "line1\ngr\u{fc}\u{df}e \u{4e16}\u{754c}\n",
            "@@\n line1\n-gr\u{fc}\u{df}e \u{4e16}\u{754c}\n+gr\u{fc}\u{df}e \u{4e16}\u{754c} \u{2705}\n"
        ),
        Ok("line1\ngr\u{fc}\u{df}e \u{4e16}\u{754c} \u{2705}\n".to_owned())
    );
}

#[test]
fn crlf_files_keep_their_line_endings() {
    assert_eq!(
        update("one\r\ntwo\r\nthree\r\n", "@@\n-one\n+ONE\n two\n+between\n three\n"),
        Ok("ONE\r\ntwo\r\nbetween\r\nthree\r\n".to_owned())
    );
}

#[test]
fn mixed_line_endings_stay_on_their_lines() {
    assert_eq!(
        update("one\r\ntwo\nthree\r\nfour\n", "@@\n one\n two\n-three\n+THREE\n four\n"),
        Ok("one\r\ntwo\nTHREE\r\nfour\n".to_owned())
    );
}

#[test]
fn a_file_without_a_final_newline_keeps_that() {
    assert_eq!(update("a\nb", "@@\n a\n-b\n+B\n"), Ok("a\nB".to_owned()));
    assert_eq!(update("a\nb", "@@\n a\n b\n+c\n"), Ok("a\nb\nc".to_owned()));
    assert_eq!(update("only", "@@\n-only\n+first\n+second\n"), Ok("first\nsecond".to_owned()));
    assert_eq!(update("a\r\nb", "@@\n+c\n"), Ok("a\r\nb\r\nc".to_owned()));
}

#[test]
fn a_file_with_a_final_newline_keeps_it() {
    assert_eq!(update("a\nb\n", "@@\n a\n-b\n"), Ok("a\n".to_owned()));
}

#[test]
fn an_empty_context_line_for_the_final_newline_is_dropped() {
    assert_eq!(update("a\nb\n", "@@\n a\n-b\n+B\n \n"), Ok("a\nB\n".to_owned()));
    assert_eq!(update("a\nb\n", "@@\n a\n-b\n+B\n\n"), Ok("a\nB\n".to_owned()));
}

#[test]
fn an_empty_file_takes_added_lines() {
    assert_eq!(update("", "@@\n+first\n"), Ok("first\n".to_owned()));
}

#[test]
fn an_empty_file_has_no_context() {
    assert_eq!(
        update("", "@@\n x\n+y\n"),
        Err(PatchError::NoMatch { path: "a.txt".into(), hunk: 1, nearest: Vec::new() })
    );
}

#[test]
fn removing_every_line_leaves_an_empty_file() {
    assert_eq!(update("a\nb\n", "@@\n-a\n-b\n"), Ok(String::new()));
    assert_eq!(update("a\nb", "@@\n-a\n-b\n"), Ok(String::new()));
}

#[test]
fn a_hunk_that_changes_nothing_gives_no_change() {
    assert_eq!(
        run("*** Begin Patch\n*** Update File: a\n a\n*** End Patch", &[("a", "a\n")]),
        Ok(Vec::new())
    );
}

#[test]
fn add_delete_and_update_in_one_patch() {
    let patch = "*** Begin Patch
*** Add File: new.txt
+hello
*** Delete File: gone.txt
*** Update File: kept.txt
-old
+new
*** End Patch";
    assert_eq!(
        run(patch, &[("gone.txt", "bye\n"), ("kept.txt", "old\n")]),
        Ok(vec![
            change("new.txt", ChangeKind::Added { content: "hello\n".to_owned() }),
            change("gone.txt", ChangeKind::Deleted),
            change("kept.txt", ChangeKind::Updated { content: "new\n".to_owned() }),
        ])
    );
}

#[test]
fn a_move_with_an_update_writes_the_new_content_at_the_target() {
    let patch = "*** Begin Patch
*** Update File: src/old.rs
*** Move to: src/new.rs
@@ fn run()
-    a();
+    b();
*** End Patch";
    assert_eq!(
        run(patch, &[("src/old.rs", "fn run() {\n    a();\n}\n")]),
        Ok(vec![change(
            "src/old.rs",
            ChangeKind::Moved {
                to: "src/new.rs".into(),
                content: "fn run() {\n    b();\n}\n".to_owned()
            }
        )])
    );
}

#[test]
fn a_rename_keeps_the_content() {
    let patch = "*** Begin Patch\n*** Update File: a\n*** Move to: b\n*** End Patch";
    assert_eq!(
        run(patch, &[("a", "x\r\ny")]),
        Ok(vec![change("a", ChangeKind::Moved { to: "b".into(), content: "x\r\ny".to_owned() })])
    );
}

#[test]
fn a_move_to_the_same_path_is_an_update() {
    let patch = "*** Begin Patch\n*** Update File: a\n*** Move to: a\n-x\n+y\n*** End Patch";
    assert_eq!(
        run(patch, &[("a", "x\n")]),
        Ok(vec![change("a", ChangeKind::Updated { content: "y\n".to_owned() })])
    );
}

#[test]
fn missing_and_existing_files_are_typed_errors() {
    let cases = [
        ("*** Add File: a\n+x", PatchError::Exists { path: "a".into() }),
        ("*** Delete File: none", PatchError::Missing { path: "none".into() }),
        ("*** Update File: none\n-x", PatchError::Missing { path: "none".into() }),
        ("*** Update File: none\n*** Move to: c", PatchError::Missing { path: "none".into() }),
        ("*** Update File: a\n*** Move to: b", PatchError::Exists { path: "b".into() }),
    ];
    for (body, error) in cases {
        let patch = format!("*** Begin Patch\n{body}\n*** End Patch");
        assert_eq!(run(&patch, &[("a", "x\n"), ("b", "y\n")]), Err(error), "{body}");
    }
}

#[test]
fn an_error_in_a_later_operation_fails_the_whole_patch() {
    let patch = "*** Begin Patch\n*** Add File: created.txt\n+hello\n*** Update File: missing.txt\n-old\n+new\n*** End Patch";
    assert_eq!(run(patch, &[]), Err(PatchError::Missing { path: "missing.txt".into() }));
}

#[test]
fn later_operations_see_the_earlier_ones() {
    let patch = "*** Begin Patch
*** Add File: a
+one
*** Update File: a
-one
+two
*** Delete File: b
*** Add File: b
+fresh
*** End Patch";
    assert_eq!(
        run(patch, &[("b", "stale\n")]),
        Ok(vec![
            change("a", ChangeKind::Added { content: "two\n".to_owned() }),
            change("b", ChangeKind::Updated { content: "fresh\n".to_owned() }),
        ])
    );
}

#[test]
fn an_added_then_deleted_file_is_no_change() {
    let patch = "*** Begin Patch\n*** Add File: a\n+x\n*** Delete File: a\n*** End Patch";
    assert_eq!(run(patch, &[]), Ok(Vec::new()));
}

#[test]
fn moves_in_a_chain_fold_into_one_move() {
    let patch = "*** Begin Patch
*** Update File: a
*** Move to: b
-1
+2
*** Update File: b
*** Move to: c
-2
+3
*** End Patch";
    assert_eq!(
        run(patch, &[("a", "1\n")]),
        Ok(vec![change("a", ChangeKind::Moved { to: "c".into(), content: "3\n".to_owned() })])
    );
}

#[test]
fn a_move_onto_a_file_deleted_before_is_a_delete_and_an_update() {
    let patch =
        "*** Begin Patch\n*** Delete File: b\n*** Update File: a\n*** Move to: b\n*** End Patch";
    assert_eq!(
        run(patch, &[("a", "new\n"), ("b", "old\n")]),
        Ok(vec![
            change("b", ChangeKind::Updated { content: "new\n".to_owned() }),
            change("a", ChangeKind::Deleted),
        ])
    );
}

#[test]
fn an_update_after_a_move_folds_into_the_move() {
    let patch = "*** Begin Patch
*** Update File: a
*** Move to: b
*** Update File: b
-1
+2
*** End Patch";
    assert_eq!(
        run(patch, &[("a", "1\n")]),
        Ok(vec![change("a", ChangeKind::Moved { to: "b".into(), content: "2\n".to_owned() })])
    );
}

#[test]
fn a_file_moved_away_and_added_again_is_an_update_and_an_add() {
    let patch = "*** Begin Patch\n*** Update File: a\n*** Move to: b\n*** Add File: a\n+fresh\n*** End Patch";
    assert_eq!(
        run(patch, &[("a", "old\n")]),
        Ok(vec![
            change("a", ChangeKind::Updated { content: "fresh\n".to_owned() }),
            change("b", ChangeKind::Added { content: "old\n".to_owned() }),
        ])
    );
}

#[test]
fn the_second_hunk_of_an_update_counts_in_errors() {
    assert!(matches!(
        update("a\nb\n", "@@\n-a\n+A\n@@\n-zzz\n+y\n"),
        Err(PatchError::NoMatch { hunk: 2, .. })
    ));
}
