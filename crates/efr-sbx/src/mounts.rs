//! `efr-sbx mounts`: the `df` and `findmnt --df` tables of the host's mounts inside a
//! call.
//!
//! efrd wrote the snapshot of a call that runs `df` or `findmnt` (`mounts` and
//! `mounts.json`), and the plan binds both next to the launcher, in the inside dir.
//! The child shell's `df` and `findmnt` functions run these subcommands. Exit status 2
//! with no output means "run the real command": an operand, an option that the tables
//! do not follow, `--help`, a variable that changes `df`'s units, or a snapshot that is
//! missing or does not read.

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use efr_sandbox::{
    MAX_MOUNTS_BYTES, MOUNTS_FILE, MountEntry, MountSnapshot, SIZES_FILE, parse_mountinfo,
    render_df, render_findmnt_df,
};

use crate::MountsTable;

/// The variables that change `df`'s units or format; with one of them set, the real
/// `df` answers.
const DF_VARIABLES: &[&str] = &["DF_BLOCK_SIZE", "BLOCK_SIZE", "BLOCKSIZE", "POSIXLY_CORRECT"];

/// The exit status that tells the child shell to run the real command.
const RUN_REAL: u8 = 2;

/// Prints the table of `table`, or exits 2 with no output.
pub(crate) fn main(table: MountsTable) -> ExitCode {
    let Some(dir) =
        std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return ExitCode::from(RUN_REAL);
    };
    let text = match table {
        MountsTable::Df { args } => {
            if DF_VARIABLES.iter().any(|name| crate::os::var(name).is_some()) {
                return ExitCode::from(RUN_REAL);
            }
            snapshot(&dir).and_then(|(entries, sizes)| render_df(&entries, &sizes, &args))
        }
        MountsTable::FindmntDf { args } => findmnt_inodes(&args).and_then(|inodes| {
            snapshot(&dir).map(|(entries, sizes)| render_findmnt_df(&entries, &sizes, inodes))
        }),
    };
    let Some(text) = text else { return ExitCode::from(RUN_REAL) };
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// True for `--dfi`, false for `--df`: the arguments may hold only `-D`, `--df`, `-I`
/// and `--dfi`. `None` for anything else.
pub(crate) fn findmnt_inodes(args: &[OsString]) -> Option<bool> {
    let mut inodes = false;
    for arg in args {
        match arg.to_str()? {
            "-D" | "--df" => {}
            "-I" | "--dfi" => inodes = true,
            _ => return None,
        }
    }
    Some(inodes)
}

/// The mount table and the sizes in `dir`, or `None` when either is missing or does
/// not read.
fn snapshot(dir: &Path) -> Option<(Vec<MountEntry>, MountSnapshot)> {
    let table = read(&dir.join(MOUNTS_FILE))?;
    let sizes = MountSnapshot::from_json(&read(&dir.join(SIZES_FILE))?).ok()?;
    Some((parse_mountinfo(&table), sizes))
}

fn read(path: &PathBuf) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_MOUNTS_BYTES + 1).ok()?).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= MAX_MOUNTS_BYTES).then_some(bytes)
}

#[cfg(test)]
mod tests;
