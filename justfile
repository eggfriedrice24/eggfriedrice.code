# The human entry point. Recipes only compose commands; anything that needs Rust
# lives in the xtask. Recipes for crates that do not exist yet report that and exit 0,
# so `just lint` and `just test` work from the first commit.

set shell := ["bash", "-euo", "pipefail", "-c"]

# The leaf gate: crates that never pull a daemon edge, so their features stay light.
leaf_crates := "efr-stdx efr-protocol efr-store efr-credentials efr-permissions efr-scope efr-holder efr-http efr-screen efr-provider efr-test-support efr-screen-vt100"

# The tests of CI's shell job: these four packages, out of the test binaries that
# test-full builds (CI's build job archives them), so the two share one build. They
# build with the features of the whole workspace, not only those the four turn on.
shell_tests := "package(efr-shell) | package(efr-cli) | package(efr-daemon) | package(efr-test-daemon)"

# List the recipes.
default:
    @just --list

# Build the workspace without Zig.
build:
    cargo build

# Release build of efrd and efr with the ghostty screen backend (needs Zig 0.16.0).
build-release:
    cargo build --release -p efr-daemon -p efr-cli --features efr-daemon/screen-ghostty

# Run efrd in the foreground on vt100 screens with throwaway directories; Ctrl+C stops it.
run:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    echo "run: efrd on vt100 screens in $tmp (config, data, state, runtime); Ctrl+C stops it"
    echo "run: point efr at it with EFR_RUNTIME_DIR=$tmp/runtime"
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
    if ! command -v zig >/dev/null; then
        echo "test-ghostty: zig is not on PATH; install the version docs/ghostty-pin.md pins" >&2
        exit 1
    fi
    # The daemon half joins once efr-daemon exists (milestone 1, step 5).
    if [[ -d crates/efr-daemon ]]; then
        cargo nextest run -p efr-screen-ghostty -p efr-daemon --features efr-daemon/screen-ghostty
    else
        cargo nextest run -p efr-screen-ghostty
    fi

# Tests that drive a real zsh; skips when zsh is missing.
test-shell:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v zsh >/dev/null; then
        echo "test-shell: zsh is not installed; skipping"
        exit 0
    fi
    # The workspace builds efr-daemon with its default local-pty, which the shell_ tests
    # over a TestDaemon need for a real zsh.
    EFR_TEST_ZSH=1 cargo nextest run --workspace --exclude efr-screen-ghostty -E '{{ shell_tests }}'

# CI's shell job in an Ubuntu 24.04 container set up like GitHub's runner (needs Docker).
# CI runs the job from the build job's archive; the container builds the same test
# binaries itself.
test-shell-ubuntu:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v docker >/dev/null; then
        echo "test-shell-ubuntu: docker is not installed" >&2
        exit 1
    fi
    toolchain="$(sed -nE 's/^channel = "(.*)"/\1/p' rust-toolchain.toml)"
    nextest="$(sed -nE 's/.*tool: cargo-nextest@([0-9.]+).*/\1/p' .github/workflows/ci.yml | head -n1)"
    image=efr-shell-ubuntu
    echo "test-shell-ubuntu: building $image (Rust $toolchain, cargo-nextest $nextest)"
    docker build -t "$image" --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" \
        --build-arg TOOLCHAIN="$toolchain" --build-arg NEXTEST="$nextest" \
        - < .github/ubuntu-shell.Dockerfile
    # The registry and the target directory stay on the host between runs; the target
    # directory is apart from target/, because the container links against another libc.
    cache="${XDG_CACHE_HOME:-$HOME/.cache}/efr-ci"
    mkdir -p "$cache/registry" "$cache/git" "$cache/target"
    echo "test-shell-ubuntu: running the shell job's tests, cached in $cache"
    docker run --rm --init \
        -v "$PWD:/home/runner/work/efr" \
        -v "$cache/registry:/home/runner/.cargo/registry" \
        -v "$cache/git:/home/runner/.cargo/git" \
        -v "$cache/target:/home/runner/target" \
        -e CARGO_TARGET_DIR=/home/runner/target \
        -e CARGO_TERM_COLOR=always -e INSTA_UPDATE=no -e EFR_TEST_ZSH=1 \
        -w /home/runner/work/efr "$image" \
        cargo nextest run --workspace --exclude efr-screen-ghostty --profile ci -E '{{ shell_tests }}'

# Formatting, spelling, clippy, the docs, cargo-deny, tidy, the dependency rule and every
# feature combination: CI's fmt, lint, deny, tidy and hack jobs.
lint: fmt-check typos clippy doc-check deny tidy deps hack

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

