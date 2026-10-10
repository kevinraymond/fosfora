#!/usr/bin/env bash
# The Fosfora VR release keystore: print the keytool command that creates it,
# or create it with --create <path>. See docs/xr/RELEASE.md.
#
#   scripts/xr/release-keystore.sh                  print the command
#   scripts/xr/release-keystore.sh --create <path>  run it (keytool prompts)
#
# The keystore is the app's identity. Meta ties the app to the signing
# certificate of its first uploaded build, and every later build must be
# signed with the same key, so:
#   - keep it outside the repo (the repo is public; *.p12, *.jks and
#     *.keystore are git-ignored as a backstop, not as a hiding place);
#   - back it up, with its passwords, somewhere that outlives this machine;
#   - never replace it once a build has been uploaded: a lost or changed key
#     means a new app on the store, not an update.
#
# The script never takes a password on its command line (it would land in
# the shell history and the process list): keytool asks for the store
# password, and the key shares it (PKCS12).
set -euo pipefail

ALIAS=fosfora-xr
DNAME="CN=<your name>, O=<organization>, L=<city>, ST=<state>, C=<two-letter country code>"

usage() {
    sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
}

print_command() {
    local path="$1" dname="$2"
    cat <<CMD
keytool -genkeypair -v \\
  -storetype PKCS12 \\
  -keystore "$path" \\
  -alias $ALIAS \\
  -keyalg RSA -keysize 4096 \\
  -validity 10000 \\
  -dname "$dname"
CMD
}

case "${1:-}" in
    "")
        echo "# Create the release keystore (outside the repo; keytool prompts for the password):"
        print_command "\$HOME/keys/fosfora-xr-release.p12" "$DNAME"
        ;;
    --create)
        path="${2:-}"
        if [ -z "$path" ]; then
            echo "--create needs the keystore path" >&2
            exit 2
        fi
        if [ -e "$path" ]; then
            echo "$path exists; refusing to overwrite a release keystore" >&2
            exit 1
        fi
        case "$(realpath -m "$path")" in
            "$(realpath "$(dirname "$0")/../..")"/*)
                echo "$path is inside the repo; put the release keystore outside it" >&2
                exit 1
                ;;
        esac
        mkdir -p "$(dirname "$path")"
        read -r -p "Certificate DN [$DNAME]: " dname
        dname="${dname:-$DNAME}"
        if [[ "$dname" == *"<"* ]]; then
            echo "fill in the DN placeholders (CN=..., O=..., L=..., ST=..., C=...)" >&2
            exit 1
        fi
        keytool -genkeypair -v \
            -storetype PKCS12 \
            -keystore "$path" \
            -alias "$ALIAS" \
            -keyalg RSA -keysize 4096 \
            -validity 10000 \
            -dname "$dname"
        chmod 600 "$path"
        abs="$(realpath "$path")"
        cat <<EXPORTS

Created $abs. Back it up now, with its password.
Add to your shell profile (fill in the password, or read it from a password manager):

export XR_RELEASE_KEYSTORE="$abs"
export XR_RELEASE_KEYSTORE_PASS='<the store password>'
export XR_RELEASE_KEY_ALIAS=$ALIAS
EXPORTS
        ;;
    -h|--help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
