//! The arguments of `efr-sbx mounts`: raw, so `df`'s options never reach clap.

use std::ffi::OsString;

use clap::Parser as _;

use super::*;
use crate::{Cli, Command};

fn parsed(args: &[&str]) -> Command {
    let argv = ["efr-sbx"].iter().chain(args).copied();
    Cli::try_parse_from(argv).unwrap_or_else(|error| panic!("{args:?}: {error}")).command
}

fn words(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[test]
fn mounts_df_dash_h_is_df_h() {
    for args in [&["-h"][..], &["--help"], &["-V"], &["--version"], &["-hT", "--total"], &[]] {
        let mut argv = vec!["mounts", "df"];
        argv.extend_from_slice(args);
        match parsed(&argv) {
            Command::Mounts { table: MountsTable::Df { args: got } } => {
                assert_eq!(got, words(args), "clap took one of {args:?}");
            }
            other => panic!("{args:?}: {other:?}"),
        }
    }
    match parsed(&["mounts", "findmnt-df", "-D", "-I"]) {
        Command::Mounts { table: MountsTable::FindmntDf { args } } => {
            assert_eq!(args, words(&["-D", "-I"]));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn findmnt_df_takes_only_its_four_options() {
    assert_eq!(findmnt_inodes(&words(&[])), Some(false));
    assert_eq!(findmnt_inodes(&words(&["-D"])), Some(false));
    assert_eq!(findmnt_inodes(&words(&["--df", "--dfi"])), Some(true));
    assert_eq!(findmnt_inodes(&words(&["-I"])), Some(true));
    for other in ["-DT", "-o", "--help", "/", "-a"] {
        assert_eq!(findmnt_inodes(&words(&["-D", other])), None, "{other}");
    }
}
