use super::buffer::CircularBuffer;
use super::control::{
    AUTO_PRIOR_MAX_BPM, AUTO_PRIOR_MIN_BPM, MAX_LEAD_PERIOD_FRAC, MIN_PRIOR_SIGMA,
};
use super::kalman::KalmanBpm;
use super::scheduler::{ArbEvidence, BEAT_LEAD_SECS, PhaseArbiterCfg};
use super::tempo::{acf_at, comb_score};
use super::*;

fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() < eps
}
fn approx_eq_f64(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() < eps
}

// ---- CircularBuffer tests ----

#[test]
fn circular_buffer_new_empty() {
    let buf = CircularBuffer::new(10);
    assert_eq!(buf.len(), 0);
    assert!(buf.values().is_empty());
}

#[test]
fn circular_buffer_push_under_capacity() {
    let mut buf = CircularBuffer::new(5);
    buf.push(1.0);
    buf.push(2.0);
    buf.push(3.0);
    assert_eq!(buf.len(), 3);
    assert_eq!(buf.values(), vec![1.0, 2.0, 3.0]);
}

#[test]
fn circular_buffer_push_wrap_around() {
    let mut buf = CircularBuffer::new(3);
    buf.push(1.0);
    buf.push(2.0);
    buf.push(3.0);
    buf.push(4.0); // wraps, oldest (1.0) overwritten
    assert_eq!(buf.len(), 3);
    let vals = buf.values();
    assert_eq!(vals, vec![2.0, 3.0, 4.0]);
}

#[test]
fn circular_buffer_median_odd() {
    let mut buf = CircularBuffer::new(5);
    for v in [3.0, 1.0, 4.0, 1.0, 5.0] {
        buf.push(v);
    }
    // sorted: [1.0, 1.0, 3.0, 4.0, 5.0], median = 3.0
    assert!(approx_eq_f64(buf.median(), 3.0, 1e-10));
}

#[test]
fn circular_buffer_median_even() {
    let mut buf = CircularBuffer::new(4);
    for v in [1.0, 2.0, 3.0, 4.0] {
        buf.push(v);
    }
    // sorted: [1.0, 2.0, 3.0, 4.0], median = (2.0+3.0)/2 = 2.5
    assert!(approx_eq_f64(buf.median(), 2.5, 1e-10));
}

#[test]
fn circular_buffer_median_empty() {
    let mut buf = CircularBuffer::new(5);
    assert_eq!(buf.median(), 0.0);
}

#[test]
fn circular_buffer_mad() {
    let mut buf = CircularBuffer::new(5);
    for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
        buf.push(v);
    }
    // median=3.0, deviations=[2,1,0,1,2], sorted=[0,1,1,2,2], mad=1.0
    assert!(approx_eq_f64(buf.mad(), 1.0, 1e-10));
}

#[test]
fn circular_buffer_max() {
    let mut buf = CircularBuffer::new(5);
    for v in [1.0, 5.0, 3.0] {
        buf.push(v);
    }
    assert!(approx_eq_f64(buf.max(), 5.0, 1e-10));
}

#[test]
fn circular_buffer_max_empty() {
    let buf = CircularBuffer::new(5);
    assert_eq!(buf.max(), 0.0);
}

#[test]
fn circular_buffer_mean() {
    let mut buf = CircularBuffer::new(5);
    for v in [2.0, 4.0, 6.0] {
        buf.push(v);
    }
    assert!(approx_eq_f64(buf.mean(), 4.0, 1e-10));
}

// ---- KalmanBpm tests ----

#[test]
fn kalman_first_measurement_returns_raw() {
    let mut k = KalmanBpm::new();
    let bpm = k.update(120.0, 0.5, false, false);
    assert!(approx_eq_f64(bpm, 120.0, 1e-6));
}

#[test]
fn kalman_stable_input_stays_near() {
    let mut k = KalmanBpm::new();
    k.update(120.0, 0.8, false, false);
    for _ in 0..50 {
        let bpm = k.update(120.0, 0.8, false, false);
        assert!((bpm - 120.0).abs() < 5.0, "got {}", bpm);
    }
}

/// Q2 replaced the snap-then-count-to-30 octave preprocessing with graded
/// evidence upstream (continuity weight in candidate scoring). At the
/// filter, brief octave noise is only *damped* now — the small gain keeps a
/// couple of wild frames from moving the estimate far — and the track
/// recovers as soon as sane measurements resume.
#[test]
fn kalman_brief_octave_noise_is_damped() {
    let mut k = KalmanBpm::new();
    k.update(120.0, 0.8, false, false);
    for _ in 0..10 {
        k.update(120.0, 0.8, false, false);
    }
    for _ in 0..3 {
        let bpm = k.update(240.0, 0.8, false, false);
        assert!(
            bpm < 160.0,
            "3 noisy frames must not reach the far octave, got {bpm}"
        );
    }
    let mut bpm = 0.0;
    for _ in 0..20 {
        bpm = k.update(120.0, 0.8, false, false);
    }
    assert!(
        (bpm - 120.0).abs() < 6.0,
        "must recover after noise, got {bpm}"
    );
}

/// A sustained, genuine change to the double tempo is followed promptly —
/// the leaky divergence counter hard-resets after ~15 divergent frames
/// (~1 s at the tempo cadence) instead of chasing through the filter gain.
#[test]
fn kalman_sustained_octave_change_is_followed() {
    let mut k = KalmanBpm::new();
    k.update(120.0, 0.8, false, false);
    for _ in 0..20 {
        k.update(240.0, 0.8, false, false);
    }
    let bpm = k.update(240.0, 0.8, false, false);
    assert!((bpm - 240.0).abs() < 12.0, "expected near 240, got {}", bpm);
}

#[test]
fn kalman_divergence_reset() {
    let mut k = KalmanBpm::new();
    k.update(120.0, 0.8, false, false);
    for _ in 0..5 {
        k.update(120.0, 0.8, false, false);
    }
    // Feed completely different BPM — should reset after 15 frames
    for _ in 0..20 {
        k.update(80.0, 0.8, false, false);
    }
    let bpm = k.update(80.0, 0.8, false, false);
    assert!((bpm - 80.0).abs() < 15.0, "expected near 80, got {}", bpm);
}

// ---- OnsetDetector tests ----

#[test]
fn onset_silence_gate() {
    let mut od = OnsetDetector::new(44100.0, 50, 400);
    let bass = vec![0.0; 2049]; // 4096-pt fft
    let mid = vec![0.0; 513]; // 1024-pt fft
    let high = vec![0.0; 257]; // 512-pt fft
    // Perceptual silence gate (A10): loud_silent = true → no onset.
    let (is_onset, strength, _) = od.process(&bass, &mid, &high, true);
    assert!(!is_onset);
    assert!(approx_eq(strength, 0.0, 1e-6));
}

#[test]
fn onset_sustained_silence() {
    let mut od = OnsetDetector::new(44100.0, 50, 400);
    let bass = vec![0.0; 2049];
    let mid = vec![0.0; 513];
    let high = vec![0.0; 257];
    for _ in 0..40 {
        od.process(&bass, &mid, &high, true);
    }
    assert!(od.is_sustained_silence());
}

#[test]
fn superflux_fires_on_broadband_onset_not_vibrato() {
    // Bands are built on first process; a broadband magnitude jump must produce flux,
    // while a partial merely sliding ±1 band (vibrato) must be suppressed by the freq
    // max filter.
    let mut od = OnsetDetector::new(44100.0, 50, 400);
    let bins = 2049;
    let (mid, high) = (vec![0.0; 513], vec![0.0; 257]);
    let quiet = vec![0.01f32; bins];
    // Warm up on a steady quiet spectrum (fills prev_log, no flux).
    for _ in 0..4 {
        od.process(&quiet, &mid, &high, false);
    }
    // Broadband jump → onset flux well above the quiet baseline.
    let mut loud = vec![0.01f32; bins];
    for m in loud.iter_mut().take(400).skip(4) {
        *m = 3.0;
    }
    let (_, _, onset_flux) = od.process(&loud, &mid, &high, false);

    // A single tone that slides up one FFT bin each frame (vibrato) should barely register.
    let mut vib = OnsetDetector::new(44100.0, 50, 400);
    let mut vibrato_flux = 0.0;
    for k in 0..6 {
        let mut s = vec![0.01f32; bins];
        s[40 + k] = 3.0; // partial drifts one bin/frame
        let (_, _, f) = vib.process(&s, &mid, &high, false);
        vibrato_flux = f;
    }
    assert!(
        onset_flux > vibrato_flux * 3.0,
        "broadband onset ({onset_flux}) should dominate vibrato flux ({vibrato_flux})"
    );
}

