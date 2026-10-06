//! The end mark of a sandboxed call: `ESC ] 133 ; efr-sbx ; <nonce> BEL`.
//!
//! efr's trusted zsh wrapper prints it after the launcher of a sandboxed call has
//! returned. The nonce is 128 random bits as 32 lowercase hex digits; efrd keeps it in
//! its memory and in a file that the sandbox cannot see, so sandboxed code cannot print
//! a mark that `efr-shell` accepts. A body of any other shape is not this mark.

/// The text after `133;` that starts the mark.
pub(crate) const PREFIX: &[u8] = b"efr-sbx;";

/// The bytes of a nonce.
pub(crate) const NONCE_BYTES: usize = 16;

/// The nonce of a body after `133;`, when it is exactly [`PREFIX`] and 32 lowercase hex
/// digits. `None` for anything else, also for upper-case digits or extra fields, so one
/// nonce has exactly one form on the wire.
pub(crate) fn parse(data: &[u8]) -> Option<[u8; NONCE_BYTES]> {
    let hex = data.strip_prefix(PREFIX)?;
    if hex.len() != NONCE_BYTES * 2 {
        return None;
    }
    let mut nonce = [0_u8; NONCE_BYTES];
    for (slot, pair) in nonce.iter_mut().zip(hex.chunks_exact(2)) {
        *slot = lower_hex(pair[0])? << 4 | lower_hex(pair[1])?;
    }
    Some(nonce)
}

fn lower_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
