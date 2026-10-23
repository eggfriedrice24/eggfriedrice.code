use std::path::PathBuf;

use proptest::prelude::*;

use super::Facts;
use crate::{CallFacts, CoverAsk, Coverage, TargetKind};

/// Paths that are equal in several spellings, so a map must find a fact by any of
/// them, as the scan of [`CallFacts`] does.
const PATHS: [&str; 9] =
    ["/h", "/h/a", "/h/a/", "/h//a", "/h/./a", "/h/a/b", "/h/b", "/tmp/x", "/h/a/b/c"];

fn path() -> impl Strategy<Value = PathBuf> {
    prop::sample::select(PATHS.to_vec()).prop_map(PathBuf::from)
}

fn kind() -> impl Strategy<Value = Option<TargetKind>> {
    prop_oneof![
        Just(None),
        Just(Some(TargetKind::File)),
        Just(Some(TargetKind::Dir)),
        Just(Some(TargetKind::Other)),
    ]
}

fn ask() -> impl Strategy<Value = CoverAsk> {
    prop_oneof![
        path().prop_map(CoverAsk::Path),
        path().prop_map(CoverAsk::Home),
        path().prop_map(CoverAsk::HomeDir),
        path().prop_map(CoverAsk::Tracked),
        path().prop_map(CoverAsk::Refs),
        Just(CoverAsk::CronTable),
        Just(CoverAsk::Linger(None)),
        Just(CoverAsk::Linger(Some("u".to_owned()))),
        Just(CoverAsk::UnitLinks("a.service".to_owned())),
    ]
}

fn coverage() -> impl Strategy<Value = Coverage> {
    prop::sample::select(vec![
        Coverage::Covered,
        Coverage::LeftOut,
        Coverage::OverLimit,
        Coverage::Masked,
        Coverage::NoSnapshot,
        Coverage::NoMirror,
        Coverage::NotAllowed,
        Coverage::Unknown,
    ])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The maps find what the scans of [`CallFacts`] find: the first fact of a key, in
    /// any spelling of its path, and nothing for a key that no fact names. The lists
    /// are short and long, so both ways of [`Facts`] are tested.
    #[test]
    fn the_index_finds_what_the_scan_of_the_lists_finds(
        targets in prop::collection::vec((path(), kind()), 0..40),
        tracked_counts in prop::collection::vec((path(), 0_u32..3), 0..40),
        covered in prop::collection::vec((ask(), coverage()), 0..40),
        asks in prop::collection::vec(ask(), 0..20),
    ) {
        let facts = CallFacts { targets, tracked_counts, covered, ..CallFacts::default() };
        let indexed = Facts::new(&facts);
        for path in PATHS.iter().map(PathBuf::from) {
            prop_assert_eq!(indexed.target(&path), facts.target(&path), "{:?}", path);
            prop_assert_eq!(indexed.tracked(&path), facts.tracked(&path), "{:?}", path);
        }
        for ask in &asks {
            prop_assert_eq!(indexed.coverage(ask), facts.coverage(ask), "{:?}", ask);
        }
    }
}