// ---- BeatScheduler tests ----

/// Q1 PLL: the old scheduler fired a beat on EVERY gated onset whenever
/// tempo confidence was low — the mechanical source of the 1.5-1.7×
/// over-firing. With no tempo hypothesis there is now no grid and no beat;
/// /onset still carries the transient (user-approved pre-lock behavior).
#[test]
fn scheduler_no_tempo_means_no_beats() {
    let mut bs = BeatScheduler::new();
    bs.update_tempo(0.0, 0.0, 0.0, false);
    for i in 0..8 {
        let (is_beat, _, _, _) = bs.process(true, 0.8, 1.0 + i as f64 * 0.3, false);
        assert!(!is_beat, "onset {i} must not fire without a tempo");
    }
}

#[test]
fn scheduler_silence_no_beat() {
    let mut bs = BeatScheduler::new();
    bs.update_tempo(120.0, 0.5, 0.8, false);
    let (is_beat, _, phase, _) = bs.process(false, 0.0, 1.0, true);
    assert!(!is_beat);
    assert!(approx_eq_f64(phase, 0.0, 1e-6));
}

#[test]
fn scheduler_phase_in_range() {
    let mut bs = BeatScheduler::new();
    bs.update_tempo(120.0, 0.5, 0.8, false);
    // Anchor the grid
    bs.process(true, 0.8, 1.0, false);
    // Advance time
    for i in 1..100 {
        let t = 1.0 + (i as f64) * 0.01;
        let (_, _, phase, _) = bs.process(false, 0.0, t, false);
        assert!((0.0..=1.0).contains(&phase), "phase={} at t={}", phase, t);
    }
}

/// Q1 PLL, locked freerun: beats fire AT grid instants (beat_time carries
/// the instant, not the hop that noticed it), exactly one per period.
#[test]
fn scheduler_locked_fires_on_grid() {
    let mut bs = BeatScheduler::new();
    bs.update_tempo(120.0, 0.5, 0.9, true);
    bs.process(true, 0.9, 1.0, false); // anchor
    let mut beats = Vec::new();
    let dt = 512.0 / 44100.0;
    let mut t: f64 = 1.0;
    while t < 9.0 {
        t += dt;
        // Supporting onsets exactly on the (true) beat grid at 120 BPM.
        let near_beat = ((t - 1.0) / 0.5 - ((t - 1.0) / 0.5).round()).abs() * 0.5 < dt * 0.5;
        let (fired, bt, _, _) = bs.process(near_beat, 0.8, t, false);
        if fired {
            beats.push(bt);
        }
    }
    assert!(
        (14..=17).contains(&beats.len()),
        "expected ~16 beats in 8 s at 120 BPM, got {}",
        beats.len()
    );
    for pair in beats.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            (gap - 0.5).abs() < 0.05,
            "grid beats must be one period apart, got {gap}"
        );
    }
}

/// Q1 PLL, muted freerun: a breakdown (no onsets) mutes emission after a
/// few unsupported beats, the grid keeps phase, and the return of support
/// resumes on-grid without re-acquisition.
#[test]
fn scheduler_breakdown_mutes_then_resumes_on_grid() {
    let mut bs = BeatScheduler::new();
    bs.update_tempo(120.0, 0.5, 0.9, true);
    let dt = 512.0 / 44100.0;
    let on_grid = |t: f64| ((t - 1.0) / 0.5 - ((t - 1.0) / 0.5).round()).abs() * 0.5 < dt * 0.5;
    bs.process(true, 0.9, 1.0, false); // anchor
    let mut t = 1.0;
    // 8 s supported.
    while t < 9.0 {
        t += dt;
        bs.process(on_grid(t), 0.8, t, false);
    }
    // 8 s breakdown: count emissions — must stop after a few beats.
    let mut gap_fires = 0;
    while t < 17.0 {
        t += dt;
        let (fired, _, _, _) = bs.process(false, 0.0, t, false);
        gap_fires += fired as u32;
    }
    assert!(
        gap_fires <= 8,
        "breakdown must mute freerun, got {gap_fires} fires in 16 bars"
    );
    // Support returns: emission resumes, and the resumed beats sit on the
    // ORIGINAL grid (phase preserved through the gap).
    let mut resumed = Vec::new();
    while t < 25.0 {
        t += dt;
        let (fired, bt, _, _) = bs.process(on_grid(t), 0.8, t, false);
        if fired {
            resumed.push(bt);
        }
    }
    assert!(
        resumed.len() >= 12,
        "must resume firing, got {}",
        resumed.len()
    );
    for &bt in &resumed[2..] {
        let phase_err = ((bt - 1.0) / 0.5 - ((bt - 1.0) / 0.5).round()).abs() * 0.5;
        assert!(
            phase_err < 0.08,
            "resumed beat at {bt} is {phase_err}s off the original grid"
        );
    }
}

/// Drives a locked 120 BPM scheduler for `secs` from an anchor at `anchor`.
/// True beats sit at 1.0 + k*0.5. `onsets_at(t)` / `kick_at(t)` say whether a
/// hop at t carries an onset / a kick event. Returns the emitted beat times.
fn drive_locked(
    bs: &mut BeatScheduler,
    anchor: f64,
    secs: f64,
    onsets_at: impl Fn(f64) -> bool,
    kick_at: impl Fn(f64) -> bool,
) -> Vec<f64> {
    bs.update_tempo(120.0, 0.5, 0.9, true);
    bs.process(true, 0.9, anchor, false);
    let dt = 512.0 / 44100.0;
    let mut t = anchor;
    let mut beats = Vec::new();
    while t < anchor + secs {
        t += dt;
        bs.kick_event = kick_at(t);
        let (fired, bt, _, _) = bs.process(onsets_at(t), 0.8, t, false);
        if fired {
            beats.push(bt);
        }
    }
    beats
}

/// #4474: a locked grid's beats are emitted `BEAT_LEAD_SECS` before their instants:
/// each emitted time plus the lead sits on the grid, the same beats fire as with no
/// lead (which places them on the instants themselves), and the phase output, which
/// clocks off the instants, does not move.
#[test]
fn scheduler_emits_locked_beats_a_lead_before_their_instants() {
    let drive = |lead: f64| {
        let mut bs = BeatScheduler::new();
        bs.beat_lead = lead;
        bs.arbiter = None;
        let beats = drive_locked(&mut bs, 1.0, 20.0, |t| near(t, 1.0), |_| false);
        (beats, bs.last_beat_time)
    };
    let (led, last_led) = drive(BEAT_LEAD_SECS);
    let (plain, last_plain) = drive(0.0);
    assert_eq!(
        led.len(),
        plain.len(),
        "the same beats fire with and without the lead"
    );
    assert!(led.len() >= 30, "{}", led.len());
    for (a, b) in led.iter().zip(&plain).skip(2) {
        assert!(
            (a + BEAT_LEAD_SECS - b).abs() < 1e-9,
            "emitted {a:.4} + lead should be the instant {b:.4}"
        );
        // The PLL's corrections leave a few ms of residual against the ideal grid.
        assert!(off_grid_by(*b) < 0.02, "instant {b:.4} off the grid");
    }
    assert!(
        (last_led - last_plain).abs() < 1e-9,
        "the instants themselves do not move"
    );
}

