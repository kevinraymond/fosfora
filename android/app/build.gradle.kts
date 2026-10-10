import java.security.MessageDigest
import java.util.Properties

plugins {
    id("com.android.application")
}

// The version policy lives in android/version.properties (the rule is
// documented there); XR_VERSION_CODE / XR_VERSION_NAME override it, so CI can
// stamp a build without a commit. Debug and release share the numbers.
val versionProps = Properties().apply {
    rootProject.file("version.properties").inputStream().use { load(it) }
}
val xrVersionCode: Int = (System.getenv("XR_VERSION_CODE") ?: versionProps.getProperty("versionCode"))
    ?.toIntOrNull()
    ?: error("versionCode must be an integer (android/version.properties or XR_VERSION_CODE)")
val xrVersionName: String = System.getenv("XR_VERSION_NAME")
    ?: versionProps.getProperty("versionName")
    ?: error("versionName missing from android/version.properties")

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
        versionCode = xrVersionCode
        versionName = xrVersionName
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

    // The release key, from the environment like the debug key above
    // (scripts/xr/release-keystore.sh creates it; docs/xr/RELEASE.md). Meta
    // ties the app to this certificate at its first upload, so the keystore
    // lives outside the repo and never changes. Without XR_RELEASE_KEYSTORE
    // the release build stops at checkReleaseSigning (below) instead of
    // producing an unsigned APK.
    val releaseKeystore = System.getenv("XR_RELEASE_KEYSTORE")?.ifBlank { null }
    if (releaseKeystore != null) {
        val pass = System.getenv("XR_RELEASE_KEYSTORE_PASS")?.ifBlank { null }
            ?: error("XR_RELEASE_KEYSTORE is set but XR_RELEASE_KEYSTORE_PASS is not")
        signingConfigs.create("release") {
            storeFile = file(releaseKeystore)
            storePassword = pass
            // Blank counts as unset: CI passes an absent optional secret as "".
            keyAlias = System.getenv("XR_RELEASE_KEY_ALIAS")?.ifBlank { null } ?: "fosfora-xr"
            keyPassword = System.getenv("XR_RELEASE_KEY_PASS")?.ifBlank { null } ?: pass
            // minSdk 29 alone would drop the v1 (JAR) signature; Meta's
            // signing docs have asked for v1 and v2 both, and v1 costs
            // only the META-INF entries.
            enableV1Signing = true
            enableV2Signing = true
        }
    }

    buildTypes {
        release {
            // No Java or Kotlin code to shrink (hasCode="false").
            isMinifyEnabled = false
            isShrinkResources = false
            // Meta's store rejects a debuggable APK; the manifest never sets
            // android:debuggable, so this is the only switch.
            isDebuggable = false
            if (releaseKeystore != null) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    // lintVitalRelease applies Google Play's target-API floor (33 and up) as a
    // fatal error. The app ships on Meta's store, whose public docs specify
    // targetSdk 32 for Quest apps; every other release lint check stays on.
    lint {
        disable += "ExpiredTargetSdkVersion"
    }

    // The speech model (board #3751) is stored, not deflated: it barely
    // compresses, and a stored asset streams out of the APK without
    // inflating 148 MB at its first launch. The decision model (V5, .onnx)
    // likewise.
    androidResources {
        noCompress += listOf("bin", "onnx")
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
// models; no images or fonts.
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
    // The voice path's speech model (board #3751; MIT) and, V5, the
    // on-device provider's decision model, its tokenizer files (fetched
    // with their checksums by scripts/xr/fetch-model.sh from
    // assets/xr/models/MODELS.txt, never committed) and its spec
    // (committed); sources in assets/xr/models/LICENSE.md. Installed to
    // <internal data>/assets/xr/models/ with the rest. A file not fetched
    // is simply absent, and the app says so in its log.
    from(rootProject.file("../assets/xr/models")) {
        include("*.bin", "*.onnx", "s1-17m-*.json")
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

// The release build fails first thing, naming the variables, when the release
// key is not configured: an unsigned release APK is never what was wanted.
val checkReleaseSigning by tasks.registering {
    val configured = !System.getenv("XR_RELEASE_KEYSTORE").isNullOrBlank()
    doLast {
        if (!configured) {
            throw GradleException(
                "The release build needs the release key: set XR_RELEASE_KEYSTORE " +
                    "(path to the PKCS12 keystore) and XR_RELEASE_KEYSTORE_PASS, and " +
                    "optionally XR_RELEASE_KEY_ALIAS (default fosfora-xr) and " +
                    "XR_RELEASE_KEY_PASS (default: the store password). " +
                    "See scripts/xr/release-keystore.sh and docs/xr/RELEASE.md.",
            )
        }
    }
}
tasks.matching { it.name == "preReleaseBuild" }.configureEach { dependsOn(checkReleaseSigning) }

// ONNX Runtime for the voice path's on-device provider (board #3751, V5):
// the official AAR from Maven Central (MIT),
// https://repo1.maven.org/maven2/com/microsoft/onnxruntime/onnxruntime-android/1.28.0/
// Only its jni/arm64-v8a/libonnxruntime.so reaches the APK's
// lib/arm64-v8a/, which the `ort` crate dlopens by name: the AAR's Java API
// (classes.jar) and its JNI binding (libonnxruntime4j_jni.so) serve Java
// callers, and this app has no code (hasCode="false").
val onnxRuntime: Configuration by configurations.creating
val onnxRuntimeLibs = layout.buildDirectory.dir("generated/onnxruntime")

val extractOnnxRuntime by tasks.registering(Sync::class) {
    from(provider { zipTree(onnxRuntime.singleFile) }) {
        include("jni/arm64-v8a/libonnxruntime.so")
        eachFile { path = path.removePrefix("jni/") }
        includeEmptyDirs = false
    }
    into(onnxRuntimeLibs)
}

android.sourceSets.getByName("main").jniLibs.srcDir(onnxRuntimeLibs)
tasks.named("preBuild") { dependsOn(extractOnnxRuntime) }

dependencies {
    // Khronos OpenXR loader (Apache-2.0). The AAR carries
    // jni/arm64-v8a/libopenxr_loader.so, which Gradle packages into the APK;
    // the openxr crate dlopens it at startup. Source: Maven Central,
    // https://repo1.maven.org/maven2/org/khronos/openxr/openxr_loader_for_android/1.1.63/
    implementation("org.khronos.openxr:openxr_loader_for_android:1.1.63")
    onnxRuntime("com.microsoft.onnxruntime:onnxruntime-android:1.28.0@aar")
}
