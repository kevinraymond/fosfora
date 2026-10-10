#!/usr/bin/env bash
# Fosfora VR dev loop for a Quest over adb: build the Rust cdylib with cargo-ndk,
# package it with the Gradle project in android/, install, launch and follow the
# filtered logcat. See docs/xr/BUILD_PLAN.md (S1).
#
#   scripts/xr/run.sh [all|build|lint|install|launch|stop|log|uninstall|
#                      release|verify-release|install-release] [--debug]
#
#   all             build + install + launch + log (default)
#   build           cargo ndk (release unless --debug) + the voice models + gradle assembleDebug
#   lint            cargo clippy for the Android target with -D warnings
#   install         adb install -r the debug APK
#   launch          adb shell am start the NativeActivity
#   stop            force-stop the app
#   log             adb logcat filtered to this app, the runtime and crashes
#   release         cargo ndk --release + the voice models + gradle assembleRelease,
#                   signed with the release key (XR_RELEASE_KEYSTORE, below)
#   verify-release  apksigner, zipalign and aapt checks on the release APK: fails
#                   on an unsigned, misaligned or debuggable APK
#   install-release adb install -r the release APK. Its signature differs from the
#                   debug key's, so over an installed debug build Android refuses
#                   it (INSTALL_FAILED_UPDATE_INCOMPATIBLE): run `uninstall` first,
#                   which also deletes the app's data on the headset. Same the
#                   other way round, back to debug.
#
# Env: ANDROID_HOME (default ~/Android/Sdk), ANDROID_NDK_HOME (default: newest
# NDK under $ANDROID_HOME/ndk), ANDROID_SERIAL to pick a device.
# Release signing (docs/xr/RELEASE.md, scripts/xr/release-keystore.sh):
# XR_RELEASE_KEYSTORE, XR_RELEASE_KEYSTORE_PASS, XR_RELEASE_KEY_ALIAS (default
# fosfora-xr), XR_RELEASE_KEY_PASS (default: the store password).
# XR_VERSION_CODE / XR_VERSION_NAME override android/version.properties.
set -euo pipefail

cd "$(dirname "$0")/../.."

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
if [ -z "${ANDROID_NDK_HOME:-}" ]; then
    ANDROID_NDK_HOME="$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
    export ANDROID_NDK_HOME
fi
export ANDROID_SDK_ROOT="$ANDROID_HOME"

PACKAGE=dev.fosfora.xr
ACTIVITY=android.app.NativeActivity
ABI=arm64-v8a
MIN_SDK=29
JNI_DIR=android/app/src/main/jniLibs
APK=android/app/build/outputs/apk/debug/app-debug.apk
RELEASE_APK=android/app/build/outputs/apk/release/app-release.apk
LOG_TAG=fosfora_xr

cmd="${1:-all}"
profile=--release
for arg in "$@"; do
    case "$arg" in
        --debug) profile="" ;;
    esac
done
# A release APK always carries the optimized cdylib.
if [ "$cmd" = release ]; then
    profile=--release
fi

# Newest build-tools (apksigner, zipalign, aapt), found like the NDK above.
build_tools() {
    local dir
    dir="$(ls -d "$ANDROID_HOME"/build-tools/* 2>/dev/null | sort -V | tail -1 || true)"
    if [ -z "$dir" ]; then
        echo "no build-tools under $ANDROID_HOME/build-tools" >&2
        exit 1
    fi
    echo "$dir"
}

# whisper.cpp inside whisper-rs-sys (the voice path, board #3751) is built
# by cmake, and whisper-rs-sys forwards only CMAKE_*, GGML_* and WHISPER_*
# variables to it: the toolchain wrapper sets the NDK's ABI and platform,
# GGML_NATIVE=OFF because the host is not the target, and the Quest 3's
# Cortex-A78C ISA is named instead (dotprod and fp16, no i8mm).
whisper_env() {
    export CMAKE_TOOLCHAIN_FILE="$PWD/android/cmake/android.toolchain.cmake"
    export GGML_NATIVE=OFF
    export GGML_CPU_ARM_ARCH=armv8.2-a+dotprod+fp16
}

cargo_build() {
    echo "== cargo ndk ($ABI, api $MIN_SDK, ${profile:-dev})"
    whisper_env
    # shellcheck disable=SC2086
    cargo ndk -t "$ABI" --platform "$MIN_SDK" -o "$JNI_DIR" build -p fosfora-xr $profile
    # whisper.cpp links the NDK's C++ runtime dynamically: package it next
    # to the cdylib, as the loader AAR's libopenxr_loader.so is.
    local host
    host="$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64"
    cp "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" \
        "$JNI_DIR/$ABI/"
    ls -la "$JNI_DIR/$ABI/"
}

fetch_model() {
    scripts/xr/fetch-model.sh
}

gradle_build() {
    echo "== gradle assembleDebug"
    (cd android && ./gradlew --quiet assembleDebug)
    ls -la "$APK"
}