/// #80: a user sync lead stacks on [`BEAT_LEAD_SECS`] without moving the grid, and the
/// whole lead is capped at [`MAX_LEAD_PERIOD_FRAC`] of a period.
#[test]
fn scheduler_user_lead_stacks_on_the_lead_and_is_capped() {
    let drive = |user: f64| {
        let mut bs = BeatScheduler::new();
        bs.arbiter = None;
        bs.user_lead = user;
        let beats = drive_locked(&mut bs, 1.0, 20.0, |t| near(t, 1.0), |_| false);
        (beats, bs.last_beat_time)
    };
    let (plain, last_plain) = drive(0.0);
    assert!(plain.len() >= 30, "{}", plain.len());
    // 120 BPM: the cap is 0.45 × 0.5 s, of which BEAT_LEAD_SECS is already spent. It
    // scales with the trimmed grid period, which the PLL moves by a few hundredths of a
    // percent, so the capped lead is matched to 0.1 ms.
    let cap = MAX_LEAD_PERIOD_FRAC * 0.5 - BEAT_LEAD_SECS;
    for (user, expect, tol) in [(0.08, 0.08, 1e-9), (10.0, cap, 1e-4)] {
        let (led, last_led) = drive(user);
        // A lead can bring the beat after the last one inside the run's window.
        assert!(
            led.len() == plain.len() || led.len() == plain.len() + 1,
            "lead {user}: {} beats vs {}",
            led.len(),
            plain.len()
        );
        for (a, b) in led.iter().zip(&plain).skip(2) {
            assert!(
                (a + expect - b).abs() < tol,
                "lead {user}: emitted {a:.6} should sit {expect} before {b:.6} (off {:.2e})",
                a + expect - b
            );
        }
        assert!(
            (last_led - last_plain).abs() < 1e-9,
            "lead {user}: the instants themselves do not move"
        );
    }
}

#[test]
fn tempo_control_clamps_the_sync_offset() {
    let mut ctl = TempoControl::with_sync_offset(TempoConfig::default(), 35.0);
    assert_eq!(ctl.sync_offset_ms(), 35.0);
    ctl.set_sync_offset_ms(1e6);
    assert_eq!(ctl.sync_offset_ms(), SYNC_OFFSET_MAX_MS);
    ctl.set_sync_offset_ms(-1e6);
    assert_eq!(ctl.sync_offset_ms(), SYNC_OFFSET_MIN_MS);
    ctl.set_sync_offset_ms(f32::NAN);
    assert_eq!(ctl.sync_offset_ms(), 0.0);
}

/// Is a hop at `t` (hop length dt) the one nearest an instant `offset + k*0.5`?
fn near(t: f64, offset: f64) -> bool {
    let dt = 512.0 / 44100.0;
    let x = (t - offset) / 0.5;
    (x - x.round()).abs() * 0.5 < dt * 0.5
}

fn off_grid_by(bt: f64) -> f64 {
    let x = (bt - 1.0) / 0.5;
    (x - x.round()).abs() * 0.5
}

/// #4160: a grid anchored on the off-beat, held there by louder off-beat
/// onsets (the PLL's +-25% capture never reaches the true beat), is moved
/// onto the beat by kick evidence, and stays there.
#[test]
fn arbiter_moves_an_off_beat_grid_onto_the_kicks() {
    let mut bs = BeatScheduler::new();
    bs.arbiter = Some(PHASE_ARBITER);
    // Onsets on beats AND off-beats; kicks only on the beats.
    let beats = drive_locked(
        &mut bs,
        1.25,
        16.0,
        |t| near(t, 1.0) || near(t, 1.25),
        |t| near(t, 1.0),
    );
    assert_eq!(bs.phase_shifts, 1, "exactly one half-beat shift");
    let late: Vec<f64> = beats.iter().copied().filter(|&b| b > 12.0).collect();
    assert!(late.len() >= 6, "keeps firing after the shift");
    for b in late {
        assert!(
            off_grid_by(b) < 0.05,
            "beat at {b} is still {:.3} s off the kick grid",
            off_grid_by(b)
        );
    }
}

/// The same music without the arbiter stays on the off-beat: the case the
/// arbiter exists for (guards the test above against passing vacuously).
#[test]
fn without_the_arbiter_the_off_beat_grid_stays_off() {
    let mut bs = BeatScheduler::new();
    bs.arbiter = None;
    let beats = drive_locked(
        &mut bs,
        1.25,
        16.0,
        |t| near(t, 1.0) || near(t, 1.25),
        |t| near(t, 1.0),
    );
    let late: Vec<f64> = beats.iter().copied().filter(|&b| b > 12.0).collect();
    assert!(
        late.iter().all(|&b| off_grid_by(b) > 0.2),
        "expected an off-beat lock, got {late:?}"
    );
}

/// A grid already on the kicks is never moved, and with no kicks at all
/// the arbiter abstains: emitted beats are exactly the disabled ones.
#[test]
fn arbiter_leaves_a_right_grid_alone_and_abstains_without_kicks() {
    let mut on = BeatScheduler::new();
    on.arbiter = Some(PHASE_ARBITER);
    drive_locked(
        &mut on,
        1.0,
        16.0,
        |t| near(t, 1.0) || near(t, 1.25),
        |t| near(t, 1.0),
    );
    assert_eq!(on.phase_shifts, 0, "kicks agree with the grid: no shift");

    let mut with = BeatScheduler::new();
    with.arbiter = Some(PHASE_ARBITER);
    let mut without = BeatScheduler::new();
    without.arbiter = None;
    let a = drive_locked(
        &mut with,
        1.25,
        16.0,
        |t| near(t, 1.0) || near(t, 1.25),
        |_| false,
    );
    let b = drive_locked(
        &mut without,
        1.25,
        16.0,
        |t| near(t, 1.0) || near(t, 1.25),
        |_| false,
    );
    assert_eq!(with.phase_shifts, 0);
    assert_eq!(
        a, b,
        "no kick evidence: identical to the disabled scheduler"
    );
}

/// The chiptune case (batida chiptune s2): a grid that kicks have backed
/// for 16 s is hit by a 10 s burst where off-beat misfires outnumber the
/// real kicks about 3:1 (batida measured 10:4). The 32-beat guard holds the
/// grid; without it the same burst moves the grid onto the off-beat.
#[test]
fn arbiter_guard_ignores_a_short_off_beat_burst() {
    let kicks = |t: f64| {
        let burst = (17.0..27.0).contains(&t);
        let beat = ((t - 1.0) / 0.5).round() as i64;
        (near(t, 1.0) && (!burst || beat % 3 == 0)) || (burst && near(t, 1.25))
    };
    let mut guarded = BeatScheduler::new();
    guarded.arbiter = Some(PHASE_ARBITER);
    drive_locked(&mut guarded, 1.0, 28.0, |t| near(t, 1.0), kicks);
    assert_eq!(
        guarded.phase_shifts, 0,
        "the guard must hold a long-backed grid"
    );

    let mut unguarded = BeatScheduler::new();
    unguarded.arbiter = Some(PhaseArbiterCfg {
        long_beats: 0.0,
        ..PHASE_ARBITER
    });
    drive_locked(&mut unguarded, 1.0, 28.0, |t| near(t, 1.0), kicks);
    assert!(
        unguarded.phase_shifts >= 1,
        "without the guard the burst moves the grid (test is not vacuous)"
    );
}

/// #4234, the Ballroom case (Cha-cha-cha, Media-103405): events on every beat and on
/// the 'and' of every other beat, on a grid locked to the off-beat. The beat leads 2:1,
/// but a third of the long window sits on the grid, which the #4160 guard (at most .3)
/// read as a grid the evidence still backs; the shipped guard moves it.
#[test]
fn arbiter_moves_a_grid_off_a_recurring_and() {
    let events = |t: f64| {
        let and = ((t - 1.25) / 0.5).round() as i64;
        near(t, 1.0) || (near(t, 1.25) && and % 2 == 0)
    };
    let mut bs = BeatScheduler::new();
    bs.arbiter = Some(PHASE_ARBITER);
    let beats = drive_locked(
        &mut bs,
        1.25,
        24.0,
        |t| near(t, 1.0) || near(t, 1.25),
        events,
    );
    assert_eq!(bs.phase_shifts, 1, "exactly one half-beat shift");
    let late: Vec<f64> = beats.iter().copied().filter(|&b| b > 20.0).collect();
    assert!(late.len() >= 6, "keeps firing after the shift");
    for b in late {
        assert!(
            off_grid_by(b) < 0.05,
            "beat at {b} is still {:.3} s off the beat",
            off_grid_by(b)
        );
    }

    let mut held = BeatScheduler::new();
    held.arbiter = Some(PhaseArbiterCfg {
        streak: 4,
        long_ratio: 2.0,
        long_share: 0.3,
        ..PHASE_ARBITER
    });
    drive_locked(
        &mut held,
        1.25,
        24.0,
        |t| near(t, 1.0) || near(t, 1.25),
        events,
    );
    assert_eq!(
        held.phase_shifts, 0,
        "the #4160 guard holds this grid (test is not vacuous)"
    );
}

