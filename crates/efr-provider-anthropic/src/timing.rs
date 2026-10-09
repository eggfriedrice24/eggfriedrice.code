//! The debug lines that say how fast the API answers.
//!
//! Each model call writes two `phase` lines at debug level, in the format of
//! `efr-provider-openai` and the other phase lines (`docs/sandbox.md`):
//! `provider_accepted` when the first event of the stream arrives, and
//! `provider_first_event` when the first canonical event (text, reasoning or a tool
//! call) is ready, both with `transport=http connection=pool input=full`, so the log
//! compares the providers line for line.
