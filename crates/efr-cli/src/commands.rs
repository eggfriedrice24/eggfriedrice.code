//! One module per command. Each one parses nothing and decides little: it turns its
//! arguments into protocol calls, and the replies into text for `output`.

pub(crate) mod compact;
pub(crate) mod config;
pub(crate) mod diff;
pub(crate) mod history;
pub(crate) mod login;
pub(crate) mod logout;
pub(crate) mod models;
pub(crate) mod new;
pub(crate) mod paths;
pub(crate) mod project;
pub(crate) mod sandbox;
pub(crate) mod send;
pub(crate) mod settings;
pub(crate) mod status;
