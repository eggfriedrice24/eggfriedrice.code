//! `examples/config.toml`: every key of the file with a comment, and a value or its
//! default commented out. `efr config edit` and the writer start a missing file from it.

/// The example file. Every key is named in it; a test keeps it equal to the schema.
pub const EXAMPLE: &str = include_str!("../examples/config.toml");

#[cfg(test)]
mod tests;
