# Release builds

How the Fosfora XR APK is built for a Meta release channel: the signing key,
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
| `XR_RELEASE_KEYSTORE_PASS` | Store password. `run.sh release` asks for it at a terminal when unset; a non-interactive run must export it. |
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
scripts/xr/run.sh release          # cargo ndk --release + the voice models + the NOTICE check + gradlew assembleRelease
scripts/xr/run.sh verify-release   # apksigner, zipalign and aapt checks
```

`release` prints the APK path,
`android/app/build/outputs/apk/release/app-release.apk`. Without
`XR_RELEASE_KEYSTORE` it stops before building; without
`XR_RELEASE_KEYSTORE_PASS` it asks for the password at the terminal (no echo,
kept in that process only), and a direct `./gradlew assembleRelease` stops at its first task
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

## The NOTICE (board #4198)

Every APK, debug and release, carries a `NOTICE`: what the app ships and
the license of each part. It is `assets/xr/NOTICE` in the repository,
which `scripts/xr/notice.py` makes and which is committed. The Android
build only copies it (`stageXrAssets` in `android/app/build.gradle.kts`),
so building it needs no network. It sits at the root of the staged
assets, beside `xr_manifest.txt`: `assets/assets/NOTICE` in the APK
(`unzip -l app-debug.apk | grep -i notice`). Like the other staged assets
it is listed in the manifest and unpacked to `<internal data>/assets/NOTICE`
at first run.

What it holds, in order:

- The parts besides the Rust crates, from
  `assets/xr/licenses/components.txt` (by hand): Fosfora, whisper.cpp, the
  Whisper base.en weights, the voice model and Ettin, ONNX Runtime, the
  OpenXR loader, the NDK's libc++, the bundled tracks and the "Ember Glow"
  test clip, each with its license, its copyright line where its license
  asks for one, and its source. A new bundled file or library gets an
  entry there.
- The Rust crates linked into `libfosfora_xr.so` for the headset, with
  each crate's SPDX license from its manifest. The list is `cargo tree -p
  fosfora-xr --target aarch64-linux-android -e normal,no-proc-macro`: the
  features `cargo ndk` builds the crate with, without build scripts, proc
  macros and dev dependencies, none of which is linked.
- The copyright lines in the license files of every crate that may be
  used under MIT.
- The license files, verbatim, of the crates whose terms MIT or
  Apache-2.0 alone do not cover (MPL-2.0, ISC, BSD-3-Clause, Zlib,
  CDLA-Permissive-2.0, Unicode-3.0, and the OFL and Ubuntu font licenses
  of egui's fonts), each distinct text once, and any crate's NOTICE file.
- The MIT text and the Apache License 2.0, once each.

```sh
scripts/xr/notice.py           # write assets/xr/NOTICE
scripts/xr/notice.py --check   # exit 1 when it is stale
```

Both cargo calls run `--locked --offline` on the sources cargo already
has, so run it after a build. `run.sh build`, `run.sh release` and
`run.sh all` run the check after the cargo build and stop when the file is
stale; regenerate it and commit it with the dependency change. The desktop
test `release::tests::the_notice_lists_crates_cargo_lock_has_and_every_part`
catches a listed crate bumped or dropped in `Cargo.lock` in `cargo test`.
It cannot see a crate added to the headset's library, which only cargo's
resolver knows; the check in `run.sh` does.

The wearer reads a short version on the voice card by saying "credits"
([VOICE_DESIGN.md](VOICE_DESIGN.md), "Credits"). The hand menu has no
Credits cell: every cell and half is taken, and a new one would resize or
move an existing one.

## What the release build shows

The debug and release APKs carry the same native library, so the app
decides at launch from the APK itself. It reads `ApplicationInfo.flags`
through JNI: `FLAG_DEBUGGABLE` set means the debug APK and the developer
surface, as before; clear means the release surface. If the flags can't be
read, the app shows the release surface. The launch logs one line, for
example `build: release APK · release surface`.

`debug.fosfora.devmenu 0|1` forces a surface on either APK, read at
launch: `0` the release surface, `1` the developer's. Unset, the APK
decides. This is how the release look is checked on the debug APK:

```sh
adb shell setprop debug.fosfora.devmenu 0   # release surface on the debug APK
scripts/xr/run.sh launch
adb shell setprop debug.fosfora.devmenu ""  # back to the APK's own
```

On the release surface:

- **The hand menu** has no Debug panel row, no Pitcher toggle and no "All:
  none" cell. The Pitcher and Debug panel row is gone and the rows under it
  move up, so the menu is six rows instead of seven (nine with Edit room
  on). The Limit flashing row, the wearer's photosensitive setting, is on
  both surfaces (board #5534). The edge bars' control takes the whole cell
  "All: none" shared. The
  debug panel stays off whatever `hand_menu.json` saved, the
  `debug.fosfora.hud` knob is ignored, and the saved flag is left in the
  file for the next developer launch. The voice commands for the hidden
  rows ("pitcher on", "all none") change nothing and the label says
  "That isn't available".
- **The log** leaves out what the wearer said. Voice transcripts, the
  grammar's and the agent's readings of them, and the agent's replies go
  to the debug level, which the developer surface logs and the release one
  doesn't. Timings and errors stay at info.
- **Nothing is written under `debug/`.** The located room is saved to
  `debug/anchors.json` only when `debug.fosfora.anchors` is `save` or
  `replay`.

On both surfaces the scan label reads "Loading your room" once the room is
found, instead of "Room: N surfaces"; the count is in the log's
`room scan: Found(N)` line. When the session ends on an error the app can't
recover from, the wearer sees "Fosfora stopped: <reason>" in front of them
for 4 s before the app exits (the session was lost, out of memory, a
headset error, an app error). The app keeps drawing frames with only the
label. When the runtime has already lost the session (`LOSS_PENDING`, an
instance loss) or the next frame fails, nothing can be drawn: the app logs
`fatal label: ... could not be drawn` or `... not shown` and exits as
before. An error before the session starts has no label either.

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

- The package id is `dev.fosfora.xr` (`build.gradle.kts`), on the project's
  own domain. It cannot change once uploaded.
- Create the real keystore and back it up (above); its certificate becomes
  the app's for good.
- Bump `versionCode` past any build already uploaded.
