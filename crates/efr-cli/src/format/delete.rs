//! The lines of a conversation delete (`efr conversation delete`, Ctrl+D in the
//! `,resume` picker): the questions before it and the line after it, inline.

use std::path::Path;

use super::one_line;

/// The conversation as a question or a line names it: its title in quotes, else its
/// short id.
fn named(title: Option<&str>, short_id: &str) -> String {
    match title {
        Some(title) => format!("\"{}\"", one_line(title)),
        None => format!("the conversation {}", one_line(short_id)),
    }
}

/// The question before a delete.
pub(crate) fn question(title: Option<&str>, short_id: &str) -> String {
    format!("Delete {}? Nothing takes a delete back. [y/N]", named(title, short_id))
}

/// The question when another open terminal holds the conversation.
pub(crate) fn held_question(title: Option<&str>, short_id: &str, tty: &str) -> String {
    format!("{} is open in {}. Delete it there too? [y/N]", named(title, short_id), one_line(tty))
}

/// The question when a turn runs in the conversation.
pub(crate) fn busy_question(title: Option<&str>, short_id: &str) -> String {
    format!("A turn runs in {}. Stop it and delete? [y/N]", named(title, short_id))
}

/// The line after a delete, with the jobs that efrd stopped first.
pub(crate) fn deleted_line(title: Option<&str>, short_id: &str, jobs: u32) -> String {
    let mut line = format!("deleted {}", named(title, short_id));
    match jobs {
        0 => {}
        1 => line.push_str("; efrd stopped its background job"),
        n => line.push_str(&format!("; efrd stopped its {n} background jobs")),
    }
    line
}

/// The line for a path of the conversation that efrd could not remove.
pub(crate) fn not_removed_line(path: &Path) -> String {
    format!(
        "efrd could not remove {}; it tries again at its next start",
        one_line(&path.display().to_string())
    )
}

#[cfg(test)]
mod tests;
