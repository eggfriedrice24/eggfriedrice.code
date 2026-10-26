//! The `reason` of `approval_requested`: why the turn's mode asks about a call, in one
//! plain line for a person, such as `cautious asks before any network access.`
//!
//! The line comes from the reasons of the engine's decision that ask, and from the
//! turn's mode. It uses the words of the engine's own reasons where they exist. The
//! terminal does not show it: its question shows the summary and the exit facts, as
//! before. A phone shows it below the command.

use efr_permissions::{Access, Cause, Decision, Effect, Layer, PathClass, Policy, Subject};
use efr_protocol::{ExitKind, Mode, Origin};

/// The `reason` of the question about a call that `decision` asks about, in a turn of
/// `mode` (the mode that the engine decided with, after the cap of a remote turn).
/// `None` when no reason of the decision asks.
pub(super) fn approval_reason(decision: &Decision, mode: Mode) -> Option<String> {
    let asking: Vec<_> =
        decision.reasons().iter().filter(|reason| reason.effect == Effect::Ask).collect();
    if asking.is_empty() {
        return None;
    }
    // NOTE: the number of the mode's own rules: a rule of the machine policy with a
    // lower index is built in, the others are the user's.
    let built_in = Policy::base(mode).rules().len();
    // NOTE: a guarded delete that undo cannot revert is named once, as the guarded
    // delete, as the first line of a full question does.
    let critical = decision.critical().next().is_some();
    let mut mode_parts: Vec<String> = Vec::new();
    let mut exit_parts: Vec<&'static str> = Vec::new();
    let mut sentences: Vec<String> = Vec::new();
    for reason in asking {
        match &reason.cause {
            Cause::Rule { layer: Layer::Machine, index }
            | Cause::Part { layer: Layer::Machine, index, .. }
                if *index < built_in =>
            {
                add(&mut mode_parts, mode_part(&reason.subject, mode));
            }
            Cause::Opaque { construct, layer: Layer::Machine, index } if *index < built_in => {
                add(
                    &mut sentences,
                    format!(
                        "The line holds {construct}, which no command rule can judge, so {mode} \
                         asks."
                    ),
                );
            }
            Cause::Rule { layer: Layer::Machine, .. }
            | Cause::Part { layer: Layer::Machine, .. }
            | Cause::Opaque { layer: Layer::Machine, .. } => {
                add(&mut sentences, "A rule of yours in config.toml asks for this.".to_owned());
            }
            Cause::Rule { .. } | Cause::Part { .. } | Cause::Opaque { .. } => {
                add(&mut sentences, "A rule of this conversation asks for this.".to_owned());
            }
            Cause::Exit { kind: ExitKind::Destructive } | Cause::Destructive if critical => {}
            Cause::Exit { kind } => {
                let part = exit_part(*kind);
                if !exit_parts.contains(&part) {
                    exit_parts.push(part);
                }
            }
            cause => {
                if let Some(sentence) = cause_sentence(cause) {
                    add(&mut sentences, sentence);
                }
            }
        }
    }
    let mut line: Vec<String> = Vec::new();
    if !mode_parts.is_empty() {
        line.push(format!("{mode} asks before {}.", join(&mode_parts)));
    }
    if !exit_parts.is_empty() {
        let parts: Vec<String> = exit_parts.iter().map(|part| (*part).to_owned()).collect();
        line.push(match mode {
            Mode::Full => format!("full mode still asks before {}.", join(&parts)),
            _ => format!("The auto sandbox holds back {}.", join(&parts)),
        });
    }
    line.extend(sentences);
    if line.is_empty() {
        return None;
    }
    // NOTE: one line: a program name with a line break must not make a second one.
    Some(line.join(" ").replace('\n', "\\n").replace('\r', "\\r"))
}

/// What the built-in rules of `mode` ask before, for one requirement, such as
/// `any network access`.
fn mode_part(subject: &Subject, mode: Mode) -> String {
    let manual = mode == Mode::Manual;
    match subject {
        Subject::Command { .. } if manual => "every command".to_owned(),
        Subject::Command { .. } if mode == Mode::Cautious => {
            "a command that is not on its list of read-only commands".to_owned()
        }
        Subject::Command { .. } => "this command".to_owned(),
        Subject::Network => "any network access".to_owned(),
        Subject::Path { access: Access::Read | Access::ReadTree, .. } if manual => {
            "every read".to_owned()
        }
        Subject::Path { access: Access::Write, .. } if manual => "every write".to_owned(),
        Subject::Path { access: Access::Write, class: Some(class), .. } => match class {
            PathClass::UserData => {
                "a write to user data that is not below the turn's project root".to_owned()
            }
            PathClass::System => "a write outside the home directory".to_owned(),
            class => format!("a write to {class}"),
        },
        _ => "this call".to_owned(),
    }
}

/// The words for an exit of `kind`, after "The auto sandbox holds back".
fn exit_part(kind: ExitKind) -> &'static str {
    match kind {
        ExitKind::Write => "a write outside its write roots",
        ExitKind::Host => "network access",
        ExitKind::HostView => "a view of the host's network",
        ExitKind::Socket => "a socket",
        ExitKind::DesktopIpc => "a desktop's IPC socket",
        ExitKind::Bus => "a message bus",
        ExitKind::Device => "a device node",
        ExitKind::MaskedRead => "a read of a masked file",
        ExitKind::Destructive => "a destructive change",
        ExitKind::Outside => "a run outside the sandbox",
        ExitKind::Privilege => "a command with more rights",
        ExitKind::Persistence => "a change that runs later",
        ExitKind::Upload => "an upload with credentials",
        ExitKind::SyncedWrite => "a write to a synced folder",
        ExitKind::AboveRoot => "a write at or above a project root",
        ExitKind::Secret => "a secret",
        ExitKind::Config => "a write of efr's config",
        // NOTE: a kind that this build does not name yet.
        _ => "an action that leaves the sandbox",
    }
}

/// The sentence for a cause that is not a rule and not an exit, in the words of the
/// engine's reasons. `None` for a cause that never asks.
fn cause_sentence(cause: &Cause) -> Option<String> {
    let sentence = match cause {
        Cause::Privileged { program } => {
            format!("{program} runs commands as another user, so efr asks in every mode.")
        }
        Cause::ReachesSecret { .. } => "A secret lies below this directory.".to_owned(),
        Cause::ReachesWriteSealed { .. } => {
            "efr's configuration lies below this directory.".to_owned()
        }
        Cause::RemoteOrigin { origin } => {
            format!("A turn from {} asks for everything outside $SCRATCH.", origin_name(*origin))
        }
        Cause::Interactive => {
            "The call may wait for input at the terminal, so the user must answer there.".to_owned()
        }
        Cause::SettingsChange => "Only the user approves a change of efr's settings.".to_owned(),
        Cause::Destructive => {
            "The user approves each delete and move that efr undo cannot bring back.".to_owned()
        }
        Cause::JobAsks => {
            "Outside auto, the user approves each background job before it starts.".to_owned()
        }
        Cause::CriticalDelete => "Full mode still asks before a delete of this tree, and only \
                                  the user's own machine answers."
            .to_owned(),
        _ => return None,
    };
    Some(sentence)
}

/// The surface of a remote turn, for a person.
fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Phone => "the phone",
        _ => "a remote client",
    }
}

fn add(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// `a`, `a and b`, `a, b and c`.
fn join(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests;
