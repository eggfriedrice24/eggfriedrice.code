//! The refusals of one call (DP10): one record for each host, port and reason, with a
//! count, at most a fixed number of them, and the count of the rest.

use efr_protocol::{BlockReason, Blocked};

/// The host of a record whose request named no valid host.
pub(crate) const INVALID: &str = "<invalid>";

/// The refusal records of one call.
#[derive(Debug)]
pub(crate) struct Records {
    entries: Vec<Blocked>,
    more: u32,
    cap: usize,
}

impl Records {
    /// No record yet; at most `cap` of them.
    pub(crate) fn new(cap: usize) -> Records {
        Records { entries: Vec::new(), more: 0, cap }
    }

    /// One more refusal. A host, port and reason that has a record counts there; a new
    /// one gets a record while there is room, else it counts in the rest.
    pub(crate) fn add(&mut self, host: &str, port: u16, reason: BlockReason) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.host == host && entry.port == port && entry.reason == reason)
        {
            entry.count = entry.count.saturating_add(1);
        } else if self.entries.len() < self.cap {
            self.entries.push(Blocked { host: host.to_owned(), port, reason, count: 1 });
        } else {
            self.more = self.more.saturating_add(1);
        }
    }

    /// The records in the order of their first refusal, and the count of the rest.
    pub(crate) fn snapshot(&self) -> (Vec<Blocked>, u32) {
        (self.entries.clone(), self.more)
    }

    /// Every refusal: the counts of the records and the rest.
    pub(crate) fn total(&self) -> u64 {
        self.entries.iter().map(|entry| u64::from(entry.count)).sum::<u64>() + u64::from(self.more)
    }
}

#[cfg(test)]
mod tests;
