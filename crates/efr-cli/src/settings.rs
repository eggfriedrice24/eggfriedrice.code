//! The client's settings from `config.toml`: the `[render]` table, and the turn
//! defaults that `efr settings` shows (`permissions.mode`, `model.name`,
//! `model.effort`).
//!
//! `efr-config` reads and checks the whole file with the one schema that `efrd` uses
//! too, unknown keys refused, so a file the daemon would refuse is refused here as
//! well. A file the client cannot use costs a warning, never a failed prompt: the
//! defaults apply. The theme's name is checked here, because the themes live in
//! `efr-render`.
//!
//! `render.theme = "auto"` takes `render.theme_dark` or `render.theme_light` as the
//! terminal's background is dark or light (`EFR_TERMINAL_BG`, which the zsh plugin
//! sets); without it, the dark one. The colour of each role comes from
//! `[render.colors]`, then the theme file that `render.palette` names, then the
//! terminal's 16 colours; the theme file's `code_theme`, a `.tmTheme` file, wins over
//! `render.theme`. A theme file or a code theme that cannot be used is a warning, and
//! the layers below it apply.
//!
//! ```toml
//! [render]
//! theme = "catppuccin-mocha"   # any name efr_render::Theme::from_name accepts, or auto
//! palette = "~/.config/efr/theme.toml"
//! ```

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use efr_config::{AUTO_THEME, CONFIG_FILE, ConfigError, RenderColors, RoleColor, ThemeFile};
use efr_protocol::Mode;
use efr_render::{CodeTheme, Colour, Palette, RenderError, Role, Theme};

use crate::terminal::Background;

/// The client's settings and where they came from.
#[derive(Debug, Default)]
pub(crate) struct Settings {
    /// The theme for code blocks and diffs.
    pub(crate) theme: Theme,
    /// Where `theme` came from.
    pub(crate) theme_source: Source,
    /// The terminal's background, as `EFR_TERMINAL_BG` says, for `theme = "auto"`.
    pub(crate) background: Option<Background>,
    /// The colour of each role: `[render.colors]` over the theme file.
    pub(crate) palette: Palette,
    /// The code theme of the theme file, which wins over `theme`.
    pub(crate) code_theme: Option<CodeTheme>,
    /// The file `code_theme` was read from.
    pub(crate) code_theme_path: Option<PathBuf>,
    /// The whole `[render]` table: motion, the end-of-turn line and the progress bar.
    /// The defaults when the file cannot be used.
    pub(crate) render: efr_config::RenderSettings,
    /// The turn defaults that the file sets.
    pub(crate) turn: TurnDefaults,
    /// What went wrong while reading the file.
    pub(crate) warnings: Vec<Warning>,
}

/// The defaults of a turn's settings that `config.toml` sets; `None` for each one it
/// leaves out. The daemon reads its own copy of the file, so these are what a prompt
/// gets only when both read the same config root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnDefaults {
    /// The file, when it was read and is valid.
    pub(crate) path: Option<PathBuf>,
    /// `permissions.mode`.
    pub(crate) mode: Option<Mode>,
    /// `model.name`.
    pub(crate) model: Option<String>,
    /// `model.effort`.
    pub(crate) effort: Option<String>,
}

/// Where a setting came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Source {
    /// The built-in default.
    #[default]
    Default,
    /// `config.toml` at this path.
    File(PathBuf),
}

/// A problem with `config.toml` that the defaults covered.
#[derive(Debug)]
pub(crate) enum Warning {
    /// The file exists but could not be read.
    Unreadable { path: PathBuf, source: io::Error },
    /// The file is not valid TOML, has an unknown key, or a value `efr-config` refuses.
    Invalid { path: PathBuf, source: Box<ConfigError> },
    /// `render.theme`, or for `auto` the theme it picks, names no embedded theme.
    UnknownTheme { path: PathBuf, name: String },
    /// The theme file that `render.palette` names cannot be read or is not valid.
    Palette { source: Box<ConfigError> },
    /// The code theme of the theme file cannot be read.
    CodeThemeUnreadable { path: PathBuf, source: io::Error },
    /// The code theme of the theme file is not a `.tmTheme` file.
    CodeThemeInvalid { path: PathBuf, source: RenderError },
    /// `EFR_TERMINAL_BG` is neither `dark` nor `light`.
    Background { value: String },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::Unreadable { path, source } => {
                write!(f, "{} could not be read: {source}", path.display())
            }
            Warning::Invalid { path, source } => {
                write!(f, "{} is not valid: {}", path.display(), describe(source))
            }
            Warning::UnknownTheme { path, name } => {
                write!(f, "{} names the theme {name:?}, which does not exist", path.display())
            }
            Warning::Palette { source } => {
                write!(f, "the theme file is not used: {}", describe(source))
            }
            Warning::CodeThemeUnreadable { path, source } => {
                write!(f, "the code theme {} could not be read: {source}", path.display())
            }
            Warning::CodeThemeInvalid { path, source } => {
                write!(f, "the code theme {} is not used: {source}", path.display())
            }
            Warning::Background { value } => write!(
                f,
                "EFR_TERMINAL_BG is {value:?}, which is neither dark nor light; auto takes the dark theme"
            ),
        }
    }
}

