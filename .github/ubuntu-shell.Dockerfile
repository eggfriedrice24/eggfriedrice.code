# The shell job of .github/workflows/ci.yml on a local machine: Ubuntu 24.04 (the
# ubuntu-latest image) with /usr/share world-writable as on the runner, zsh from apt, a
# non-root user with a home under /home, the pinned Rust toolchain and cargo-nextest.
# `just test-shell-ubuntu` builds it without a context (nothing is copied in) and runs
# the job's tests in it over the mounted checkout. The job takes its test binaries from
# the archive of CI's build job; the container builds the same binaries itself.

FROM ubuntu:24.04

ARG UID=1000
ARG GID=1000
ARG TOOLCHAIN
ARG NEXTEST

ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git pkg-config sudo \
    && rm -rf /var/lib/apt/lists/*

# The runner image makes everything under /usr/share world-writable when it is built
# (configure-system.sh in actions/runner-images), long before the job installs zsh. By
# then curl has put its zsh completion in /usr/share/zsh/vendor-completions, a
# directory of zsh's fpath, so that directory stays world-writable after zsh is
# installed, and compinit in Ubuntu's /etc/zsh/zshrc finds it insecure.
RUN chmod -R 777 /usr/share

# The shell job's own step.
RUN apt-get update && apt-get install -y zsh && rm -rf /var/lib/apt/lists/*

# bubblewrap 0.13.0 for `just test-sandbox-ubuntu`: Ubuntu 24.04 ships 0.9.0, which lacks
# --overlay and --tmp-overlay. Built from the release tarball, checked by its SHA-256,
# owned by root and not setuid, as the sandbox probe requires.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libcap-dev meson ninja-build xz-utils \
    && rm -rf /var/lib/apt/lists/* \
    && cd /tmp \
    && curl -fsSLO https://github.com/containers/bubblewrap/releases/download/v0.13.0/bubblewrap-0.13.0.tar.xz \
    && echo "4734237473c0e5d695e4e9034a34e43b2dbf5164655bd13fa59ae376b2b7a765  bubblewrap-0.13.0.tar.xz" \
        | sha256sum -c - \
    && tar -xJf bubblewrap-0.13.0.tar.xz \
    && meson setup /tmp/bwrap-build bubblewrap-0.13.0 --prefix=/usr/local -Dman=disabled \
        -Dselinux=disabled -Dtests=false -Dbash_completion=disabled -Dzsh_completion=disabled \
    && meson compile -C /tmp/bwrap-build \
    && meson install -C /tmp/bwrap-build \
    && rm -rf /tmp/bwrap-build /tmp/bubblewrap-0.13.0 /tmp/bubblewrap-0.13.0.tar.xz

# The image's own user holds uid 1000; the runner user takes the host's ids instead,
# so the files it writes into the mounted checkout and caches belong to the host user.
RUN if id ubuntu >/dev/null 2>&1; then userdel -r ubuntu; fi \
    && if ! getent group "$GID" >/dev/null; then groupadd -g "$GID" runner; fi \
    && useradd -m -o -u "$UID" -g "$GID" -s /bin/bash runner \
    && echo 'runner ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/runner

USER runner
WORKDIR /home/runner
ENV PATH=/home/runner/.cargo/bin:$PATH

RUN test -n "$TOOLCHAIN" && test -n "$NEXTEST" \
    && curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none \
    && rustup toolchain install "$TOOLCHAIN" --profile minimal -c clippy,rustfmt \
    && rustup default "$TOOLCHAIN" \
    && curl -fsSL "https://get.nexte.st/$NEXTEST/linux" | tar -xzf - -C /home/runner/.cargo/bin
