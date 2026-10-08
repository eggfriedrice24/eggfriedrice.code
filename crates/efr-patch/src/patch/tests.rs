use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{Hunk, HunkLine, Operation, Patch};

fn sample() -> Patch {
    Patch {
        operations: vec![
            Operation::Add { path: "new.txt".into(), content: "hi\n".to_owned() },
            Operation::Update {
                path: "src/a.rs".into(),
                move_to: Some("src/b.rs".into()),
                hunks: vec![Hunk {
                    anchors: vec!["fn main".to_owned()],
                    lines: vec![
                        HunkLine::Context("    let x = 1;".to_owned()),
                        HunkLine::Remove("    old();".to_owned()),
                        HunkLine::Add("    new();".to_owned()),
                    ],
                    end_of_file: false,
                }],
            },
            Operation::Delete { path: "gone.txt".into() },
            Operation::Update { path: "new.txt".into(), move_to: None, hunks: Vec::new() },
        ],
    }
}

#[test]
fn paths_lists_each_path_once_in_order_with_the_target_of_a_move() {
    let patch = sample();
    assert_eq!(
        patch.paths(),
        vec![
            Path::new("new.txt"),
            Path::new("src/a.rs"),
            Path::new("src/b.rs"),
            Path::new("gone.txt"),
        ]
    );
}

#[test]
fn map_paths_resolves_every_path_and_keeps_the_rest() {
    let patch = sample().map_paths(|path| Path::new("/work").join(path));
    assert_eq!(
        patch.paths(),
        vec![
            Path::new("/work/new.txt"),
            Path::new("/work/src/a.rs"),
            Path::new("/work/src/b.rs"),
            Path::new("/work/gone.txt"),
        ]
    );
    assert_eq!(
        patch.operations[0],
        Operation::Add { path: PathBuf::from("/work/new.txt"), content: "hi\n".to_owned() }
    );
}

#[test]
fn delete_and_move_are_destructive_and_the_rest_is_not() {
    let destructive: Vec<bool> =
        sample().operations.iter().map(Operation::is_destructive).collect();
    assert_eq!(destructive, vec![false, true, true, false]);
}

#[test]
fn a_hunk_splits_into_old_and_new_lines() {
    let Operation::Update { hunks, .. } = &sample().operations[1] else {
        unreachable!("the second operation of the sample is an update");
    };
    assert_eq!(hunks[0].old_lines(), vec!["    let x = 1;", "    old();"]);
    assert_eq!(hunks[0].new_lines(), vec!["    let x = 1;", "    new();"]);
}
