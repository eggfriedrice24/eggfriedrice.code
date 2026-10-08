use pretty_assertions::assert_eq;

use super::parse;
use crate::{Hunk, HunkLine, Operation, ParseProblem, Patch, PatchError};

fn context(text: &str) -> HunkLine {
    HunkLine::Context(text.to_owned())
}

fn remove(text: &str) -> HunkLine {
    HunkLine::Remove(text.to_owned())
}

fn add(text: &str) -> HunkLine {
    HunkLine::Add(text.to_owned())
}

fn hunk(anchors: &[&str], lines: Vec<HunkLine>) -> Hunk {
    Hunk { anchors: anchors.iter().map(|&a| a.to_owned()).collect(), lines, end_of_file: false }
}

fn update(path: &str, hunks: Vec<Hunk>) -> Operation {
    Operation::Update { path: path.into(), move_to: None, hunks }
}

fn problem(text: &str) -> (usize, ParseProblem) {
    match parse(text) {
        Err(PatchError::Parse { line, problem }) => (line, problem),
        other => panic!("expected a parse error, got {other:?}"),
    }
}

#[test]
fn every_operation_parses_in_order() {
    let text = "*** Begin Patch
*** Add File: docs/new.md
+# New
+
*** Delete File: old.txt
*** Update File: src/lib.rs
*** Move to: src/core.rs
@@ impl Engine
@@     fn run(&self) {
-        old();
+        new();
         done();
*** End of File
*** End Patch
";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![
                Operation::Add { path: "docs/new.md".into(), content: "# New\n\n".to_owned() },
                Operation::Delete { path: "old.txt".into() },
                Operation::Update {
                    path: "src/lib.rs".into(),
                    move_to: Some("src/core.rs".into()),
                    hunks: vec![Hunk {
                        anchors: vec!["impl Engine".to_owned(), "    fn run(&self) {".to_owned()],
                        lines: vec![
                            remove("        old();"),
                            add("        new();"),
                            context("        done();")
                        ],
                        end_of_file: true,
                    }],
                },
            ],
        })
    );
}

#[test]
fn an_update_may_start_without_an_anchor_and_have_several_hunks() {
    let text = "*** Begin Patch
*** Update File: a.txt
 one
-two
+TWO
@@
-four
+FOUR
@@ fn five
+six
*** End Patch";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![update(
                "a.txt",
                vec![
                    hunk(&[], vec![context("one"), remove("two"), add("TWO")]),
                    hunk(&[], vec![remove("four"), add("FOUR")]),
                    hunk(&["fn five"], vec![add("six")]),
                ],
            )],
        })
    );
}

#[test]
fn a_move_without_hunks_is_a_rename() {
    let text = "*** Begin Patch\n*** Update File: a.rs\n*** Move to: b.rs\n*** End Patch\n";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![Operation::Update {
                path: "a.rs".into(),
                move_to: Some("b.rs".into()),
                hunks: Vec::new(),
            }],
        })
    );
}

#[test]
fn an_add_without_lines_is_an_empty_file() {
    let text = "*** Begin Patch\n*** Add File: empty.txt\n*** Add File: one.txt\n+\n*** End Patch";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![
                Operation::Add { path: "empty.txt".into(), content: String::new() },
                Operation::Add { path: "one.txt".into(), content: "\n".to_owned() },
            ],
        })
    );
}

#[test]
fn whitespace_around_markers_and_blank_lines_around_the_patch_are_accepted() {
    let text = "\n\n  *** Begin Patch \n  *** Update File:   a.txt  \n@@\n-old\n+new\n\n*** Delete File: b.txt\n\n *** End Patch \n\n";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![
                update("a.txt", vec![hunk(&[], vec![remove("old"), add("new"), context("")])]),
                Operation::Delete { path: "b.txt".into() },
            ],
        })
    );
}

#[test]
fn crlf_line_ends_are_read_as_newlines() {
    let text =
        "*** Begin Patch\r\n*** Update File: a.txt\r\n@@\r\n-old\r\n+new\r\n*** End Patch\r\n";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![update("a.txt", vec![hunk(&[], vec![remove("old"), add("new")])])]
        })
    );
}

#[test]
fn a_patch_in_a_heredoc_is_accepted() {
    let inner = "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch";
    let expected = Ok(Patch { operations: vec![Operation::Delete { path: "a.txt".into() }] });
    for open in ["<<EOF", "<<'EOF'", "<<\"EOF\""] {
        assert_eq!(parse(&format!("{open}\n{inner}\nEOF\n")), expected, "{open}");
    }
    assert_eq!(problem(&format!("<<\"EOF'\n{inner}\nEOF\n")), (1, ParseProblem::NoBegin));
    assert_eq!(
        problem("<<EOF\n*** Begin Patch\n*** Delete File: a.txt\nEOF\n"),
        (3, ParseProblem::NoEnd)
    );
}

#[test]
fn an_indented_marker_inside_an_update_is_a_context_line() {
    let text = "*** Begin Patch
*** Update File: a.txt
@@
-old a
+new a
 *** Update File: b.txt
@@
-old b
+new b
*** End Patch";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![update(
                "a.txt",
                vec![
                    hunk(
                        &[],
                        vec![remove("old a"), add("new a"), context("*** Update File: b.txt")]
                    ),
                    hunk(&[], vec![remove("old b"), add("new b")]),
                ],
            )],
        })
    );
}

