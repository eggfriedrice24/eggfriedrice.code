use std::collections::HashSet;
use std::path::{Path, PathBuf};

use efr_stdx::rng::Rng as _;
use efr_test_support::TestRng;
use pretty_assertions::assert_eq;

use super::{RootEnd, push_once};
use crate::capture::{LeftOut, LeftOutReason};

/// The reasons that the random records draw from.
const REASONS: [LeftOutReason; 4] = [
    LeftOutReason::TooLarge,
    LeftOutReason::IgnoredOff,
    LeftOutReason::SkippedDir,
    LeftOutReason::RootTooLarge,
];

/// The left-out record of one root as the old code built it: a linear `contains` for
/// each path. It is the reference of the order.
fn old_add_left_out(list: &mut Vec<(PathBuf, LeftOutReason)>, root: &Path, left_out: &[LeftOut]) {
    for left in left_out {
        let path = if left.path.is_empty() { root.to_path_buf() } else { root.join(&left.path) };
        let entry = (path, left.reason);
        if !list.contains(&entry) {
            list.push(entry);
        }
    }
}

/// The merge of the roots' records at the end of a turn as the old code did it.
fn old_merge(roots: &[Vec<(PathBuf, LeftOutReason)>]) -> Vec<(PathBuf, LeftOutReason)> {
    let mut all = Vec::new();
    for root in roots {
        for left in root {
            if !all.contains(left) {
                all.push(left.clone());
            }
        }
    }
    all
}

/// A record of up to `most` paths drawn from a small pool, so the same path comes
/// often, with the root itself (the empty path) among them.
fn record(rng: &TestRng, most: u64) -> Vec<LeftOut> {
    (0..rng.next_u64() % most)
        .map(|_| {
            let n = rng.next_u64() % 12;
            let path = if n == 0 { String::new() } else { format!("d{}/f{n}", n % 3) };
            let reason = REASONS[usize::try_from(rng.next_u64() % 4).unwrap()];
            LeftOut { path, reason }
        })
        .collect()
}

#[test]
fn the_left_out_record_keeps_the_order_of_the_old_list() {
    let rng = TestRng::new(7);
    for _ in 0..500 {
        let roots: Vec<PathBuf> = (0..1 + rng.next_u64() % 3)
            .map(|n| PathBuf::from(format!("/work/r{}", n % 2)))
            .collect();
        let (mut old, mut new) = (Vec::new(), Vec::new());
        for root in &roots {
            let (pre, post) = (record(&rng, 20), record(&rng, 20));
            let mut reference = Vec::new();
            old_add_left_out(&mut reference, root, &pre);
            old_add_left_out(&mut reference, root, &post);
            let mut end = RootEnd::default();
            end.add_left_out(root, &pre);
            end.add_left_out(root, &post);
            assert_eq!(end.left_out, reference, "one root");
            assert_eq!(end.left_seen, reference.iter().cloned().collect::<HashSet<_>>());
            old.push(reference);
            new.push(end.left_out);
        }
        let (mut merged, mut seen) = (Vec::new(), HashSet::new());
        for left in new.into_iter().flatten() {
            push_once(&mut merged, &mut seen, left);
        }
        assert_eq!(merged, old_merge(&old), "the turn's record");
    }
}

#[test]
fn the_left_out_record_is_in_the_order_of_first_sight_not_sorted() {
    let root = Path::new("/work/app");
    let left = |path: &str| LeftOut { path: path.to_owned(), reason: LeftOutReason::TooLarge };
    let mut end = RootEnd::default();
    end.add_left_out(root, &[left("z.sql"), left("m.sql")]);
    end.add_left_out(root, &[left("b.sql"), left("m.sql"), left("z.sql")]);
    let paths: Vec<&Path> = end.left_out.iter().map(|(path, _)| path.as_path()).collect();
    assert_eq!(
        paths,
        [Path::new("/work/app/z.sql"), Path::new("/work/app/m.sql"), Path::new("/work/app/b.sql")]
    );
}
