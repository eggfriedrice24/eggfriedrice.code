//! One module per command. Each one parses nothing and decides little: it turns its
//! arguments into protocol calls, and the replies into text for `output`.

pub(crate) mod new;
pub(crate) mod send;
