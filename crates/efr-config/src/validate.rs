//! The checks that the types alone do not make: sets of names, ranges, the form of
//! paths and URLs, and the entries of `sandbox.allow` and `dbus.allow`.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::slice;

use efr_permissions::{HostError, HostList, PermissionsError, check_rules};
use efr_protocol::Mode;

use crate::tables::diff::{check_glob, check_name};
use crate::tables::render::{COLOR_EXPECTED, CONFIG_COLOR_KEYS};
use crate::{
    ClassifierSettings, CompactionSettings, DiffSettings, JobsSettings, ModelEntry,
    NetworkSettings, PROVIDERS, PresenceSettings, RecordingSettings, RenderColors, SandboxSettings,
    Settings, SnapshotSettings,
};

/// What `permissions.mode = "full"` needs: the gate key next to it. The text names both
/// ways out, because the user who turns the gate off while full is the default reads it
/// too.
const FULL_NEEDS_THE_GATE: &str = "valid without permissions.allow_full = true; set \
     permissions.allow_full = true in [permissions] to allow full mode, or set \
     permissions.mode to manual, cautious or auto";

/// A value outside its allowed set or range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invalid {
    pub(crate) key: &'static str,
    /// The value as TOML.
    pub(crate) value: String,
    pub(crate) expected: &'static str,
}

/// An entry of an array of tables that `efr-permissions` refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BadEntry {
    /// The dotted key of the array, such as `sandbox.allow`.
    pub(crate) key: &'static str,
    /// The entry's place in the array, counted from 0.
    pub(crate) index: usize,
    pub(crate) source: PermissionsError,
}

/// The first entry of `sandbox.allow` or `dbus.allow` that `efr-permissions` refuses, in
/// the order of the file: an always-allow rule of a bad shape
/// ([`AllowRule::check_shape`](efr_permissions::AllowRule::check_shape)), or a bus rule
/// that [`check_rules`] refuses. The floor checks of an always-allow rule need the
/// machine; efrd runs them.
pub(crate) fn entries(settings: &Settings) -> Result<(), BadEntry> {
    for (index, rule) in settings.sandbox.allow.iter().enumerate() {
        rule.check_shape(index).map_err(|source| BadEntry {
            key: "sandbox.allow",
            index,
            source,
        })?;
    }
    check_rules(&settings.dbus.allow).map_err(|source| {
        let index = match &source {
            PermissionsError::BusRule { index, .. } => *index,
            _ => 0,
        };
        BadEntry { key: "dbus.allow", index, source }
    })
}

