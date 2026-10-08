//! Runtime permissions (board #3264): the app asks for what it needs
//! instead of relying on `adb shell pm grant`.
//!
//! Two of the manifest's permissions are runtime ones: `USE_SCENE` (without
//! it the scene query returns nothing and the room is the stage floor) and
//! `RECORD_AUDIO` (without it the microphones do not open and the analysis
//! runs on the synthetic groove, and the voice path's window stays closed,
//! board #3751). At launch the app checks both, asks for
//! the missing ones in one `Activity.requestPermissions` call (the OS shows
//! one dialog per permission it has not decided) and carries on; the result
//! callback never reaches native code, so the frame loop polls the grants
//! once a second for [`WATCH_S`] and picks a grant up without a relaunch
//! (`app.rs`).
//!
//! The decisions are plain functions that build and test on the desktop
//! ([`to_ask`], [`Watch`]); on Android [`Permissions`] is a thin JNI layer
//! over the `Activity` that `android-activity` wraps (`NativeActivity`
//! extends it; `requestPermissions` and `checkSelfPermission` are API 23,
//! `targetSdk` is 32), with no Java source.

/// The scene anchors (`XR_FB_scene`): the room.
pub const USE_SCENE: &str = "com.oculus.permission.USE_SCENE";
/// The microphones.
pub const RECORD_AUDIO: &str = "android.permission.RECORD_AUDIO";

/// How long after the ask the frame loop keeps polling for a grant (s).
pub const WATCH_S: f32 = 120.0;

/// The `debug.fosfora.audio` sources that open the microphones: the core's
/// capture (`mic`), the XR-owned cpal stream (`micxr`) and raw AAudio
/// (`aaudio`, and `loop`, which plays the clip and listens to it).
pub const MIC_SOURCES: [&str; 4] = ["mic", "micxr", "aaudio", "loop"];

/// Whether `audio_source` (the `debug.fosfora.audio` knob) needs the
/// microphones.
pub fn wants_mic(audio_source: &str) -> bool {
    MIC_SOURCES.contains(&audio_source)
}

/// What to ask for, given the audio source, whether the voice path is on,
/// and what is granted (`scene`: `USE_SCENE`, `mic`: `RECORD_AUDIO`):
/// `USE_SCENE` whenever it is missing (the room is the product),
/// `RECORD_AUDIO` only when it is missing and the source or the voice path
/// needs the microphones (with voice on, even on the synthetic groove).
pub fn to_ask(audio_source: &str, voice: bool, scene: bool, mic: bool) -> Vec<&'static str> {
    let mut ask = Vec::new();
    if !scene {
        ask.push(USE_SCENE);
    }
    if !mic && (voice || wants_mic(audio_source)) {
        ask.push(RECORD_AUDIO);
    }
    ask
}

