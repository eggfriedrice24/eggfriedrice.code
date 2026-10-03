//! Rebuilds the SQLite projections from the append-only event log through efr-store.
//! Stubbed until efr-store exists.

use crate::output;

pub(crate) fn run() -> bool {
    output::line(
        "rebuild-projections: not implemented until milestone 1 (efr-store does not exist yet)",
    );
    true
}
