use std::collections::BTreeSet;

use efr_protocol::ChangeKind;

use super::{counts, skipped};
use crate::changes::RootChange;

fn change(path: &str, from: Option<&str>) -> RootChange {
    RootChange {
        path: path.to_owned(),
        kind: ChangeKind::Modified,
        from: from.map(str::to_owned),
        added: 0,
        removed: 0,
        binary: false,
    }
}

/// The paths of a turn's `last` and `post` and of an undo's `before` and `after` count;
/// `pre`, the markers and the mirrors do not.
#[test]
fn only_the_refs_that_hold_changed_paths_count() {
    for name in [
        "refs/efr/c/t/last",
        "refs/efr/c/t/post",
        "refs/efr-undo/c/u/before",
        "refs/efr-undo/c/u/after",
    ] {
        assert!(counts(name), "{name}");
    }
    for name in [
        "refs/efr/c/t/pre",
        "refs/efr/c/t/undone",
        "refs/efr/c/t/home/1",
        "refs/efr-undo/c/u/started",
        "refs/efr-undo/c/u/mirror/refs/heads/main",
    ] {
        assert!(!counts(name), "{name}");
    }
}

/// A rename that starts or ends at a path of another turn stays out whole.
#[test]
fn a_change_that_names_a_skipped_path_on_either_side_stays_out() {
    let skip: BTreeSet<String> = ["later.txt".to_owned()].into();
    assert!(skipped(&change("later.txt", None), &skip));
    assert!(skipped(&change("new.txt", Some("later.txt")), &skip));
    assert!(!skipped(&change("job.txt", None), &skip));
}
