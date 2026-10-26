#!/usr/bin/env bash
# Says whether CI's `sandbox` job must run: prints `sandbox=true` or `sandbox=false` for
# $GITHUB_OUTPUT, and the reason on stderr.
#
#   sandbox-paths.sh EVENT BASE
#
# EVENT is the event of the run, BASE the commit to compare HEAD with (the base of a pull
# request, or the commit before a push). The job runs on a scheduled or a manual run,
# when there is no usable BASE, and when any changed file can change what the job tests.
# Only the files below cannot: docs and the docs site, the CLI and the renderer (the
# sandbox tests never run `efr`), the Ghostty screen (not in the job's archive),
# packaging, xtask, lint configs and the phone app (app/).
# Every other file, a new one too, runs the job.
set -euo pipefail

event="${1:?EVENT}"
base="${2:-}"

decide() {
    echo "sandbox=$1"
    echo "sandbox-paths: $2" >&2
    exit 0
}

case "$event" in
    schedule | workflow_dispatch) decide true "a $event run tests everything" ;;
esac
if [[ -z "$base" || "$base" =~ ^0+$ ]] || ! git cat-file -e "$base^{commit}" 2>/dev/null; then
    decide true "no base commit to compare with"
fi

changed="$(git diff --name-only "$base" HEAD)"
if [[ -z "$changed" ]]; then
    decide false "no file changed since $base"
fi
while IFS= read -r path; do
    case "$path" in
        CHANGELOG.md | README.md | AGENTS.md | ARCHITECTURE.md | CONVENTIONS.md | LICENSE) ;;
        docs/* | book/* | packaging/* | xtask/*) ;;
        crates/efr-cli/* | crates/efr-render/* | crates/efr-screen-ghostty/*) ;;
        crates/*/README.md) ;;
        typos.toml | rustfmt.toml | clippy.toml | deny.toml | .githooks/*) ;;
        app/* | .github/app-paths.sh) ;;
        .github/workflows/aur.yml | .github/workflows/release.yml | .github/workflows/pages.yml) ;;
        *) decide true "$path can change what the sandbox job tests" ;;
    esac
done <<<"$changed"
decide false "every changed file is outside the sandbox job's reach"
