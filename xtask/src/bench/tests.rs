use std::collections::BTreeMap;

use serde_json::json;

use super::{budgets, compare_table, parse_lines, size_table};

#[test]
fn budgets_read_the_max_bytes_table() {
    let budget = budgets(r#"{"max_bytes": {"efrd": 100, "efr": 50}}"#).unwrap();
    assert_eq!(budget.get("efrd"), Some(&100));
    assert_eq!(budget.get("efr"), Some(&50));
    assert!(budgets(r#"{"max_bytes": {"efrd": "big"}}"#).is_err());
    assert!(budgets("{}").is_err());
}

#[test]
fn a_binary_over_its_budget_or_without_one_fails() {
    let budget: BTreeMap<String, u64> =
        [("efrd".to_owned(), 2_000_000), ("efr".to_owned(), 1_000_000)].into();
    let (lines, fits) = size_table(&[("efrd", 1_500_000), ("efr", 900_000)], &budget);
    assert!(fits, "{lines:?}");
    let (lines, fits) = size_table(&[("efrd", 2_500_000)], &budget);
    assert!(!fits);
    assert!(lines[0].contains("OVER by 0.500 MB"), "{}", lines[0]);
    let (lines, fits) = size_table(&[("efr-sbx", 1)], &budget);
    assert!(!fits);
    assert!(lines[0].contains("has no budget"), "{}", lines[0]);
}

#[test]
fn compare_gives_the_change_of_each_line_of_both_runs() {
    let line = |label: &str, measure: serde_json::Value| {
        json!({"bench": "bench_perf_x", "label": label, "measure": measure}).to_string()
    };
    let old: BTreeMap<_, _> = parse_lines(&format!(
        "{}\n{}\nnot json\n{}\n",
        line("send", json!({"kind": "time", "p50_ns": 2_000_000, "p95_ns": 4_000_000})),
        line("rss", json!({"kind": "bytes", "bytes": 1_048_576})),
        line("gone", json!({"kind": "count", "count": 3})),
    ))
    .into_iter()
    .collect();
    let new: BTreeMap<_, _> = parse_lines(&format!(
        "{}\n{}\n{}\n",
        line("send", json!({"kind": "time", "p50_ns": 1_000_000, "p95_ns": 4_000_000})),
        line("rss", json!({"kind": "bytes", "bytes": 2_097_152})),
        line("added", json!({"kind": "slope", "slope": 1.0})),
    ))
    .into_iter()
    .collect();
    let table = compare_table(&old, &new);
    assert_eq!(table.len(), 4, "{table:#?}");
    let find = |label: &str| table.iter().find(|row| row.contains(label)).unwrap().clone();
    assert!(find("send").contains("p50      2.000 ->      1.000ms  -50.0%"), "{}", find("send"));
    assert!(find("send").contains("p95      4.000 ->      4.000ms   +0.0%"), "{}", find("send"));
    assert!(find("rss").contains("+100.0%"), "{}", find("rss"));
    assert!(find("gone").ends_with("only in the old run"));
    assert!(find("added").ends_with("only in the new run"));
}
