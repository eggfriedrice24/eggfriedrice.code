//! The links that a named change of a user unit makes (efr's auto spec 10.8, plan
//! PB34): `systemctl --user enable`, `reenable`, `preset`, `mask`, `add-wants` and
//! `add-requires` write links in the user's unit config dir,
//! `~/.config/systemd/user`, so efrd copies that dir before the call and lists it
//! again after it, and `efr undo` brings the links back.
//!
//! The prediction is a check before the copy: efrd finds the unit's file in the user
//! unit dirs and reads `WantedBy=`, `RequiredBy=`, `UpheldBy=` and `Alias=` of its
//! `[Install]` section, with one level of `Also=`. Every name must match the unit
//! name grammar (no `/`, no `..`, a known suffix), so each predicted link lies below
//! the config dir. A template or an instance (`foo@.service`, `foo@bar.service`), a
//! specifier (`%i`), a unit that efrd cannot find, and a config dir that is a link get
//! no prediction: the change is "not covered", and the user approves it. The copy of
//! the whole config dir within the dir limits catches a link that the prediction
//! missed, also one that `--force` replaced.

use std::path::{Path, PathBuf};

use efr_permissions::{CoverAsk, Coverage};

use crate::home::{self, HomeLimits, Target};

/// The suffixes of a unit name that efr knows.
pub(crate) const UNIT_SUFFIXES: &[&str] = &[
    ".service",
    ".socket",
    ".timer",
    ".path",
    ".target",
    ".mount",
    ".automount",
    ".swap",
    ".slice",
];

/// The longest unit name that systemd takes.
const MAX_NAME: usize = 255;

/// The largest unit file that efrd reads for a prediction.
const MAX_UNIT_BYTES: u64 = 64 * 1024;

/// The keys of `[Install]` that make a link in a dir of another unit, with the suffix
/// of that dir.
const DEPENDENCY_KEYS: &[(&str, &str)] =
    &[("WantedBy", ".wants"), ("RequiredBy", ".requires"), ("UpheldBy", ".upholds")];

/// The user's unit config dir, where `systemctl --user` writes its links.
pub(crate) fn config_dir(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
}

/// The dirs where systemd finds the units of the user, in its order.
pub(crate) fn search_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        config_dir(home),
        home.join(".local/share/systemd/user"),
        PathBuf::from("/etc/systemd/user"),
        PathBuf::from("/usr/local/lib/systemd/user"),
        PathBuf::from("/usr/lib/systemd/user"),
    ]
}

/// True when `name` is a unit name of the grammar that efr takes: a known suffix, a
/// stem of letters, digits and `:-_.\`, no `/` and no `..`. A template or an instance
/// (`@`) is no such name.
pub(crate) fn valid_name(name: &str) -> bool {
    let Some(stem) = UNIT_SUFFIXES.iter().find_map(|suffix| name.strip_suffix(suffix)) else {
        return false;
    };
    !stem.is_empty()
        && name.len() <= MAX_NAME
        && !name.contains("..")
        && stem.bytes().all(|byte| byte.is_ascii_alphanumeric() || b":-_.\\".contains(&byte))
}

/// The unit file of `name` in the user unit dirs of `home`: the first dir that holds
/// one, as systemd finds it. `None` for a name that [`valid_name`] refuses. It blocks.
pub(crate) fn unit_file(name: &str, home: &Path) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    search_dirs(home)
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| std::fs::symlink_metadata(path).is_ok())
}

/// What efrd predicts that a change of a unit writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prediction {
    /// The user's unit config dir, which the copy holds whole.
    pub(crate) dir: PathBuf,
    /// The links below [`dir`](Self::dir) that enabling or masking the unit makes.
    pub(crate) links: Vec<PathBuf>,
}

