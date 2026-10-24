use std::path::Path;

use pretty_assertions::assert_eq;

use super::{busy_question, deleted_line, held_question, not_removed_line, question};

#[test]
fn the_questions_name_the_title_else_the_short_id() {
    assert_eq!(
        question(Some("rsync the photos"), "019a9b1c"),
        "Delete \"rsync the photos\"? Nothing takes a delete back. [y/N]"
    );
    assert_eq!(
        question(None, "019a9b1c"),
        "Delete the conversation 019a9b1c? Nothing takes a delete back. [y/N]"
    );
    assert_eq!(
        held_question(Some("fix\nthe tests"), "019a9b1c", "/dev/pts/4"),
        "\"fix the tests\" is open in /dev/pts/4. Delete it there too? [y/N]"
    );
    assert_eq!(
        busy_question(Some("fix the tests"), "019a9b1c"),
        "A turn runs in \"fix the tests\". Stop it and delete? [y/N]"
    );
}

#[test]
fn the_line_after_a_delete_counts_the_stopped_jobs() {
    assert_eq!(deleted_line(Some("a"), "019a9b1c", 0), "deleted \"a\"");
    assert_eq!(
        deleted_line(Some("a"), "019a9b1c", 1),
        "deleted \"a\"; efrd stopped its background job"
    );
    assert_eq!(
        deleted_line(None, "019a9b1c", 3),
        "deleted the conversation 019a9b1c; efrd stopped its 3 background jobs"
    );
    assert_eq!(
        not_removed_line(Path::new("/d/scratch/x")),
        "efrd could not remove /d/scratch/x; it tries again at its next start"
    );
}