/// The name the log uses: the last dotted part ("USE_SCENE").
pub fn short(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// What a poll found.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// `name` turned granted, `after_s` seconds after the ask.
    Granted { name: &'static str, after_s: f32 },
    /// [`WATCH_S`] passed with these still missing: polling stops.
    GaveUp(Vec<&'static str>),
}

/// The permissions still missing after the ask, polled until each is
/// granted or [`WATCH_S`] passes.
#[derive(Debug, Clone, Default)]
pub struct Watch {
    missing: Vec<&'static str>,
    gave_up: bool,
}

impl Watch {
    /// Watch `missing` (every one the app needs and lacks, asked for or
    /// not).
    pub fn new(missing: Vec<&'static str>) -> Self {
        Self {
            missing,
            gave_up: false,
        }
    }

    /// Whether polls still have work: something missing, time left.
    pub fn active(&self) -> bool {
        !self.gave_up && !self.missing.is_empty()
    }

    /// Whether the watch is still waiting for `name`.
    pub fn waiting_for(&self, name: &str) -> bool {
        self.active() && self.missing.contains(&name)
    }

    /// One poll, `elapsed_s` after the ask; `granted` checks a permission.
    /// Returns the grants it found, then [`Change::GaveUp`] once
    /// [`WATCH_S`] has passed with anything still missing. Nothing once
    /// inactive.
    pub fn poll(&mut self, elapsed_s: f32, mut granted: impl FnMut(&str) -> bool) -> Vec<Change> {
        if !self.active() {
            return Vec::new();
        }
        let mut changes = Vec::new();
        self.missing.retain(|&name| {
            let now = granted(name);
            if now {
                changes.push(Change::Granted {
                    name,
                    after_s: elapsed_s,
                });
            }
            !now
        });
        if !self.missing.is_empty() && elapsed_s >= WATCH_S {
            self.gave_up = true;
            changes.push(Change::GaveUp(self.missing.clone()));
        }
        changes
    }
}

#[cfg(target_os = "android")]
pub use device::Permissions;

#[cfg(target_os = "android")]
mod device {
    use android_activity::AndroidApp;
    use anyhow::{Context, Result};
    use jni::objects::{GlobalRef, JObject, JString, JValue};
    use jni::{JNIEnv, JavaVM};

    /// `requestPermissions`' request code; the result callback is never
    /// read (the frame loop polls), so any value does.
    const REQUEST_CODE: i32 = 3264;
    /// `PackageManager.PERMISSION_GRANTED`.
    const PERMISSION_GRANTED: i32 = 0;
    /// Local references a call makes at most (a string per permission and
    /// the array), freed when it returns: the `android_main` thread never
    /// returns to Java, so nothing else would free them.
    const LOCAL_FRAME: i32 = 16;

    /// The JVM and a global reference to the app's `Activity`.
    pub struct Permissions {
        vm: JavaVM,
        activity: GlobalRef,
    }

    impl Permissions {
        pub fn new(app: &AndroidApp) -> Result<Self> {
            // SAFETY: `vm_as_ptr` is the process's `JavaVM*` from the
            // `ANativeActivity`, valid for the life of the process;
            // `JavaVM` only wraps the pointer (it never destroys the VM).
            let vm =
                unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.context("JavaVM::from_raw")?;
            let activity = {
                let env = vm.attach_current_thread().context("attaching to the VM")?;
                // SAFETY: `activity_as_ptr` is the glue's JNI global
                // reference to the `Activity` instance, live while the
                // activity is; `JObject` borrows it without deleting it
                // (no `Drop`), and the global reference taken from it here
                // is this struct's own.
                let raw = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
                env.new_global_ref(raw)
                    .context("NewGlobalRef on the activity")?
            };
            Ok(Self { vm, activity })
        }

        /// `Activity.checkSelfPermission(name) == PERMISSION_GRANTED`.
        pub fn granted(&self, name: &str) -> Result<bool> {
            let mut env = self
                .vm
                .attach_current_thread()
                .context("attaching to the VM")?;
            let code = call(&mut env, |env| {
                let jname = env.new_string(name)?;
                env.call_method(
                    self.activity.as_obj(),
                    "checkSelfPermission",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&jname)],
                )?
                .i()
            })
            .with_context(|| format!("Activity.checkSelfPermission({name})"))?;
            Ok(code == PERMISSION_GRANTED)
        }

        /// `Activity.requestPermissions(names, REQUEST_CODE)`: one dialog
        /// per permission the OS has not decided; returns at once.
        pub fn request(&self, names: &[&str]) -> Result<()> {
            if names.is_empty() {
                return Ok(());
            }
            let len = i32::try_from(names.len()).context("too many permissions")?;
            let mut env = self
                .vm
                .attach_current_thread()
                .context("attaching to the VM")?;
            call(&mut env, |env| {
                let array = env.new_object_array(len, "java/lang/String", JObject::null())?;
                for (i, name) in (0..len).zip(names) {
                    let jname = env.new_string(name)?;
                    env.set_object_array_element(&array, i, jname)?;
                }
                env.call_method(
                    self.activity.as_obj(),
                    "requestPermissions",
                    "([Ljava/lang/String;I)V",
                    &[JValue::Object(&array), JValue::Int(REQUEST_CODE)],
                )?;
                Ok(())
            })
            .with_context(|| format!("Activity.requestPermissions({names:?})"))
        }
    }

    impl Permissions {
        /// The app's native library directory
        /// (`Activity.getApplicationInfo().nativeLibraryDir`): where the
        /// voice path's on-device provider looks for `libonnxruntime.so`
        /// when loading it by name fails (board #3751, V5).
        pub fn native_library_dir(&self) -> Result<String> {
            let mut env = self
                .vm
                .attach_current_thread()
                .context("attaching to the VM")?;
            call(&mut env, |env| {
                let info = env
                    .call_method(
                        self.activity.as_obj(),
                        "getApplicationInfo",
                        "()Landroid/content/pm/ApplicationInfo;",
                        &[],
                    )?
                    .l()?;
                let dir = env
                    .get_field(&info, "nativeLibraryDir", "Ljava/lang/String;")?
                    .l()?;
                let dir = JString::from(dir);
                let dir: String = env.get_string(&dir)?.into();
                Ok(dir)
            })
            .context("ApplicationInfo.nativeLibraryDir")
        }
    }

    /// Run `f` in a local frame of its own and clear any Java exception it
    /// left pending (logged to logcat first), so the next JNI call on this
    /// thread starts clean.
    fn call<T>(
        env: &mut JNIEnv<'_>,
        f: impl FnOnce(&mut JNIEnv<'_>) -> jni::errors::Result<T>,
    ) -> Result<T> {
        let result = env.with_local_frame(LOCAL_FRAME, |env| {
            let result = f(env);
            if result.is_err() && env.exception_check().unwrap_or(false) {
                // Best effort: the error below carries the failure either way.
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            result
        });
        Ok(result?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_synth_asks_for_the_room_only() {
        assert_eq!(to_ask("synth", false, false, false), vec![USE_SCENE]);
        assert_eq!(to_ask("file", false, false, false), vec![USE_SCENE]);
        assert!(to_ask("synth", false, true, false).is_empty());
    }

    #[test]
    fn voice_asks_for_the_mic_on_any_source() {
        for source in ["synth", "file", "mic", "aaudio"] {
            assert_eq!(
                to_ask(source, true, false, false),
                vec![USE_SCENE, RECORD_AUDIO],
                "{source}"
            );
            assert_eq!(
                to_ask(source, true, true, false),
                vec![RECORD_AUDIO],
                "{source}"
            );
            assert!(to_ask(source, true, true, true).is_empty(), "{source}");
        }
    }

    #[test]
    fn a_mic_source_asks_for_both_when_both_are_missing() {
        for source in ["mic", "micxr", "aaudio", "loop"] {
            assert_eq!(
                to_ask(source, false, false, false),
                vec![USE_SCENE, RECORD_AUDIO],
                "{source}"
            );
            assert_eq!(
                to_ask(source, false, true, false),
                vec![RECORD_AUDIO],
                "{source}"
            );
            assert_eq!(
                to_ask(source, false, false, true),
                vec![USE_SCENE],
                "{source}"
            );
        }
    }

    #[test]
    fn nothing_missing_asks_for_nothing() {
        for source in ["synth", "mic", "micxr", "aaudio", "loop", "file"] {
            assert!(to_ask(source, false, true, true).is_empty(), "{source}");
        }
    }

    #[test]
    fn the_log_names_are_the_last_part() {
        assert_eq!(short(USE_SCENE), "USE_SCENE");
        assert_eq!(short(RECORD_AUDIO), "RECORD_AUDIO");
        assert_eq!(short("plain"), "plain");
    }

    #[test]
    fn the_watch_picks_up_each_grant_once_and_stops() {
        let mut w = Watch::new(vec![USE_SCENE, RECORD_AUDIO]);
        assert!(w.active());
        assert!(w.waiting_for(USE_SCENE));
        assert!(w.poll(1.0, |_| false).is_empty());
        // The room granted at 7 s: reported once, the mic still watched.
        assert_eq!(
            w.poll(7.0, |n| n == USE_SCENE),
            vec![Change::Granted {
                name: USE_SCENE,
                after_s: 7.0
            }]
        );
        assert!(!w.waiting_for(USE_SCENE));
        assert!(w.waiting_for(RECORD_AUDIO));
        assert!(w.poll(8.0, |n| n == USE_SCENE).is_empty());
        // The mic too: nothing left, the polls stop.
        assert_eq!(
            w.poll(9.0, |_| true),
            vec![Change::Granted {
                name: RECORD_AUDIO,
                after_s: 9.0
            }]
        );
        assert!(!w.active());
        assert!(w.poll(10.0, |_| true).is_empty());
    }

    #[test]
    fn the_watch_gives_up_after_its_time_and_a_denial_keeps_the_fallback() {
        let mut w = Watch::new(vec![USE_SCENE, RECORD_AUDIO]);
        assert!(w.poll(WATCH_S - 1.0, |_| false).is_empty());
        // A grant in the last poll still counts; the rest is given up.
        assert_eq!(
            w.poll(WATCH_S, |n| n == RECORD_AUDIO),
            vec![
                Change::Granted {
                    name: RECORD_AUDIO,
                    after_s: WATCH_S
                },
                Change::GaveUp(vec![USE_SCENE]),
            ]
        );
        assert!(!w.active());
        assert!(!w.waiting_for(USE_SCENE));
        // Later grants are not polled for.
        assert!(w.poll(WATCH_S + 5.0, |_| true).is_empty());
    }

    #[test]
    fn nothing_missing_watches_nothing() {
        let mut w = Watch::new(Vec::new());
        assert!(!w.active());
        assert!(w.poll(WATCH_S + 1.0, |_| false).is_empty());
        assert!(!Watch::default().active());
    }
}
