//! No-heap embedded ML / DSP primitives (f32, no sqrt - FPU-friendly, bounded).
//! - [`RunningStats`] streaming mean/variance + z^2 anomaly test (M34)
//! - [`Ewma`] online adaptive baseline (M40)
//! - [`reject`] confidence-threshold reject option (M42)
//! - [`complementary`] two-source sensor fusion (M46)
#![cfg_attr(not(test), no_std)]

/// Welford streaming mean/variance (no stored window).
#[derive(Clone, Copy, Debug, Default)]
pub struct RunningStats {
    n: u32,
    mean: f32,
    m2: f32,
}

impl RunningStats {
    pub const fn new() -> Self {
        Self { n: 0, mean: 0.0, m2: 0.0 }
    }
    pub fn update(&mut self, x: f32) {
        self.n += 1;
        let d = x - self.mean;
        self.mean += d / self.n as f32;
        self.m2 += d * (x - self.mean);
    }
    pub fn mean(&self) -> f32 {
        self.mean
    }
    pub fn variance(&self) -> f32 {
        if self.n < 2 {
            0.0
        } else {
            self.m2 / (self.n - 1) as f32
        }
    }
    /// (x - mean)^2 / variance - a z-score squared, no sqrt needed.
    pub fn z_squared(&self, x: f32) -> f32 {
        let v = self.variance();
        if v <= 0.0 {
            0.0
        } else {
            let d = x - self.mean;
            d * d / v
        }
    }
    /// Anomaly if the point is more than `k` standard deviations from the mean.
    pub fn is_anomaly(&self, x: f32, k: f32) -> bool {
        self.n >= 2 && self.z_squared(x) > k * k
    }
}

/// Exponentially-weighted moving average - an online adaptive baseline.
#[derive(Clone, Copy, Debug)]
pub struct Ewma {
    value: f32,
    alpha: f32,
    primed: bool,
}

impl Ewma {
    pub const fn new(alpha: f32) -> Self {
        Self { value: 0.0, alpha, primed: false }
    }
    pub fn update(&mut self, x: f32) -> f32 {
        if self.primed {
            self.value += self.alpha * (x - self.value);
        } else {
            self.value = x;
            self.primed = true;
        }
        self.value
    }
    pub fn value(&self) -> f32 {
        self.value
    }
}

/// Confidence-threshold reject: argmax class only if its score clears `threshold`,
/// else `None` ("unknown"/abstain).
pub fn reject(scores: &[f32], threshold: f32) -> Option<usize> {
    let mut best = 0usize;
    let mut best_v = f32::MIN;
    for (i, &s) in scores.iter().enumerate() {
        if s > best_v {
            best_v = s;
            best = i;
        }
    }
    if best_v >= threshold {
        Some(best)
    } else {
        None
    }
}

/// Complementary filter fusing two estimates of the same quantity (e.g. accel-tilt and
/// gyro-integrated angle): `alpha*a + (1-alpha)*b`.
pub fn complementary(a: f32, b: f32, alpha: f32) -> f32 {
    alpha * a + (1.0 - alpha) * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anomaly_flags_outliers_only() {
        let mut s = RunningStats::new();
        for x in [1000.0f32, 1001.0, 999.0, 1000.0, 1002.0, 998.0] {
            s.update(x);
        }
        assert!(!s.is_anomaly(1001.0, 3.0)); // within noise
        assert!(s.is_anomaly(1200.0, 3.0)); // far outlier
    }

    #[test]
    fn ewma_tracks_and_adapts() {
        let mut e = Ewma::new(0.5);
        assert_eq!(e.update(100.0), 100.0); // primes to first value
        let v = e.update(200.0);
        assert!((v - 150.0).abs() < 1e-3); // 0.5*200 + 0.5*100
    }

    #[test]
    fn reject_abstains_below_threshold() {
        assert_eq!(reject(&[0.1, 0.85, 0.05], 0.6), Some(1));
        assert_eq!(reject(&[0.4, 0.35, 0.25], 0.6), None); // low confidence -> abstain
    }

    #[test]
    fn complementary_fuses() {
        assert!((complementary(10.0, 20.0, 0.98) - 10.2).abs() < 1e-3);
    }
}

