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

# Release build of efrd, efr and the sandbox launcher efr-sbx, with the ghostty screen
# backend (needs Zig 0.16.0). Only these three packages, so efrd never gets the test
# seams that efr-test-daemon turns on.
build-release:
    cargo build --release -p efr-daemon -p efr-cli -p efr-sbx --features efr-daemon/screen-ghostty

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

# The auto sandbox on the real bwrap, Landlock and seccomp: the escape suite, the behaviour
# tests, the probe, the launch cost gate (1000 calls) and the corpus run of efr-sbx.
# Fails when a test skips on a machine whose probe says the sandbox is ready.
test-sandbox:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "test-sandbox: building efr-sbx"
    bin="$(cargo build -p efr-sbx --message-format=json-render-diagnostics \
        | sed -nE 's/.*"executable":"([^"]*\/efr-sbx)".*/\1/p' | tail -n1)"
    if [[ -z "$bin" || ! -x "$bin" ]]; then
        echo "test-sandbox: cargo built no efr-sbx binary" >&2
        exit 1
    fi
    # The probe's fake call lives in the target dir: never below /tmp, which the
    # sandbox replaces with its private tmp.
    probe_dir="$(dirname "$bin")/sbx-probe"
    mkdir -p -m 700 "$probe_dir"
    report="$("$bin" probe --json --dir "$probe_dir")"
    if [[ "$report" == *'"failure":null'* ]]; then
        echo "test-sandbox: the probe says the sandbox is ready here; a skipped test fails"
        require=1
    else
        echo "test-sandbox: the probe says the sandbox is unavailable; the tests skip:"
        echo "$report"
        require=0
    fi
    export EFR_TEST_SBX_BIN="$bin" EFR_TEST_SBX_REQUIRE="$require" EFR_TEST_SBX_GATE=1
    cargo nextest run -p efr-sbx -E 'not test(/^cost::|^corpus::/)'
    # The launch cost and the corpus print their tables; show them.
    cargo nextest run -p efr-sbx --success-output immediate -E 'test(/^cost::|^corpus::/)'
    # efr-shell's behaviour tests through a real zsh and this launcher. They say
    # `skipped:` on stderr when they cannot run, which fails on a ready machine.
    log="$(dirname "$bin")/sbx-shell-tests.log"
    EFR_TEST_ZSH=1 cargo nextest run -p efr-shell --success-output immediate \
        -E 'test(/e2e_zsh::launcher::/)' 2>&1 | tee "$log"
    if [[ "$require" == 1 ]] && grep -q 'skipped:' "$log"; then
        echo "test-sandbox: efr-shell's launcher tests skipped on a ready machine" >&2
        exit 1
    fi
    # efrd's sandbox tests: a real zsh, the daemon's probe and one call through this
    # launcher.
    log="$(dirname "$bin")/sbx-daemon-tests.log"
    EFR_TEST_ZSH=1 cargo nextest run -p efr-daemon --success-output immediate \
        -E 'test(/^sandbox::/)' 2>&1 | tee "$log"
    if [[ "$require" == 1 ]] && grep -q 'skipped:' "$log"; then
        echo "test-sandbox: efr-daemon's sandbox tests skipped on a ready machine" >&2
        exit 1
    fi

# test-sandbox in the Ubuntu 24.04 container of test-shell-ubuntu, which shares this
# machine's kernel (needs Docker and a kernel with Landlock ABI 9). bwrap needs three
# relaxed container defaults: no seccomp profile (user namespaces), no AppArmor profile,
# and a writable /proc/sys (--disable-userns writes max_user_namespaces).
test-sandbox-ubuntu:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v docker >/dev/null; then
        echo "test-sandbox-ubuntu: docker is not installed" >&2
        exit 1
    fi
    toolchain="$(sed -nE 's/^channel = "(.*)"/\1/p' rust-toolchain.toml)"
    nextest="$(sed -nE 's/.*tool: cargo-nextest@([0-9.]+).*/\1/p' .github/workflows/ci.yml | head -n1)"
    image=efr-shell-ubuntu
    echo "test-sandbox-ubuntu: building $image (Rust $toolchain, cargo-nextest $nextest)"
    docker build -t "$image" --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" \
        --build-arg TOOLCHAIN="$toolchain" --build-arg NEXTEST="$nextest" \
        - < .github/ubuntu-shell.Dockerfile
    cache="${XDG_CACHE_HOME:-$HOME/.cache}/efr-ci"
    mkdir -p "$cache/registry" "$cache/git" "$cache/target"
    echo "test-sandbox-ubuntu: running the sandbox tests, cached in $cache"
    docker run --rm --init \
        --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
        --security-opt systempaths=unconfined \
        -v "$PWD:/home/runner/work/efr" \
        -v "$cache/registry:/home/runner/.cargo/registry" \
        -v "$cache/git:/home/runner/.cargo/git" \
        -v "$cache/target:/home/runner/target" \
        -e CARGO_TARGET_DIR=/home/runner/target -e CARGO_TERM_COLOR=always \
        -w /home/runner/work/efr "$image" bash -euo pipefail -c '
            cargo build -p efr-sbx
            bin=/home/runner/target/debug/efr-sbx
            mkdir -p -m 700 /home/runner/target/debug/sbx-probe
            report="$("$bin" probe --json --dir /home/runner/target/debug/sbx-probe)"
            if [[ "$report" == *"\"failure\":null"* ]]; then
                echo "test-sandbox-ubuntu: the probe says ready; a skipped test fails"
                require=1
            else
                echo "test-sandbox-ubuntu: the probe says unavailable: $report"
                require=0
            fi
            export EFR_TEST_SBX_BIN="$bin" EFR_TEST_SBX_REQUIRE="$require"
            # procps of Ubuntu cannot find itself in the procfs of the container from a new
            # pid namespace ("fatal library error, lookup self"), with plain bwrap too.
            cargo nextest run -p efr-sbx -E "not test(ps_sees_host_processes)"'

# The VM boots the kernel of .github/sandbox-vm.sh with virtme-ng. The container is set
# up like GitHub's runner: it builds the archive that CI's build job makes, then runs
# the job's own step.
# CI's sandbox job: the tests of test-sandbox in a VM (needs Docker and /dev/kvm).
test-sandbox-vm:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v docker >/dev/null; then
        echo "test-sandbox-vm: docker is not installed" >&2
        exit 1
    fi
    if [[ ! -c /dev/kvm ]]; then
        echo "test-sandbox-vm: /dev/kvm does not exist; the VM needs KVM" >&2
        exit 1
    fi
    toolchain="$(sed -nE 's/^channel = "(.*)"/\1/p' rust-toolchain.toml)"
    nextest="$(sed -nE 's/.*tool: cargo-nextest@([0-9.]+).*/\1/p' .github/workflows/ci.yml | head -n1)"
    echo "test-sandbox-vm: building efr-shell-ubuntu (Rust $toolchain, cargo-nextest $nextest)"
    docker build -t efr-shell-ubuntu --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" \
        --build-arg TOOLCHAIN="$toolchain" --build-arg NEXTEST="$nextest" \
        - < .github/ubuntu-shell.Dockerfile
    echo "test-sandbox-vm: building efr-sandbox-vm (QEMU, virtme-ng, the kernel)"
    docker build -t efr-sandbox-vm --build-arg BASE=efr-shell-ubuntu \
        -f .github/sandbox-vm.Dockerfile .github
    cache="${XDG_CACHE_HOME:-$HOME/.cache}/efr-ci"
    mkdir -p "$cache/registry" "$cache/git" "$cache/target"
    echo "test-sandbox-vm: running the sandbox job's step, cached in $cache"
    docker run --rm --init --device /dev/kvm --group-add "$(stat -c %g /dev/kvm)" \
        -v "$PWD:/home/runner/work/efr" \
        -v "$cache/registry:/home/runner/.cargo/registry" \
        -v "$cache/git:/home/runner/.cargo/git" \
        -v "$cache/target:/home/runner/target" \
        -e CARGO_TARGET_DIR=/home/runner/target -e CARGO_TERM_COLOR=always \
        -w /home/runner/work/efr efr-sandbox-vm bash -euo pipefail -c '
            out=/home/runner/target/sandbox-vm
            mkdir -p "$out"
            cargo nextest archive --workspace --exclude efr-screen-ghostty \
                --archive-file "$out/nextest-archive.tar.zst"
            .github/sandbox-vm.sh run /opt/efr-kernel "$out/nextest-archive.tar.zst" . "$out"'

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

# Install efrd and efr to ~/.local/bin, the sandbox launcher to ~/.local/lib/efr and the
# user unit, reload systemd; never starts it.
install:
    #!/usr/bin/env bash
    set -euo pipefail
    bin="$HOME/.local/bin"
    lib="$HOME/.local/lib/efr"
    unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    echo "install: building the release binaries (just build-release)"
    just build-release
    # A build with the sandbox's test seams lets a test replace the launcher and the
    # probe; it must never be installed.
    if [[ "$(target/release/efrd --test-seams)" != "off" ]]; then
        echo "install: target/release/efrd has the test-sandbox-fake feature; refusing" >&2
        exit 1
    fi
    echo "install: copying target/release/efrd to $bin/efrd"
    install -Dm755 target/release/efrd "$bin/efrd"
    echo "install: copying target/release/efr to $bin/efr"
    install -Dm755 target/release/efr "$bin/efr"
    # efrd finds the launcher in ../lib/efr/ next to its own directory, never on PATH.
    echo "install: copying target/release/efr-sbx to $lib/efr-sbx"
    install -Dm755 target/release/efr-sbx "$lib/efr-sbx"
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
    # The source package downloads the ghostty archive of the commit; its sum is the
    # second of b2sums in the PKGBUILD.
    pkgbuild=packaging/efr-code/PKGBUILD
    old_b2="$(sed -n '/^b2sums=/,/)/p' "$pkgbuild" | sed -n 2p | tr -d " '()")"
    archive="https://github.com/ghostty-org/ghostty/archive/$ghostty.tar.gz"
    new_b2="$(curl -fsSL "$archive" | b2sum | cut -d ' ' -f 1)"
    if [[ ! "$old_b2" =~ ^[0-9a-f]{128}$ || ! "$new_b2" =~ ^[0-9a-f]{128}$ ]]; then
        echo "bump-ghostty: no ghostty b2sum in $pkgbuild, or no archive at $archive" >&2
        exit 1
    fi
    # The full commits go first, because the short forms are their prefixes.
    sed -i "s/$old/$rev/g; s/$old_ghostty/$ghostty/g; s/${old_ghostty:0:8}/${ghostty:0:8}/g" \
        Cargo.toml docs/ghostty-pin.md "$pkgbuild"
    sed -i "s/$old_b2/$new_b2/" "$pkgbuild"
    # Resolving again moves Cargo.lock to the new rev without building anything.
    cargo fetch --quiet
    echo "libghostty-rs $rev builds ghostty $ghostty, which needs Zig ${zig:-(not found)}"
    echo "local zig: $(zig version 2>/dev/null || echo 'not on PATH')"
    echo "rewritten: the rev and the ghostty commit in Cargo.toml, Cargo.lock, docs/ghostty-pin.md and $pkgbuild, and the b2sum of the ghostty archive in $pkgbuild"
    echo "left to do: the Zig version and the dates in docs/ghostty-pin.md, the Zig version in ci.yml, release.yml and $pkgbuild (with its b2sum), then just test-ghostty"

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
