# The shell job of .github/workflows/ci.yml on a local machine: Ubuntu 24.04 (the
# ubuntu-latest image) with /usr/share world-writable as on the runner, zsh from apt, a
# non-root user with a home under /home, the pinned Rust toolchain and cargo-nextest.
# `just test-shell-ubuntu` builds it without a context (nothing is copied in) and runs
# the job's test command in it over the mounted checkout.

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