/// One node's prediction in a distributed inference round (M60).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vote {
    pub class: u8,
    pub confidence_milli: u16,
}

/// Confidence-weighted majority vote across mesh nodes: each node runs the model locally
/// and reports (class, confidence); the coordinator fuses them into one decision plus an
/// overall confidence (winning mass / total mass, in milli). (M60)
pub fn ensemble_vote(votes: &[Vote], max_classes: usize) -> Option<(u8, u16)> {
    if votes.is_empty() {
        return None;
    }
    let n = max_classes.min(8);
    let mut acc = [0u32; 8];
    for v in votes {
        let c = v.class as usize;
        if c < n {
            acc[c] += u32::from(v.confidence_milli);
        }
    }
    let total: u32 = acc[..n].iter().sum();
    if total == 0 {
        return None;
    }
    let mut best = 0usize;
    for c in 1..n {
        if acc[c] > acc[best] {
            best = c;
        }
    }
    let conf = (acc[best] * 1000 / total) as u16;
    Some((best as u8, conf))
}

#[cfg(test)]
mod ensemble_tests {
    use super::*;

    #[test]
    fn ensemble_fuses_distributed_votes() {
        // three nodes: two vote class 1, one votes class 0; weighted by confidence.
        let votes = [
            Vote { class: 1, confidence_milli: 900 },
            Vote { class: 0, confidence_milli: 600 },
            Vote { class: 1, confidence_milli: 800 },
        ];
        let (cls, conf) = ensemble_vote(&votes, 3).unwrap();
        assert_eq!(cls, 1); // 1700 mass vs 600
        assert_eq!(conf, 739); // 1700*1000/2300
        assert_eq!(ensemble_vote(&[], 3), None);
    }
}

/// Gesture classes recognized by [`GestureDetector`] (M143).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    None,
    Tap,
    Shake,
    Tilt,
}

/// Streaming IMU gesture recognition over |accel| magnitude samples (milli-g), no heap.
///
/// - `Tap`: a short excursion above `spike` mg that returns to baseline within
///   `tap_max_len` samples.
/// - `Shake`: at least `shake_swings` alternating above/below-baseline swings of
///   amplitude > `swing` mg inside one window.
/// - `Tilt`: the recent mean shifts from the calibrated baseline by more than `tilt` mg
///   and stays there (sustained, not oscillating).
pub struct GestureDetector {
    baseline: Ewma,
    spike: i32,
    swing: i32,
    tilt: i32,
    tap_max_len: u8,
    shake_swings: u8,
    excursion_len: u8,
    swings: u8,
    last_sign: i8,
    sustained: u8,
}

impl GestureDetector {
    pub fn new(spike_mg: i32, swing_mg: i32, tilt_mg: i32) -> Self {
        Self {
            baseline: Ewma::new(0.02),
            spike: spike_mg,
            swing: swing_mg,
            tilt: tilt_mg,
            tap_max_len: 6,
            shake_swings: 6,
            excursion_len: 0,
            swings: 0,
            last_sign: 0,
            sustained: 0,
        }
    }

    /// Calibrate the resting baseline from a quiet sample (call during startup).
    pub fn calibrate(&mut self, mag_mg: i32) {
        self.baseline.update(mag_mg as f32);
    }

