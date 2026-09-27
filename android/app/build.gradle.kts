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
