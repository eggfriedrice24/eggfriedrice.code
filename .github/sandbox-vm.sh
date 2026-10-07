#!/usr/bin/env bash
# The tests of `just test-sandbox` (real bwrap, Landlock ABI 9 and seccomp) in a VM.
# GitHub's runner kernel is older than 7.1, and the runner's AppArmor blocks
# unprivileged user namespaces, so the suite cannot run on the runner itself. This
# script boots the kernel that it pins with virtme-ng in QEMU and KVM, and runs the
# tests of a nextest archive inside. The guest sees the host's root file system
# read-only through virtiofs. CI's sandbox job and `just test-sandbox-vm` both run it,
# so the pins below are the only copy.
#
#   sandbox-vm.sh tools           QEMU, virtiofsd, virtme-ng and zsh (root or sudo)
#   sandbox-vm.sh bwrap           bubblewrap 0.13.0 in /usr (root or sudo)
#   sandbox-vm.sh kernel DIR      the pinned kernel, checked and unpacked into DIR
#   sandbox-vm.sh run KERNEL ARCHIVE WORKSPACE EXTRACT
#                                 unpack the archive into EXTRACT, boot, run the tests
#   sandbox-vm.sh guest ...       inside the VM only: called by `run`
#   sandbox-vm.sh tests ...       inside the VM only: called by `guest`
set -euo pipefail

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

# The same bubblewrap as .github/ubuntu-shell.Dockerfile: Ubuntu 24.04 ships 0.9.0,
# which lacks --overlay and --tmp-overlay.
bwrap=0.13.0
bwrap_sha256=4734237473c0e5d695e4e9034a34e43b2dbf5164655bd13fa59ae376b2b7a765

self="$(realpath "${BASH_SOURCE[0]}")"

say() { echo "sandbox-vm: $*"; }
fail() {
    echo "sandbox-vm: $*" >&2
    exit 1
}
as_root() {
    if [[ "$(id -u)" == 0 ]]; then "$@"; else sudo "$@"; fi
}

# The packages that boot the kernel, plus zsh for the tests. busybox and zstd build the
# initramfs that carries the kernel's virtiofs and overlay modules, virtme-ng reads the
# kernel version with file, and its init finds the serial port that returns the exit
# status of the guest through udev.
cmd_tools() {
    as_root apt-get update -q
    as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends \
        busybox-static ca-certificates curl file kmod python3-argcomplete python3-requests \
        python3-venv qemu-system-x86 udev virtiofsd zsh zstd
    # The wheel by its pinned hash, without pip's resolver: its two dependencies come
    # from apt.
    local dl wheel="virtme_ng-$virtme_ng-py3-none-any.whl"
    dl="$(mktemp -d)"
    curl -fsSL -o "$dl/$wheel" "https://files.pythonhosted.org/packages/py3/v/virtme-ng/$wheel"
    echo "$virtme_ng_sha256  $dl/$wheel" | sha256sum -c -
    as_root python3 -m venv --system-site-packages "$virtme_ng_dir"
    as_root "$virtme_ng_dir/bin/pip" install -q --no-deps --no-index "$dl/$wheel"
    rm -rf "$dl"
    "$virtme_ng_dir/bin/vng" --version
}

# Built from the release tarball, owned by root and not setuid, as the probe requires.
# It goes to /usr/bin, where Arch has it and where efr-shell's launcher tests look; on
# the runner it replaces Ubuntu's 0.9.0 if that is installed.
cmd_bwrap() {
    as_root apt-get update -q
    as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends \
        build-essential ca-certificates curl libcap-dev meson ninja-build pkg-config xz-utils
    local dl tarball="bubblewrap-$bwrap.tar.xz"
    dl="$(mktemp -d)"
    curl -fsSL -o "$dl/$tarball" \
        "https://github.com/containers/bubblewrap/releases/download/v$bwrap/$tarball"
    echo "$bwrap_sha256  $dl/$tarball" | sha256sum -c -
    tar -xJf "$dl/$tarball" -C "$dl"
    meson setup "$dl/build" "$dl/bubblewrap-$bwrap" --prefix=/usr -Dman=disabled \
        -Dselinux=disabled -Dtests=false -Dbash_completion=disabled -Dzsh_completion=disabled
    meson compile -C "$dl/build"
    as_root meson install -C "$dl/build"
    rm -rf "$dl"
    /usr/bin/bwrap --version
}

cmd_kernel() {
    local dir="${1:?kernel DIR}" entry sum deb
    mkdir -p "$dir/debs"
    for entry in "${kernel_debs[@]}"; do
        read -r sum deb <<<"$entry"
        if [[ ! -f "$dir/debs/$deb" ]]; then
            curl -fsSL -o "$dir/debs/$deb.part" "$kernel_url/$deb"
            mv "$dir/debs/$deb.part" "$dir/debs/$deb"
        fi
        echo "$sum  $dir/debs/$deb" | sha256sum -c -
        dpkg-deb -x "$dir/debs/$deb" "$dir"
    done
    # virtme-ng finds the modules in usr/lib/modules next to boot/ and runs depmod. The
    # package makes the image readable by root only, and QEMU runs as the user.
    chmod 0644 "$dir/boot/vmlinuz-$kernel_release"
    test -d "$dir/usr/lib/modules/$kernel_release"
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
    guest="$(printf '%q ' "$self" guest "$user" "$nextest" "$workspace" "$extract/target" "$tmpdir")"
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
            sed -nE 's/^sandbox-vm: (kernel|lsm|overlay|landlock|probe) /\1 /p' "$log"
            sed -nE 's/^ *((efr-sbx run|the child shell|launch cost|setup failures|gate:).*)/\1/p' "$log"
            echo '```'
        } >>"$GITHUB_STEP_SUMMARY"
    fi
    return "$status"
}

# Inside the VM, as root: the writable tmpfs, then the tests as the user.
cmd_guest() {
    local user="$1" nextest="$2" workspace="$3" target="$4" tmpdir="$5"
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
    exec runuser -u "$user" -- "$self" tests "$nextest" "$workspace" "$target" "$tmpdir" \
        </dev/null
}

# Inside the VM, as the user: what `just test-sandbox` runs, on a machine that must be
# ready, so a skipped test fails.
cmd_tests() {
    local nextest="$1" workspace="$2" target="$3" tmpdir="$4"
    export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
    export HOME
    HOME="$(getent passwd "$(id -un)" | cut -d: -f6)"
    cd "$workspace"
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
    export EFR_TEST_SBX_BIN="$bin" EFR_TEST_SBX_REQUIRE=1 EFR_TEST_SBX_GATE=1
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
    tools | bwrap | kernel | run | guest | tests) "cmd_$command" "$@" ;;
    *) fail "usage: sandbox-vm.sh tools | bwrap | kernel DIR | run KERNEL ARCHIVE WORKSPACE EXTRACT" ;;
esac