/// The links that enabling or masking the unit `name` of the user of `home` makes, or
/// `None` when efrd cannot tell (see the module doc). It blocks.
pub(crate) fn predict(name: &str, home: &Path) -> Option<Prediction> {
    let dir = config_dir(home);
    if std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return None;
    }
    let mut links = vec![dir.join(name)];
    let install = install_of(name, home)?;
    let mut units = vec![(name.to_owned(), install)];
    let also: Vec<String> = units[0].1.also.clone();
    for other in also {
        let install = install_of(&other, home)?;
        links.push(dir.join(&other));
        units.push((other, install));
    }
    for (unit, install) in &units {
        for (target, suffix) in &install.dependencies {
            links.push(dir.join(format!("{target}{suffix}")).join(unit));
        }
        for alias in &install.aliases {
            links.push(dir.join(alias));
        }
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for link in links {
        if !unique.contains(&link) {
            unique.push(link);
        }
    }
    Some(Prediction { dir, links: unique })
}

/// What a copy holds before a change of the unit `name`: the config dir whole, within
/// the dir limits, and each predicted link that does not exist yet, as missing. A link
/// that exists is in the dir and is copied as it is, never followed. `None` when
/// [`predict`] has no prediction. It blocks.
pub(crate) fn targets(name: &str, home: &Path) -> Option<Vec<Target>> {
    let prediction = predict(name, home)?;
    let mut targets =
        vec![Target { ask: CoverAsk::HomeDir(prediction.dir.clone()), floor: true, link: None }];
    for link in prediction.links {
        if std::fs::symlink_metadata(&link).is_err() {
            targets.push(Target { ask: CoverAsk::Home(link), floor: true, link: None });
        }
    }
    Some(targets)
}

/// Whether efrd can copy what a change of the unit `name` writes, for the facts of a
/// call: a prediction, and the config dir within `limits` with no mask in it. It
/// blocks.
pub(crate) fn coverage(name: &str, home: &Path, masks: &[PathBuf], limits: HomeLimits) -> Coverage {
    match predict(name, home) {
        Some(prediction) => home::coverage(&CoverAsk::HomeDir(prediction.dir), masks, limits),
        None => Coverage::NotAllowed,
    }
}

/// The `[Install]` section of a unit file.
#[derive(Debug, Default)]
struct Install {
    /// `(target, ".wants")` for each `WantedBy=` and the like.
    dependencies: Vec<(String, &'static str)>,
    aliases: Vec<String>,
    also: Vec<String>,
}

/// The `[Install]` section of the unit file of `name`, or `None` when efrd cannot read
/// it whole or a value is no unit name of the grammar.
fn install_of(name: &str, home: &Path) -> Option<Install> {
    let path = unit_file(name, home)?;
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_UNIT_BYTES {
        return None;
    }
    let text = String::from_utf8(std::fs::read(&path).ok()?).ok()?;
    parse_install(&text)
}

/// The `[Install]` section of the unit file `text`.
fn parse_install(text: &str) -> Option<Install> {
    let mut install = Install::default();
    let mut in_install = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        // NOTE: a continued line could hide a value that efrd does not read.
        if line.ends_with('\\') {
            return None;
        }
        if line.starts_with('[') {
            in_install = line == "[Install]";
            continue;
        }
        if !in_install {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if value.contains('%') {
            return None;
        }
        let names: Vec<String> = value.split_whitespace().map(str::to_owned).collect();
        if !names.iter().all(|name| valid_name(name)) {
            return None;
        }
        if let Some((_, suffix)) = DEPENDENCY_KEYS.iter().find(|(known, _)| *known == key) {
            if names.is_empty() {
                install.dependencies.retain(|(_, known)| known != suffix);
            }
            install.dependencies.extend(names.into_iter().map(|name| (name, *suffix)));
        } else if key == "Alias" {
            if names.is_empty() {
                install.aliases.clear();
            }
            install.aliases.extend(names);
        } else if key == "Also" {
            if names.is_empty() {
                install.also.clear();
            }
            install.also.extend(names);
        }
    }
    Some(install)
}

#[cfg(test)]
mod tests;
