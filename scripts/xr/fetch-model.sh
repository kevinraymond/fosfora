#!/usr/bin/env bash
# Fetch the voice path's speech-to-text model (board #3751) into
# assets/xr/models/, git-ignored, when it is absent or its SHA-256 does not
# match. Source and license: assets/xr/models/LICENSE.md.
set -euo pipefail

cd "$(dirname "$0")/../.."

MODEL=ggml-base.en.bin
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$MODEL"
SHA256=a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002
DIR=assets/xr/models
DEST="$DIR/$MODEL"

matches() {
    [ -f "$1" ] && [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$SHA256" ]
}

if matches "$DEST"; then
    echo "== model $DEST present, sha256 ok"
else
    echo "== fetching $MODEL"
    mkdir -p "$DIR"
    tmp="$DEST.part"
    curl -sSfL --retry 3 -o "$tmp" "$URL"
    if ! matches "$tmp"; then
        rm -f "$tmp"
        echo "fetch-model: $URL does not match sha256 $SHA256" >&2
        exit 1
    fi
    mv "$tmp" "$DEST"
fi
echo "== model $DEST: $(stat -c %s "$DEST") bytes"
