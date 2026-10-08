#!/usr/bin/env bash
# Fetch the voice path's models (board #3751) into assets/xr/models/,
# git-ignored: each file assets/xr/models/MODELS.txt lists (name, URL,
# SHA-256), when it is absent or its SHA-256 does not match. Sources and
# licenses: assets/xr/models/LICENSE.md.
#
# A URL of `placeholder` is a model not hosted yet (the on-device decision
# model, V5): a file already present whose SHA-256 matches is accepted
# ("present, sha256 ok"; copy it in from the training export for a dev
# build), an absent one is a warning and the script goes on (the APK builds
# without it and the provider stays off at runtime, saying so in the log),
# and a present one that does not match is an error.
set -euo pipefail

cd "$(dirname "$0")/../.."

DIR=assets/xr/models
LIST="$DIR/MODELS.txt"

matches() {
    [ -f "$1" ] && [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$2" ]
}

warn() {
    echo "== $1" >&2
    # On GitHub Actions the warning also shows as a notice on the run.
    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
        echo "::notice title=Model not fetched::$1"
    fi
}

while read -r name url sha <&3; do
    case "$name" in
        "" | "#"*) continue ;;
    esac
    if [ -z "$url" ] || [ -z "$sha" ]; then
        echo "fetch-model: $LIST: no URL or SHA-256 for $name" >&2
        exit 1
    fi
    dest="$DIR/$name"
    if matches "$dest" "$sha"; then
        echo "== $dest present, sha256 ok ($(stat -c %s "$dest") bytes)"
        continue
    fi
    if [ "$url" = "placeholder" ]; then
        if [ -f "$dest" ]; then
            echo "fetch-model: $dest does not match sha256 $sha (and $name has no URL to fetch it again)" >&2
            exit 1
        fi
        warn "$name: not hosted yet (placeholder in MODELS.txt) and not in $DIR; building without it"
        continue
    fi
    echo "== fetching $name"
    mkdir -p "$DIR"
    tmp="$dest.part"
    curl -sSfL --retry 3 -o "$tmp" "$url"
    if ! matches "$tmp" "$sha"; then
        rm -f "$tmp"
        echo "fetch-model: $url does not match sha256 $sha" >&2
        exit 1
    fi
    mv "$tmp" "$dest"
    echo "== $dest: $(stat -c %s "$dest") bytes"
done 3< "$LIST"
