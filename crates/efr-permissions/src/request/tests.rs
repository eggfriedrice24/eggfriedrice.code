use std::path::PathBuf;

use crate::{CallFacts, CoverAsk, Coverage};

/// A lost fact must never make a destructive line run without a question: an ask that
/// the daemon did not answer is unknown, and unknown is not covered.
#[test]
fn an_ask_without_a_fact_is_unknown_and_never_covered() {
    let path = CoverAsk::Path(PathBuf::from("/home/u/p/app/src"));
    let facts = CallFacts::default();
    assert_eq!(facts.coverage(&path), Coverage::Unknown);
    assert!(!facts.coverage(&path).is_covered());

    let facts = CallFacts {
        covered: vec![
            (path.clone(), Coverage::Covered),
            (CoverAsk::Tracked(PathBuf::from("/home/u/p/app")), Coverage::NoSnapshot),
        ],
        ..CallFacts::default()
    };
    assert!(facts.coverage(&path).is_covered());
    assert_eq!(
        facts.coverage(&CoverAsk::Tracked(PathBuf::from("/home/u/p/app"))),
        Coverage::NoSnapshot
    );
    assert_eq!(facts.coverage(&CoverAsk::CronTable), Coverage::Unknown);
}
