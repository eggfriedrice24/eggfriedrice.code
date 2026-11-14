use serde_json::json;

use super::{AccountLimits, LimitKind};
use crate::CatalogStatus;

#[test]
fn a_catalog_from_an_earlier_daemon_has_a_login_and_no_limits() {
    let old = json!({ "provider": "openai-subscription", "origin": "backend" });
    let status: CatalogStatus = serde_json::from_value(old.clone()).unwrap();
    assert!(!status.signed_out);
    assert_eq!(status.label, None);
    assert_eq!(status.limits, None);
    assert_eq!(serde_json::to_value(&status).unwrap(), old, "nothing new is written");
}

#[test]
fn a_limit_kind_that_this_build_does_not_know_reads_as_unknown() {
    let limits: AccountLimits = serde_json::from_value(json!({
        "observed_at": "2026-10-03T07:58:00Z",
        "reached": { "kind": "per_model_tokens" },
    }))
    .unwrap();
    assert_eq!(limits.reached.map(|reached| reached.kind), Some(LimitKind::Unknown));
    assert!(limits.windows.is_empty());
}
