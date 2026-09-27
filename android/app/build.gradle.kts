plugins {
    id("com.android.application")
}

android {
    // Placeholder package id (board #3205): confirm with Kevin before the first
    // dashboard upload; it cannot change afterwards.
    namespace = "dev.fosfora.xr"
    compileSdk = 34

    defaultConfig {
        applicationId = "dev.fosfora.xr"
        // Meta's documented values for Quest apps: minSdk 29, targetSdk 32.
        minSdk = 29
        targetSdk = 32
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            // Quest 3 and the glasses are arm64 only; a second ABI would only
            // double the APK.
            abiFilters += listOf("arm64-v8a")
        }
    }

    // CI signs debug APKs with one stable key (xr-apk.yml decodes the
    // XR_DEBUG_KEYSTORE_B64 secret) so each build installs over the last with
    // `adb install -r`. With XR_DEBUG_KEYSTORE unset, Gradle's default
    // ~/.android/debug.keystore applies as before.
    System.getenv("XR_DEBUG_KEYSTORE")?.let { keystore ->
        val pass = System.getenv("XR_DEBUG_KEYSTORE_PASS")
            ?: error("XR_DEBUG_KEYSTORE is set but XR_DEBUG_KEYSTORE_PASS is not")
        signingConfigs.getByName("debug") {
            storeFile = file(keystore)
            storePassword = pass
            keyAlias = System.getenv("XR_DEBUG_KEY_ALIAS") ?: "androiddebugkey"
            keyPassword = pass
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    packaging {
        jniLibs {
            // Keep the Rust symbols: readable native crash backtraces, and no
            // NDK lookup by Gradle for its strip step.
            keepDebugSymbols += "**/*.so"
        }
    }
}

dependencies {
    // Khronos OpenXR loader (Apache-2.0). The AAR carries
    // jni/arm64-v8a/libopenxr_loader.so, which Gradle packages into the APK;
    // the openxr crate dlopens it at startup. Source: Maven Central,
    // https://repo1.maven.org/maven2/org/khronos/openxr/openxr_loader_for_android/1.1.63/
    implementation("org.khronos.openxr:openxr_loader_for_android:1.1.63")
}
