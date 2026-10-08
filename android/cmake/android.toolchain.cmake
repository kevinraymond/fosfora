# Wrapper around the NDK's cmake toolchain file for the whisper.cpp build
# inside whisper-rs-sys (board #3751). whisper-rs-sys forwards only CMAKE_*,
# GGML_* and WHISPER_* environment variables to cmake as defines, so
# ANDROID_ABI and ANDROID_PLATFORM set as plain environment variables never
# reach the NDK file; this wrapper sets them and includes it.
# scripts/xr/run.sh points CMAKE_TOOLCHAIN_FILE here and exports
# ANDROID_NDK_HOME. The file keeps the NDK's name, android.toolchain.cmake,
# because the `cmake` crate recognizes an Android toolchain by that name.
set(ANDROID_ABI arm64-v8a CACHE STRING "")
set(ANDROID_PLATFORM android-29 CACHE STRING "")
include($ENV{ANDROID_NDK_HOME}/build/cmake/android.toolchain.cmake)
