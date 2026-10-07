# CI's sandbox job on a local machine: the image of `just test-shell-ubuntu` (Ubuntu
# 24.04 like GitHub's runner, zsh, the pinned Rust toolchain and cargo-nextest) with
# what the job installs through .github/sandbox-vm.sh: QEMU, virtiofsd, virtme-ng,
# bubblewrap in /usr and the pinned kernel. `just test-sandbox-vm` builds it with .github
# as the context and runs the job's tests in it with /dev/kvm.

ARG BASE=efr-shell-ubuntu
FROM ${BASE}

USER root
COPY sandbox-vm.sh /usr/local/lib/efr-ci/sandbox-vm.sh
RUN /usr/local/lib/efr-ci/sandbox-vm.sh tools \
    && /usr/local/lib/efr-ci/sandbox-vm.sh bwrap \
    && /usr/local/lib/efr-ci/sandbox-vm.sh kernel /opt/efr-kernel \
    && rm -rf /opt/efr-kernel/debs /var/lib/apt/lists/*
USER runner
