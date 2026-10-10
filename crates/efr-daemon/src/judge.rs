//! The daemon's judge of an exit (efr's auto spec, sections 9.2 to 9.5, phase A): the
//! classifier request of each exit of the `auto` sandbox, behind
//! `efr_conversation::ExitJudge`. It owns the chain of models (the small model of the
//! turn's provider, then the turn's model, then the user), the per-turn cache of
//! verdicts, the model that each conversation pins and the models it skips, and the
//! SHA-256 of each record.
//!
//! The judge comes in the next change; until then efrd passes no judge, and the user
//! answers every exit.
