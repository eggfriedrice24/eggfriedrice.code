//! Full mode's settle pass (efr's full mode, DF3): the reasons of a call that the engine
//! decided as an `auto` call, changed to what full mode does with them.
//!
//! Full mode runs the model's commands as the user, with no sandbox and no question.
//! What the `auto` sandbox would hold runs, and so does what a built-in row of the mode
//! asks about. What stays:
//!
//! - every denial: a floor exit (`secret`, `config`), a sealed or efr's own path, the
//!   built-in secrets row, a user's or a conversation's `deny` rule;
//! - every question that is not about the sandbox: a privileged program, a `privilege`,
//!   `persistence` or `destructive` exit, a change of efr's settings, a file tool delete
//!   that undo does not cover, a delete that full mode guards, a user's or a
//!   conversation's `ask` rule.
//!
//! Each other exit runs, and keeps its reason, so [`Decision::exits`] lists it for the
//! record and the home copies. In a background job a `destructive` exit is refused
//! (`Cause::NotAsJob`), as the parts of [`job_refusal`](crate::job_refusal) already are.
//! A view of the host is dropped: with no sandbox, the call sees the host.
//!
//! [`Decision::exits`]: crate::Decision::exits

use efr_protocol::ExitKind;

use crate::decision::{Cause, Effect, Reason, Subject};

/// The reasons of a full call. `reasons` are those of the same call in `auto` (with the
/// support of [`AutoSupport::for_full`](crate::AutoSupport::for_full)) plus the deletes
/// that full mode guards; `job` is true for a call that starts a background job;
/// `built_in` tells whether a cause is a built-in rule of the machine that decided.
pub(crate) fn settle(
    reasons: Vec<Reason>,
    job: bool,
    built_in: impl Fn(&Cause) -> bool,
) -> Vec<Reason> {
    let mut settled = Vec::with_capacity(reasons.len());
    for reason in reasons {
        match &reason.subject {
            Subject::View { .. } => continue,
            Subject::Exit { need } if !need.kind.is_floor() => {
                if job && need.kind == ExitKind::Destructive {
                    // NOTE: a job answers at once, so nobody can answer this question;
                    // and undo of a job's turn needs `--force`, so it is not covered.
                    settled.push(Reason {
                        subject: Subject::JobPart { part: need.part.clone(), kind: need.kind },
                        effect: Effect::Deny,
                        cause: Cause::NotAsJob,
                    });
                }
                if asks(need.kind) {
                    settled.push(reason);
                } else {
                    settled.push(Reason { effect: Effect::Allow, ..reason });
                }
                continue;
            }
            _ => {}
        }
        let lifted = match reason.effect {
            Effect::Contain => true,
            Effect::Ask => built_in(&reason.cause),
            Effect::Allow | Effect::Deny => false,
        };
        if lifted {
            settled.push(Reason { effect: Effect::Allow, cause: Cause::Full, ..reason });
        } else {
            settled.push(reason);
        }
    }
    settled
}

/// True for an exit that still asks in full mode: `sudo` and the other ways to more
/// rights, a process or a change that outlives the turn, and a delete that undo does
/// not cover.
fn asks(kind: ExitKind) -> bool {
    matches!(kind, ExitKind::Privilege | ExitKind::Persistence | ExitKind::Destructive)
}

#[cfg(test)]
mod tests;
