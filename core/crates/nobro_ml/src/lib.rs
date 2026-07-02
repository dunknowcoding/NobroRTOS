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
