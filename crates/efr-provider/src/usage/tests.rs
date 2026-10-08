use pretty_assertions::assert_eq;
use serde_json::json;

use super::TokenUsage;

fn usage(input: u64, output: u64, cached: u64, reasoning: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
        reasoning_tokens: reasoning,
    }
}

#[test]
fn usages_add_field_by_field() {
    let mut total = TokenUsage::default();
    total += usage(1200, 80, 1024, 64);
    total += usage(1500, 20, 1200, 0);
    assert_eq!(total, usage(2700, 100, 2224, 64));
}

#[test]
fn addition_saturates_instead_of_overflowing() {
    let sum = usage(u64::MAX, 1, 0, 0) + usage(5, u64::MAX, 0, 0);
    assert_eq!(sum, usage(u64::MAX, u64::MAX, 0, 0));
}

#[test]
fn converts_to_the_wire_counts_without_the_context() {
    let wire: efr_protocol::Usage = usage(2700, 100, 2224, 64).into();
    assert_eq!(
        wire,
        efr_protocol::Usage {
            input_tokens: 2700,
            output_tokens: 100,
            cached_input_tokens: 2224,
            reasoning_tokens: 64,
            context_tokens: 0,
        }
    );
}

#[test]
fn round_trips_and_defaults_the_parts() {
    let full = usage(10, 5, 4, 2);
    let wire = json!({
        "input_tokens": 10,
        "output_tokens": 5,
        "cached_input_tokens": 4,
        "reasoning_tokens": 2,
    });
    assert_eq!(serde_json::to_value(full).unwrap(), wire);
    assert_eq!(serde_json::from_value::<TokenUsage>(wire).unwrap(), full);

    let totals_only: TokenUsage =
        serde_json::from_value(json!({"input_tokens": 10, "output_tokens": 5})).unwrap();
    assert_eq!(totals_only, usage(10, 5, 0, 0));
}