# Checked before the cargo build so a missing key costs nothing; Gradle's
# checkReleaseSigning makes the same check for a direct assembleRelease.
require_release_key() {
    if [ -z "${XR_RELEASE_KEYSTORE:-}" ] || [ -z "${XR_RELEASE_KEYSTORE_PASS:-}" ]; then
        echo "release: set XR_RELEASE_KEYSTORE and XR_RELEASE_KEYSTORE_PASS" \
            "(scripts/xr/release-keystore.sh, docs/xr/RELEASE.md)" >&2
        exit 1
    fi
}

gradle_release() {
    echo "== gradle assembleRelease"
    (cd android && ./gradlew --quiet assembleRelease)
    ls -la "$RELEASE_APK"
    echo "release APK: $RELEASE_APK"
}

do_verify_release() {
    local bt certs badging debuggable
    bt="$(build_tools)"
    if [ ! -f "$RELEASE_APK" ]; then
        echo "no release APK at $RELEASE_APK; run: scripts/xr/run.sh release" >&2
        exit 1
    fi
    echo "== apksigner verify ($bt)"
    # --min-sdk-version 23: apksigner skips the v1 (JAR) signature for an APK
    # whose minSdk is 24 or more; the release signs v1 and v2 (build.gradle.kts)
    # and this checks both.
    if ! certs="$("$bt/apksigner" verify --verbose --print-certs --min-sdk-version 23 \
        "$RELEASE_APK" 2>&1)"; then
        printf '%s\n' "$certs"
        echo "FAIL: $RELEASE_APK is not signed (or its signature does not verify)" >&2
        exit 1
    fi
    printf '%s\n' "$certs" | grep -E '^Verifie|^Number of signers|certificate (DN|SHA-256|SHA-1)' \
        || printf '%s\n' "$certs"
    if printf '%s\n' "$certs" | grep -q 'CN=Android Debug'; then
        echo "FAIL: signed with an Android debug certificate" >&2
        exit 1
    fi
    echo "== zipalign -c 4"
    if ! "$bt/zipalign" -c -v 4 "$RELEASE_APK" > /dev/null; then
        "$bt/zipalign" -c -v 4 "$RELEASE_APK" | grep -v '(OK' || true
        echo "FAIL: $RELEASE_APK is not 4-byte aligned" >&2
        exit 1
    fi
    echo "Verification successful (4-byte alignment)"
    echo "== aapt dump badging"
    badging="$("$bt/aapt" dump badging "$RELEASE_APK")"
    printf '%s\n' "$badging" | grep -E "^package:|^(sdkVersion|targetSdkVersion|native-code):"
    debuggable=no
    if printf '%s\n' "$badging" | grep -q "^application-debuggable"; then
        debuggable=yes
    fi
    echo "debuggable: $debuggable"
    if [ "$debuggable" = yes ]; then
        echo "FAIL: $RELEASE_APK is debuggable" >&2
        exit 1
    fi
    ls -la "$RELEASE_APK"
    echo "release APK verified"
}

do_install_release() {
    echo "== adb install (release)"
    adb install -r "$RELEASE_APK"
    adb shell sleep 2
}

do_lint() {
    echo "== clippy ($ABI)"
    whisper_env
    cargo ndk -t "$ABI" --platform "$MIN_SDK" clippy -p fosfora-xr --all-targets -- -D warnings
}

do_install() {
    echo "== adb install"
    adb install -r "$APK"
    # The package-replaced broadcast lands after `install` returns; a launch
    # that races it starts a bare process with no activity (seen on v207).
    adb shell sleep 2
}

do_launch() {
    echo "== launch $PACKAGE/$ACTIVITY"
    adb logcat -c || true
    adb shell am start -n "$PACKAGE/$ACTIVITY"
}

do_stop() {
    adb shell am force-stop "$PACKAGE"
}

do_log() {
    echo "== logcat (ctrl-c to stop)"
    # Our tag, the runtime's frame stats, the loader, and native/Java crashes.
    adb logcat -v time \
        "$LOG_TAG:V" VrApi:I OpenXR:I OpenXR-Loader:I VrRuntimeService:W \
        AndroidRuntime:E DEBUG:I libc:F RustStdoutStderr:V '*:S'
}

case "$cmd" in
    all)       cargo_build; fetch_model; gradle_build; do_install; do_launch; do_log ;;
    build)     cargo_build; fetch_model; gradle_build ;;
    lint)      do_lint ;;
    install)   do_install ;;
    launch)    do_launch ;;
    stop)      do_stop ;;
    log)       do_log ;;
    uninstall) adb uninstall "$PACKAGE" ;;
    release)   require_release_key; cargo_build; fetch_model; gradle_release ;;
    verify-release)  do_verify_release ;;
    install-release) do_install_release ;;
    *) echo "unknown command: $cmd" >&2; exit 2 ;;
esac