/// The first value of `settings` that is outside its allowed set or range, in the
/// order of the file.
pub(crate) fn check(settings: &Settings) -> Result<(), Invalid> {
    let Settings {
        log,
        model,
        openai,
        anthropic,
        permissions,
        shell,
        conversation,
        context,
        compaction,
        sandbox,
        network,
        classifier,
        snapshot,
        recording,
        presence,
        jobs,
        diff,
        render,
        ..
    } = settings;
    non_empty("log", log, "a tracing filter such as info")?;

    if !PROVIDERS.contains(&model.provider.as_str()) {
        return Err(invalid(
            "model.provider",
            text(&model.provider),
            "openai-subscription, openai-api or anthropic-api",
        ));
    }
    if let Some(name) = &model.name {
        non_empty("model.name", name, "a model id such as gpt-5.5")?;
    }
    if let Some(effort) = &model.effort
        && !efr_protocol::is_effort_word(effort)
    {
        return Err(invalid(
            "model.effort",
            text(effort),
            "a reasoning effort such as low, medium or high",
        ));
    }
    if let Some(tokens) = model.max_output_tokens {
        within(
            "model.max_output_tokens",
            u64::from(tokens),
            1..=1_000_000,
            "between 1 and 1000000",
        )?;
    }

    non_empty("openai.originator", &openai.originator, "a name such as efr")?;
    for entry in openai.models.iter().flatten() {
        check_model_entry("openai.models", entry)?;
    }
    if let Some(url) = &openai.subscription_base_url {
        http_url("openai.subscription_base_url", url)?;
    }
    if let Some(url) = &openai.api_base_url {
        http_url("openai.api_base_url", url)?;
    }
    if let Some(organization) = &openai.organization {
        header_word(
            "openai.organization",
            organization,
            "an organization id such as org-AbC, without spaces",
        )?;
    }
    if let Some(project) = &openai.project {
        header_word("openai.project", project, "a project id such as proj_AbC, without spaces")?;
    }

    if let Some(url) = &anthropic.base_url {
        http_url("anthropic.base_url", url)?;
    }
    for entry in anthropic.models.iter().flatten() {
        check_model_entry("anthropic.models", entry)?;
    }
    if let Some(workspace) = &anthropic.workspace_id {
        header_word(
            "anthropic.workspace_id",
            workspace,
            "a workspace id such as wrkspc_01AbC, without spaces",
        )?;
    }

    // NOTE: full mode runs commands with no sandbox and no questions, so the file
    // names it as the default only next to the gate key that allows it at all.
    if permissions.mode == Mode::Full && !permissions.allow_full {
        return Err(invalid("permissions.mode", text(Mode::Full.as_str()), FULL_NEEDS_THE_GATE));
    }
    if let Some(path) =
        permissions.secret_paths.iter().find(|path| !path.is_absolute() && !path.starts_with("~"))
    {
        return Err(invalid(
            "permissions.secret_paths",
            path_text(path),
            "absolute paths or paths that start with ~/",
        ));
    }

    if let Some(program) = &shell.program
        && !program.is_absolute()
    {
        return Err(invalid("shell.program", path_text(program), "an absolute path"));
    }
    within("shell.idle_minutes", shell.idle_minutes, 0..=525_600, "between 0 and 525600 (a year)")?;
    within(
        "shell.interactive_timeout_minutes",
        shell.interactive_timeout_minutes,
        1..=1_440,
        "between 1 and 1440 (a day)",
    )?;

    let queued = u64::try_from(conversation.max_queued).unwrap_or(u64::MAX);
    within("conversation.max_queued", queued, 1..=1024, "between 1 and 1024")?;
    if let Some(secs) = conversation.approval_timeout_secs {
        within(
            "conversation.approval_timeout_secs",
            secs,
            1..=604_800,
            "between 1 and 604800 (a week)",
        )?;
    }
    within(
        "conversation.update_interval_ms",
        conversation.update_interval_ms,
        0..=60_000,
        "between 0 and 60000",
    )?;
    within(
        "conversation.draft_interval_ms",
        conversation.draft_interval_ms,
        0..=1_000,
        "between 0 and 1000 (a second)",
    )?;
    within(
        "conversation.tty_idle_hours",
        conversation.tty_idle_hours,
        0..=8_760,
        "between 0 and 8760 (a year)",
    )?;

    within(
        "context.recent_commands",
        u64::from(context.recent_commands),
        0..=10,
        "between 0 and 10",
    )?;
    check_compaction(compaction)?;
    check_sandbox(sandbox)?;
    check_network(network)?;
    check_classifier(classifier)?;
    check_snapshot(snapshot)?;
    check_recording(recording)?;
    check_presence(presence)?;
    check_jobs(jobs)?;
    check_diff(diff)?;

    within("render.diff_lines", u64::from(render.diff_lines), 0..=1_000, "between 0 and 1000")?;
    if let Some(theme) = &render.theme {
        non_empty("render.theme", theme, "a theme name such as catppuccin-mocha")?;
    }
    non_empty("render.theme_dark", &render.theme_dark, "a theme name such as catppuccin-mocha")?;
    non_empty("render.theme_light", &render.theme_light, "a theme name such as catppuccin-latte")?;
    if let Some(palette) = &render.palette
        && !palette.is_absolute()
        && !palette.starts_with("~")
    {
        return Err(invalid(
            "render.palette",
            path_text(palette),
            "an absolute path or a path that starts with ~/",
        ));
    }
    colors(&render.colors, &CONFIG_COLOR_KEYS)
}

