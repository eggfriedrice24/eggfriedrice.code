# Agent instructions

Rules for every coding agent that works in this repo. They add to the human docs and do
not repeat them.

## Read first

- `ARCHITECTURE.md` and `CONVENTIONS.md` are binding. Read them before you change code.
- Read the `README.md` of each crate that you change.

## Gates

Run the gates for what you change before you commit. Before a push, all of these must
pass. Local gates must equal CI.

- `just check` (lint, then the leaf tests)
- `just test`
- `just test-shell-ubuntu` (the CI shell job in the Ubuntu container; needs Docker)
- `just test-sandbox` (the sandbox suite on real bwrap; it fails when the tests skip on
  a ready machine)
- `just test-sandbox-vm` (the CI sandbox job: the same suite in a VM with a 7.1 kernel;
  needs Docker and `/dev/kvm`)
- `just test-ghostty` when `zig` is on `PATH`

## Safety

- Never reach the user's real daemon. `~/.local/bin/efr` is the real, billed client.
  Run a built `efr`, `efrd` or `efr-sbx` only with `EFR_HOME`, or all of
  `EFR_RUNTIME_DIR`, `EFR_DATA_DIR`, `EFR_CONFIG_DIR` and `EFR_STATE_DIR`, set to fresh
  temp dirs in the same command.
- Never read or write the user's real efr dirs (`~/.config/efr`, `~/.local/share/efr`,
  `~/.local/state/efr`, `$XDG_RUNTIME_DIR/efr`, `~/.local/lib/efr`).
- Never touch systemd units, never run `just install`, never restart `efrd`.
- Never put a cargo target dir under `/tmp`. It is a RAM-backed tmpfs.
- Edit `deny.toml` only with the user's approval.

## Commits

- One lowercase imperative line, with no body and no trailers of any kind.
- Never amend. Never use `--no-verify`.
- Push only with the user's approval. After each push, watch CI to the end
  (`gh run watch`) and report the result.

## Changelog

Add each change that a user can notice to the `[Unreleased]` section of
`CHANGELOG.md`, in the same commit or the next one. `docs/releasing.md` tells how.

## Writing

Docs and user-facing text follow ASD-STE100: short sentences, active voice, simple
words.
