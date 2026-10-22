#!/usr/bin/env bash
# The tests of `just test-sandbox` (real bwrap, Landlock ABI 9 and seccomp) in a VM.
# GitHub's runner kernel is older than 7.1, and the runner's AppArmor blocks
# unprivileged user namespaces, so the suite cannot run on the runner itself. This
# script boots the kernel that it pins with virtme-ng in QEMU and KVM, and runs the
# tests of a nextest archive inside. The guest sees the host's root file system
# read-only through virtiofs. CI's sandbox job and `just test-sandbox-vm` both run it,
# so the pins below are the only copy.
#
#   sandbox-vm.sh cache-keys      the keys of the download cache, for $GITHUB_OUTPUT
#   sandbox-vm.sh tools           QEMU, virtiofsd, virtme-ng and zsh (root or sudo)
#   sandbox-vm.sh bwrap           Ubuntu's bubblewrap in /usr (root or sudo)
#   sandbox-vm.sh kernel DIR      the pinned kernel, checked and unpacked into DIR
#   sandbox-vm.sh run KERNEL ARCHIVE WORKSPACE EXTRACT
#                                 unpack the archive into EXTRACT, boot, run the tests
#   sandbox-vm.sh guest ...       inside the VM only: called by `run`
#   sandbox-vm.sh tests ...       inside the VM only: called by `guest`
#
# The download cache: when SBX_VM_CACHE names a dir, `tools`, `bwrap` and `kernel` keep
# what they download below it, and take it from there when it is there. CI's sandbox job
# keeps that dir with actions/cache, so a slow Ubuntu mirror slows down a run that hits
# the cache only by its package lists. `downloads/` holds the files of the pins below,
# each checked by its SHA-256 on every use. `apt/<set>/` holds the .deb files of one set
# of packages, with all the dependencies that the machine did not have. A hit gets the
# package lists first (apt-get update), checks each file by the SHA-256 that the signed
# lists give for its package and version, and installs the packages by name from those
# files with no download. So a cached file that does not match the lists never installs,
# as a download that apt checks. Without SBX_VM_CACHE, as in the image of
# `just test-sandbox-vm`, the same steps run with temporary dirs, so a cache hit, a miss
# and a local run install the same way.
set -euo pipefail
shopt -s nullglob

# A mainline build of kernel.ubuntu.com: vanilla sources with Ubuntu's config, which
# has Landlock first in CONFIG_LSM and lacks Ubuntu's AppArmor restriction of user
# namespaces. 7.1 is the first kernel with Landlock ABI 9, so CI tests the oldest
# kernel that `auto` accepts; the development machine tests a newer one.
kernel_release=7.1.13-070113-generic
kernel_url=https://kernel.ubuntu.com/mainline/v7.1.13/amd64
# The SHA-256 sums of the release's CHECKSUMS file.
kernel_debs=(
    "9fbf0b2b9331dfe2d4f1226b711b863fc2ea5e1a77ce2a1bfa277827f6fb9a46 linux-image-unsigned-7.1.13-070113-generic_7.1.13-070113.202609032040_amd64.deb"
    "1737e4686fa8b72421cdf095ee3d26a537b7e334f77d2d36bba3bc653e947cfa linux-modules-7.1.13-070113-generic_7.1.13-070113.202609032040_amd64.deb"
)

virtme_ng=1.41
virtme_ng_sha256=2031f0f2c67947829e030d66b28950adcef351a17a8455483b57d27b6246f2d2
virtme_ng_dir=/opt/virtme-ng-$virtme_ng

# efr's bus filter needs xdg-dbus-proxy 0.1.9 or later: 0.1.5, Ubuntu 24.04's package,
# cannot serve sd-bus clients such as hostnamectl (that came in 0.1.6), and versions
# before 0.1.9 let some messages past the filter. So `tools` builds the release tarball,
# by its pinned hash, into /usr/bin, where root owns it as efrd requires.
dbus_proxy=0.1.9
dbus_proxy_sha256=5450dda586ec3bb3ca709d311e845487883faa3b09cf562608d7e84f4311dced