/// What is wrong in `error` on one line, with its key and place when it has them, such
/// as `unknown field ... (shell.idle_minuets, line 2, column 1)`.
pub(crate) fn describe(error: &ConfigError) -> String {
    let mut text = reason(error);
    match (error.location(), error.key()) {
        (Some(at), Some(key)) => text.push_str(&format!(" ({key}, {at})")),
        (Some(at), None) => text.push_str(&format!(" ({at})")),
        (None, Some(key)) => text.push_str(&format!(" ({key})")),
        (None, None) => {}
    }
    crate::format::one_line(&text)
}

/// Why `text`, the contents of the config file `path` (`None`: no file), cannot be
/// used, as `efrd` and `efr` see it: the daemon's checks, the names of the themes, then
/// the theme file and its code theme, with `~` in their paths taken as `home`.
pub(crate) async fn problem(path: &Path, text: Option<&str>, home: &Path) -> Option<String> {
    let settings = match efr_config::Settings::parse(path, text) {
        Ok(settings) => settings,
        Err(error) => return Some(describe(&error)),
    };
    let render = &settings.render;
    let names = [
        ("render.theme", render.theme.as_deref().filter(|name| *name != AUTO_THEME)),
        ("render.theme_dark", Some(render.theme_dark.as_str())),
        ("render.theme_light", Some(render.theme_light.as_str())),
    ];
    for (key, name) in names {
        if let Some(name) = name
            && Theme::from_name(name).is_err()
        {
            return Some(format!("{key} names the theme {name:?}, which does not exist"));
        }
    }
    let theme_file = match read_theme_file(render, home).await? {
        Ok(file) => file,
        Err(error) => return Some(format!("render.palette: {}", describe(&error))),
    };
    let path = theme_file.code_theme_path(home)?;
    match read_code_theme(&path).await {
        Ok(_) => None,
        Err(warning) => Some(format!("render.palette: {warning}")),
    }
}

/// The theme file that `render` names, read and checked; `None` without one.
async fn read_theme_file(
    render: &efr_config::RenderSettings,
    home: &Path,
) -> Option<Result<ThemeFile, ConfigError>> {
    let path = render.palette_path(home)?;
    // NOTE: ThemeFile::load blocks; the file is read here and only parsed there.
    Some(match tokio::fs::read_to_string(&path).await {
        Ok(text) => ThemeFile::parse(&path, &text),
        Err(source) => Err(ConfigError::ThemeRead { path, source }),
    })
}

/// The code theme at `path`, or the warning that says why it cannot be used.
async fn read_code_theme(path: &Path) -> Result<CodeTheme, Warning> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|source| Warning::CodeThemeUnreadable { path: path.to_path_buf(), source })?;
    CodeTheme::from_tmtheme(&bytes)
        .map_err(|source| Warning::CodeThemeInvalid { path: path.to_path_buf(), source })
}

/// The palette with the colour of each role that `colors` sets.
pub(crate) fn palette(colors: &RenderColors) -> Palette {
    let mut palette = Palette::new();
    for (name, color) in colors.colors() {
        let Some(role) = Role::from_name(name) else { continue };
        let colour = match color {
            RoleColor::Slot(slot) => Colour::Palette(slot),
            RoleColor::Rgb(red, green, blue) => Colour::Rgb(red, green, blue),
            _ => continue,
        };
        palette.set(role, colour);
    }
    palette
}

