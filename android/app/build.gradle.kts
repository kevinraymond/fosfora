import java.security.MessageDigest

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

    // The speech model (board #3751) is stored, not deflated: it barely
    // compresses, and a stored asset streams out of the APK without
    // inflating 148 MB at its first launch.
    androidResources {
        noCompress += "bin"
    }

    packaging {
        jniLibs {
            // libc++_shared.so (whisper.cpp's C++ runtime, copied in by
            // scripts/xr/run.sh from the NDK) is packaged from jniLibs/
            // with the cdylib and stripped of nothing, like every .so here.
            // Keep the Rust symbols: readable native crash backtraces, and no
            // NDK lookup by Gradle for its strip step.
            keepDebugSymbols += "**/*.so"
        }
    }
}

// Fosfora assets the XR app needs, staged out of the repo's assets/ tree into
// the APK's assets/ with a manifest (stamp + file list) the app uses to unpack
// them into internal storage on first run or when the stamp changes
// (docs/xr/XR_DESIGN.md, "Assets on Android"). The NDK asset API cannot list
// subdirectories, hence the manifest. Kept to what the effects need: shaders,
// effect definitions, the XR scenes, the S6 test track and the voice path's
// speech model; no images or fonts.
val xrAssetsDir = layout.buildDirectory.dir("generated/xrassets")

val stageXrAssets by tasks.registering(Sync::class) {
    from(rootProject.file("../assets")) {
        include("effects/**", "shaders/**")
    }
    // XR-only effect variants (spike profiling, later XR-tuned effects) join
    // the shared effects so the same loader finds them.
    from(rootProject.file("../assets/xr/effects")) {
        into("effects")
    }
    // XR-only sims keep their repo path, so a preset's
    // "compute_shader": "../xr/shaders/<name>.wgsl" (resolved under shaders/)
    // finds them in the APK and in a desktop checkout alike.
    from(rootProject.file("../assets/xr/shaders")) {
        into("xr/shaders")
    }
    // The S6 test track (CC0; assets/xr/audio/LICENSE.md).
    from(rootProject.file("../assets/xr/audio")) {
        include("*.ogg")
        into("audio")
    }
    // The voice path's speech model (board #3751; MIT, fetched with its
    // checksum by scripts/xr/fetch-model.sh, never committed:
    // assets/xr/models/LICENSE.md). Installed to <internal data>/assets/
    // xr/models/ with the rest.
    from(rootProject.file("../assets/xr/models")) {
        include("*.bin")
        into("xr/models")
    }
    into(xrAssetsDir.map { it.dir("assets") })
    doLast {
        val root = xrAssetsDir.get().dir("assets").asFile
        val files = root.walkTopDown().filter { it.isFile && it.name != "xr_manifest.txt" }
            .map { it.relativeTo(root).path.replace(File.separatorChar, '/') }
            .sorted().toList()
        val digest = MessageDigest.getInstance("SHA-256")
        // Streamed: the speech model alone is 148 MB.
        val buf = ByteArray(1 shl 20)
        for (f in files) {
            digest.update(f.toByteArray())
            File(root, f).inputStream().use { input ->
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    digest.update(buf, 0, n)
                }
            }
        }
        val stamp = digest.digest().joinToString("") { "%02x".format(it) }
        File(root, "xr_manifest.txt").writeText((listOf(stamp) + files).joinToString("\n") + "\n")
    }
}

android.sourceSets.getByName("main").assets.srcDir(xrAssetsDir)
tasks.named("preBuild") { dependsOn(stageXrAssets) }

dependencies {
    // Khronos OpenXR loader (Apache-2.0). The AAR carries
    // jni/arm64-v8a/libopenxr_loader.so, which Gradle packages into the APK;
    // the openxr crate dlopens it at startup. Source: Maven Central,
    // https://repo1.maven.org/maven2/org/khronos/openxr/openxr_loader_for_android/1.1.63/
    implementation("org.khronos.openxr:openxr_loader_for_android:1.1.63")
}
