//! Renders `docs/protocol.md` from efr-protocol with its `schema` feature. The crate
//! lands in milestone 1, step 1; until then the command reports that and succeeds so
//! the justfile recipe can already be wired.

use crate::output;

pub(crate) fn run() -> bool {
    output::line(
        "protocol-docs: not implemented until milestone 1 (efr-protocol does not exist yet)",
    );
    true
}
