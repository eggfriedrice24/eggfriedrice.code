# 0014: Fold four small crates back

Status: accepted, 2026-10-20.

## Context

ADR 0001 gives the rule for a crate: "A module becomes a crate when it gets heavy
dependencies of its own or a second binary needs it, and folds back when neither
holds." The workspace had 32 crates. Each crate costs a manifest, a README, an
allowlist entry, a row in the crate map and a place in the CI lists. Four crates did
not meet the rule:

- `efr-screen-vt100`: about 230 lines of product code. Its only dependency was the
  small, pure-Rust `vt100` crate. Only `efr-daemon` used it, and `efr-shell` in its
  tests.
- `efr-patch`: about 1000 lines and no dependency except `thiserror`. Only `efr-tools`
  used it.
- `efr-provider-openai` and `efr-provider-anthropic`: two crates with the same
  allowlist and almost the same third-party crates. Only `efr-daemon` used them.
- `efr-client`: its dependencies were a subset of those of `efr-transport`, and its
  tests already used `efr-transport` to build their fake daemons.

## Decision

1. `efr-screen-vt100` becomes the module `efr_screen::vt100`. It has no cargo
   feature, because every user of `efr-screen` (`efr-shell`, `efr-daemon`) needs the
   vt100 backend. Its conformance test is now `tests/it/conformance.rs` of
   `efr-screen`, and its snapshots stay in `crates/efr-screen/fixtures/vt/snapshots/`.
2. `efr-patch` becomes the public module `efr_tools::patch`. The engine stays pure. The
   forbidden edge `efr-patch -> tokio` cannot guard a module, so `cargo xtask tidy`
   does: the files of the module name no `tokio`, no `async`, no IO of std, no
   workspace crate and no `crate::` path outside the module (`PURE_MODULES` in
   `xtask/src/tidy.rs`). The module keeps `missing_docs` of its old tier 0.
3. `efr-provider-openai` and `efr-provider-anthropic` become one crate,
   `efr-providers`, with the public modules `openai` and `anthropic`. The modules share
   no code. The forbidden edge becomes `efr-providers -> efr-oauth-openai`, so no
   provider sees a refresh token. CI checks it again with `cargo tree`.
4. `efr-client` becomes the public module `efr_transport::client`. It has no cargo
   feature: the server side adds only `nix` and `tracing`, and `efr` already links
   `tracing`. The linker drops the server code that `efr` does not call.

A module that was a crate keeps its own error enum (`PatchError`, `OpenAiError`,
`AnthropicError`, `ClientError`) in its own `error.rs`. CONVENTIONS.md lists them.

## The small crates that stay

Each of these crates holds one of the two conditions of ADR 0001, or it protects a
boundary that a crate edge checks:

- `efr-screen-ghostty`: its build needs Zig. A bare build never compiles it.
- `efr-pty`: the only `unsafe` PTY code (`openpty`, `setsid` and `TIOCSCTTY` in
  `pre_exec`), kept apart so that the allowlist of unsafe files stays short.
- `efr-holder`: the `PtyHolder` boundary. `efr-ptyd`, a second binary, needs it at
  milestone 5.
- `efr-credentials`: its optional `keyring` feature brings zbus and the Secret Service
  stack, a heavy dependency of its own.
- `efr-oauth-openai`: the only crate that sees a refresh token. The forbidden edge
  from `efr-providers` keeps it so.
- `efr-proxy`: it parses untrusted bytes from the sandbox (request heads and
  ClientHello). The edge `efr-proxy -> efr-daemon` is forbidden, so the parser knows
  nothing of the daemon.
- `efr-sbx` and `efr-sandbox`: `efr-sbx` is a binary of its own, and both must stay
  free of tokio. Two crate edges check that.
- `efr-test-support` and `efr-test-daemon`: test crates. The dependency rule keeps
  them out of the shipped binaries.
- `efr-protocol` and `efr-stdx`: every binary needs them, and `efr-protocol` must stay
  free of a runtime so that any client can compile it.

## Consequences

- The workspace has 28 crates instead of 32.
- No behaviour changes. The binaries, the config and the wire protocol stay the same.
  Only the tracing targets of the moved code change, for example
  `efr_providers::openai` for the old `efr_provider_openai`. efrd prints the target of
  each log line, so `CHANGELOG.md` tells the user. No default filter and no document
  names an old target.
- The leaf gate (`leaf_crates` in the justfile, the CI job `test-leaf` and the
  `check()` of the AUR package) no longer runs the patch engine's tests and the vt100
  tests alone. The vt100 tests run with `efr-screen`, which is a leaf crate. The patch
  engine's tests run with `efr-tools` in the full gate.
- `efr` now links `nix` through `efr-transport`.
- A later fold or split follows the same rule. The deps file is the only place that
  changes for an edge, and tidy is the place for a pure module.
