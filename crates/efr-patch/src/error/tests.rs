use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::{ParseProblem, PatchError};

#[test]
fn messages_name_the_line_the_file_and_the_hunk() {
    let cases = [
        (
            PatchError::Parse { line: 1, problem: ParseProblem::NoBegin },
            "line 1 of the patch is wrong: the patch must start with `*** Begin Patch`",
        ),
        (PatchError::Missing { path: PathBuf::from("src/a.rs") }, "src/a.rs does not exist"),
        (
            PatchError::NoMatch { path: PathBuf::from("src/a.rs"), hunk: 2, nearest: Vec::new() },
            "hunk 2 does not match the lines of src/a.rs",
        ),
        (PatchError::NotUnique { count: 3 }, "the old text occurs 3 times, not once"),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}