/// Every role of `colors` that is set holds a colour. `keys` names the roles in the
/// order of `COLOR_ROLES`.
pub(crate) fn colors(colors: &RenderColors, keys: &[&'static str; 13]) -> Result<(), Invalid> {
    match colors.first_invalid() {
        Some((index, value)) => {
            let key = keys.get(index).copied().unwrap_or("colors");
            Err(invalid(key, value.as_toml(), COLOR_EXPECTED))
        }
        None => Ok(()),
    }
}

/// The checks of one entry of `[openai] models` or `[anthropic] models`, the list that
/// `key` names.
fn check_model_entry(key: &'static str, entry: &ModelEntry) -> Result<(), Invalid> {
    const EXPECTED: &str = "a list of model ids, or of tables { id, context_window, \
                            max_output_tokens } with a window from 1000 to 100000000 and \
                            an output limit from 1 to 1000000 below the window";
    let id = entry.id();
    if id.trim().is_empty() {
        return Err(invalid(key, text(id), EXPECTED));
    }
    let window = entry.context_window();
    let output = entry.max_output_tokens().map(u64::from);
    let window_fits = window.is_none_or(|window| (1_000..=100_000_000).contains(&window));
    let output_fits = output.is_none_or(|output| (1..=1_000_000).contains(&output));
    let output_below = match (window, output) {
        (Some(window), Some(output)) => output < window,
        _ => true,
    };
    if !(window_fits && output_fits && output_below) {
        return Err(invalid(key, text(id), EXPECTED));
    }
    Ok(())
}

/// The checks of `[compaction]`.
fn check_compaction(compaction: &CompactionSettings) -> Result<(), Invalid> {
    within("compaction.auto_at", u64::from(compaction.auto_at), 1..=99, "between 1 and 99")
}

/// The checks of `[recording]`.
fn check_recording(recording: &RecordingSettings) -> Result<(), Invalid> {
    within(
        "recording.max_age_days",
        u64::from(recording.max_age_days),
        0..=3_650,
        "between 0 and 3650",
    )?;
    within("recording.max_gib", u64::from(recording.max_gib), 0..=10_000, "between 0 and 10000")
}

/// The checks of `[snapshot]`, in the order of the table.
fn check_snapshot(snapshot: &SnapshotSettings) -> Result<(), Invalid> {
    within(
        "snapshot.max_file_mib",
        u64::from(snapshot.max_file_mib),
        1..=1_024,
        "between 1 and 1024",
    )?;
    within(
        "snapshot.max_files",
        u64::from(snapshot.max_files),
        100..=1_000_000,
        "between 100 and 1000000",
    )?;
    within(
        "snapshot.keep_turns",
        u64::from(snapshot.keep_turns),
        1..=10_000,
        "between 1 and 10000",
    )?;
    within(
        "snapshot.max_age_days",
        u64::from(snapshot.max_age_days),
        1..=3_650,
        "between 1 and 3650",
    )?;
    within(
        "snapshot.mirror_max_mib",
        u64::from(snapshot.mirror_max_mib),
        0..=65_536,
        "between 0 and 65536",
    )?;
    within(
        "snapshot.home_max_files",
        u64::from(snapshot.home_max_files),
        0..=100_000,
        "between 0 and 100000",
    )?;
    within(
        "snapshot.home_max_mib",
        u64::from(snapshot.home_max_mib),
        0..=16_384,
        "between 0 and 16384",
    )
}

/// The checks of `[classifier]`, in the order of the table.
fn check_classifier(classifier: &ClassifierSettings) -> Result<(), Invalid> {
    if let Some(model) = &classifier.model {
        non_empty("classifier.model", model, "a model id such as gpt-6-luna")?;
    }
    if !efr_protocol::is_effort_word(&classifier.effort) {
        return Err(invalid(
            "classifier.effort",
            text(&classifier.effort),
            "a reasoning effort such as low, medium or high",
        ));
    }
    within(
        "classifier.timeout_secs",
        u64::from(classifier.timeout_secs),
        1..=300,
        "between 1 and 300",
    )
}

/// The checks of `[presence]`, in the order of the table.
fn check_presence(presence: &PresenceSettings) -> Result<(), Invalid> {
    within(
        "presence.away_after_minutes",
        u64::from(presence.away_after_minutes),
        1..=1_440,
        "between 1 and 1440 (a day)",
    )?;
    within(
        "presence.away_wait_secs",
        u64::from(presence.away_wait_secs),
        1..=86_400,
        "between 1 and 86400 (a day)",
    )
}

/// The checks of `[jobs]`, in the order of the table. `default_minutes` is checked
/// against `max_minutes` once both are in their ranges.
fn check_jobs(jobs: &JobsSettings) -> Result<(), Invalid> {
    const MINUTES: &str = "between 1 and 1440 (a day)";
    within("jobs.default_minutes", u64::from(jobs.default_minutes), 1..=1_440, MINUTES)?;
    within("jobs.max_minutes", u64::from(jobs.max_minutes), 1..=1_440, MINUTES)?;
    if jobs.default_minutes > jobs.max_minutes {
        return Err(invalid(
            "jobs.default_minutes",
            jobs.default_minutes.to_string(),
            "at most jobs.max_minutes",
        ));
    }
    within("jobs.max_running", u64::from(jobs.max_running), 1..=64, "between 1 and 64")?;
    within(
        "jobs.max_running_daemon",
        u64::from(jobs.max_running_daemon),
        1..=256,
        "between 1 and 256",
    )?;
    within(
        "jobs.max_wakes_per_hour",
        u64::from(jobs.max_wakes_per_hour),
        0..=60,
        "between 0 and 60",
    )?;
    within(
        "jobs.max_wakes_per_hour_daemon",
        u64::from(jobs.max_wakes_per_hour_daemon),
        0..=120,
        "between 0 and 120",
    )?;
    within(
        "jobs.max_wakes_per_watch",
        u64::from(jobs.max_wakes_per_watch),
        0..=10,
        "between 0 and 10",
    )?;
    within("jobs.nice", u64::from(jobs.nice), 0..=19, "between 0 and 19")
}

/// The checks of `[sandbox]`, in the order of the table.
fn check_sandbox(sandbox: &SandboxSettings) -> Result<(), Invalid> {
    const HOME_OR_ABSOLUTE: &str = "absolute paths or paths that start with ~/";
    if let Some(bwrap) = &sandbox.bwrap
        && !bwrap.is_absolute()
    {
        return Err(invalid("sandbox.bwrap", path_text(bwrap), "an absolute path"));
    }
    home_or_absolute("sandbox.write_roots", &sandbox.write_roots, HOME_OR_ABSOLUTE)?;
    if let Some(root) = sandbox
        .write_roots
        .iter()
        .find(|root| root.as_path() == Path::new("~") || root.parent().is_none())
    {
        return Err(invalid(
            "sandbox.write_roots",
            path_text(root),
            "directories below ~/ or absolute paths, never ~ or / itself",
        ));
    }
    home_or_absolute("sandbox.caches", &sandbox.caches, HOME_OR_ABSOLUTE)?;
    within("sandbox.cache_days", u64::from(sandbox.cache_days), 1..=3_650, "between 1 and 3650")?;
    within(
        "sandbox.cache_max_gib",
        u64::from(sandbox.cache_max_gib),
        1..=10_000,
        "between 1 and 10000",
    )?;
    home_or_absolute("sandbox.mask", &sandbox.mask, HOME_OR_ABSOLUTE)?;
    patterns("sandbox.mask_globs", &sandbox.mask_globs)?;
    home_or_absolute("sandbox.protect", &sandbox.protect, HOME_OR_ABSOLUTE)?;
    variables("sandbox.env_deny", &sandbox.env_deny)?;
    variables("sandbox.env_keep", &sandbox.env_keep)?;
    variables("sandbox.promote_env", &sandbox.promote_env)?;
    variables("sandbox.export_deny", &sandbox.export_deny)?;
    home_or_absolute("sandbox.synced_dirs", &sandbox.synced_dirs, HOME_OR_ABSOLUTE)?;
    patterns("sandbox.surface_files", &sandbox.surface_files)?;
    if let Some(name) = sandbox
        .rebuildable
        .iter()
        .find(|name| name.is_empty() || name.contains(['/', '\0']) || *name == "." || *name == "..")
    {
        return Err(invalid("sandbox.rebuildable", text(name), "directory names such as target"));
    }
    Ok(())
}

/// The checks of `[diff]`: each group's name, and each of its globs, at least one.
fn check_diff(diff: &DiffSettings) -> Result<(), Invalid> {
    for (name, globs) in diff.groups.iter() {
        if let Some(expected) = check_name(name) {
            return Err(invalid("diff.groups", text(name), expected));
        }
        if globs.is_empty() {
            return Err(invalid("diff.groups", text(name), "a list of one or more globs"));
        }
        if let Some((glob, expected)) =
            globs.iter().find_map(|glob| check_glob(glob).map(|expected| (glob, expected)))
        {
            return Err(invalid("diff.groups", text(glob), expected));
        }
    }
    Ok(())
}

/// The checks of `[network]`: the host list that efrd and the proxy compile, with the
/// first entry it refuses in the order of the table (`allow`, `deny`, `hosts`), then
/// the upload limit.
fn check_network(network: &NetworkSettings) -> Result<(), Invalid> {
    if let Err(error) = network.host_list(&[]) {
        let refused = |result: Result<HostList, HostError>| result.err();
        let first = network
            .allow
            .iter()
            .find_map(|raw| {
                refused(HostList::compile(slice::from_ref(raw), &[], &[], &[]))
                    .map(|error| host_invalid("network.allow", raw, &error))
            })
            .or_else(|| {
                network.deny.iter().find_map(|raw| {
                    refused(HostList::compile(&[], slice::from_ref(raw), &[], &[]))
                        .map(|error| host_invalid("network.deny", raw, &error))
                })
            })
            .or_else(|| {
                network.hosts.iter().find_map(|host| {
                    refused(HostList::compile(&[], &[], &[host.table()], &[]))
                        .map(|error| host_invalid("network.hosts", &host.host, &error))
                })
            });
        // NOTE: the list refuses only single entries, so one of them is the cause.
        return Err(first.unwrap_or_else(|| host_invalid("network.allow", "", &error)));
    }
    within(
        "network.max_upload_bytes",
        network.max_upload_bytes,
        65_536..=1_073_741_824,
        "between 65536 (64 KiB) and 1073741824 (1 GiB)",
    )
}

/// The entry `raw` of `key`, which the host list refused with `error`.
fn host_invalid(key: &'static str, raw: &str, error: &HostError) -> Invalid {
    let expected = match error {
        HostError::BareStar => {
            "a host or a suffix such as *.example.com; * alone allows every host"
        }
        HostError::ShortSuffix(_) => {
            "a suffix with at least two labels after *., such as *.example.com"
        }
        HostError::NotAscii(_) => "a host name in ASCII; write the ASCII (xn--) form",
        HostError::UnknownPreset(_) => {
            "a host or a preset: @crates, @npm, @pypi, @go, @github-read or @pacman-mirrors"
        }
        HostError::IpLiteral(_) => "a host name; an IP address goes in [[network.hosts]]",
        HostError::BadPort(_) => "a host with a port from 1 to 65535",
        _ => "a host name such as example.com, or *.example.com",
    };
    invalid(key, text(raw), expected)
}

/// Every path is absolute or starts with the `~` component.
fn home_or_absolute(
    key: &'static str,
    paths: &[PathBuf],
    expected: &'static str,
) -> Result<(), Invalid> {
    match paths.iter().find(|path| !path.is_absolute() && !path.starts_with("~")) {
        Some(path) => Err(invalid(key, path_text(path), expected)),
        None => Ok(()),
    }
}

/// Every entry is a variable name, or a pattern of one with `*`.
fn variables(key: &'static str, names: &[String]) -> Result<(), Invalid> {
    let bad = |name: &String| {
        let mut bytes = name.bytes();
        let first_ok =
            bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'*');
        !first_ok || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'*')
    };
    match names.iter().find(|name| bad(name)) {
        Some(name) => Err(invalid(key, text(name), "variable names, or patterns of them with *")),
        None => Ok(()),
    }
}