/// A shift moves the grid LATER by half a beat: a longer gap, never two
/// beats closer than the scheduler's 0.3-period floor.
/// #4163: the evidence knob decides which events reach the arbiter's window, and an
/// activation event (placed before this hop) is inserted in time order.
#[test]
fn arbiter_window_follows_the_evidence_mode() {
    let window = |mode: ArbEvidence| {
        let mut bs = BeatScheduler::new();
        bs.arbiter = Some(PHASE_ARBITER);
        bs.arb_evidence = mode;
        bs.update_tempo(120.0, 0.5, 1.0, true);
        // Anchor a grid first so `process` reaches the window code.
        bs.process(true, 1.0, 1.0, false);
        bs.kick_event = true;
        bs.act_event = Some(1.5 - 0.035);
        bs.process(false, 0.0, 1.5, false);
        bs.arb_kicks.iter().copied().collect::<Vec<f64>>()
    };
    assert_eq!(window(ArbEvidence::Kicks), vec![1.5]);
    assert_eq!(window(ArbEvidence::Activation), vec![1.5 - 0.035]);
    assert_eq!(window(ArbEvidence::Union), vec![1.5 - 0.035, 1.5]);
}

/// #4402, the batida techno case (#3344): a grid .3 of a beat late sits outside the
/// PLL's capture window with nothing on its instants, and every event lands in the
/// 3/4 bin. With the quarter candidates it moves three quarters of a beat LATER onto
/// the events (and the half-beat rule, which reads those events as neither phase,
/// leaves it there: the test is not vacuous). The 24-event minimum in the 32-beat
/// window holds the move until that window has filled (a bar or two after lock).
#[test]
fn arbiter_quarters_move_a_quarter_late_grid_onto_the_events() {
    let late = 1.15; // beats at 1.0 + k * 0.5; the grid anchors .3 period late
    let drive = |quarters: bool| {
        let mut bs = BeatScheduler::new();
        bs.arbiter = Some(PhaseArbiterCfg {
            quarters,
            ..PHASE_ARBITER
        });
        // Onsets on the beats (outside capture from this grid) and on the grid's own
        // instants (hi-hat-like support, so the grid keeps firing); kicks on the beats.
        let beats = drive_locked(
            &mut bs,
            late,
            32.0,
            |t| near(t, 1.0) || near(t, late),
            |t| near(t, 1.0),
        );
        (bs.phase_shifts, beats)
    };
    let (shifts, beats) = drive(true);
    assert_eq!(shifts, 1, "exactly one quarter shift");
    let after: Vec<f64> = beats.iter().copied().filter(|&b| b > 26.0).collect();
    assert!(after.len() >= 8, "keeps firing after the shift: {after:?}");
    for b in &after {
        assert!(
            off_grid_by(*b) < 0.05,
            "beat at {b} is still {:.3} s off the beat",
            off_grid_by(*b)
        );
    }
    let (shifts, beats) = drive(false);
    assert_eq!(shifts, 0, "the half-beat rule never votes here");
    let after: Vec<f64> = beats.iter().copied().filter(|&b| b > 26.0).collect();
    assert!(
        after.iter().all(|&b| off_grid_by(b) > 0.1),
        "without the quarters the grid stays late: {after:?}"
    );
}

/// #4402: a grid on the beat with kicks on every beat and on the 'e' (a quarter after)
/// of every beat is never moved, and the quarter candidates change nothing against the
/// half-beat-only arbiter (#4234) on the half-beat scenarios of the tests above (a
/// strict superset: they are consulted only where the half-beat rule abstains).
#[test]
fn arbiter_quarters_leave_right_and_half_beat_grids_to_the_shipped_rule() {
    let without = PhaseArbiterCfg {
        quarters: false,
        ..PHASE_ARBITER
    };
    let mut bs = BeatScheduler::new();
    bs.arbiter = Some(PHASE_ARBITER);
    let beats = drive_locked(
        &mut bs,
        1.0,
        16.0,
        |t| near(t, 1.0) || near(t, 1.125),
        |t| near(t, 1.0) || near(t, 1.125),
    );
    assert_eq!(bs.phase_shifts, 0, "beats match the 'e' accents: no move");
    assert!(beats.iter().all(|&b| off_grid_by(b) < 0.05));

    // The half-beat scenarios: an off-beat grid moved by the kicks, a recurring 'and',
    // and the Latin case where a quarter accent outnumbers the beats while the grid
    // sits on the half-beat (the half-beat rule votes first and wins).
    type Scenario = (f64, f64, fn(f64) -> bool, fn(f64) -> bool);
    let scenarios: [Scenario; 3] = [
        (
            1.25,
            16.0,
            |t| near(t, 1.0) || near(t, 1.25),
            |t| near(t, 1.0),
        ),
        (
            1.25,
            24.0,
            |t| near(t, 1.0) || near(t, 1.25),
            |t| {
                let and = ((t - 1.25) / 0.5).round() as i64;
                near(t, 1.0) || (near(t, 1.25) && and % 2 == 0)
            },
        ),
        (
            1.25,
            16.0,
            |t| near(t, 1.0) || near(t, 1.25),
            |t| near(t, 1.0) || near(t, 1.125),
        ),
    ];
    for (anchor, secs, onsets, kicks) in scenarios {
        let mut a = BeatScheduler::new();
        a.arbiter = Some(without);
        let mut b = BeatScheduler::new();
        b.arbiter = Some(PHASE_ARBITER);
        let ba = drive_locked(&mut a, anchor, secs, onsets, kicks);
        let bb = drive_locked(&mut b, anchor, secs, onsets, kicks);
        assert_eq!(a.phase_shifts, 1, "the half-beat rule moves this grid once");
        assert_eq!(ba, bb, "identical beats with the quarters on");
    }
}

#[test]
fn arbiter_shift_never_doubles_a_beat() {
    let mut bs = BeatScheduler::new();
    bs.arbiter = Some(PHASE_ARBITER);
    let beats = drive_locked(
        &mut bs,
        1.25,
        16.0,
        |t| near(t, 1.0) || near(t, 1.25),
        |t| near(t, 1.0),
    );
    assert_eq!(bs.phase_shifts, 1);
    for pair in beats.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap > 0.3 * 0.5,
            "beats {:.3} and {:.3} only {gap:.3} s apart",
            pair[0],
            pair[1]
        );
        assert!(
            gap < 1.6 * 0.5,
            "gap {gap:.3} s is more than the half-beat shift"
        );
    }
}

/// Q1 PLL invariant: emitted beat_times are STRICTLY increasing and never
/// closer than 0.3 period, across lock flips, corrections, silences and
/// mode churn. (A pending negative slot correction around an acquisition
/// fire once produced a −1.6 ms inversion that crashed mir_eval.)
#[test]
fn scheduler_beat_times_strictly_monotonic_through_mode_churn() {
    let mut bs = BeatScheduler::new();
    let dt = 512.0 / 44100.0;
    let period = 0.5;
    let mut rng: u64 = 0xBEEF;
    let mut emitted: Vec<f64> = Vec::new();
    let mut t = 1.0;
    for step in 0..(120.0 / dt) as usize {
        t += dt;
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let r = (rng >> 40) as f64 / (1u64 << 24) as f64;
        // Lock flaps every ~2 s; onsets arrive ~5/s at pseudo-random
        // instants; occasional silence patches.
        let locked = (step / 172) % 2 == 0;
        bs.update_tempo(120.0, period, if locked { 0.8 } else { 0.3 }, locked);
        let onset = r < 0.06;
        let silence = (step / 800) % 7 == 6;
        let (fired, bt, _, _) = bs.process(onset, 0.5 + (r * 0.5) as f32, t, silence);
        if fired {
            if let Some(&prev) = emitted.last() {
                assert!(
                    bt > prev + 0.3 * period - 1e-9,
                    "beat at {bt:.4} too close to previous {prev:.4} (step {step})"
                );
            }
            emitted.push(bt);
        }
    }
    assert!(
        emitted.len() > 40,
        "churn drive should still emit beats, got {}",
        emitted.len()
    );
}

