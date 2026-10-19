//! The probe's view of the sandbox: whether `auto` can run here, and why not.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CacheMode, NetworkMode};

/// The result of the sandbox probe, which efrd runs at start, after a reload that
/// changes `[sandbox]`, after a failed launch, and before an `auto` turn when the last
/// probe failed. `auto` requires `available`; without it a turn runs as `cautious`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxStatus {
    /// True when every check passed.
    pub available: bool,
    /// Why the sandbox is unavailable, in one sentence, such as `bubblewrap is not
    /// installed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What fixes it, such as `Arch: pacman -S bubblewrap`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// The kernel's Landlock ABI; absent when Landlock is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landlock_abi: Option<u32>,
    /// The bit mask of Landlock errata that the kernel reports fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errata: Option<u32>,
    /// The bubblewrap program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bwrap: Option<PathBuf>,
    /// Its version, such as `0.13.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bwrap_version: Option<String>,
    /// How the caches are mounted; the probe picks `tmp` when an overlay fails.
    pub cache_mode: CacheMode,
    /// What network a contained call has.
    pub network_mode: NetworkMode,
    /// Things that work but need the user's attention, one sentence each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// True when the probe's network view self-test gave every expected refusal, so a
    /// contained call can get `Grant::NetView`. False before phase D1 and when a check
    /// failed.
    #[serde(default)]
    pub netview: bool,
    /// True when the user's `network.netview` is false, so no call gets the network
    /// view, whatever the self-test found. efrd sets it when it sends the status; the
    /// probe leaves it false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub netview_off: bool,
    /// True when the probe's proxy self-test gave every expected result, so a contained
    /// call can reach the network through efr's proxy (phase D2). False when absent.
    #[serde(default)]
    pub proxy: bool,
    /// True when efrd runs the filtered bus proxy: the program exists, its self-test
    /// passed and `dbus.proxy` is true (phase E). False when absent.
    #[serde(default)]
    pub bus_proxy: bool,
    /// Why the bus filter is off, when it is (phase E). Absent when it is on, and from
    /// an earlier daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus_proxy_off: Option<BusProxyOff>,
}

/// Why the bus filter is off (phase E).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BusProxyOff {
    /// `dbus.proxy` is false.
    Config,
    /// `xdg-dbus-proxy` is not installed as a trusted program.
    NoProgram,
    /// The self-test of the proxy failed.
    SelfTest,
    /// The installed `xdg-dbus-proxy` is older than the oldest version that efr tested.
    OldVersion,
    /// A reason that this build does not know.
    #[serde(other)]
    Unknown,
}

impl BusProxyOff {
    /// Every reason, in the order of the enum.
    pub const ALL: [BusProxyOff; 5] = [
        BusProxyOff::Config,
        BusProxyOff::NoProgram,
        BusProxyOff::SelfTest,
        BusProxyOff::OldVersion,
        BusProxyOff::Unknown,
    ];
}

impl SandboxStatus {
    /// The status before the first probe: unavailable, with `reason`.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        SandboxStatus {
            available: false,
            reason: Some(reason.into()),
            fix: None,
            landlock_abi: None,
            errata: None,
            bwrap: None,
            bwrap_version: None,
            cache_mode: CacheMode::default(),
            network_mode: NetworkMode::default(),
            warnings: Vec::new(),
            netview: false,
            netview_off: false,
            proxy: false,
            bus_proxy: false,
            bus_proxy_off: None,
        }
    }
}

/// The outcome of one check of the probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckOutcome {
    /// The check passed.
    Ok,
    /// It passed, but the user should know something.
    Warn,
    /// It failed: `auto` is unavailable.
    Fail,
    /// It did not run, because an earlier check failed.
    Skipped,
}

/// One check of the probe, as `efr sandbox check` prints it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxCheck {
    /// The check, such as `landlock` or `bwrap`.
    pub name: String,
    /// How it went.
    pub outcome: CheckOutcome,
    /// What it found, such as `Landlock ABI 10, errata 0xf`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What fixes a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Where the sandbox keeps its files, for `efr paths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxPaths {
    /// The launcher that the hidden shells run, `$XDG_RUNTIME_DIR/efr/bin/efr-sbx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<PathBuf>,
    /// The installed launcher it was copied from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher_source: Option<PathBuf>,
    /// True when the copy's SHA-256 matches its source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher_sha256_ok: Option<bool>,
    /// The sandbox state: private `/tmp`, cache upper layers, quarantine.
    pub state: PathBuf,
    /// The sandbox runtime: call dirs, shell snapshots and sandbox state.
    pub runtime: PathBuf,
}
