use std::sync::{Arc, Mutex};

use tracing::{Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

use super::TARGET;
use crate::testing::terminal;

/// The level of every event with the library's target.
#[derive(Clone, Default)]
struct Levels(Arc<Mutex<Vec<Level>>>);

impl<S: Subscriber> Layer<S> for Levels {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        if metadata.target() == TARGET {
            self.0.lock().unwrap().push(*metadata.level());
        }
    }
}

#[test]
fn a_library_warning_reaches_tracing_at_debug() {
    let levels = Levels::default();
    let subscriber = tracing_subscriber::registry().with(levels.clone());
    // NOTE: `init` reads the filter once per process; nextest runs this test in a
    // process of its own, so the first `init` sees this subscriber.
    tracing::subscriber::with_default(subscriber, || {
        let (mut terminal, _effects) = terminal(10, 2);
        // A cursor move with two numbers: libghostty-vt logs a warning and ignores it.
        terminal.vt_write(b"\x1b[1;2A");
    });
    let levels = levels.0.lock().unwrap().clone();
    // A warning arrives at debug, and nothing arrives above it.
    assert!(levels.contains(&Level::DEBUG), "{levels:?}");
    assert!(levels.iter().all(|level| *level >= Level::DEBUG), "{levels:?}");
}
