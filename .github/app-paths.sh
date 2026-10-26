#!/usr/bin/env bash
# Says whether CI's `app` job must run: prints `app=true` or `app=false` for
# $GITHUB_OUTPUT, and the reason on stderr.
#
#   app-paths.sh EVENT BASE
#
# EVENT is the event of the run, BASE the commit to compare HEAD with (the base of a pull
# request, or the commit before a push). The job runs on a scheduled or a manual run,
# when there is no usable BASE, and when a changed file can change what the job tests:
# the phone app, the protocol crate and its schema, which the app's types follow, and
# the CI files that run the job.
set -euo pipefail

event="${1:?EVENT}"
base="${2:-}"

decide() {
    echo "app=$1"
    echo "app-paths: $2" >&2
    exit 0
}

case "$event" in
    schedule | workflow_dispatch) decide true "a $event run tests everything" ;;
esac
if [[ -z "$base" || "$base" =~ ^0+$ ]] || ! git cat-file -e "$base^{commit}" 2>/dev/null; then
    decide true "no base commit to compare with"
fi

changed="$(git diff --name-only "$base" HEAD)"
while IFS= read -r path; do
    case "$path" in
        app/* | crates/efr-protocol/* | docs/protocol.schema.json | .github/workflows/ci.yml | \
            .github/app-paths.sh)
            decide true "$path can change what the app job tests"
            ;;
    esac
done <<<"$changed"
decide false "no changed file is in the app job's reach"
