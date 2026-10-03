//! Verifies, or with `--bless` rewrites, the frozen protocol fixtures under
//! `crates/efr-protocol/fixtures/v1/`. Stubbed until efr-protocol exists.

use crate::output;

pub(crate) fn run(bless: bool) -> bool {
    let mode = if bless { "bless" } else { "verify" };
    output::line(&format!(
        "fixtures ({mode}): not implemented until milestone 1 (efr-protocol does not exist yet)"
    ));
    true
}
