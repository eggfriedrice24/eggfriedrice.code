use std::path::PathBuf;

use efr_stdx::rng::Rng as _;
use efr_test_support::TestRng;
use pretty_assertions::assert_eq;

use super::{HomeDelta, Item, Kind, deltas};

/// The old `deltas`, with a linear search of the other copy for each path: the
/// reference of the first-wins rule and of the order.
fn old_deltas(before: &[Item], after: &[Item]) -> Vec<HomeDelta> {
    let mut out = Vec::new();
    for item in after {
        let missing = Item { path: item.path.clone(), ..Item::default() };
        let was = before.iter().find(|known| known.path == item.path).unwrap_or(&missing);
        let covered = was.kind != Kind::NotCovered && item.kind != Kind::NotCovered;
        out.push(HomeDelta {
            path: PathBuf::from(&item.path),
            floor: item.floor || was.floor,
            changed: !covered || !was.same_state(item),
            covered,
        });
    }
    for was in before.iter().filter(|was| !after.iter().any(|item| item.path == was.path)) {
        out.push(HomeDelta {
            path: PathBuf::from(&was.path),
            floor: was.floor,
            changed: true,
            covered: false,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// A copy of up to 12 items of a few paths, so the same path comes often, also twice
/// in one copy.
fn some_copy(rng: &TestRng) -> Vec<Item> {
    let pick = |below: u64| rng.next_u64() % below;
    (0..pick(12))
        .map(|_| {
            let kinds = [Kind::File, Kind::Link, Kind::Missing, Kind::State, Kind::NotCovered];
            Item {
                path: format!("/home/u/f{}", pick(6)),
                kind: kinds[usize::try_from(pick(5)).unwrap()],
                mode: Some(0o644),
                blob: Some(format!("{:040}", pick(2))),
                floor: pick(2) == 0,
                ..Item::default()
            }
        })
        .collect()
}

#[test]
fn the_deltas_of_a_copy_are_those_of_the_old_search() {
    let rng = TestRng::new(5);
    for _ in 0..1000 {
        let (before, after) = (some_copy(&rng), some_copy(&rng));
        assert_eq!(deltas(&before, &after), old_deltas(&before, &after));
    }
}