/// What is wrong, without the path that the warning already names.
fn reason(error: &ConfigError) -> String {
    match error {
        // NOTE: the parser's Display quotes the file with a caret under the error; the
        // place follows the reason instead.
        ConfigError::Parse { source, .. } => source.message().to_owned(),
        ConfigError::Invalid { key, value, expected, .. } => {
            format!("{key} = {value} is not {expected}")
        }
        other => efr_stdx::with_causes(other),
    }
}

impl Settings {
    /// Reads `config.toml` from the config root `config_dir`, then the theme file and
    /// its code theme, with `~` in their paths taken as `home`. `background` is the
    /// value of `EFR_TERMINAL_BG`.
    pub(crate) async fn load(config_dir: &Path, background: Option<&str>, home: &Path) -> Settings {
        let path = config_dir.join(CONFIG_FILE);
        let mut settings = match tokio::fs::read_to_string(&path).await {
            Ok(text) => Settings::parse_on(&path, Some(&text), background),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                Settings::parse_on(&path, None, background)
            }
            Err(source) => Settings {
                warnings: vec![Warning::Unreadable { path, source }],
                ..Settings::default()
            },
        };
        settings.load_colours(home).await;
        settings
    }

    /// The settings in `text`, the contents of the file at `path`, on a terminal whose
    /// background is not known.
    #[cfg(test)]
    pub(crate) fn parse(path: &Path, text: &str) -> Settings {
        Settings::parse_on(path, Some(text), None)
    }

    /// The settings in `text`, the contents of the file at `path`, on a terminal whose
    /// background `EFR_TERMINAL_BG` gives. `None` is a missing file: the defaults apply.
    pub(crate) fn parse_on(path: &Path, text: Option<&str>, background: Option<&str>) -> Settings {
        let mut settings = Settings::default();
        match background.map(Background::from_name) {
            Some(Ok(background)) => settings.background = Some(background),
            Some(Err(value)) => settings.warnings.push(Warning::Background { value }),
            None => {}
        }
        let Some(text) = text else {
            return settings;
        };
        let file = match efr_config::Settings::parse(path, Some(text)) {
            Ok(file) => file,
            Err(source) => {
                let path = path.to_path_buf();
                settings.warnings.push(Warning::Invalid { path, source: Box::new(source) });
                return settings;
            }
        };
        let from_file = |key: &str| file.source(key) == efr_config::Source::File;
        settings.turn = TurnDefaults {
            path: Some(path.to_path_buf()),
            mode: from_file("permissions.mode").then_some(file.permissions.mode),
            model: file.model.name.clone(),
            effort: file.model.effort.clone(),
        };
        settings.render = file.render.clone();
        let render = &file.render;
        let name = match render.theme.as_deref() {
            Some(AUTO_THEME) if settings.background == Some(Background::Light) => {
                Some(render.theme_light.clone())
            }
            Some(AUTO_THEME) => Some(render.theme_dark.clone()),
            other => other.map(str::to_owned),
        };
        if let Some(name) = name {
            match Theme::from_name(&name) {
                Ok(theme) => {
                    settings.theme = theme;
                    settings.theme_source = Source::File(path.to_path_buf());
                }
                Err(_) => {
                    settings.warnings.push(Warning::UnknownTheme { path: path.to_path_buf(), name })
                }
            }
        }
        settings
    }

    /// True when `render.theme` is `auto`.
    pub(crate) fn auto_theme(&self) -> bool {
        self.render.theme.as_deref() == Some(AUTO_THEME)
    }

    /// Lays `[render.colors]` over the theme file, and reads its code theme.
    async fn load_colours(&mut self, home: &Path) {
        let mut colors = self.render.colors.clone();
        match read_theme_file(&self.render, home).await {
            None => {}
            Some(Err(source)) => self.warnings.push(Warning::Palette { source: Box::new(source) }),
            Some(Ok(file)) => {
                colors = colors.over(&file.colors);
                if let Some(path) = file.code_theme_path(home) {
                    match read_code_theme(&path).await {
                        Ok(theme) => {
                            self.code_theme = Some(theme);
                            self.code_theme_path = Some(path);
                        }
                        Err(warning) => self.warnings.push(warning),
                    }
                }
            }
        }
        self.palette = palette(&colors);
    }
}

#[cfg(test)]
mod tests;