/// Q1 PLL, acquisition: unlocked fires need onset+grid agreement and are
/// spaced at least 0.6 period — a hi-hat run cannot spray beats.
#[test]
fn scheduler_acquisition_is_evidence_gated_and_spaced() {
    let mut bs = BeatScheduler::new();
    bs.update_tempo(120.0, 0.5, 0.3, false);
    bs.process(true, 0.8, 1.0, false); // anchor fire
    // Onsets every 100 ms (16th-note spam at 150 BPM): only those agreeing
    // with the 0.5 s grid may fire, so ≤ 1 fire per 0.3 s minimum spacing.
    let mut fires = 0;
    let dt = 512.0 / 44100.0;
    let mut t = 1.0;
    let mut next_onset = 1.1;
    while t < 6.0 {
        t += dt;
        let onset = t + dt * 0.5 >= next_onset;
        if onset {
            next_onset += 0.1;
        }
        let (fired, _, _, _) = bs.process(onset, 0.7, t, false);
        fires += fired as u32;
    }
    assert!(
        fires <= 11,
        "acquisition must not fire above ~1 per period on onset spam, got {fires} in 5 s"
    );
}

// ---- Integration test ----

/// Per-frame band magnitude for a kick train with sub-hop placement. Kick
/// `i` (0-based) lands at `(i+1) * interval`; its magnitude rises to
/// `amp(i)` GEOMETRICALLY across the two frames the instant falls between,
/// so the SuperFlux log-domain rise splits (1−frac)/frac — which is how the
/// real 87.5%-overlap analysis window encodes sub-hop onset position in the
/// flux envelope. Binary single-frame impulses instead quantize an off-bin
/// tempo like 175 BPM into a hard 29/30-frame alternation whose
/// every-other-kick lag is EXACTLY 59 frames, making half tempo genuinely
/// sharper in the synthetic signal than the tempo it claims to carry.
fn kick_amplitude_track(
    num_frames: usize,
    dt: f64,
    interval: f64,
    amp: impl Fn(usize) -> f32,
) -> Vec<f32> {
    const BASE: f64 = 0.001;
    let mut track = vec![BASE as f32; num_frames];
    let mut i = 0usize;
    loop {
        let f = (i + 1) as f64 * interval / dt;
        let idx = f as usize;
        if idx + 1 >= num_frames {
            return track;
        }
        let frac = f - idx as f64;
        let a = f64::from(amp(i));
        let first = BASE * (a / BASE).powf(1.0 - frac);
        track[idx] = track[idx].max(first as f32);
        track[idx + 1] = track[idx + 1].max(a as f32);
        i += 1;
    }
}

/// Run a BPM convergence test with synthetic kicks at the given tempo.
/// Returns the detected BPM after `duration_secs` seconds.
fn run_bpm_convergence_test(target_bpm: f64, duration_secs: f64) -> f32 {
    run_bpm_convergence_with(target_bpm, duration_secs, TempoConfig::default())
}

fn run_bpm_convergence_with(target_bpm: f64, duration_secs: f64, tempo: TempoConfig) -> f32 {
    let sample_rate = 44100.0;
    let mut detector = BeatDetector::new(sample_rate, tempo);

    let bass_len = 2049; // 4096/2 + 1
    let mid_len = 513; // 1024/2 + 1
    let high_len = 257; // 512/2 + 1

    // Frames must be spaced at the detector's own clock. Since A5 (#1456) that is derived
    // from the fixed analysis hop (sr / ANALYSIS_HOP ~= 86.1 Hz @ 44.1 kHz), not the 100 Hz
    // this harness used to assume — at 100 Hz every `target_bpm` below actually reached the
    // estimator 13.9% low, and only the +/-15% tolerance bands hid it.
    let dt = crate::audio::ANALYSIS_HOP as f64 / sample_rate as f64;
    let kick_interval = 60.0 / target_bpm;
    let num_frames = (duration_secs / dt) as usize;
    // Sub-hop onset placement, like a real windowed analyzer sees it: a kick
    // between two hops splits its energy across both in proportion to its
    // fractional position. Single-frame impulses instead quantize an off-bin
    // tempo like 175 BPM (29.53 frames) into a hard 29/30 alternation whose
    // every-other-kick lag is EXACTLY 59 — making half tempo genuinely
    // sharper in the synthetic signal than the tempo it claims to carry.
    let kick_amp = kick_amplitude_track(num_frames, dt, kick_interval, |_| 2.0);

    let mut last_bpm = 0.0f32;

    for (frame, &amp) in kick_amp.iter().enumerate() {
        let t = frame as f64 * dt;

        let mut bass = vec![0.001f32; bass_len];
        let mid = vec![0.001f32; mid_len];
        let high = vec![0.001f32; high_len];

        for bin in 1..12 {
            bass[bin] = amp;
        }

        let result = detector.process(&bass, &mid, &high, t, false);
        last_bpm = result.bpm;
    }

    eprintln!(
        "BPM convergence: target={target_bpm}, detected={last_bpm}, duration={duration_secs}s"
    );
    last_bpm
}

/// Like `run_bpm_convergence_with`, but alternating strong/weak hits — the
/// strong-hits-only grid (half tempo) carries real ACF evidence of its own,
/// like a kick/snare backbeat does.
fn run_backbeat_convergence(target_bpm: f64, duration_secs: f64, preset: TempoPreset) -> f32 {
    let (center, sigma) = preset.values();
    let tempo = TempoConfig {
        prior_center_bpm: center,
        prior_sigma: sigma,
        auto_prior: false,
    };
    let sample_rate = 44100.0;
    let mut detector = BeatDetector::new(sample_rate, tempo);
    let (bass_len, mid_len, high_len) = (2049, 513, 257);
    let dt = crate::audio::ANALYSIS_HOP as f64 / sample_rate as f64;
    let kick_interval = 60.0 / target_bpm;
    let num_frames = (duration_secs / dt) as usize;
    let kick_amp = kick_amplitude_track(num_frames, dt, kick_interval, |i| {
        if i % 2 == 0 { 2.0 } else { 0.8 }
    });
    let mut last_bpm = 0.0f32;
    for (frame, &amp) in kick_amp.iter().enumerate() {
        let t = frame as f64 * dt;
        let mut bass = vec![0.001f32; bass_len];
        let mid = vec![0.001f32; mid_len];
        let high = vec![0.001f32; high_len];
        for bin in 1..12 {
            bass[bin] = amp;
        }
        last_bpm = detector.process(&bass, &mid, &high, t, false).bpm;
    }
    eprintln!("Backbeat convergence: target={target_bpm}, detected={last_bpm}");
    last_bpm
}

// Q2: bands tightened from the ±15-20% that hid the lag quantization and the
// sign-flipped parabola to ±5% — continuous-lag scoring lands well inside.
#[test]
fn bpm_converges_120() {
    let bpm = run_bpm_convergence_test(120.0, 8.0);
    assert!(
        (f64::from(bpm) - 120.0).abs() / 120.0 < 0.05,
        "120 BPM: expected within 5%, got {bpm}"
    );
}

#[test]
fn bpm_converges_90() {
    let bpm = run_bpm_convergence_test(90.0, 10.0);
    assert!(
        (f64::from(bpm) - 90.0).abs() / 90.0 < 0.05,
        "90 BPM: expected within 5%, got {bpm}"
    );
}

#[test]
fn bpm_converges_140() {
    let bpm = run_bpm_convergence_test(140.0, 10.0);
    assert!(
        (f64::from(bpm) - 140.0).abs() / 140.0 < 0.05,
        "140 BPM: expected within 5%, got {bpm}"
    );
}

#[test]
fn bpm_converges_170() {
    let bpm = run_bpm_convergence_test(170.0, 10.0);
    assert!(
        (f64::from(bpm) - 170.0).abs() / 170.0 < 0.05,
        "170 BPM: expected within 5%, got {bpm}"
    );
}

