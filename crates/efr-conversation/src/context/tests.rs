use efr_protocol::ContextUse;
use pretty_assertions::assert_eq;

use super::{
    CompactionConfig, ContextLimits, DEFAULT_CONTEXT_WINDOW, HARD_CAP_PERCENT, estimate_tokens,
};

#[test]
fn the_trigger_is_auto_at_percent_of_the_window_and_the_cap_95() {
    let limits = ContextLimits::new(Some(272_000), CompactionConfig::default());

    assert_eq!(limits.window, 272_000);
    assert_eq!(limits.trigger, 206_720);
    assert_eq!(limits.hard_cap, 258_400);
    assert_eq!(limits.limit(), 206_720);
    assert_eq!(
        limits.gauge(89_000),
        ContextUse { tokens: 89_000, limit: 206_720, window: 272_000 }
    );
}

#[test]
fn with_auto_off_a_client_counts_against_the_hard_cap() {
    let limits = ContextLimits::new(Some(200_000), CompactionConfig::new(false, 76));

    assert_eq!(limits.limit(), 200_000 * HARD_CAP_PERCENT / 100);
}

#[test]
fn an_unknown_window_counts_as_the_default_and_auto_at_stays_in_range() {
    let unknown = ContextLimits::new(None, CompactionConfig::default());
    let zero = ContextLimits::new(Some(0), CompactionConfig::new(true, 0));
    let full = ContextLimits::new(Some(100_000), CompactionConfig::new(true, 150));

    assert_eq!(unknown.window, DEFAULT_CONTEXT_WINDOW);
    assert_eq!(zero.trigger, DEFAULT_CONTEXT_WINDOW / 100);
    assert_eq!(full.trigger, 99_000);
}

#[test]
fn the_estimate_is_four_bytes_a_token_rounded_up() {
    assert_eq!(estimate_tokens(0), 0);
    assert_eq!(estimate_tokens(1), 1);
    assert_eq!(estimate_tokens(4_000), 1_000);
    assert_eq!(estimate_tokens(4_001), 1_001);
}
