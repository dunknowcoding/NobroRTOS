//! No-heap sensor utilities, transport-agnostic.
//! - [`SensorHealth`] stale / stuck / out-of-range fault detection (M47)
//! - [`Calibration`] streaming bias (offset) calibration (M48)
//! - [`Decimator`] sample-rate decimation / downsampling (M49)
#![cfg_attr(not(test), no_std)]

/// Detects sensor faults: a stuck value, a stale stream, or out-of-range readings.
pub struct SensorHealth {
    last: i32,
    same_count: u16,
    min: i32,
    max: i32,
    primed: bool,
}

impl SensorHealth {
    pub const fn new(min: i32, max: i32) -> Self {
        Self { last: 0, same_count: 0, min, max, primed: false }
    }
    pub fn update(&mut self, value: i32) {
        if self.primed && value == self.last {
            self.same_count = self.same_count.saturating_add(1);
        } else {
            self.same_count = 0;
        }
        self.last = value;
        self.primed = true;
    }
    /// The reading hasn't changed for `n` updates (a stuck/frozen sensor).
    pub fn is_stuck(&self, n: u16) -> bool {
        self.same_count >= n
    }
    pub fn out_of_range(&self, value: i32) -> bool {
        value < self.min || value > self.max
    }
}

/// Streaming offset calibration: average N samples at a known reference, store the bias.
#[derive(Clone, Copy, Debug, Default)]
pub struct Calibration {
    acc: i64,
    n: u32,
    bias: i32,
}

impl Calibration {
    pub const fn new() -> Self {
        Self { acc: 0, n: 0, bias: 0 }
    }
    pub fn observe(&mut self, raw: i32) {
        self.acc += i64::from(raw);
        self.n += 1;
    }
    /// Compute the bias = measured_mean - reference (call once after observing).
    pub fn finalize(&mut self, reference: i32) {
        if self.n > 0 {
            self.bias = (self.acc / i64::from(self.n)) as i32 - reference;
        }
    }
    pub fn apply(&self, raw: i32) -> i32 {
        raw - self.bias
    }
    pub fn bias(&self) -> i32 {
        self.bias
    }
}

/// Decimator: emit every `factor`-th sample (downsampling a fast stream).
pub struct Decimator {
    factor: u16,
    count: u16,
}

impl Decimator {
    pub const fn new(factor: u16) -> Self {
        Self { factor: if factor == 0 { 1 } else { factor }, count: 0 }
    }
    /// Call per input sample; returns true when an output should be emitted.
    pub fn tick(&mut self) -> bool {
        self.count += 1;
        if self.count >= self.factor {
            self.count = 0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_flags_stuck_and_range() {
        let mut h = SensorHealth::new(800, 1200);
        for _ in 0..5 {
            h.update(1000);
        }
        assert!(h.is_stuck(4)); // frozen at 1000
        assert!(h.out_of_range(1500));
        assert!(!h.out_of_range(1000));
        h.update(1001);
        assert!(!h.is_stuck(4)); // changed -> not stuck
    }

    #[test]
    fn calibration_removes_bias() {
        let mut c = Calibration::new();
        for v in [1050, 1048, 1052, 1050] {
            c.observe(v); // sensor reads ~1050 when true value is 1000
        }
        c.finalize(1000);
        assert_eq!(c.bias(), 50);
        assert_eq!(c.apply(1050), 1000);
    }

    #[test]
    fn decimator_downsamples() {
        let mut d = Decimator::new(4);
        let emits: u32 = (0..16).map(|_| u32::from(d.tick())).sum();
        assert_eq!(emits, 4); // 16 in / 4 = 4 out
    }
}

/// Triple-modular-redundancy vote (M165): given three redundant readings, return the
/// median (tolerates one arbitrary fault) and whether the sources agree within `tol`.
pub fn tmr_vote(a: i32, b: i32, c: i32, tol: i32) -> (i32, bool) {
    // median of three
    let median = a.max(b).min(a.min(b).max(c));
    let agree = (a - median).abs() <= tol && (b - median).abs() <= tol && (c - median).abs() <= tol;
    (median, agree)
}

#[cfg(test)]
mod tmr_tests {
    use super::*;

    #[test]
    fn tmr_masks_a_single_fault() {
        // one sensor wildly wrong -> median still correct, disagreement flagged
        let (v, agree) = tmr_vote(1000, 1002, 5000, 10);
        assert_eq!(v, 1002);
        assert!(!agree);
        // all healthy -> agreement
        let (v, agree) = tmr_vote(1000, 1002, 999, 10);
        assert_eq!(v, 1000);
        assert!(agree);
    }
}
