//! ssh control sockets (v0.0.5, OQS17 of the v0.0.4 shell items).
//!
//! An ssh control socket is a logged-in session: a process that connects to it runs
//! commands on the remote host with no key. The engine treats one like a credential
//! agent's socket: only the user approves a socket grant of it. efrd gives the engine
//! two facts that it cannot read itself:
//!
//! - [`control_paths`]: the `ControlPath` patterns of the user's ssh config
//!   (`~/.ssh/config`, `/etc/ssh/ssh_config` and the files of their `Include` lines),
//!   read at start. Every `ControlPath` counts, whatever `Host` or `Match` block holds
//!   it, so a pattern can only add questions.
//! - [`listened`]: which sockets of a call's `needs.sockets` an ssh process listens on,
//!   from `/proc`: the socket inodes of each process whose name is `ssh`, and the
//!   listening sockets of `/proc/<pid>/net/unix` with those inodes. It never connects to
//!   a socket. When efrd cannot read `/proc`, every socket of the call counts as an ssh
//!   socket, so a lost fact asks the user.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The most bytes of one ssh config file that efrd reads.
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// The most config files that efrd reads, with the files of `Include` lines.
const MAX_CONFIG_FILES: usize = 64;

/// The process names of the programs that listen on an ssh control socket. autossh and
/// sshfs run `ssh`, which listens itself.
const SSH_NAMES: &[&str] = &["ssh"];

/// The flag of a listening socket in `/proc/net/unix` (`__SO_ACCEPTCON`).
const ACCEPTING: u32 = 0x0001_0000;

/// The `ControlPath` patterns of the ssh config of the user whose home is `home`: the
/// user's `~/.ssh/config`, then the system's `/etc/ssh/ssh_config`, each with the files
/// of its `Include` lines.
pub(crate) fn control_paths(home: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut read = 0;
    let user = home.join(".ssh");
    for (file, base) in
        [(user.join("config"), user), (PathBuf::from("/etc/ssh/ssh_config"), "/etc/ssh".into())]
    {
        read_config(&file, &base, home, &mut found, &mut read);
    }
    found
}

/// Reads the ssh config `file` into `found`, then each file of its `Include` lines;
/// a relative `Include` is below `base`. `read` counts the files so far.
fn read_config(file: &Path, base: &Path, home: &Path, found: &mut Vec<String>, read: &mut usize) {
    if *read >= MAX_CONFIG_FILES {
        return;
    }
    *read += 1;
    let Some(text) = small_text(file) else { return };
    let (paths, includes) = parse_config(&text);
    for path in paths {
        if !found.contains(&path) {
            found.push(path);
        }
    }
    for include in includes {
        for file in include_files(&include, base, home) {
            read_config(&file, base, home, found, read);
        }
    }
}

/// The text of `file` when it is a small regular file.
fn small_text(file: &Path) -> Option<String> {
    let meta = fs::metadata(file).ok()?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return None;
    }
    fs::read_to_string(file).ok()
}

/// The `ControlPath` values and the `Include` words of the ssh config `text`. A key is
/// one word in any case, then spaces or an `=`; a value may be in double quotes, and a
/// `#` starts a comment line.
pub(crate) fn parse_config(text: &str) -> (Vec<String>, Vec<String>) {
    let mut paths = Vec::new();
    let mut includes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(at) = line.find(|c: char| c.is_whitespace() || c == '=') else { continue };
        let (key, rest) = line.split_at(at);
        let value = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim();
        if key.eq_ignore_ascii_case("controlpath") {
            let value = value.trim_matches('"');
            if !value.is_empty() {
                paths.push(value.to_owned());
            }
        } else if key.eq_ignore_ascii_case("include") {
            includes.extend(value.split_whitespace().map(|word| word.trim_matches('"').to_owned()));
        }
    }
    (paths, includes)
}

/// The files of the `Include` word `word`: `~/` is the home dir, a relative word is
/// below `base`, and a `*` in the last part matches any text of a name in its dir.
fn include_files(word: &str, base: &Path, home: &Path) -> Vec<PathBuf> {
    let path = match word.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if word.starts_with('/') => PathBuf::from(word),
        None => base.join(word),
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else { return Vec::new() };
    if !name.contains('*') {
        return vec![path];
    }
    let Some(dir) = path.parent() else { return Vec::new() };
    if dir.to_string_lossy().contains('*') {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().is_some_and(|found| star_match(name, found)))
        .map(|entry| entry.path())
        .collect();
    files.sort();
    files
}

/// True when `name` matches `pattern`, whose each `*` matches any text.
pub(crate) fn star_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let Some((first, rest)) = parts.split_first() else { return false };
    let Some(mut tail) = name.strip_prefix(first) else { return false };
    let Some((last, middle)) = rest.split_last() else { return tail.is_empty() };
    for part in middle {
        match tail.find(part) {
            Some(at) => tail = &tail[at + part.len()..],
            None => return false,
        }
    }
    tail.ends_with(last)
}

/// The sockets of `sockets` that an ssh process listens on. Every one of them when
/// `/proc` cannot be read. It blocks for the reads of `/proc`.
pub(crate) fn listened(sockets: &[PathBuf]) -> Vec<PathBuf> {
    if sockets.is_empty() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir("/proc") else { return sockets.to_vec() };
    let mut paths: HashSet<PathBuf> = HashSet::new();
    for entry in entries.filter_map(Result::ok) {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(comm) = fs::read_to_string(format!("/proc/{pid}/comm")) else { continue };
        if !SSH_NAMES.contains(&comm.trim()) {
            continue;
        }
        let inodes = socket_inodes(pid);
        if inodes.is_empty() {
            continue;
        }
        let Ok(table) = fs::read_to_string(format!("/proc/{pid}/net/unix")) else { continue };
        paths.extend(listening(&table, &inodes));
    }
    sockets.iter().filter(|socket| paths.contains(*socket)).cloned().collect()
}

/// The inodes of the sockets that the process `pid` holds open.
fn socket_inodes(pid: u32) -> HashSet<u64> {
    let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else { return HashSet::new() };
    fds.filter_map(Result::ok)
        .filter_map(|fd| fs::read_link(fd.path()).ok())
        .filter_map(|link| {
            let link = link.to_str()?;
            link.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
        })
        .collect()
}

/// The paths of the listening sockets of the `/proc/net/unix` text `table` whose inode
/// is in `inodes`. A line is `Num RefCount Protocol Flags Type St Inode Path`, with
/// `Flags` in hex; a socket with no path or an abstract name (`@`) has no place here.
pub(crate) fn listening(table: &str, inodes: &HashSet<u64>) -> Vec<PathBuf> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let flags = u32::from_str_radix(fields.nth(3)?, 16).ok()?;
            let inode: u64 = fields.nth(2)?.parse().ok()?;
            let path = fields.next()?;
            (flags & ACCEPTING != 0 && inodes.contains(&inode) && path.starts_with('/'))
                .then(|| PathBuf::from(path))
        })
        .collect()
}

#[cfg(test)]
mod tests;
