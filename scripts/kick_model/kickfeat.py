"""Features, labels and scoring for the kick model, matched to the live analyzer.

`hop_features` must compute exactly what `audio/kick_model.rs` computes from the
analyzer's spectra, or the shipped model scores inputs it was never trained on. The
frames match `FftResolution::compute` in `audio/analyzer.rs`: the frame ending at each
512-sample hop, a symmetric Hann window, magnitudes scaled by 2 / sum(window).
"""

import bisect

import numpy as np

HOP = 512
FLUX_FLOOR = 2e-3  # analyzer.rs FLUX_FLOOR
EDGES = [30, 60, 90, 130, 180, 250, 400, 700, 1200, 2500, 5000, 10000]
LARGE_FFT_MAX_HZ = 250
CONTEXT = 3


def frames(mono, n):
    """Magnitude spectrum of the frame ending at each hop boundary."""
    w = 0.5 * (1 - np.cos(2 * np.pi * np.arange(n) / (n - 1)))
    hops = len(mono) // HOP
    pad = np.concatenate([np.zeros(n), mono])
    idx = (np.arange(hops)[:, None] + 1) * HOP + np.arange(n)[None, :]
    return np.abs(np.fft.rfft(pad[idx] * w, axis=1)) * (2.0 / w.sum())


def logflux(mag, lo, hi):
    lm = np.log(np.maximum(mag[:, lo:hi], FLUX_FLOOR))
    d = np.diff(lm, axis=0, prepend=lm[:1])
    return np.maximum(d, 0).sum(axis=1) / (hi - lo)


def hop_features(mono, sr):
    """Per-hop model inputs: this hop's 24 features and the previous two hops'."""
    L, M = frames(mono, 4096), frames(mono, 1024)
    bh_l, bh_m = sr / 4096, sr / 1024

    def band(lo, hi):
        X, bh = (L, bh_l) if hi <= LARGE_FFT_MAX_HZ else (M, bh_m)
        a = round(lo / bh)  # Python rounds half to even; kick_model.rs does the same
        return X, max(1, a), max(a + 1, round(hi / bh))

    bands = [band(lo, hi) for lo, hi in zip(EDGES[:-1], EDGES[1:])]
    cols = [logflux(X, a, b) for X, a, b in bands]
    lv = np.stack([np.log(np.maximum((X[:, a:b] ** 2).mean(1), 1e-12)) for X, a, b in bands], 1)
    cols += list((lv - lv.mean(1, keepdims=True)).T)
    seg = L[:, round(30 / bh_l): round(250 / bh_l)]
    cols.append(np.exp(np.log(np.maximum(seg, 1e-9)).mean(1)) / np.maximum(seg.mean(1), 1e-12))
    cols.append(seg.max(1) / np.maximum(seg.mean(1), 1e-12))
    F = np.stack(cols, 1).astype(np.float32)
    ctx = [F] + [np.vstack([np.zeros((k, F.shape[1]), np.float32), F[:-k]]) for k in range(1, CONTEXT)]
    return np.hstack(ctx)


def hop_labels(n_hops, sr, kicks, lag_hops=(0, 1, 2)):
    """1 on the hops whose analysis frame has just taken in a kick's rise."""
    y = np.zeros(n_hops, np.int8)
    for k in kicks:
        h = int(np.floor(k * sr / HOP))
        for d in lag_hops:
            if 0 <= h + d < n_hops:
                y[h + d] = 1
    return y


def kick_from_probability(p, thr):
    """kick_model.rs `kick_from_probability`: thr maps to 0.5, linear either side."""
    return np.clip(np.where(p < thr, 0.5 * p / thr, 0.5 + 0.5 * (p - thr) / (1 - thr)), 0, 1)


def app_events(kick, sr, attack=0.002, release=0.06):
    """What a consumer sees: the FeatureSmoother (schema `kick` = ar(0.002, 0.06)), then the
    0.5 / 0.3 hysteresis with a 0.1 s minimum gap (signal/emitter.rs drums onset)."""
    dt = HOP / sr
    up, dn = 1 - np.exp(-dt / max(attack, 1e-3)), 1 - np.exp(-dt / max(release, 1e-3))
    out, st, armed, last = [], 0.0, True, -1e9
    for i, v in enumerate(kick):
        st += (up if v > st else dn) * (v - st)
        ts = (i + 1) * HOP / sr
        if st < 0.3:
            armed = True
        if armed and st >= 0.5 and ts - last >= 0.10:
            out.append(ts)
            armed, last = False, ts
    return out


def near(xs, x, tol=0.05):
    i = bisect.bisect_left(xs, x - tol)
    return i < len(xs) and xs[i] <= x + tol


def score(det, kicks):
    """Precision and recall of detections against kick times, matched within 50 ms."""
    tp = sum(near(kicks, x) for x in det)
    rec = sum(near(det, k) for k in kicks)
    return tp / max(len(det), 1), rec / max(len(kicks), 1)
