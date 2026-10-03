# The human entry point. Recipes only compose commands; anything that needs Rust
# lives in the xtask. Recipes for crates that do not exist yet report that and exit 0,
# so `just lint` and `just test` work from the first commit.

set shell := ["bash", "-euo", "pipefail", "-c"]

# The leaf gate: crates that never pull a daemon edge, so their features stay light.
leaf_crates := "efr-stdx efr-protocol efr-store efr-credentials efr-permissions efr-scope efr-holder efr-http efr-screen efr-provider efr-screen-vt100"

# List the recipes.
default:
    @just --list

# Build the workspace without Zig.
build:
    cargo build

# Release build of efrd and efr with the ghostty screen backend (needs Zig 0.16.0).
build-release:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d crates/efr-daemon || ! -d crates/efr-cli ]]; then
        echo "build-release: not available until milestone 1 lands (efr-daemon and efr-cli do not exist yet)"
        exit 0
    fi
    cargo build --release -p efr-daemon -p efr-cli --features efr-daemon/screen-ghostty

# Run efrd in the foreground on vt100 screens with throwaway directories.
run:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d crates/efr-daemon ]]; then
        echo "run: not available until milestone 1 lands (efr-daemon does not exist yet)"
        exit 0
    fi
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    EFR_SCREEN=vt100 EFR_LOG=debug \
        EFR_CONFIG_DIR="$tmp/config" EFR_DATA_DIR="$tmp/data" \
        EFR_STATE_DIR="$tmp/state" EFR_RUNTIME_DIR="$tmp/runtime" \
        cargo run -p efr-daemon --bin efrd

# The leaf gate, then the full gate, as in CI.
test: test-leaf test-full

# The fast gate over the leaf crates that exist.
test-leaf:
    #!/usr/bin/env bash
    set -euo pipefail
    args=()
    for crate in {{ leaf_crates }}; do
        if [[ -d "crates/$crate" ]]; then args+=(-p "$crate"); fi
    done
    if [[ ${#args[@]} -eq 0 ]]; then
        echo "test-leaf: no leaf crates yet (milestone 1, step 1 adds them); nothing to run"
        exit 0
    fi
    cargo nextest run "${args[@]}" --lib --tests

# The full gate: every member except the ghostty screen.
test-full:
    cargo nextest run --workspace --exclude efr-screen-ghostty --profile ci

# The ghostty conformance suite and daemon tests on the real backend (needs Zig 0.16.0).
test-ghostty:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d crates/efr-screen-ghostty || ! -d crates/efr-daemon ]]; then
        echo "test-ghostty: not available until milestone 1 lands (efr-screen-ghostty does not exist yet)"
        exit 0
    fi
    cargo nextest run -p efr-screen-ghostty -p efr-daemon --features efr-daemon/screen-ghostty

# Tests that drive a real zsh; skips when zsh is missing.
test-shell:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d crates/efr-shell || ! -d crates/efr-test-daemon ]]; then
        echo "test-shell: not available until milestone 1 lands (efr-shell does not exist yet)"
        exit 0
    fi
    if ! command -v zsh >/dev/null; then
        echo "test-shell: zsh is not installed; skipping"
        exit 0
    fi
    EFR_TEST_ZSH=1 cargo nextest run -p efr-shell -p efr-test-daemon

# Formatting, clippy, cargo-deny, tidy and the dependency rule.
lint: fmt-check clippy deny tidy deps

# The pre-push gate: lint plus the leaf tests.
check: lint test-leaf

# Format the workspace.
fmt:
    cargo fmt --all

# Apply clippy's suggestions.
fix:
    cargo clippy --fix --workspace --exclude efr-screen-ghostty --all-targets --allow-dirty

# Fail on unformatted code.
fmt-check:
    cargo fmt --all --check

# Clippy with warnings denied, without the ghostty screen.
clippy:
    cargo clippy --workspace --exclude efr-screen-ghostty --all-targets -- -D warnings

# Licences, bans, advisories and sources.
deny:
    cargo deny check

# The file rules of CONVENTIONS.md.
tidy:
    cargo xtask tidy

# The dependency rule of ARCHITECTURE.md.
deps:
    cargo xtask deps

# Spelling.
typos:
    typos

# Accept snapshot changes and rewrite the frozen protocol fixtures.
bless:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d crates/efr-protocol ]]; then
        echo "bless: not available until milestone 1 lands (no snapshots or fixtures exist yet)"
        exit 0
    fi
    cargo insta accept
    cargo xtask fixtures --bless

# Build the docs with warnings denied, then open them.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --exclude efr-screen-ghostty --no-deps --open

# Regenerate docs/protocol.md from efr-protocol.
protocol-docs:
    cargo xtask protocol-docs

# Install efrd and efr to ~/.local/bin and the user unit, then reload systemd.
install:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d crates/efr-daemon || ! -d crates/efr-cli ]]; then
        echo "install: not available until milestone 1 lands (efr-daemon and efr-cli do not exist yet)"
        exit 0
    fi
    just build-release
    install -Dm755 target/release/efrd "$HOME/.local/bin/efrd"
    install -Dm755 target/release/efr "$HOME/.local/bin/efr"
    install -Dm644 systemd/efrd.service "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/efrd.service"
    systemctl --user daemon-reload
    echo "installed; start with: systemctl --user enable --now efrd"

# Use the repository's git hooks.
install-hooks:
    git config core.hooksPath .githooks
    @echo "git hooks: core.hooksPath set to .githooks"

# Move the libghostty-rs pin to another rev (see docs/ghostty-pin.md).
bump-ghostty rev:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -d crates/efr-screen-ghostty ]]; then
        echo "bump-ghostty: not available until milestone 1 lands (efr-screen-ghostty does not exist, so a new pin cannot be verified)"
        exit 0
    fi
    old="$(sed -nE 's/^libghostty-vt = .*rev = "([0-9a-f]{40})".*/\1/p' Cargo.toml)"
    sed -i "s/$old/{{ rev }}/g" Cargo.toml docs/ghostty-pin.md
    build_rs="https://raw.githubusercontent.com/uzaaft/libghostty-rs/{{ rev }}/crates/libghostty-vt-sys/build.rs"
    ghostty="$(curl -fsSL "$build_rs" | sed -nE 's/.*GHOSTTY_COMMIT[^"]*"([0-9a-f]{40})".*/\1/p' | head -n1)"
    zon="https://raw.githubusercontent.com/ghostty-org/ghostty/$ghostty/build.zig.zon"
    zig="$(curl -fsSL "$zon" | sed -nE 's/.*minimum_zig_version = "([^"]+)".*/\1/p')"
    echo "libghostty-rs {{ rev }} builds ghostty $ghostty, which needs Zig $zig"
    echo "update the ghostty commit and the Zig version in docs/ghostty-pin.md and ci.yml, then run just test-ghostty"

# Move the pinned toolchain and the MSRV together.
bump-toolchain version:
    #!/usr/bin/env bash
    set -euo pipefail
    version="{{ version }}"
    if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "bump-toolchain: expected a full version such as 1.100.0" >&2
        exit 1
    fi
    sed -i -E "s/^channel = \".*\"/channel = \"$version\"/" rust-toolchain.toml
    sed -i -E "s/^rust-version = \".*\"/rust-version = \"${version%.*}\"/" Cargo.toml
    grep -n '^channel' rust-toolchain.toml
    grep -n '^rust-version' Cargo.toml
