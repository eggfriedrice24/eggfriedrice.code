//! `just bless` for the scenario fixtures: every outbound record of every scenario is
//! rewritten with what the daemon actually sends. Review the diff before committing.

// NOTE: an integration test crate is always built with cfg(test); saying so lets
// clippy treat its helpers as test code, as it does for unit tests.
#![cfg(test)]

use efr_test_daemon::{Replay, SCENARIOS, Scenario};

#[tokio::test]
#[ignore = "rewrites the fixtures; `just bless` runs it"]
async fn bless_scenarios() {
    for spec in SCENARIOS {
        let path = Replay::bless(spec.name).await.unwrap();
        // The blessed fixture must replay as it now stands.
        Replay::run(spec.name).await.unwrap().stop().await.unwrap();
        assert_eq!(path, Scenario::path_of(spec.name).unwrap());
    }
}
