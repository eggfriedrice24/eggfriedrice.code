//! Helpers that more than one module of the test binary needs.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Everything every span and event of this process logs, at every level.
pub(crate) fn logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    Arc::clone(LOGS.get_or_init(|| {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || LogWriter(Arc::clone(&sink)))
            .finish();
        // NOTE: nextest runs each test in a process of its own; under cargo test the
        // second test finds the subscriber already set, which is the same one.
        let _ = tracing::subscriber::set_global_default(subscriber);
        logs
    }))
}

/// What [`logs`] holds so far, as text.
pub(crate) fn logged() -> String {
    String::from_utf8_lossy(&logs().lock().unwrap()).into_owned()
}

/// Every file under `root` that holds `needle`.
pub(crate) fn files_holding(root: &Path, needle: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        // A directory or file that went away while the daemon stopped holds nothing.
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_symlink() {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if let Ok(bytes) = std::fs::read(&path)
                && bytes.windows(needle.len()).any(|window| window == needle)
            {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// True when the tests that drive a real zsh may run (`EFR_TEST_ZSH=1`); otherwise
/// `test` says on stderr that it skips.
#[expect(clippy::print_stderr, reason = "a skipped test says why, as atuin's e2e tests do")]
pub(crate) fn zsh_enabled(test: &str) -> bool {
    let on = efr_stdx::env::flag(efr_stdx::env::Var::TestZsh).unwrap_or(false);
    if !on {
        eprintln!("skipping {test}: set EFR_TEST_ZSH=1 to run the tests that drive a real zsh");
    }
    on
}

/// True while a socket of this process listens on TCP port `port` of the host.
///
/// NOTE: the efrd of a test runs in the test's process, so this sees efrd's own
/// listener. A program that takes the port after efrd let it go does not count, so a
/// test that waits for a closed listener never waits on a port that another program
/// reused.
pub(crate) fn listens_here(port: u16) -> bool {
    let own: std::collections::BTreeSet<String> = std::fs::read_dir("/proc/self/fd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter_map(|target| {
            let target = target.to_string_lossy().into_owned();
            Some(target.strip_prefix("socket:[")?.strip_suffix(']')?.to_owned())
        })
        .collect();
    ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|table| {
        std::fs::read_to_string(table).unwrap_or_default().lines().skip(1).any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let listens = fields.get(3) == Some(&"0A");
            let on_port = fields
                .get(1)
                .and_then(|local| local.rsplit_once(':'))
                .and_then(|(_, hex)| u16::from_str_radix(hex, 16).ok())
                == Some(port);
            listens && on_port && fields.get(9).is_some_and(|inode| own.contains(*inode))
        })
    })
}
