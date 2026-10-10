# Release builds

How the Fosfora VR APK is built for a Meta release channel: the signing key,
the version policy, the build and its checks, what Meta's store checks in the
manifest, and the upload. Day-to-day development stays on the debug APK
(`scripts/xr/run.sh build`, [BUILD_PLAN.md](BUILD_PLAN.md) S1).

## The release keystore

The release APK is signed with one key, kept in a PKCS12 keystore:

```sh
scripts/xr/release-keystore.sh                       # print the keytool command
scripts/xr/release-keystore.sh --create ~/keys/fosfora-xr-release.p12
```

`--create` runs `keytool -genkeypair` with alias `fosfora-xr`, RSA 4096,
validity 10,000 days, and asks for the certificate's DN and the store
password (the key shares it). It refuses a path inside the repo or an
existing file, and never takes a password on its command line. It then
prints the variables the build reads:

| Variable | Meaning |
|---|---|
| `XR_RELEASE_KEYSTORE` | Path to the keystore. Required. |
| `XR_RELEASE_KEYSTORE_PASS` | Store password. Required. |
| `XR_RELEASE_KEY_ALIAS` | Key alias; default `fosfora-xr`. |
| `XR_RELEASE_KEY_PASS` | Key password; default the store password. |

**The never-change rule.** Meta ties the app to the certificate of its first
uploaded build: every later build must be signed with the same key, and a
build signed with another is refused at upload. A lost key means a new app,
not an update. So:

- Keep the keystore outside the repo. The repo is public; `.gitignore`
  ignores `*.p12`, `*.jks` and `*.keystore` at the root and under `android/`
  as a backstop only.
- Back it up, with its password, somewhere that outlives this machine (a
  password manager holds both).
- Never regenerate or replace it once a build has been uploaded.

**Local only.** Release builds are made on a developer's machine with
the keystore in its shell environment. The key is never stored as a CI
secret, CI never builds a release APK, and no release APK is ever
uploaded as a workflow artifact or committed: the only copies are the
local build output and the build uploaded to Meta.

## Version policy

`versionCode` and `versionName` live in one place,
[`android/version.properties`](../../android/version.properties), read by
`android/app/build.gradle.kts` for the debug and release builds alike:

- **`versionCode`**: every upload to a Meta release channel needs a strictly
  higher `versionCode` than any build uploaded before. Bump it before each
  upload; never reuse or lower it.
- **`versionName`**: what people see in the store and the headset's app info.

`XR_VERSION_CODE` and `XR_VERSION_NAME` in the environment override the file,
so CI can stamp a build without a commit.

## Build and verify

```sh
scripts/xr/run.sh release          # cargo ndk --release + the voice models + gradlew assembleRelease
scripts/xr/run.sh verify-release   # apksigner, zipalign and aapt checks
```

`release` prints the APK path,
`android/app/build/outputs/apk/release/app-release.apk`. Without
`XR_RELEASE_KEYSTORE` and `XR_RELEASE_KEYSTORE_PASS` it stops before building,
and a direct `./gradlew assembleRelease` stops at its first task
(`checkReleaseSigning`) with a message naming the variables: an unsigned
release APK is never produced.

The release build type: not debuggable, no minification or resource
shrinking (the app has no code, `hasCode="false"`), native symbols kept so
crash backtraces stay readable, signed with the v1 (JAR) and v2 schemes.
Gradle's release lint would refuse targetSdk 32 as below Google Play's
floor; that one check is off, since the app ships on Meta's store.

`verify-release` uses the newest `$ANDROID_HOME/build-tools/` and fails on an
APK that is unsigned, signed with an Android debug certificate, not 4-byte
aligned, or debuggable. It prints:

- `apksigner verify --verbose --print-certs`: the scheme versions (v1 and v2
  true) and the certificate's DN and SHA-256 digest. The digest is the app's
  identity; it must be the same on every upload.
- `zipalign -c -v 4`: the alignment check.
- `aapt dump badging`: package, `versionCode`, `versionName`, the SDK levels,
  `native-code: 'arm64-v8a'`, and `debuggable: no`.

`scripts/xr/run.sh install-release` installs the release APK over adb. Its
signature differs from the debug key's, so Android refuses it over an
installed debug build (`INSTALL_FAILED_UPDATE_INCOMPATIBLE`):
`scripts/xr/run.sh uninstall` first, which deletes the app's data on the
headset, and the same going back to debug.

## What Meta's store checks in the manifest

From Meta's public "Android Manifest Settings" page and the
VRC.Quest.Packaging checks; the live manifest is
`android/app/src/main/AndroidManifest.xml` ([XR_DESIGN.md](XR_DESIGN.md),
"Android manifest essentials").

| Requirement | Where | State |
|---|---|---|
| `<uses-feature android:name="android.hardware.vr.headtracking" android:required="true" android:version="1"/>` (also what lets the APK be v2 signed for Quest) | manifest | present |
| Launcher intent filter with category `com.oculus.intent.category.VR` | manifest | present |
| `com.oculus.supportedDevices` meta-data, pipe-separated device ids | manifest | present, `quest3\|quest3s` |
| Not debuggable | `isDebuggable = false`; the manifest never sets `android:debuggable` | `verify-release` checks it |
| Native code arm64-v8a only | `abiFilters` in `build.gradle.kts` | `verify-release` prints it |
| `minSdkVersion` 29, `targetSdkVersion` 32 | `build.gradle.kts` | present |
| APK Signature Scheme v2 (v1-only is refused) | release signing config | v1 and v2 |
| `android:screenOrientation="landscape"` on the activity | manifest | present |

The page's sample also sets `android:installLocation="auto"` on the manifest
and `android:resizeableActivity="false"` on the activity, without calling
them required; neither is set here.

## Upload

With Meta's public Platform Command Line Utility (`ovr-platform-util`):

```sh
ovr-platform-util upload-quest-build \
  --app-id <app id> \
  --token <user token> \
  --apk android/app/build/outputs/apk/release/app-release.apk \
  --channel <channel> \
  --age-group <TEENS_AND_ADULTS|MIXED_AGES|CHILDREN> \
  --notes "<release notes>"
```

The app id is on the app's API tab in the developer dashboard; the token is a
user token from the dashboard (or `--app-secret <app secret>` instead). Without
`--age-group` the build lands as a draft. Keep the token out of shell history,
for example `--token "$(<password manager command>)"`.

Before the first upload:

- The package id, `dev.fosfora.xr`, is a placeholder (`build.gradle.kts`);
  confirm it, since it cannot change once uploaded.
- Create the real keystore and back it up (above); its certificate becomes
  the app's for good.
- Bump `versionCode` past any build already uploaded.
