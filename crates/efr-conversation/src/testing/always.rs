//! A fake writer of always-allow rules (phase E).

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_permissions::AllowRule;
use efr_protocol::AlwaysSaved;

use crate::AllowWriter;

/// A writer that keeps every rule it got and answers each with `outcome`, or with
/// `Saved` at the next index when the test gave none.
#[derive(Debug, Default)]
pub(crate) struct FakeAllowWriter {
    rules: Mutex<Vec<AllowRule>>,
    outcome: Option<AlwaysSaved>,
}

impl FakeAllowWriter {
    /// A writer that saves every rule.
    pub(crate) fn saving() -> Arc<Self> {
        Arc::new(FakeAllowWriter::default())
    }

    /// A writer that answers every rule with `outcome`.
    pub(crate) fn answering(outcome: AlwaysSaved) -> Arc<Self> {
        Arc::new(FakeAllowWriter { rules: Mutex::default(), outcome: Some(outcome) })
    }

    /// Every rule that it got, in order.
    pub(crate) fn rules(&self) -> Vec<AllowRule> {
        self.rules.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl AllowWriter for FakeAllowWriter {
    async fn add(&self, rule: AllowRule) -> AlwaysSaved {
        let mut rules = self.rules.lock().unwrap_or_else(PoisonError::into_inner);
        let described = rule.describe();
        rules.push(rule);
        match &self.outcome {
            Some(outcome) => outcome.clone(),
            None => AlwaysSaved::Saved {
                index: u32::try_from(rules.len() - 1).unwrap_or(u32::MAX),
                rule: described,
            },
        }
    }
}
