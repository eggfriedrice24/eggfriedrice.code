use std::path::PathBuf;

use efr_protocol::Size;
use pretty_assertions::assert_eq;

use super::HolderError;

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let cases = [
        (
            HolderError::ProgramNotAbsolute { program: PathBuf::from("zsh") },
            "the program zsh is not an absolute path",
        ),
        (
            HolderError::CwdNotAbsolute { cwd: PathBuf::from("src") },
            "the working directory src is not an absolute path",
        ),
        (
            HolderError::EmptySize { size: Size { cols: 0, rows: 24 } },
            "the terminal size 0x24 has no cells",
        ),
        (
            HolderError::InvalidEnvName { name: "A=B".to_owned() },
            r#""A=B" is not a valid environment variable name"#,
        ),
        (
            HolderError::NulInEnvValue { name: "TOKEN".to_owned() },
            "the value of the environment variable TOKEN contains a NUL byte",
        ),
        (
            HolderError::NulByte { field: "argument" },
            "the argument of the spawn spec contains a NUL byte",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}
