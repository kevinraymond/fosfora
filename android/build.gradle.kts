// Minimal Gradle project that packages the Rust cdylib built by cargo-ndk
// (see scripts/xr/run.sh). No Java or Kotlin sources: the activity is
// android.app.NativeActivity and the manifest declares hasCode="false".
plugins {
    id("com.android.application") version "8.13.2" apply false
}
