use pretty_assertions::assert_eq;
use serde_json::json;

use super::TokenUsage;

fn usage(input: u64, output: u64, cached: u64, reasoning: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
        reasoning_tokens: reasoning,
        ..TokenUsage::default()
    }
}

/// `usage` with `written` tokens written to the cache, `written_1h` of them for an hour.
fn with_writes(usage: TokenUsage, written: u64, written_1h: u64) -> TokenUsage {
    TokenUsage { cache_write_tokens: written, cache_write_1h_tokens: written_1h, ..usage }
}

#[test]
fn usages_add_field_by_field() {
    let mut total = TokenUsage::default();
    total += usage(1200, 80, 1024, 64);
    total += usage(1500, 20, 1200, 0);
    assert_eq!(total, usage(2700, 100, 2224, 64));
}

#[test]
fn cache_writes_add_field_by_field_and_saturate() {
    let sum = with_writes(usage(30_000, 10, 0, 0), 30_000, 30_000)
        + with_writes(usage(31_000, 10, 30_000, 0), 1_000, 0);
    assert_eq!(sum, with_writes(usage(61_000, 20, 30_000, 0), 31_000, 30_000));
    let full = with_writes(usage(0, 0, 0, 0), u64::MAX, u64::MAX);
    assert_eq!(full + full, full);
}

#[test]
fn addition_saturates_instead_of_overflowing() {
    let sum = usage(u64::MAX, 1, 0, 0) + usage(5, u64::MAX, 0, 0);
    assert_eq!(sum, usage(u64::MAX, u64::MAX, 0, 0));
}

#[test]
fn converts_to_the_wire_counts_without_the_context() {
    let wire: efr_protocol::Usage = with_writes(usage(2700, 100, 2224, 64), 400, 300).into();
    assert_eq!(
        wire,
        efr_protocol::Usage {
            input_tokens: 2700,
            output_tokens: 100,
            cached_input_tokens: 2224,
            reasoning_tokens: 64,
            cache_write_tokens: 400,
            cache_write_1h_tokens: 300,
            context_tokens: 0,
        }
    );
}

#[test]
fn round_trips_and_defaults_the_parts() {
    let full = with_writes(usage(10, 5, 4, 2), 3, 1);
    let wire = json!({
        "input_tokens": 10,
        "output_tokens": 5,
        "cached_input_tokens": 4,
        "reasoning_tokens": 2,
        "cache_write_tokens": 3,
        "cache_write_1h_tokens": 1,
    });
    assert_eq!(serde_json::to_value(full).unwrap(), wire);
    assert_eq!(serde_json::from_value::<TokenUsage>(wire).unwrap(), full);

    let totals_only: TokenUsage =
        serde_json::from_value(json!({"input_tokens": 10, "output_tokens": 5})).unwrap();
    assert_eq!(totals_only, usage(10, 5, 0, 0));
}