# screen-ghostty is left out: it builds the ghostty crate, which needs Zig, and the
# ghostty job checks it.
# Every feature combination of every crate without Zig, as CI's three hack partitions run it.
hack:
    cargo hack check --feature-powerset --workspace --exclude efr-screen-ghostty --exclude-features screen-ghostty

# Spelling.
typos:
    typos

# Accept snapshot changes, rewrite the frozen protocol fixtures and the scenario fixtures.
bless:
    cargo insta accept
    cargo xtask fixtures --bless
    # The scenario fixtures of efr-test-daemon: outbound records only; review the diff.
    cargo nextest run -p efr-test-daemon --test it --run-ignored only -E 'test(/^bless::/)'

# The docs with warnings denied, as CI's lint job builds them.
doc-check:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --exclude efr-screen-ghostty --no-deps

# Build the docs with warnings denied, then open them.
doc: doc-check
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --exclude efr-screen-ghostty --no-deps --open

# Regenerate docs/protocol.md from efr-protocol.
protocol-docs:
    cargo xtask protocol-docs

# Install efrd and efr to ~/.local/bin and the user unit, reload systemd; never starts it.
install:
    #!/usr/bin/env bash
    set -euo pipefail
    bin="$HOME/.local/bin"
    unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    echo "install: building the release binaries (just build-release)"
    just build-release
    echo "install: copying target/release/efrd to $bin/efrd"
    install -Dm755 target/release/efrd "$bin/efrd"
    echo "install: copying target/release/efr to $bin/efr"
    install -Dm755 target/release/efr "$bin/efr"
    echo "install: installing systemd/efrd.service to $unit_dir/efrd.service"
    install -Dm644 systemd/efrd.service "$unit_dir/efrd.service"
    echo "install: running systemctl --user daemon-reload"
    systemctl --user daemon-reload
    echo "install: done; the unit is neither enabled nor started"
    echo "install: start it with: systemctl --user enable --now efrd"

# Use the repository's git hooks.
install-hooks:
    git config core.hooksPath .githooks
    @echo "git hooks: core.hooksPath set to .githooks"

# Move the libghostty-rs pin to another rev (see docs/ghostty-pin.md).
bump-ghostty rev:
    #!/usr/bin/env bash
    set -euo pipefail
    rev="{{ rev }}"
    if [[ ! "$rev" =~ ^[0-9a-f]{40}$ ]]; then
        echo "bump-ghostty: expected a full 40-character libghostty-rs commit" >&2
        exit 1
    fi
    old="$(sed -nE 's/^libghostty-vt = .*rev = "([0-9a-f]{40})".*/\1/p' Cargo.toml)"
    old_ghostty="$(sed -nE 's/^\| ghostty \| `([0-9a-f]{40})`.*/\1/p' docs/ghostty-pin.md)"
    if [[ -z "$old" || -z "$old_ghostty" ]]; then
        echo "bump-ghostty: the current pin is missing from Cargo.toml or docs/ghostty-pin.md" >&2
        exit 1
    fi
    # Everything is fetched before anything is written, so a failed download changes nothing.
    build_rs="https://raw.githubusercontent.com/uzaaft/libghostty-rs/$rev/crates/libghostty-vt-sys/build.rs"
    ghostty="$(curl -fsSL "$build_rs" | sed -nE 's/.*GHOSTTY_COMMIT[^"]*"([0-9a-f]{40})".*/\1/p' | head -n1)"
    if [[ -z "$ghostty" ]]; then
        echo "bump-ghostty: no GHOSTTY_COMMIT in $build_rs" >&2
        exit 1
    fi
    zon="https://raw.githubusercontent.com/ghostty-org/ghostty/$ghostty/build.zig.zon"
    zig="$(curl -fsSL "$zon" | sed -nE 's/.*minimum_zig_version = "([^"]+)".*/\1/p')"
    # The full commits go first, because the short forms are their prefixes.
    sed -i "s/$old/$rev/g; s/$old_ghostty/$ghostty/g; s/${old_ghostty:0:8}/${ghostty:0:8}/g" \
        Cargo.toml docs/ghostty-pin.md
    # Resolving again moves Cargo.lock to the new rev without building anything.
    cargo fetch --quiet
    echo "libghostty-rs $rev builds ghostty $ghostty, which needs Zig ${zig:-(not found)}"
    echo "local zig: $(zig version 2>/dev/null || echo 'not on PATH')"
    echo "rewritten: the rev and the ghostty commit in Cargo.toml, Cargo.lock and docs/ghostty-pin.md"
    echo "left to do: the Zig version and the dates in docs/ghostty-pin.md, the Zig version in ci.yml, then just test-ghostty"

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
