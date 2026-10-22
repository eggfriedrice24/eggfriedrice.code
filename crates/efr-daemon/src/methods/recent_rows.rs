//! The rows of the list of `,resume`, which `conversations.recent` and
//! `conversations.search` both answer: a stored row plus what only efrd knows, which
//! terminal holds the conversation and the name of its project.

use std::path::Path;

use efr_protocol::RecentConversation;
use efr_scope::Registry;
use efr_store::conversations::RecentRow;

use crate::conversations;
use crate::routing;
use crate::state::State;

/// The wire row of each stored row of `rows`, in their order. `tty` is the terminal
/// that asks: its rows are `here`, and another open terminal that holds a row is its
/// `open_in`. `registry` names the projects.
pub(crate) fn recent_rows(
    state: &State,
    rows: Vec<RecentRow>,
    registry: &Registry,
    tty: Option<&str>,
) -> Vec<RecentConversation> {
    rows.into_iter()
        .map(|row| {
            let id = row.summary.id;
            let here = tty.is_some_and(|tty| {
                state.conversations.holders(id).iter().any(|(holder, _, _)| holder == tty)
            });
            let open_in = conversations::held(state, id, tty).map(|held| held.tty().to_owned());
            let project = routing::label(registry, &row.route, Path::new("")).name;
            RecentConversation {
                summary: row.summary,
                route: row.route,
                project,
                turns: row.turns,
                open_in,
                here,
            }
        })
        .collect()
}