#[test]
fn bpm_converges_200() {
    let bpm = run_bpm_convergence_test(200.0, 10.0);
    assert!(
        (f64::from(bpm) - 200.0).abs() / 200.0 < 0.05,
        "200 BPM: expected within 5%, got {bpm}"
    );
}

#[test]
fn bpm_converges_230() {
    let bpm = run_bpm_convergence_test(230.0, 10.0);
    // Accept 230 or half-tempo 115 (prior centered at 150 favors the lower octave)
    let b = f64::from(bpm);
    let in_range = (b - 230.0).abs() / 230.0 < 0.05 || (b - 115.0).abs() / 115.0 < 0.05;
    assert!(
        in_range,
        "230 BPM: expected within 5% of 230 or 115, got {bpm}"
    );
}

#[test]
fn bpm_no_octave_double_145() {
    let bpm = run_bpm_convergence_test(145.0, 10.0);
    assert!(
        (f64::from(bpm) - 145.0).abs() / 145.0 < 0.05,
        "145 BPM: expected within 5% (not 290 octave double), got {bpm}"
    );
}

/// Q2: 175 BPM sits at lag 29.53 — between ACF bins that are 3.4% apart at
/// this tempo, against a 4% Acc1 tolerance. Integer-lag candidates could
/// not represent it (the old GiantSteps floor); continuous-lag candidates
/// with peak interpolation must land within 1%.
#[test]
fn off_bin_tempo_locks_within_one_percent() {
    let bpm = run_bpm_convergence_test(175.0, 10.0);
    assert!(
        (f64::from(bpm) - 175.0).abs() / 175.0 < 0.01,
        "175 BPM: expected within 1%, got {bpm}"
    );
}

/// Q2 seed gate: white-noise onset input produces low-confidence ACF
/// readings only, and the estimator must never adopt one as its seed — the
/// old code seeded on the very first post-2s measurement unconditionally
/// and then defended it.
#[test]
fn tempo_seed_requires_confidence() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    let mut rng: u64 = 12345;
    for _ in 0..(6.0 * 86.13) as usize {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let v = (rng >> 32) as f64 / (1u64 << 32) as f64;
        let (bpm, _conf, _period) = est.update(v, v);
        assert_eq!(bpm, 0.0, "white-noise onsets must never seed a tempo");
    }
}

/// Drive a TempoEstimator with an impulse train at the given period (in
/// frames, fractional OK) for `secs`, returning the last published BPM. The train
/// feeds both the flux and the activation (#4250).
fn drive_estimator(est: &mut TempoEstimator, period_frames: f64, secs: f64) -> f64 {
    let mut bpm = 0.0;
    let mut next = period_frames;
    for i in 0..(secs * 86.13) as usize {
        let v = if i as f64 + 0.5 >= next {
            next += period_frames;
            1.0
        } else {
            0.05
        };
        bpm = est.update(v, v).0;
    }
    bpm
}

/// #4250: drive the estimator with two impulse trains, `flux_period` frames on the
/// flux (the seed gate's input) and `act_period` on the beat activation; `None`
/// feeds zeros. Returns the last published BPM.
fn drive_estimator_split(
    est: &mut TempoEstimator,
    flux_period: Option<f64>,
    act_period: Option<f64>,
    secs: f64,
) -> f64 {
    let periods = [flux_period, act_period];
    let mut next = periods.map(|p| p.unwrap_or(f64::INFINITY));
    let mut bpm = 0.0;
    for i in 0..(secs * 86.13) as usize {
        let mut v = [0.0, 0.0];
        for k in 0..2 {
            if let Some(p) = periods[k] {
                v[k] = if i as f64 + 0.5 >= next[k] {
                    next[k] += p;
                    1.0
                } else {
                    0.05
                };
            }
        }
        bpm = est.update(v[0], v[1]).0;
    }
    bpm
}

/// #4250: once the flux has seeded, the activation's autocorrelation chooses the
/// period: a ~148 BPM flux train with a ~96 BPM activation train publishes 96.
#[test]
fn activation_chooses_the_period_once_the_flux_has_seeded() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    let bpm = drive_estimator_split(&mut est, Some(34.9), Some(53.83), 20.0);
    assert!(
        (bpm - 96.0).abs() / 96.0 < 0.04,
        "the activation's period must win once seeded, got {bpm}"
    );
}

/// #4250: with no activation peak, the flux's reading is the measurement.
#[test]
fn flux_reading_is_the_fallback_without_an_activation_peak() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    let bpm = drive_estimator_split(&mut est, Some(34.9), None, 20.0);
    assert!(
        (bpm - 148.0).abs() / 148.0 < 0.04,
        "the flux fallback must publish ~148, got {bpm}"
    );
}

/// #4250: the activation alone never seeds. A clean activation train over white-noise
/// flux stays unpublished, because the seed gate reads the flux only: the beatless
/// guard (sparse activation texture reads as periodic to a normalized ACF).
#[test]
fn activation_alone_never_seeds() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    let mut rng: u64 = 12345;
    let mut next = 40.4;
    for i in 0..(6.0 * 86.13) as usize {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let noise = (rng >> 32) as f64 / (1u64 << 32) as f64;
        let act = if i as f64 + 0.5 >= next {
            next += 40.4;
            1.0
        } else {
            0.0
        };
        let (bpm, _, _) = est.update(noise, act);
        assert_eq!(
            bpm, 0.0,
            "a periodic activation over noisy flux must never seed"
        );
    }
}

/// Q3 lock dynamics: a locked estimator rides out a brief burst of
/// contradictory readings (a fill, a breakdown bar) without its published
/// BPM leaving the ±4% band — the excursion class that reset the bench's
/// trailing-lock clock up to 10× per track — and re-locks seamlessly.
#[test]
fn tempo_lock_survives_brief_wobble() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    // ~128 BPM: period 40.4 frames.
    let bpm = drive_estimator(&mut est, 40.4, 20.0);
    assert!(
        est.locked,
        "20 s of a clean train must lock (support {})",
        est.support
    );
    assert!(
        (bpm - 128.0).abs() / 128.0 < 0.04,
        "locked near 128, got {bpm}"
    );
    // 2.5 s of white noise — support dips, published BPM must hold the band.
    let mut rng: u64 = 777;
    for _ in 0..(2.5 * 86.13) as usize {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let v = (rng >> 32) as f64 / (1u64 << 32) as f64;
        let (b, _, _) = est.update(v, v);
        assert!(
            (b - 128.0).abs() / 128.0 < 0.04,
            "published BPM left the band during a brief wobble: {b}"
        );
    }
    // Train resumes: still (or again) locked shortly after.
    drive_estimator(&mut est, 40.4, 5.0);
    assert!(
        est.locked,
        "must re-lock after the wobble (support {})",
        est.support
    );
}

/// Q2b: a brief stretch of winners at a metrically-related level (the
/// double-kick-over-four-on-the-floor case from the 2026-08-08 live smoke)
/// folds into the anchor grid instead of teleporting the filter — the
/// published tempo never leaves the band and the lock holds.
#[test]
fn anchor_ignores_brief_related_level_flip() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    drive_estimator(&mut est, 40.4, 20.0); // ~128 BPM
    assert!(est.locked && est.anchor_log2.is_some(), "anchor earned");
    // 5 s at the ½ level (~64 BPM, period 80.8 frames): pre-Q2b this fed
    // the divergence hard reset within ~1 s.
    drive_estimator(&mut est, 80.8, 5.0);
    let published = est.published_bpm();
    assert!(
        (published - 128.0).abs() / 128.0 < 0.04,
        "published held the anchor level, got {published}"
    );
    assert!(est.locked, "related-level winners must not drain the lock");
    assert!(est.anchor_log2.is_some(), "anchor survives");
}

/// Q2b: during probation, sustained NON-octave evidence displaces — this
/// is the correction path for the unrecoverable class (a wrong ⅔/¾ first
/// anchor), the measured GiantSteps win.
#[test]
fn nonoctave_evidence_displaces_during_probation() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    drive_estimator(&mut est, 40.4, 16.0); // ~128 BPM, anchor ~13 s in
    assert!(est.anchor_log2.is_some());
    // ⅔ level (~85.3 BPM, period 60.6): flush + challenge + span lands
    // inside the 30 s probation window.
    let bpm = drive_estimator(&mut est, 60.6, 24.0);
    assert!(
        (bpm - 85.3).abs() / 85.3 < 0.05,
        "non-octave evidence in probation must displace, got {bpm}"
    );
}