#[test]
fn a_unified_diff_header_reads_as_its_trailing_text() {
    let text = "*** Begin Patch
*** Update File: a.rs
@@ -12,7 +12,8 @@ fn run()
-old
+new
@@ -40 +41 @@
-x
+y
*** End Patch";
    assert_eq!(
        parse(text),
        Ok(Patch {
            operations: vec![update(
                "a.rs",
                vec![
                    hunk(&["fn run()"], vec![remove("old"), add("new")]),
                    hunk(&[], vec![remove("x"), add("y")]),
                ],
            )],
        })
    );
}

#[test]
fn an_anchor_that_only_looks_like_a_diff_header_stays() {
    let text = "*** Begin Patch\n*** Update File: a\n@@ -1 x\n-a\n*** End Patch";
    assert_eq!(
        parse(text),
        Ok(Patch { operations: vec![update("a", vec![hunk(&["-1 x"], vec![remove("a")])])] })
    );
}

#[test]
fn blank_lines_after_end_of_file_are_skipped() {
    let text =
        "*** Begin Patch\n*** Update File: a\n@@\n+x\n*** End of File\n\n@@\n-y\n*** End Patch";
    let Ok(patch) = parse(text) else { panic!("the patch parses") };
    let Operation::Update { hunks, .. } = &patch.operations[0] else { panic!("an update") };
    assert_eq!(hunks.len(), 2);
    assert!(hunks[0].end_of_file);
    assert!(!hunks[1].end_of_file);
}

#[test]
fn structure_errors_name_their_line() {
    let cases: [(&str, (usize, ParseProblem)); 20] = [
        ("", (1, ParseProblem::NoBegin)),
        ("bad", (1, ParseProblem::NoBegin)),
        ("\n\nbad\n*** End Patch", (3, ParseProblem::NoBegin)),
        ("*** Begin Patch", (1, ParseProblem::NoEnd)),
        ("*** Begin Patch\n*** Delete File: a\nbad", (3, ParseProblem::NoEnd)),
        ("*** Begin Patch\n*** End Patch", (2, ParseProblem::Empty)),
        ("*** Begin Patch\nbad\n*** End Patch", (2, ParseProblem::NotAnOperation)),
        ("*** Begin Patch\n*** Add File: \n*** End Patch", (2, ParseProblem::NoPath)),
        ("*** Begin Patch\n*** Add File: a\nhello\n*** End Patch", (3, ParseProblem::NotAnAddLine)),
        (
            "*** Begin Patch\n*** Delete File: a\nbad\n*** End Patch",
            (3, ParseProblem::NotAnOperation),
        ),
        ("*** Begin Patch\n*** Update File: a\n*** End Patch", (2, ParseProblem::EmptyUpdate)),
        (
            "*** Begin Patch\n*** Update File: a\n*** Delete File: b\n*** End Patch",
            (2, ParseProblem::EmptyUpdate),
        ),
        (
            "*** Begin Patch\n*** Update File: a\n@@ fn x\n*** End Patch",
            (3, ParseProblem::EmptyHunk),
        ),
        (
            "*** Begin Patch\n*** Update File: a\n-x\n@@\n@@ y\n*** End Patch",
            (4, ParseProblem::EmptyHunk),
        ),
        (
            "*** Begin Patch\n*** Update File: a\n@@\n*** End of File\n*** End Patch",
            (4, ParseProblem::EmptyHunk),
        ),
        (
            "*** Begin Patch\n*** Update File: a\n@@\n-x\nbad\n*** End Patch",
            (5, ParseProblem::NotAHunkLine),
        ),
        (
            "*** Begin Patch\n*** Update File: a\n-x\n*** Move to: b\n*** End Patch",
            (4, ParseProblem::MisplacedMove),
        ),
        ("*** Begin Patch\n*** Move to: b\n*** End Patch", (2, ParseProblem::MisplacedMove)),
        (
            "*** Begin Patch\n*** Update File: a\n-x\n*** End of File\n-y\n*** End Patch",
            (5, ParseProblem::AfterEndOfFile),
        ),
        (
            "*** Begin Patch\n*** Delete File: a\n*** End Patch\n*** Delete File: b\n*** End Patch",
            (4, ParseProblem::AfterEnd),
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(problem(text), expected, "{text:?}");
    }
}

#[test]
fn a_second_end_patch_is_text_after_the_end() {
    let text = "*** Begin Patch\n*** Delete File: a\n*** End Patch\n\n*** End Patch";
    assert_eq!(problem(text), (5, ParseProblem::AfterEnd));
}

#[test]
fn a_second_move_is_misplaced() {
    let text = "*** Begin Patch\n*** Update File: a\n*** Move to: b\n*** Move to: c\n*** End Patch";
    assert_eq!(problem(text), (4, ParseProblem::MisplacedMove));
}

#[test]
fn a_line_that_starts_with_a_wide_character_is_not_a_hunk_line() {
    let text = "*** Begin Patch\n*** Update File: a\n\u{e9}t\u{e9}\n*** End Patch";
    assert_eq!(problem(text), (3, ParseProblem::NotAHunkLine));
}