/// Every entry is a name or a relative path, with `*`, optionally behind a `!`.
fn patterns(key: &'static str, list: &[String]) -> Result<(), Invalid> {
    let bad = |pattern: &String| {
        let body = pattern.strip_prefix('!').unwrap_or(pattern);
        body.is_empty()
            || body.starts_with('/')
            || body.contains('\0')
            || body.split('/').any(|part| part.is_empty() || part == "..")
    };
    match list.iter().find(|pattern| bad(pattern)) {
        Some(pattern) => {
            Err(invalid(key, text(pattern), "names or relative paths, with * and an optional !"))
        }
        None => Ok(()),
    }
}

fn non_empty(key: &'static str, value: &str, expected: &'static str) -> Result<(), Invalid> {
    if value.trim().is_empty() { Err(invalid(key, text(value), expected)) } else { Ok(()) }
}

/// A value that goes into an HTTP header as it is: one word of visible ASCII.
fn header_word(key: &'static str, value: &str, expected: &'static str) -> Result<(), Invalid> {
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_graphic()) {
        return Err(invalid(key, text(value), expected));
    }
    Ok(())
}

fn http_url(key: &'static str, url: &str) -> Result<(), Invalid> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(rest) if !rest.is_empty() && !rest.contains(char::is_whitespace) => Ok(()),
        _ => Err(invalid(key, text(url), "an http or https URL")),
    }
}

fn within(
    key: &'static str,
    value: u64,
    range: RangeInclusive<u64>,
    expected: &'static str,
) -> Result<(), Invalid> {
    if range.contains(&value) { Ok(()) } else { Err(invalid(key, value.to_string(), expected)) }
}

fn invalid(key: &'static str, value: String, expected: &'static str) -> Invalid {
    Invalid { key, value, expected }
}

/// `value` as a TOML string.
fn text(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

fn path_text(path: &Path) -> String {
    text(&path.to_string_lossy())
}

#[cfg(test)]
mod tests;