/// Q2b, owner-ruled after live run 4: OCTAVE evidence never displaces —
/// not even during probation. Run 4's logged chain: ½-level intro
/// evidence displaced a CORRECT 26 s-old anchor, the wrong lineage
/// out-tenured the truth, and the ghost then defended 115 against a
/// genuine 172 detection. An octave-wrong young anchor is benign; a
/// displaced correct one is not.
#[test]
fn octave_evidence_folds_even_in_probation() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    drive_estimator(&mut est, 40.4, 16.0); // ~128 BPM, probation active
    assert!(est.anchor_log2.is_some());
    let bpm = drive_estimator(&mut est, 80.8, 24.0);
    assert!(
        (bpm - 128.0).abs() / 128.0 < 0.04,
        "octave evidence must fold even in probation, got {bpm}"
    );
}

/// Q2b, owner-ruled after live run 3: once probation ends the level is
/// settled — even sustained OCTAVE evidence folds instead of displacing
/// (the 172→86→172→86 ping-pong followed a half-time feel, not a tempo
/// change). Escape hatches: unrelated divergence, tap, octave override.
#[test]
fn post_probation_octave_evidence_folds_not_displaces() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    // ~128 BPM well past probation (earn ~13 s + 30 s probation).
    drive_estimator(&mut est, 40.4, 50.0);
    assert!(est.locked && est.anchor_log2.is_some());
    // 20 s of sustained ½-level winners: pre-ruling this displaced.
    let bpm = drive_estimator(&mut est, 80.8, 20.0);
    assert!(
        (bpm - 128.0).abs() / 128.0 < 0.04,
        "post-probation octave evidence must fold, got {bpm}"
    );
    assert!(
        est.locked,
        "folded octave evidence keeps supporting the lock"
    );
}

/// Q2b ghost anchor: a level war (anchor → unrelated mush → abandon → a
/// related level locks and re-earns) must not mint an unrecoverable
/// non-octave anchor — the re-earn folds back to the ghost's level. This
/// is the 2026-08-08 re-listen failure: the orchestral section's mush
/// abandoned the 172 anchor and the ⅔ level (115) re-earned, then held
/// wrong by design for 90 s.
#[test]
fn ghost_anchor_vetoes_related_reearn() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    // ~128 BPM long enough for the anchor to be a VETERAN — the ghost
    // only forms with GHOST_MIN_TENURE_SECS of tenure (young first
    // anchors are the mistake a re-earn corrects, not a level to defend).
    drive_estimator(&mut est, 40.4, 45.0);
    assert!(est.anchor_log2.is_some());
    // ~12 s of unrelated MUSH — alternating ~105 / ~93 BPM so the winner
    // keeps deviating >10% from the anchor (the hard reset fires) but
    // support can never build 8 s of lock (no intermediate anchor may
    // earn, or it would launder the ghost — the exact live failure).
    for _ in 0..4 {
        drive_estimator(&mut est, 49.2, 1.5);
        drive_estimator(&mut est, 55.4, 1.5);
    }
    assert!(est.anchor_log2.is_none(), "mush must abandon the anchor");
    assert!(est.ghost_anchor.is_some(), "abandon leaves a ghost");
    // Sustained ⅔ level (~85.3 BPM): locks and re-earns — the ghost must
    // fold the earn back to ~128, not let 85.3 stick.
    drive_estimator(&mut est, 60.6, 22.0);
    let anchor_bpm = est.anchor_log2.map(|a| 2.0f64.powf(a)).unwrap_or(0.0);
    assert!(
        (anchor_bpm - 128.0).abs() / 128.0 < 0.06,
        "re-earn must fold to the ghost level, anchored at {anchor_bpm}"
    );
    let published = est.published_bpm();
    assert!(
        (published - 128.0).abs() / 128.0 < 0.06,
        "published must return to the ghost level, got {published}"
    );
}

/// Q2b (F3): the wire holds the last locked tempo through an unlock and
/// reads 0 only before the first lock is ever earned.
#[test]
fn published_bpm_holds_through_unlock_and_zeroes_before_first_lock() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    // Fresh estimator + noise: nothing published.
    let mut rng: u64 = 42;
    for _ in 0..(3.0 * 86.13) as usize {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let v = (rng >> 32) as f64 / (1u64 << 32) as f64;
        est.update(v, v);
    }
    assert_eq!(est.published_bpm(), 0.0, "no lock ever -> wire silent");
    // Lock at ~128, then starve support with silence-grade input.
    drive_estimator(&mut est, 40.4, 20.0);
    assert!(est.locked);
    for _ in 0..(6.0 * 86.13) as usize {
        est.update(0.0, 0.0);
    }
    let published = est.published_bpm();
    assert!(
        (published - 128.0).abs() / 128.0 < 0.04,
        "wire must hold the last locked tempo, got {published}"
    );
}

/// Q3: lock resists wobble but NOT a real change — a sustained new tempo
/// unlocks (or hard-resets) and is adopted within a few seconds.
#[test]
fn tempo_lock_releases_on_sustained_change() {
    let mut est = TempoEstimator::new(8.0, 86.13, TempoConfig::default());
    drive_estimator(&mut est, 40.4, 20.0); // ~128 BPM
    assert!(est.locked);
    // Genuine move to ~148 BPM (period 34.9 frames) for 12 s.
    let bpm = drive_estimator(&mut est, 34.9, 12.0);
    assert!(
        (bpm - 148.0).abs() / 148.0 < 0.05,
        "sustained new tempo must be adopted, got {bpm}"
    );
}

/// Q3: the lock state reaches the scheduler alongside confidence — the
/// Stage 4 PLL keys its modes off this flag.
#[test]
fn scheduler_receives_lock_state() {
    let mut bs = BeatScheduler::new();
    assert!(!bs.tempo_locked);
    bs.update_tempo(128.0, 60.0 / 128.0, 0.9, true);
    assert!(bs.tempo_locked);
    bs.update_tempo(128.0, 60.0 / 128.0, 0.2, false);
    assert!(!bs.tempo_locked);
}

/// Q2: flat ACF ⇒ every candidate scores identically no matter how many of
/// its harmonics fit inside the array. The old SUM gave a candidate with 4
/// in-range harmonics up to 2× the score of an equally-supported longer
/// period — the double-time seed bias.
#[test]
fn comb_score_is_truncation_fair() {
    let acf = vec![0.5; 101];
    let full = comb_score(&acf, 20.0); // h=1..4 → 20,40,60,80 all inside
    let cut = comb_score(&acf, 40.0); // h=3,4 → 120,160 truncated
    assert!((full - cut).abs() < 1e-12, "{full} vs {cut}");
}

#[test]
fn acf_at_interpolates_linearly() {
    let acf = vec![0.0, 1.0, 0.0];
    assert!((acf_at(&acf, 0.5) - 0.5).abs() < 1e-12);
    assert!((acf_at(&acf, 1.25) - 0.75).abs() < 1e-12);
}

// ---- A7 (#1458): tempo prior, octave override, tap tempo ----

#[test]
fn tempo_config_default_matches_pre_a7_hardcoding() {
    // Upgrading users must get byte-identical detection until they touch a preset.
    let c = TempoConfig::default();
    assert_eq!(c.prior_center_bpm, 150.0);
    assert_eq!(c.prior_sigma, 1.0);
    assert!(!c.auto_prior);
}

#[test]
fn tempo_preset_round_trips_through_config() {
    for &p in TempoPreset::ALL {
        let (center, sigma) = p.values();
        let cfg = TempoConfig {
            prior_center_bpm: center,
            prior_sigma: sigma,
            auto_prior: false,
        };
        assert_eq!(TempoPreset::from_config(&cfg), Some(p));
    }
}

#[test]
fn hand_tuned_config_matches_no_preset() {
    let cfg = TempoConfig {
        prior_center_bpm: 133.0,
        prior_sigma: 0.5,
        auto_prior: false,
    };
    assert_eq!(TempoPreset::from_config(&cfg), None);
}

