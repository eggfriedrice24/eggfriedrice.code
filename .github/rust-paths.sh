#!/usr/bin/env bash
# Says whether CI's Rust jobs must run: prints `rust=true` or `rust=false` for
# $GITHUB_OUTPUT, and the reason on stderr.
#
#   rust-paths.sh EVENT BASE
#
# EVENT is the event of the run, BASE the commit to compare HEAD with (the base of a pull
# request, or the commit before a push). The jobs run on a scheduled or a manual run,
# when there is no usable BASE, and when any changed file can change what they test.
# Only the files below cannot: the text of the docs and of the docs site, and the phone
# app (app/). Three docs files are inputs of Rust tests, so they run the jobs:
# docs/permissions.md, docs/config.schema.json and crates/efr-stdx/README.md. The `fmt`,
# `tidy`, `deny` and `docs` jobs do not read this script: they always run.
# Every other file, a new one too, runs the jobs.
set -euo pipefail

event="${1:?EVENT}"
base="${2:-}"

decide() {
    echo "rust=$1"
    echo "rust-paths: $2" >&2
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
        docs/permissions.md | docs/config.schema.json | crates/efr-stdx/README.md)
            decide true "$path is an input of a Rust test"
            ;;
        CHANGELOG.md | README.md | AGENTS.md | ARCHITECTURE.md | CONVENTIONS.md | LICENSE) ;;
        docs/* | book/* | crates/*/README.md) ;;
        app/* | .github/app-paths.sh) ;;
        *) decide true "$path can change what the Rust jobs test" ;;
    esac
done <<<"$changed"
decide false "every changed file is docs text or the phone app"
