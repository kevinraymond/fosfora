#!/usr/bin/env bash
# Fosfora VR dev loop for a Quest over adb: build the Rust cdylib with cargo-ndk,
# package it with the Gradle project in android/, install, launch and follow the
# filtered logcat. See docs/xr/BUILD_PLAN.md (S1).
#
#   scripts/xr/run.sh [all|build|lint|install|launch|stop|log|uninstall] [--debug]
#
#   all      build + install + launch + log (default)
#   build    cargo ndk (release unless --debug) + gradle assembleDebug
#   lint     cargo clippy for the Android target with -D warnings
#   install  adb install -r the debug APK
#   launch   adb shell am start the NativeActivity
#   stop     force-stop the app
#   log      adb logcat filtered to this app, the runtime and crashes
#
# Env: ANDROID_HOME (default ~/Android/Sdk), ANDROID_NDK_HOME (default: newest
# NDK under $ANDROID_HOME/ndk), ANDROID_SERIAL to pick a device.
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
LOG_TAG=fosfora_xr

cmd="${1:-all}"
profile=--release
for arg in "$@"; do
    case "$arg" in
        --debug) profile="" ;;
    esac
done

cargo_build() {
    echo "== cargo ndk ($ABI, api $MIN_SDK, ${profile:-dev})"
    # shellcheck disable=SC2086
    cargo ndk -t "$ABI" --platform "$MIN_SDK" -o "$JNI_DIR" build -p fosfora-xr $profile
}

gradle_build() {
    echo "== gradle assembleDebug"
    (cd android && ./gradlew --quiet assembleDebug)
    ls -la "$APK"
}

do_lint() {
    echo "== clippy ($ABI)"
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
    all)       cargo_build; gradle_build; do_install; do_launch; do_log ;;
    build)     cargo_build; gradle_build ;;
    lint)      do_lint ;;
    install)   do_install ;;
    launch)    do_launch ;;
    stop)      do_stop ;;
    log)       do_log ;;
    uninstall) adb uninstall "$PACKAGE" ;;
    *) echo "unknown command: $cmd" >&2; exit 2 ;;
esac
