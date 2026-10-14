//! The pacman mirror list (DP4): the `Server =` and `Include =` lines of
//! `/etc/pacman.d/mirrorlist` and `/etc/pacman.conf`.

use std::path::PathBuf;

use super::{MirrorHost, is_ip_literal, is_name};

/// What a pacman file holds: the hosts of its mirrors and the paths of its `Include =`
/// lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacmanFile {
    /// The host of each `Server =` line with an `https://` or `http://` URL, once each,
    /// in the order of the file.
    pub mirrors: Vec<MirrorHost>,
    /// The absolute paths of the `Include =` lines, once each, in the order of the file.
    pub includes: Vec<PathBuf>,
}

impl PacmanFile {
    /// The most files that efrd reads for `@pacman-mirrors`: the mirror list and the
    /// files that `pacman.conf` includes.
    pub const MAX_FILES: usize = 8;
    /// The most lines of one file that [`parse_pacman`] reads.
    pub const MAX_LINES: usize = 1000;
}

/// The `Server =` and `Include =` lines of a pacman mirror list or of `pacman.conf`, in
/// its first [`PacmanFile::MAX_LINES`] lines. A `#` starts a comment, as pacman reads
/// it. A `file://` or `ftp://` server, a server whose host is an IP address or has user
/// info, and a line that does not parse are left out. `$repo` and `$arch` in a path do
/// not matter: only the host and the port count.
pub fn parse_pacman(text: &str) -> PacmanFile {
    let mut file = PacmanFile::default();
    for line in text.lines().take(PacmanFile::MAX_LINES) {
        let line = line.split('#').next().unwrap_or_default();
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "Server" => {
                if let Some(mirror) = mirror(value)
                    && !file.mirrors.contains(&mirror)
                {
                    file.mirrors.push(mirror);
                }
            }
            "Include" => {
                let path = PathBuf::from(value);
                if path.is_absolute() && !file.includes.contains(&path) {
                    file.includes.push(path);
                }
            }
            _ => {}
        }
    }
    file
}

/// The host of a `Server =` URL.
fn mirror(url: &str) -> Option<MirrorHost> {
    let (scheme, rest) = url.split_once("://")?;
    let http = if scheme.eq_ignore_ascii_case("https") {
        false
    } else if scheme.eq_ignore_ascii_case("http") {
        true
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') || !authority.is_ascii() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok().filter(|port| *port != 0)?),
        None => (authority, if http { 80 } else { 443 }),
    };
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    (!is_ip_literal(host) && is_name(host, false)).then(|| MirrorHost {
        host: host.to_owned(),
        port,
        http,
    })
}
