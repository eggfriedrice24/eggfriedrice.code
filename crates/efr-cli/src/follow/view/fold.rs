//! The fold of reads (TM-Calls frames 6 and 7, TM-Legend 13): `read_file` calls that
//! follow each other with nothing between them fold into one growing live row, such as
//! `✓ read 6 files  crates/efr-store/src/lib.rs, session.rs, json.rs (+3)`, that is
//! written once as one row when another kind of block starts, a read fails, a question
//! shows or the turn ends. A single read is `✓ read <path>`.
//!
//! A seam for the calls slice of the terminal look: [`Fold`] holds nothing yet, and
//! [`Fold::live`] adds no row, so the view shows each read as it did. The calls slice
//! fills it, calls it from `view/calls.rs`, and commits it at each end that the list
//! above names. Only reads fold: efr has no search tool, so the `search` row of the
//! Calls board waits for one; the fold keeps a place for a second kind of call.

use efr_render::RenderOptions;

/// The reads that fold into one row. Empty until the calls slice builds the fold.
#[derive(Debug, Default)]
pub(super) struct Fold {}

impl Fold {
    /// The live row of the fold, with its newline, or nothing while it holds no read.
    /// `live_body` writes it after the live text of a message and before the row of a
    /// running call.
    pub(super) fn live(&self, _options: &RenderOptions) -> String {
        String::new()
    }
}
