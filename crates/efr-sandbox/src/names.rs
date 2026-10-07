//! Name patterns with `*`, and the variable names that look like secrets or name a
//! socket of the user's session.

/// True when `name` matches `pattern`, where `*` matches any text, also none. Case
/// matters.
pub fn matches(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == name;
    };
    if parts.len() == 1 {
        return pattern == name;
    }
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// True when any of `patterns` matches `name`.
pub(crate) fn any_matches<S: AsRef<str>>(patterns: &[S], name: &str) -> bool {
    patterns.iter().any(|pattern| matches(pattern.as_ref(), name))
}

/// Variable names that look like a secret; compared without case.
pub const SECRET_NAMES: &[&str] = &[
    "*TOKEN*",
    "*SECRET*",
    "*PASSWORD*",
    "*PASSWD*",
    "*API_KEY*",
    "*APIKEY*",
    "*_KEY",
    "*_KEY_ID",
    "*CREDENTIAL*",
    "*_PAT",
    "*SESSION_ID*",
    "AWS_*",
    "AZURE_*",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// True when `name` looks like a secret: [`SECRET_NAMES`], without case.
pub fn secret_like(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    any_matches(SECRET_NAMES, &upper)
}

/// The variables that name a socket or a session of the user's desktop or agents.
pub const SOCKET_NAMES: &[&str] = &[
    "DBUS_SESSION_BUS_ADDRESS",
    "DBUS_SYSTEM_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "GPG_AGENT_INFO",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "SWAYSOCK",
    "I3SOCK",
    "NIRI_SOCKET",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "KITTY_LISTEN_ON",
    "NVIM",
    "NVIM_LISTEN_ADDRESS",
    "TMUX",
    "ZELLIJ*",
    "KRB5CCNAME",
];

/// The proxy variables; phase 2 sets its own.
pub const PROXY_NAMES: &[&str] = &["*_proxy", "*_PROXY", "NO_PROXY", "no_proxy"];

/// efr's own variables.
pub const EFR_NAMES: &[&str] = &["EFR_*", "_EFR_*"];

/// True when `name` is a shell variable name: `[A-Za-z_][A-Za-z0-9_]*`, at most 128
/// bytes.
pub fn is_variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let first = bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    first && name.len() <= 128 && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests;