    /// Feed one |accel| sample; returns the gesture completed at this sample, if any.
    pub fn update(&mut self, mag_mg: i32) -> Gesture {
        let base = self.baseline.value() as i32;
        let dev = mag_mg - base;

        // Track alternating swings for shake.
        let sign: i8 = if dev > self.swing {
            1
        } else if dev < -self.swing {
            -1
        } else {
            0
        };
        if sign != 0 && sign != self.last_sign {
            if self.last_sign != 0 {
                self.swings = self.swings.saturating_add(1);
            }
            self.last_sign = sign;
        }
        if self.swings >= self.shake_swings {
            self.swings = 0;
            self.last_sign = 0;
            self.excursion_len = 0;
            self.sustained = 0;
            return Gesture::Shake;
        }

        // Track a single spike excursion for tap.
        if dev.abs() > self.spike {
            self.excursion_len = self.excursion_len.saturating_add(1);
        } else if self.excursion_len > 0 {
            let len = self.excursion_len;
            self.excursion_len = 0;
            // A short, isolated excursion (not part of an ongoing shake) is a tap.
            if len <= self.tap_max_len && self.swings <= 1 {
                self.swings = 0;
                self.last_sign = 0;
                return Gesture::Tap;
            }
        }

        // Track a sustained baseline shift for tilt (quiet but offset).
        if dev.abs() > self.tilt && dev.abs() < self.spike && sign == 0 {
            self.sustained = self.sustained.saturating_add(1);
            if self.sustained >= 20 {
                self.sustained = 0;
                return Gesture::Tilt;
            }
        } else if dev.abs() <= self.tilt {
            self.sustained = 0;
            // Quiet sample: let the baseline slowly re-adapt.
            self.baseline.update(mag_mg as f32);
        }

        Gesture::None
    }
}

#[cfg(test)]
mod gesture_tests {
    use super::*;

    fn detector() -> GestureDetector {
        let mut g = GestureDetector::new(400, 250, 80);
        for _ in 0..50 {
            g.calibrate(1000);
        }
        g
    }

    fn feed(g: &mut GestureDetector, samples: &[i32]) -> Gesture {
        let mut got = Gesture::None;
        for &s in samples {
            let r = g.update(s);
            if r != Gesture::None {
                got = r;
            }
        }
        got
    }

    #[test]
    fn tap_is_a_short_isolated_spike() {
        let mut g = detector();
        let mut w = [1000i32; 40];
        w[20] = 1600;
        w[21] = 1700;
        w[22] = 1550; // 3-sample spike then back
        assert_eq!(feed(&mut g, &w), Gesture::Tap);
    }

    #[test]
    fn shake_is_alternating_swings() {
        let mut g = detector();
        let mut w = [1000i32; 40];
        for i in 0..16 {
            w[10 + i] = if i % 2 == 0 { 1400 } else { 600 };
        }
        assert_eq!(feed(&mut g, &w), Gesture::Shake);
    }

    #[test]
    fn tilt_is_a_sustained_offset() {
        let mut g = detector();
        let w = [1150i32; 40]; // +150 mg sustained, quiet
        assert_eq!(feed(&mut g, &w), Gesture::Tilt);
    }

    #[test]
    fn idle_stays_none() {
        let mut g = detector();
        let mut w = [1000i32; 60];
        for (i, v) in w.iter_mut().enumerate() {
            *v += (i as i32 % 5) - 2; // +/-2 mg noise
        }
        assert_eq!(feed(&mut g, &w), Gesture::None);
    }
}


// ---- TinyML building blocks (M138/M139/M141/M142/M144/M145) ----

/// Symmetric int8 quantization helper (M139): map a float to int8 with a given scale,
/// and back. `scale` = max_abs / 127. Round-half-to-even-ish via +0.5 on magnitude.
pub fn quantize_i8(x: f32, scale: f32) -> i8 {
    if scale <= 0.0 {
        return 0;
    }
    let q = x / scale;
    let r = if q >= 0.0 { q + 0.5 } else { q - 0.5 } as i32;
    r.clamp(-127, 127) as i8
}
pub fn dequantize_i8(q: i8, scale: f32) -> f32 {
    q as f32 * scale
}

/// Choose a symmetric scale for a tensor so its max magnitude maps to 127 (M139).
pub fn choose_scale(values: &[f32]) -> f32 {
    let m = values.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
    if m <= 0.0 {
        1.0
    } else {
        m / 127.0
    }
}

