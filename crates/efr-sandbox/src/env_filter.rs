//! [`EnvFilter`]: the environment of a contained call (the spec's section 3.9).
//!
//! The launcher starts from its own environment, which is the trusted shell's, and
//! removes sockets, sessions, secret-like names, proxies and efr's own variables. It
//! then sets the private `/tmp`, an empty runtime dir, `EFR_SANDBOX=1`, the variables
//! that stop credential prompts, and the offline hints while there is no network.
//! `result.json` lists the removed names, never a value.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::names::{EFR_NAMES, PROXY_NAMES, SOCKET_NAMES, any_matches, secret_like};
use crate::paths::is_within;
use crate::plan::MountPlan;
use crate::spec::{NetworkPlan, SandboxSpec};

/// The names that efr removes from every hidden shell (`efr-shell`'s `env.rs`), which
/// the launcher removes again in case the trusted shell set them.
pub const HIDDEN_SHELL_SCRUB: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERMINFO",
    "COLORFGBG",
    "WINDOWID",
    "VTE_VERSION",
    "TMUX_PANE",
    "STY",
    "GHOSTTY_*",
    "KITTY_*",
    "WEZTERM_*",
    "ITERM_*",
    "KONSOLE_*",
];

/// The offline hints of phase 1: a build with a warm cache works with no network.
pub const OFFLINE_HINTS: &[(&str, &str)] = &[
    ("CARGO_NET_OFFLINE", "true"),
    ("npm_config_offline", "true"),
    ("UV_OFFLINE", "1"),
    ("GOPROXY", "off"),
];

/// Build caches that move below `~/.cache` when the user's value lies outside every
/// overlay, so their writes land in the conversation's private layer.
pub const MOVED_CACHES: &[(&str, &str)] =
    &[("CCACHE_DIR", "ccache"), ("SCCACHE_DIR", "sccache"), ("ZIG_GLOBAL_CACHE_DIR", "zig")];

/// The environment filter of one contained call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFilter {
    deny: Vec<String>,
    keep: Vec<String>,
    set: Vec<(String, String)>,
    home: PathBuf,
    overlays: Vec<PathBuf>,
}

impl EnvFilter {
    /// The filter of `spec`'s call under `plan`.
    pub fn new(spec: &SandboxSpec, plan: &MountPlan) -> EnvFilter {
        let mut set: Vec<(String, String)> = vec![
            ("TMPDIR".to_owned(), "/tmp".to_owned()),
            (
                "XDG_RUNTIME_DIR".to_owned(),
                spec.runtime.user_runtime.to_string_lossy().into_owned(),
            ),
            ("EFR_SANDBOX".to_owned(), "1".to_owned()),
            ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
            ("GIT_ASKPASS".to_owned(), "/bin/false".to_owned()),
            ("SSH_ASKPASS".to_owned(), "/bin/false".to_owned()),
            ("SSH_ASKPASS_REQUIRE".to_owned(), "never".to_owned()),
            ("GCM_INTERACTIVE".to_owned(), "never".to_owned()),
        ];
        let offline = spec.env.offline_hints
            && plan.unshares_network()
            && matches!(spec.network, NetworkPlan::None);
        if offline {
            set.extend(
                OFFLINE_HINTS.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
            );
        }
        set.extend(plan.env_overrides().iter().cloned());
        let overlays = plan
            .mounts()
            .iter()
            .filter_map(|mount| match &mount.op {
                crate::MountOp::Overlay { target, .. }
                | crate::MountOp::TmpOverlay { target, .. } => Some(target.clone()),
                _ => None,
            })
            .collect();
        EnvFilter {
            deny: spec.env.deny.clone(),
            keep: spec.env.keep.clone(),
            set,
            home: spec.runtime.home.clone(),
            overlays,
        }
    }

    /// True when the filter removes `name`.
    pub fn removes(&self, name: &str) -> bool {
        any_matches(EFR_NAMES, name)
            || any_matches(SOCKET_NAMES, name)
            || any_matches(HIDDEN_SHELL_SCRUB, name)
            || any_matches(PROXY_NAMES, name)
            || any_matches(&self.deny, name)
            || (secret_like(name) && !any_matches(&self.keep, name))
    }

    /// The environment of the call, and the names removed from `env` in order.
    pub fn apply(
        &self,
        env: &BTreeMap<OsString, OsString>,
    ) -> (BTreeMap<OsString, OsString>, Vec<String>) {
        let mut out = BTreeMap::new();
        let mut removed = Vec::new();
        for (name, value) in env {
            let text = name.to_string_lossy();
            if name.to_str().is_none() || self.removes(&text) {
                removed.push(text.into_owned());
            } else {
                out.insert(name.clone(), value.clone());
            }
        }
        for (name, cache) in MOVED_CACHES {
            let Some(value) = out.get(OsStr::new(name)) else { continue };
            let inside = self.overlays.iter().any(|overlay| is_within(Path::new(value), overlay));
            if !inside {
                let moved = self.home.join(".cache").join(cache);
                out.insert(OsString::from(name), moved.into_os_string());
            }
        }
        for (name, value) in &self.set {
            out.insert(OsString::from(name), OsString::from(value));
        }
        (out, removed)
    }
}

#[cfg(test)]
mod tests;
