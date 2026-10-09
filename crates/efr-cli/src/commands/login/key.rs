//! Where `efr login openai-api` and `efr login anthropic` get the key: a variable of
//! this shell with `--from-env`, a prompt that does not show what is typed when stdin is
//! a terminal, else stdin. Never the command line, which other users can read.
//!
//! The key is trimmed, and every buffer that held it is zeroed when it is dropped: the
//! prompt edits it in an `AnswerLine`, stdin and a variable land in `Zeroizing`
//! buffers, and the trimmed key goes on as `SecretText`.

use efr_protocol::SecretText;
use zeroize::Zeroizing;

use crate::answer::{AnswerLine, Edit};
use crate::context::Context;
use crate::error::CliError;
use crate::keys::Key;
use crate::output::Output;

/// The most bytes that stdin may hold. An API key is a few hundred at most.
const STDIN_LIMIT: usize = 4096;

/// What a key came from, as an error names it.
const FROM_STDIN: &str = "stdin";

/// What a key came from, as an error names it.
const FROM_PROMPT: &str = "the prompt";

/// The provider of a key, as the login names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyProvider {
    /// `openai-api`.
    OpenAi,
    /// `anthropic-api`.
    Anthropic,
}

impl KeyProvider {
    /// The provider's id.
    pub(crate) fn id(self) -> &'static str {
        match self {
            KeyProvider::OpenAi => "openai-api",
            KeyProvider::Anthropic => "anthropic-api",
        }
    }

    /// The company, as a person names it.
    pub(crate) fn company(self) -> &'static str {
        match self {
            KeyProvider::OpenAi => "OpenAI",
            KeyProvider::Anthropic => "Anthropic",
        }
    }

    /// The variable that `--from-env` reads.
    pub(crate) fn variable(self) -> &'static str {
        match self {
            KeyProvider::OpenAi => "OPENAI_API_KEY",
            KeyProvider::Anthropic => "ANTHROPIC_API_KEY",
        }
    }

    /// The command that stores a key without the check.
    pub(crate) fn without_check(self) -> &'static str {
        match self {
            KeyProvider::OpenAi => {
                "to store the key without the check: efr login openai-api --no-check"
            }
            KeyProvider::Anthropic => {
                "to store the key without the check: efr login anthropic --no-check"
            }
        }
    }

    /// The line that says how the provider bills the key, for a provider whose key a
    /// user may take for the subscription: OpenAI bills its API key per token, apart
    /// from a ChatGPT plan.
    pub(crate) fn billing(self) -> Option<&'static str> {
        match self {
            KeyProvider::OpenAi => {
                Some("note: OpenAI bills the API key per token; a ChatGPT plan does not cover it\n")
            }
            KeyProvider::Anthropic => None,
        }
    }

    /// Where the owner revokes a key.
    pub(crate) fn revoke(self) -> &'static str {
        match self {
            KeyProvider::OpenAi => "revoke it in the dashboard of the OpenAI platform",
            KeyProvider::Anthropic => "revoke it in the Claude Console",
        }
    }
}

/// The key for `provider`: from its variable when `from_env`, else from a prompt on the
/// terminal, else from stdin. Trimmed; an empty key is an error.
pub(crate) async fn read(
    ctx: &Context,
    out: &mut Output,
    provider: KeyProvider,
    from_env: bool,
) -> Result<SecretText, CliError> {
    let (raw, from) = if from_env {
        let name = provider.variable();
        let value = ctx.key_input.var(name).ok_or(CliError::KeyVariableUnset { name })?;
        (value, name)
    } else if ctx.keys.available() {
        (prompt(ctx, out, provider).await?, FROM_PROMPT)
    } else {
        (stdin(ctx).await?, FROM_STDIN)
    };
    let key = raw.trim();
    if key.is_empty() {
        return Err(CliError::NoKey { from });
    }
    Ok(SecretText::new(key))
}

/// Asks for the key on the terminal, which does not show what is typed. Keys typed
/// before the question are thrown away, and so is what is left unread after Enter.
async fn prompt(
    ctx: &Context,
    out: &mut Output,
    provider: KeyProvider,
) -> Result<Zeroizing<String>, CliError> {
    out.err(&format!("{} API key (input is hidden): ", provider.company()));
    let mut reader = ctx.keys.start()?;
    let mut interrupt = ctx.interrupt.wait();
    let mut line = AnswerLine::new();
    let typed = loop {
        let key = tokio::select! {
            () = &mut interrupt => break None,
            key = reader.next() => key,
        };
        match key {
            Some(Key::Byte(byte)) => {
                if line.key(byte) == Edit::Submit {
                    break Some(line.take());
                }
            }
            Some(Key::Esc) => {}
            None => break Some(line.take()),
        }
    };
    reader.stop_discarding().await;
    out.err("\n");
    match typed {
        Some(text) => Ok(Zeroizing::new(text.expose_secret().to_owned())),
        None => Err(CliError::Interrupted),
    }
}

/// Reads stdin to its end, off the async thread, until Ctrl+C.
async fn stdin(ctx: &Context) -> Result<Zeroizing<String>, CliError> {
    let input = std::sync::Arc::clone(&ctx.key_input);
    let read = tokio::task::spawn_blocking(move || input.stdin(STDIN_LIMIT));
    let bytes = tokio::select! {
        () = ctx.interrupt.wait() => return Err(CliError::Interrupted),
        read = read => match read {
            Ok(read) => read.map_err(|source| CliError::KeyInput { source })?,
            Err(join) => return Err(CliError::KeyInput { source: std::io::Error::other(join) }),
        },
    };
    if bytes.len() > STDIN_LIMIT {
        return Err(CliError::KeyTooLong { from: FROM_STDIN });
    }
    match std::str::from_utf8(&bytes) {
        Ok(text) => Ok(Zeroizing::new(text.to_owned())),
        // NOTE: the daemon refuses a key outside ASCII anyway; a text that is not even
        // UTF-8 is no key either.
        Err(_) => Err(CliError::NoKey { from: FROM_STDIN }),
    }
}