/// Depthwise 3x1 conv over an int8 channel (M138): the core op of a depthwise-separable
/// block, integer MAC with a requantizing right shift. `pad`-with-zeros at the ends.
pub fn depthwise_conv3(input: &[i8], kernel: [i8; 3], shift: u32, out: &mut [i8]) {
    let n = input.len();
    for i in 0..n.min(out.len()) {
        let l = if i > 0 { input[i - 1] as i32 } else { 0 };
        let c = input[i] as i32;
        let r = if i + 1 < n { input[i + 1] as i32 } else { 0 };
        let acc =
            l * kernel[0] as i32 + c * kernel[1] as i32 + r * kernel[2] as i32;
        out[i] = (acc >> shift).clamp(-127, 127) as i8;
    }
}

/// Federated averaging of model weights across nodes (M141): element-wise mean of each
/// node's weight vector, sample-count weighted (FedAvg).
pub fn fed_average(node_weights: &[&[f32]], node_samples: &[u32], out: &mut [f32]) -> bool {
    if node_weights.is_empty() || node_weights.len() != node_samples.len() {
        return false;
    }
    let dim = node_weights[0].len();
    if out.len() < dim || node_weights.iter().any(|w| w.len() != dim) {
        return false;
    }
    let total: u64 = node_samples.iter().map(|&s| u64::from(s)).sum();
    if total == 0 {
        return false;
    }
    for (j, o) in out.iter_mut().enumerate().take(dim) {
        let mut acc = 0.0f64;
        for (w, &s) in node_weights.iter().zip(node_samples) {
            acc += f64::from(w[j]) * f64::from(s);
        }
        *o = (acc / total as f64) as f32;
    }
    true
}

/// Magnitude pruning (M144): zero the weights whose |value| falls below the `keep`-th
/// percentile so only the largest survive; returns the count pruned. `sparsity` in [0,1].
pub fn prune_magnitude(weights: &mut [f32], sparsity: f32) -> usize {
    let n = weights.len();
    if n == 0 {
        return 0;
    }
    let k = ((sparsity.clamp(0.0, 1.0)) * n as f32) as usize;
    if k == 0 {
        return 0;
    }
    // find the k-th smallest magnitude as a threshold (selection via repeated scan; n is
    // small for embedded models). Bounded, no alloc.
    let mut threshold = f32::INFINITY;
    for _ in 0..k {
        let mut lo = f32::INFINITY;
        for &w in weights.iter() {
            let m = w.abs();
            if m < lo && m > last_below(weights, threshold) {
                lo = m;
            }
        }
        threshold = lo;
    }
    let mut pruned = 0;
    for w in weights.iter_mut() {
        if w.abs() <= threshold {
            *w = 0.0;
            pruned += 1;
        }
    }
    pruned
}

fn last_below(_w: &[f32], t: f32) -> f32 {
    if t.is_infinite() {
        -1.0
    } else {
        t
    }
}

/// Minimal GRU cell (M142): single-unit gated recurrent update for sequence features.
/// Weights packed as (wz, uz, bz, wr, ur, br, wh, uh, bh). f32, FPU-friendly.
#[derive(Clone, Copy, Debug)]
pub struct GruCell {
    pub wz: f32,
    pub uz: f32,
    pub bz: f32,
    pub wr: f32,
    pub ur: f32,
    pub br: f32,
    pub wh: f32,
    pub uh: f32,
    pub bh: f32,
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + exp_approx(-x))
}
fn tanh_approx(x: f32) -> f32 {
    let e = exp_approx(2.0 * x);
    (e - 1.0) / (e + 1.0)
}
fn exp_approx(x: f32) -> f32 {
    // clamp + limited Taylor/scaling; adequate for gate activations
    let x = x.clamp(-10.0, 10.0);
    let mut term = 1.0f32;
    let mut sum = 1.0f32;
    for i in 1..16 {
        term *= x / i as f32;
        sum += term;
    }
    sum
}

impl GruCell {
    /// Advance one step given input `x` and previous hidden `h`; returns the new hidden.
    pub fn step(&self, x: f32, h: f32) -> f32 {
        let z = sigmoid(self.wz * x + self.uz * h + self.bz);
        let r = sigmoid(self.wr * x + self.ur * h + self.br);
        let hh = tanh_approx(self.wh * x + self.uh * (r * h) + self.bh);
        (1.0 - z) * h + z * hh
    }
}

