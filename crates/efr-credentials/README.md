# efr-credentials

## Purpose

Vendor-neutral storage for the credentials efr logs in with. A `CredentialRecord` is
a static API key or a set of OAuth tokens; a `SecretStore` loads, saves and deletes
records by `CredentialId` (`openai-subscription`, `openai-api`, later one per MCP
server).

Modules:

- `id`: `CredentialId`, a name that is safe as a file name and a keyring account:
  1 to 64 bytes of `[a-z0-9._-]`, starting with a letter or a digit.
- `record`: `CredentialRecord` and `OAuthTokens`, and their versioned JSON form. Every
  store writes the same JSON, so a record can move between stores unchanged.
- `store`: the `SecretStore` trait. Its methods block; async callers wrap them in
  `spawn_blocking`.
- `file_store`: `FileStore`, the default. One file per record at
  `$XDG_DATA_HOME/efr/secrets/<id>.json` (or under `EFR_DATA_DIR`), directory 0700,
  files 0600, written atomically through `efr_stdx::fs::write_atomic`.
- `keyring_store` (feature `keyring`): `KeyringStore`, the same JSON in the platform
  keyring (Secret Service on Linux). Whether it can ever be the default on a headless
  systemd service is open question 14 of the structure document.

Consumers: `efr-oauth-openai` persists tokens, `efr-daemon` composes providers from
the stored records, and `efr-mcp` (milestone 2) will keep HTTP server tokens here.

## Tier

Tier 1.

## Allowed dependencies

`efr-stdx` only (the atomic write and the data root). `xtask/src/deps.rs` holds the
allowlist.

Third-party crates: `jiff`, `secrecy` (with `serde`), `serde`, `serde_json`,
`thiserror`, `zeroize`, and `keyring` behind the `keyring` feature.

## Invariant

Secrets never reach a log or a file that another user can read:

- every secret is a `secrecy::SecretString`, whose `Debug` output is redacted, and no
  `CredentialsError` variant carries secret bytes (keyring errors that hold raw
  secret data are rewritten before they are wrapped);
- the serialised record lives in a `zeroize::Zeroizing` buffer;
- the file store writes the directory with mode 0700 and files with mode 0600, and
  refuses (never repairs) a file or directory that other users can reach, or a
  symbolic link in place of either;
- the store knows no vendor: refresh rules, claims and token endpoints belong to the
  login crates.

`cfg(feature = "keyring")` appears once, on the `mod` line in `src/lib.rs`; tidy
enforces that.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-credentials
cargo nextest run -p efr-credentials --features keyring
```

The file store tests run in temporary directories. The keyring tests cover the error
rewriting only; nothing talks to a real keyring. The tests use no network, no
real-time sleeps and no Zig.