#[test]
fn prior_center_decides_the_octave() {
    // The A7 payoff, stated as something this harness can actually prove: the same 172 BPM
    // signal reads as 172 under the default prior and folds to a metrical division under a
    // prior centred low. That is the prior steering metrical-ratio selection — the mechanism
    // the genre presets exist to drive.
    //
    // Q2 note: a clean impulse train's subharmonics all carry identical normalized comb
    // evidence (every division of the grid IS a valid slower reading of it), so under a low
    // prior the winner among 86/57.4/43 is decided by prior proximity alone — the fold may
    // land on ÷2 or ÷3. `backbeat_fold_prefers_the_supported_octave` pins the realistic
    // case where rhythm evidence breaks the tie.
    let default_bpm = run_bpm_convergence_with(172.0, 10.0, TempoConfig::default());
    assert!(
        (default_bpm - 172.0).abs() / 172.0 < 0.04,
        "default prior should hold 172 within 4%, got {default_bpm}"
    );

    let (center, sigma) = TempoPreset::Ambient.values();
    let low_bpm = run_bpm_convergence_with(
        172.0,
        10.0,
        TempoConfig {
            prior_center_bpm: center,
            prior_sigma: sigma,
            auto_prior: false,
        },
    );
    assert!(
        low_bpm > 40.0 && low_bpm < 95.0,
        "a prior centred at {center} should fold 172 to a metrical division, got {low_bpm}"
    );
}

/// Q2: when the rhythm itself carries octave evidence — strong hits on the
/// half-tempo grid, weaker ones between (a backbeat) — a low prior folds to
/// the SUPPORTED division (86), not whichever division sits closest to the
/// prior centre. This is the realistic fold `prior_center_decides_the_octave`
/// admits its clean impulse train cannot reproduce.
#[test]
fn backbeat_fold_prefers_the_supported_octave() {
    let bpm = run_backbeat_convergence(172.0, 10.0, TempoPreset::Ambient);
    assert!(
        (bpm - 86.0).abs() / 86.0 < 0.05,
        "backbeat at 172 under a low prior should fold to 86, got {bpm}"
    );
}

#[test]
fn tempo_control_mailbox_drains_once() {
    let mut ctl = TempoControl::default();
    ctl.push(TempoCommand::ShiftOctave(1));
    ctl.push(TempoCommand::Tap(128.0));
    assert_eq!(
        ctl.drain(),
        vec![TempoCommand::ShiftOctave(1), TempoCommand::Tap(128.0)]
    );
    assert!(ctl.drain().is_empty(), "commands must not be redelivered");
}

#[test]
fn tempo_control_mailbox_is_bounded() {
    // A stalled/absent audio thread must not let the mailbox grow without limit.
    let mut ctl = TempoControl::default();
    for _ in 0..100 {
        ctl.push(TempoCommand::ShiftOctave(1));
    }
    assert!(ctl.drain().len() <= 16);
}

/// Drive the estimator to a stable lock, then hand back the estimator for override tests.
fn locked_estimator(target_bpm: f64) -> TempoEstimator {
    let frame_rate = 100.0;
    let mut est = TempoEstimator::new(8.0, frame_rate, TempoConfig::default());
    let dt = 1.0 / frame_rate;
    let interval = 60.0 / target_bpm;
    let mut last = -1.0f64;
    for frame in 0..1000 {
        let t = frame as f64 * dt;
        let onset = if (t - last) >= interval - dt * 0.5 && t >= interval {
            last = t;
            1.0
        } else {
            0.0
        };
        est.update(onset, onset);
    }
    est
}

#[test]
fn octave_shift_survives_the_snap_escape() {
    // The regression this guards: shifting only the Kalman state is undone within ~30
    // updates by the snap-escape counter, because the autocorrelation keeps reporting the
    // octave the user just rejected. The offset must make the override stick.
    let mut est = locked_estimator(120.0);
    let before = est.current_bpm;
    assert!(before > 100.0 && before < 140.0, "setup: got {before}");

    est.shift_octave(-1);
    assert!(
        (est.current_bpm - before / 2.0).abs() < 5.0,
        "shift should halve the readout immediately, got {}",
        est.current_bpm
    );

    // Keep feeding the same 120 BPM signal well past the 30-update escape threshold.
    let dt = 0.01;
    let interval = 0.5;
    let mut last = -1.0f64;
    for frame in 0..800 {
        let t = frame as f64 * dt;
        let onset = if (t - last) >= interval - dt * 0.5 && t >= interval {
            last = t;
            1.0
        } else {
            0.0
        };
        est.update(onset, onset);
    }
    assert!(
        est.current_bpm < 80.0,
        "octave override must hold, got {} BPM back at full tempo",
        est.current_bpm
    );
}

#[test]
fn octave_shift_out_of_range_is_rejected() {
    let mut est = locked_estimator(170.0);
    let before = est.current_bpm;
    est.shift_octave(1); // 340 BPM > BPM_MAX
    assert_eq!(
        est.current_bpm, before,
        "out-of-range shift must be a no-op"
    );
}

#[test]
fn tap_tempo_locks_across_an_octave() {
    // A tap an octave away from the estimate is exactly the case the user is correcting,
    // so it must bypass the Kalman's octave-snap preprocessing rather than be swallowed.
    let mut est = locked_estimator(86.0);
    assert!(est.current_bpm < 100.0, "setup: got {}", est.current_bpm);
    est.tap(172.0);
    assert!(
        (est.current_bpm - 172.0).abs() < 1.0,
        "tap must win, got {}",
        est.current_bpm
    );
}

#[test]
fn tap_tempo_out_of_range_is_rejected() {
    let mut est = locked_estimator(120.0);
    let before = est.current_bpm;
    est.tap(700.0);
    assert_eq!(est.current_bpm, before, "out-of-range tap must be a no-op");
}

#[test]
fn auto_prior_walks_toward_the_detected_tempo() {
    let frame_rate = 100.0;
    let mut est = TempoEstimator::new(
        8.0,
        frame_rate,
        TempoConfig {
            prior_center_bpm: 150.0,
            prior_sigma: 0.4,
            auto_prior: true,
        },
    );
    let start = est.prior_center_bpm();
    let dt = 1.0 / frame_rate;
    let interval = 60.0 / 96.0;
    let mut last = -1.0f64;
    for frame in 0..12000 {
        let t = frame as f64 * dt;
        let onset = if (t - last) >= interval - dt * 0.5 && t >= interval {
            last = t;
            1.0
        } else {
            0.0
        };
        est.update(onset, onset);
    }
    let end = est.prior_center_bpm();
    assert!(
        end < start - 1.0,
        "auto prior should drift down from {start} toward ~96, ended at {end}"
    );
    assert!(
        end >= AUTO_PRIOR_MIN_BPM as f32 && end <= AUTO_PRIOR_MAX_BPM as f32,
        "auto prior must stay clamped, got {end}"
    );
}

#[test]
fn nonsense_config_values_are_clamped_not_trusted() {
    // settings.json is hand-editable; a 0 centre would make every prior weight exp(-inf).
    let mut est = TempoEstimator::new(8.0, 100.0, TempoConfig::default());
    est.set_config(TempoConfig {
        prior_center_bpm: 0.0,
        prior_sigma: 0.0,
        auto_prior: false,
    });
    assert!(
        est.prior_center_bpm().is_finite() && est.prior_center_bpm() >= BPM_MIN as f32,
        "centre must stay finite and in range, got {}",
        est.prior_center_bpm()
    );
    assert!(est.prior_sigma >= MIN_PRIOR_SIGMA);
    // The prior must still discriminate between candidates.
    assert!(est.tempo_prior_weight(BPM_MIN) > est.tempo_prior_weight(BPM_MAX * 0.9));
}

#[test]
fn auto_prior_ignores_the_config_center() {
    // In auto mode the estimator owns the centre — a stale UI value must not stomp it.
    let mut est = TempoEstimator::new(8.0, 100.0, TempoConfig::default());
    est.set_config(TempoConfig {
        prior_center_bpm: 70.0,
        prior_sigma: 0.5,
        auto_prior: true,
    });
    assert_eq!(est.prior_center_bpm(), 150.0);
    assert_eq!(est.prior_sigma, 0.5, "sigma must still track the config");
}