/// Multi-model RAM-budget scheduler (M145): pick which models can be co-resident given a
/// total arena, choosing greedily by priority then by smallest footprint. Returns a bit
/// mask of admitted models.
pub fn schedule_models(footprints: &[u32], priorities: &[u8], arena: u32) -> u32 {
    let n = footprints.len().min(32).min(priorities.len());
    // order indices by (priority desc, footprint asc)
    let mut order = [0usize; 32];
    for (i, slot) in order.iter_mut().enumerate().take(n) {
        *slot = i;
    }
    for i in 1..n {
        let mut j = i;
        while j > 0 {
            let a = order[j - 1];
            let b = order[j];
            let better = priorities[b] > priorities[a]
                || (priorities[b] == priorities[a] && footprints[b] < footprints[a]);
            if better {
                order.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
    }
    let mut used = 0u32;
    let mut mask = 0u32;
    for &idx in order.iter().take(n) {
        if used + footprints[idx] <= arena {
            used += footprints[idx];
            mask |= 1 << idx;
        }
    }
    mask
}

#[cfg(test)]
mod tinyml_tests {
    use super::*;

    #[test]
    fn quantize_roundtrip_is_close() {
        let vals = [0.0f32, 0.5, -0.9, 1.2, -1.2];
        let scale = choose_scale(&vals);
        for &v in &vals {
            let q = quantize_i8(v, scale);
            assert!((dequantize_i8(q, scale) - v).abs() <= scale);
        }
        assert_eq!(quantize_i8(1.2, scale), 127); // max maps to full scale
    }

    #[test]
    fn depthwise_conv_smooths() {
        let input = [0i8, 0, 100, 0, 0];
        let mut out = [0i8; 5];
        depthwise_conv3(&input, [1, 1, 1], 0, &mut out); // box filter, no shift
        assert_eq!(out, [0, 100, 100, 100, 0]);
    }

    #[test]
    fn fed_average_is_sample_weighted() {
        let a = [0.0f32, 10.0];
        let b = [2.0f32, 20.0];
        let mut out = [0.0f32; 2];
        // 3:1 sample weighting toward node a
        assert!(fed_average(&[&a, &b], &[3, 1], &mut out));
        assert!((out[0] - 0.5).abs() < 1e-5); // (0*3 + 2*1)/4
        assert!((out[1] - 12.5).abs() < 1e-5);
    }

    #[test]
    fn prune_zeros_smallest_weights() {
        let mut w = [0.1f32, -5.0, 0.2, 9.0, -0.3];
        let pruned = prune_magnitude(&mut w, 0.6); // keep top ~2
        assert_eq!(pruned, 3);
        assert_eq!(w[0], 0.0);
        assert_eq!(w[2], 0.0);
        assert_eq!(w[4], 0.0);
        assert_eq!(w[1], -5.0);
        assert_eq!(w[3], 9.0);
    }

    #[test]
    fn gru_cell_is_bounded_and_reacts() {
        let g = GruCell {
            wz: 1.0, uz: 1.0, bz: 0.0, wr: 1.0, ur: 1.0, br: 0.0,
            wh: 1.0, uh: 1.0, bh: 0.0,
        };
        let mut h = 0.0f32;
        for _ in 0..10 {
            h = g.step(1.0, h);
            assert!(h.abs() <= 1.5, "hidden diverged: {h}");
        }
        assert!(h > 0.0); // positive input drives positive hidden
    }

    #[test]
    fn schedule_admits_high_priority_within_arena() {
        // footprints, priorities; arena fits ~2 of these
        let fp = [40u32, 30, 50, 10];
        let pr = [1u8, 3, 2, 3];
        let mask = schedule_models(&fp, &pr, 60);
        // highest priority (idx1=30, idx3=10) admitted first (both prio 3), total 40<=60,
        // then idx2 (prio2, 50) does not fit; idx0 (prio1) does not fit.
        assert_eq!(mask & (1 << 1), 1 << 1);
        assert_eq!(mask & (1 << 3), 1 << 3);
        assert_eq!(mask & (1 << 2), 0);
    }
}