self="$(realpath "${BASH_SOURCE[0]}")"

say() { echo "sandbox-vm: $*"; }
fail() {
    echo "sandbox-vm: $*" >&2
    exit 1
}
as_root() {
    if [[ "$(id -u)" == 0 ]]; then "$@"; else sudo "$@"; fi
}

cache="${SBX_VM_CACHE:-}"

# The packages of `tools` and of `bwrap`. Their comments are at those commands.
tools_packages=(
    busybox-static ca-certificates curl dbus-daemon file gcc gir1.2-glib-2.0 iproute2 kmod
    libglib2.0-dev meson ninja-build pkg-config python3-argcomplete python3-gi
    python3-requests python3-venv qemu-system-x86 ripgrep systemd udev virtiofsd xz-utils
    zsh zstd
)
bwrap_packages=(bubblewrap)

# Two keys for actions/cache: `downloads` changes with the pins, `apt` with the packages
# and with the runner's image (ImageOS and ImageVersion on GitHub's runners), because
# the set of .deb files that a machine needs depends on what its image has. An image
# that changes under the same key only costs a new download: see `apt_install`.
cmd_cache_keys() {
    local pins packages os arch
    pins="$(printf '%s\n' "$kernel_release" "$kernel_url" "${kernel_debs[@]}" \
        "$virtme_ng" "$virtme_ng_sha256" "$dbus_proxy" "$dbus_proxy_sha256")"
    packages="$(printf '%s\n' "${tools_packages[@]}" -- "${bwrap_packages[@]}")"
    os="$(sed -nE 's/^VERSION_ID="?([^"]*)"?$/\1/p' /etc/os-release)"
    arch="$(dpkg --print-architecture)"
    echo "downloads=$(sha256sum <<<"$pins" | cut -c1-16)"
    echo "apt=${ImageOS:-ubuntu$os}-${ImageVersion:-local}-$arch-$(sha256sum <<<"$packages" | cut -c1-16)"
}

# Downloads URL to FILE, unless FILE is there with the SHA-256 sum SUM, and then checks
# the sum. curl tries a failed download three more times, and stops a transfer that
# stays below 10 kB/s for a minute, so a stalled server fails the step, not the job's
# time limit.
fetch() {
    local url="$1" sum="$2" file="$3"
    if [[ -f "$file" ]] && echo "$sum  $file" | sha256sum -c --status -; then
        say "from the cache: ${file##*/}"
        return 0
    fi
    mkdir -p "$(dirname "$file")"
    curl -fsSL --retry 3 --retry-all-errors --connect-timeout 30 \
        --speed-limit 10000 --speed-time 60 -o "$file.part" "$url"
    mv "$file.part" "$file"
    echo "$sum  $file" | sha256sum -c -
}

# The dir of the pinned downloads: the cache's, else DEFAULT.
downloads() {
    if [[ -n "$cache" ]]; then echo "$cache/downloads"; else echo "$1"; fi
}

# apt's own limits: it tries a failed file three more times and drops a connection that
# sends nothing for a minute. The downloads run as root, into dirs that root made.
apt_options=(-o Acquire::Retries=3 -o Acquire::http::Timeout=60
    -o Acquire::https::Timeout=60 -o APT::Sandbox::User=root)

# apt-get update, once per machine: with a time limit, and once more when it fails. A
# mirror that does not answer stops the step after at most two times 5 minutes.
apt_update() {
    local marker=/tmp/efr-sandbox-vm-apt-updated try
    [[ -e "$marker" ]] && return 0
    for try in 1 2; do
        if as_root timeout 300 apt-get "${apt_options[@]}" update -q; then
            touch "$marker"
            return 0
        fi
        say "apt-get update failed (try $try of 2)"
    done
    fail "apt-get update failed twice; the mirror does not answer"
}

# Installs PACKAGES from the .deb files in DIR with no download, and checks that each
# package is installed. The package lists must be current (apt_update). Each file must
# have a SHA-256 that the signed lists give for its package and version; apt only
# checks the size of a file in its archive dir, so this check is what keeps a changed
# cache from installing anything. apt then resolves PACKAGES by name from the lists and
# takes the files from DIR as its archive dir. A file that does not match, or a
# version that the lists no longer have, fails, and the caller downloads.
apt_from() {
    local dir="$1" deb package version sum records status=0
    shift
    local debs=("$dir"/*.deb)
    for deb in "${debs[@]}"; do
        read -r package version < <(dpkg-deb -W --showformat='${Package} ${Version}\n' "$deb")
        sum="$(sha256sum "$deb" | cut -d ' ' -f 1)"
        # NOTE: the records go through a variable: with pipefail, a grep -q that stops
        # early would fail the pipe of a long record.
        records="$(apt-cache show "$package=$version" 2>/dev/null)" || records=
        if ! grep -qx "SHA256: $sum" <<<"$records"; then
            say "apt: ${deb##*/} does not have the SHA-256 of the package lists"
            return 1
        fi
    done
    if ((${#debs[@]} > 0)); then
        local archives
        archives="$(as_root mktemp -d)"
        as_root chmod 0755 "$archives"
        as_root mkdir "$archives/partial"
        as_root cp "${debs[@]}" "$archives/"
        as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -q \
            -o Dir::Cache::archives="$archives" --no-install-recommends --no-download \
            "$@" || status=$?
        as_root rm -rf "$archives"
        ((status == 0)) || return 1
    fi
    for package; do
        [[ "$(dpkg-query -W -f='${db:Status-Status}' "$package" 2>/dev/null)" == installed ]] ||
            return 1
    done
}

# Installs PACKAGES, the set NAME, with apt. A cache that holds the set installs it
# with no download, after apt-get update. Otherwise apt downloads the set's .deb files
# first, with all the dependencies that the machine lacks, into the cache or a
# temporary dir, and installs them from there: a hit and a miss install the same way.
# When a cached set does not install (the runner's image or the mirror changed under
# the same key), apt downloads what is still missing; that run does not save the
# cache, because its key is there. The download has a time limit of 10 minutes and
# gets one more try, which keeps the files that came. With the two tries of apt-get
# update, a mirror that is too slow stops the step after at most 30 minutes, within
# the job's limit of 40.
apt_install() {
    local name="$1" dir got try
    shift
    if [[ -n "$cache" ]]; then dir="$cache/apt/$name"; else dir="$(mktemp -d)"; fi
    apt_update
    if [[ -f "$dir/packages" && "$(<"$dir/packages")" == "$*" ]]; then
        if apt_from "$dir" "$@"; then
            say "apt: the $name packages came from the cache"
            return 0
        fi
        say "apt: the cached $name packages do not install here; downloading them again"
    fi
    got="$(as_root mktemp -d)"
    as_root mkdir -p "$got/partial"
    for try in 1 2; do
        if as_root env DEBIAN_FRONTEND=noninteractive timeout 600 apt-get "${apt_options[@]}" \
            -o Dir::Cache::archives="$got" install -y -q --no-install-recommends \
            --download-only "$@"; then
            break
        fi
        [[ "$try" == 1 ]] || fail "apt-get could not download the $name packages"
        say "apt: the download of the $name packages failed; once more"
    done
    # NOTE: the files go to a dir of the user, so actions/cache can read them.
    rm -rf "$dir"
    mkdir -p "$dir"
    as_root chmod 0755 "$got"
    local debs=("$got"/*.deb)
    if ((${#debs[@]} > 0)); then cp "${debs[@]}" "$dir/"; fi
    as_root rm -rf "$got"
    echo "$*" >"$dir/packages"
    say "apt: downloaded ${#debs[@]} files for the $name packages"
    apt_from "$dir" "$@" || fail "apt-get could not install the $name packages"
    if [[ -z "$cache" ]]; then rm -rf "$dir"; fi
}

# The packages that boot the kernel, plus zsh, ripgrep and iproute2 for the tests.
# busybox and zstd build the initramfs that carries the kernel's virtiofs and overlay
# modules, virtme-ng reads the kernel version with file, and its init finds the serial
# port that returns the exit status of the guest through udev. The corpus runs rg in its
# row 4. The network view tests require ss and ip (iproute2). Its cargo rows use the
# pinned Rust toolchain, which the job installs with rustup. The tests of the filtered
# bus run the real xdg-dbus-proxy and the real clients of systemd (hostnamectl,
# systemctl, loginctl, busctl) against two private dbus-daemon buses, with a fake
# service in Python that uses Gio (python3-gi and gir1.2-glib-2.0).
cmd_tools() {
    apt_install tools "${tools_packages[@]}"
    # The wheel by its pinned hash, without pip's resolver: its two dependencies come
    # from apt.
    local dl files wheel="virtme_ng-$virtme_ng-py3-none-any.whl"
    local tarball="xdg-dbus-proxy-$dbus_proxy.tar.xz"
    dl="$(mktemp -d)"
    files="$(downloads "$dl")"
    fetch "https://files.pythonhosted.org/packages/py3/v/virtme-ng/$wheel" \
        "$virtme_ng_sha256" "$files/$wheel"
    as_root python3 -m venv --system-site-packages "$virtme_ng_dir"
    as_root "$virtme_ng_dir/bin/pip" install -q --no-deps --no-index "$files/$wheel"
    "$virtme_ng_dir/bin/vng" --version
    fetch "https://github.com/flatpak/xdg-dbus-proxy/releases/download/$dbus_proxy/$tarball" \
        "$dbus_proxy_sha256" "$files/$tarball"
    tar -C "$dl" -xJf "$files/$tarball"
    meson setup -Dman=disabled -Dtests=false --prefix=/usr \
        "$dl/build" "$dl/xdg-dbus-proxy-$dbus_proxy" >/dev/null
    ninja -C "$dl/build" >/dev/null
    as_root meson install -C "$dl/build" --no-rebuild >/dev/null
    rm -rf "$dl"
    /usr/bin/xdg-dbus-proxy --version | grep -qx "xdg-dbus-proxy $dbus_proxy" ||
        fail "xdg-dbus-proxy $dbus_proxy is not the one in /usr/bin"
}

# Ubuntu's own package, as in .github/ubuntu-shell.Dockerfile: owned by root and not
# setuid, as the probe requires, in /usr/bin, where Arch has it and where efr-shell's
# launcher tests look. Upstream bwrap has --bind-fd from 0.10.0 on; Ubuntu 24.04's
# 0.9.0-1ubuntu0.3 has it as a backport, and efr needs no flag that came later.
cmd_bwrap() {
    apt_install bwrap "${bwrap_packages[@]}"
    /usr/bin/bwrap --version
    /usr/bin/bwrap --help | grep -q -- '--bind-fd' || fail "this bubblewrap lacks --bind-fd"
}

cmd_kernel() {
    local dir="${1:?kernel DIR}" entry sum deb files
    files="$(downloads "$dir/debs")"
    for entry in "${kernel_debs[@]}"; do
        read -r sum deb <<<"$entry"
        fetch "$kernel_url/$deb" "$sum" "$files/$deb"
        dpkg-deb -x "$files/$deb" "$dir"
    done
    # virtme-ng finds the modules in usr/lib/modules next to boot/. The package makes the
    # image readable by root only, and QEMU runs as the user.
    chmod 0644 "$dir/boot/vmlinuz-$kernel_release"
    test -d "$dir/usr/lib/modules/$kernel_release"
    # The packages ship no modules.dep. virtme-ng runs depmod when the file is missing,
    # but it ignores a failure: in the image of `just test-sandbox-vm` root owns this
    # dir, so the user's depmod fails, virtme-ng finds no modules and builds no
    # initramfs, and the kernel cannot mount its virtiofs root. So make it here.
    /usr/sbin/depmod -a -b "$dir/usr" "$kernel_release"
    test -s "$dir/usr/lib/modules/$kernel_release/modules.dep"
}

cmd_run() {
    local kernel="${1:?run KERNEL}" archive="${2:?run ARCHIVE}"
    local workspace extract
    workspace="$(realpath "${3:?run WORKSPACE}")"
    extract="$(realpath "${4:?run EXTRACT}")"
    # QEMU without KVM is too slow for the suite, and a fallback to it would only hide
    # the cause. GitHub gives public repositories /dev/kvm on its standard Linux
    # runners; the job's udev rule opens it to the runner user.
    if [[ ! -c /dev/kvm ]]; then
        fail "/dev/kvm does not exist: this runner has no KVM, so the VM cannot boot"
    fi
    if [[ ! -r /dev/kvm || ! -w /dev/kvm ]]; then
        fail "/dev/kvm exists but $(id -un) cannot open it: $(ls -l /dev/kvm)"
    fi
    # The guest runs the tests as this user: as root, the probe refuses every bwrap,
    # because root can write to it.
    local user="${SBX_VM_USER:-$(id -un)}"
    if [[ "$user" == root ]]; then
        fail "the tests need a user other than root; set SBX_VM_USER"
    fi
    local nextest
    nextest="$(command -v cargo-nextest)" || fail "cargo-nextest is not on PATH"
    say "unpacking $archive into $extract"
    "$nextest" nextest list --archive-file "$archive" --workspace-remap "$workspace" \
        --extract-to "$extract" --extract-overwrite --list-type binaries-only >/dev/null
    # The tests keep their fixtures in CARGO_TARGET_TMPDIR, a path compiled in below the
    # target dir of the build. The guest mounts a tmpfs there: the root it sees is
    # read-only, and the tmp of the sandbox replaces anything below /tmp.
    local tmpdir
    tmpdir="$(python3 -c 'import json, sys
print(json.load(open(sys.argv[1]))["rust-build-meta"]["target-directory"] + "/tmp")' \
        "$extract/target/nextest/binaries-metadata.json")"
    mkdir -p "$tmpdir" "$workspace/target/tmp"
    local log="$extract/sandbox-vm.log"
    local guest
    # SBX_GATE=0 runs the launch cost tests with 100 calls each and no gate, as CI does
    # on a push or a pull request; the weekly run and `just test-sandbox-vm` keep the
    # gate with its 1000 calls and more.
    local gate="${SBX_GATE:-1}"
    [[ "$gate" == 0 || "$gate" == 1 ]] || fail "SBX_GATE must be 0 or 1, not $gate"
    guest="$(printf '%q ' "$self" guest "$user" "$nextest" "$workspace" "$extract/target" "$tmpdir" "$gate")"
    # The size of GitHub's standard runner for public repositories, so a local run
    # boots the same VM.
    local cpus="${SBX_VM_CPUS:-4}" memory="${SBX_VM_MEMORY:-4G}"
    say "booting $kernel_release with $cpus CPUs and $memory of memory"
    local status=0
    PATH="$virtme_ng_dir/bin:$PATH" vng --run "$kernel/boot/vmlinuz-$kernel_release" \
        --user root --cpus "$cpus" --memory "$memory" --exec "$guest" \
        </dev/null 2>&1 | tee "$log" || status=$?
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        {
            echo "### Sandbox suite in a VM"
            echo
            echo '```'
            sed -nE 's/^sandbox-vm: (kernel|lsm|overlay|landlock|probe|launch cost gate|toolchain|xdg-dbus-proxy|systemd) /\1 /p' "$log"
            sed -nE 's/^ *((efr-sbx run|the child shell|launch cost|setup failures|gate:|job start).*)/\1/p' "$log"
            echo '```'
            echo
            echo "Corpus rows that skip because their tool is not installed. pacman is the"
            echo "package manager of Arch Linux and cannot exist on Ubuntu, so row 19 always"
            echo "skips here."
            echo
            echo '```'
            sed -nE 's/^ *(corpus [0-9]+: skipped: .*)/\1/p' "$log"
            echo '```'
        } >>"$GITHUB_STEP_SUMMARY"
    fi
    return "$status"
}

# Inside the VM, as root: the writable tmpfs, then the tests as the user, then the
# check of the kernel log.
cmd_guest() {
    local user="$1" nextest="$2" workspace="$3" target="$4" tmpdir="$5" gate="$6"
    # The guest's console is not a terminal, and stderr may take another way out.
    exec 2>&1
    # virtme-ng's init leaves its serial ports open as fds 3 to 5 in every process. The
    # child shell writes its records to fd 3, so a test that runs it without a record
    # fd would write to a port that nobody reads and block for good.
    local fd
    for fd in "/proc/$$/fd/"*; do
        fd="${fd##*/}"
        if ((fd > 2 && fd != 255)); then eval "exec $fd>&-"; fi
    done
    say "kernel $(uname -r)"
    say "lsm $(cat /sys/kernel/security/lsm)"
    # virtme-ng points /dev/stdout and /dev/stderr at its own serial ports. A write to
    # them by path then skips the descriptor that the process has, and a write from
    # inside a sandbox can block on the port for good. Restore the usual links.
    ln -sfn /proc/self/fd/0 /dev/stdin
    ln -sfn /proc/self/fd/1 /dev/stdout
    ln -sfn /proc/self/fd/2 /dev/stderr
    # Ubuntu's config leaves the index feature of overlayfs off, and Arch's turns it on.
    # The layer helper of efr-sbx mounts every overlay with index=off; the VM turns the
    # default on as on Arch, so the tests show that the option reaches the kernel.
    echo Y >/sys/module/overlay/parameters/index
    say "overlay index $(cat /sys/module/overlay/parameters/index)"
    # The filtered system bus of a call binds the call's socket at
    # /run/dbus/system_bus_socket over a tmpfs on /run/dbus, so that dir must exist, as on
    # a machine with a system bus. systemctl talks to a bus only when /run/systemd/system
    # exists (sd_booted). The guest has neither; it makes both, and nothing listens there.
    local place
    for place in /run/dbus /run/systemd/system; do
        if [[ ! -d "$place" ]]; then
            mkdir -p "$place" || fail "the guest cannot make $place"
        fi
    done
    say "bus mount points: /run/dbus /run/systemd/system"
    # The tests keep their fixtures below the target dir of the build
    # (CARGO_TARGET_TMPDIR) and below the workspace's target/tmp (efr-shell); in CI
    # the two are one dir.
    local dir
    for dir in "$tmpdir" "$workspace/target/tmp"; do
        if ! mountpoint -q "$dir"; then
            mount -t tmpfs -o mode=0755 efr-test-tmp "$dir"
            chown "$user:$(id -gn "$user")" "$dir"
        fi
    done
    local status=0
    runuser -u "$user" -- "$self" tests "$nextest" "$workspace" "$target" "$tmpdir" "$gate" \
        </dev/null || status=$?
    # The layer helper mounts each overlay with index=off and xino=off, so the kernel
    # must log no fallback and no upper dir in use. Only root reads the kernel log.
    local lines
    lines="$(dmesg | grep -E 'overlayfs: .*(falling back|in-use|undefined behavior)' || true)"
    if [[ -n "$lines" ]]; then
        say "the kernel logged $(wc -l <<<"$lines") overlayfs lines, the first ones:"
        head -n 5 <<<"$lines"
        fail "the cache overlays made the kernel fall back or share an upper dir"
    fi
    say "kernel log: no overlayfs fallback and no upper dir in use"
    return "$status"
}

# Inside the VM, as the user: what `just test-sandbox` runs, on a machine that must be
# ready, so a skipped test fails.
cmd_tests() {
    local nextest="$1" workspace="$2" target="$3" tmpdir="$4" gate="$5"
    export HOME
    HOME="$(getent passwd "$(id -un)" | cut -d: -f6)"
    # rustup's cargo, rustc and rustfmt, for the corpus rows that build a Rust project.
    export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$HOME/.cargo/bin
    cd "$workspace"
    # The corpus skips a row whose tool is not installed. These tools exist on Ubuntu, so
    # here a missing one is a fault of the job. The fixture projects of the corpus lie
    # outside the workspace, so the pinned toolchain is named here. The proxy tests use
    # git, and they make their test CA with openssl; the runner and the image have both.
    # The bus filter tests need xdg-dbus-proxy, dbus-daemon, the clients of systemd and
    # Python's Gio, which `tools` installs.
    local toolchain tool
    toolchain="$(sed -nE 's/^channel = "(.*)"/\1/p' rust-toolchain.toml)"
    export RUSTUP_TOOLCHAIN="$toolchain"
    for tool in cargo rustfmt rustup rg ss ip git openssl \
        xdg-dbus-proxy dbus-daemon hostnamectl systemctl loginctl busctl; do
        command -v "$tool" >/dev/null || fail "$tool is not on PATH in the guest"
    done
    python3 -I -c 'from gi.repository import Gio' ||
        fail "python3 cannot import gi.repository.Gio in the guest"
    say "xdg-dbus-proxy $(xdg-dbus-proxy --version)"
    say "systemd $(systemctl --version | head -n1)"
    cargo --version >/dev/null 2>&1 ||
        fail "the pinned toolchain $toolchain is not installed: run rustup toolchain install"
    say "toolchain $(cargo --version)"
    local bin="$target/debug/efr-sbx"
    mkdir -m 700 "$tmpdir/sbx-probe"
    local report
    report="$("$bin" probe --json --dir "$tmpdir/sbx-probe")" || true
    say "landlock abi $(sed -nE 's/.*"landlock_abi":([0-9]+).*/\1/p' <<<"$report")"
    if [[ "$report" != *'"failure":null'* ]]; then
        say "probe failed: $report"
        fail "the probe says the sandbox is unavailable on this kernel"
    fi
    say "probe ready"
    say "launch cost gate $gate"
    export EFR_TEST_SBX_BIN="$bin" EFR_TEST_SBX_REQUIRE=1 EFR_TEST_SBX_GATE="$gate"
    export INSTA_WORKSPACE_ROOT="$workspace" INSTA_UPDATE=no
    local run=("$nextest" nextest run --workspace-remap "$workspace" --target-dir-remap "$target"
        --binaries-metadata "$target/nextest/binaries-metadata.json"
        --cargo-metadata "$target/nextest/cargo-metadata.json")
    "${run[@]}" -E 'package(efr-sbx) & not test(/^cost::|^corpus::/)'
    # The launch cost and the corpus print their tables; show them.
    "${run[@]}" --success-output immediate -E 'package(efr-sbx) & test(/^cost::|^corpus::/)'
    # efr-shell's and efrd's tests say `skipped:` on stderr when they cannot run.
    local log="$tmpdir/sbx-shell-tests.log"
    EFR_TEST_ZSH=1 "${run[@]}" --success-output immediate \
        -E 'package(efr-shell) & test(/e2e_zsh::launcher::/)' 2>&1 | tee "$log"
    if grep -q 'skipped:' "$log"; then
        fail "efr-shell's launcher tests skipped on a ready machine"
    fi
    log="$tmpdir/sbx-daemon-tests.log"
    EFR_TEST_ZSH=1 "${run[@]}" --success-output immediate \
        -E 'package(efr-daemon) & test(/^sandbox::/)' 2>&1 | tee "$log"
    if grep -q 'skipped:' "$log"; then
        fail "efr-daemon's sandbox tests skipped on a ready machine"
    fi
    say "all sandbox tests passed"
}

command="${1:-}"
shift || true
case "$command" in
    cache-keys) cmd_cache_keys ;;
    tools | bwrap | kernel | run | guest | tests) "cmd_$command" "$@" ;;
    *) fail "usage: sandbox-vm.sh cache-keys | tools | bwrap | kernel DIR | run KERNEL ARCHIVE WORKSPACE EXTRACT" ;;
esac
