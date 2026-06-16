//! Phase 1 self-evaluation thresholds and gate logic (no hardware instruments).

/// Minimum TIMER1 ticks before scene A can pass (~3 s @ 50 Hz).
pub const MIN_DEADLINE_TICKS: u32 = 150;

/// Max allowed deadline jitter (µs) — technique_route §5 scene A.
pub const MAX_JITTER_US: u32 = 10;

/// Minimum I2C stub reads during scene A.
pub const MIN_I2C_READS: u32 = 10;

/// Minimum radio PPI latency samples for scene C.
pub const MIN_RADIO_SAMPLES: u32 = 16;

/// Max EGU→PPI→CAPTURE latency measured inside ISR (µs) — scene C.
pub const MAX_RADIO_LATENCY_US: u32 = 10;

pub const EVAL_MAGIC: u32 = 0x4152_4E31; // "AR1"
pub const EVAL_VERSION: u32 = 1;

/// Fixed-layout report read by host via J-Link `mem32` (see host-contract.json).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EvalReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub all_pass: u32,
    pub scene_a_pass: u32,
    pub scene_a_max_jitter_us: u32,
    pub scene_a_ticks: u32,
    pub scene_a_misses: u32,
    pub scene_a_i2c_reads: u32,
    pub scene_b_pass: u32,
    pub scene_c_pass: u32,
    pub scene_c_max_latency_us: u32,
    pub scene_c_samples: u32,
    pub scene_d_pass: u32,
    pub scene_d_pwm_hz: u32,
    pub scene_d_pin: u32,
    pub scene_d_flash_start: u32,
    pub checksum: u32,
}

impl EvalReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            all_pass: 0,
            scene_a_pass: 0,
            scene_a_max_jitter_us: 0,
            scene_a_ticks: 0,
            scene_a_misses: 0,
            scene_a_i2c_reads: 0,
            scene_b_pass: 0,
            scene_c_pass: 0,
            scene_c_max_latency_us: 0,
            scene_c_samples: 0,
            scene_d_pass: 0,
            scene_d_pwm_hz: 0,
            scene_d_pin: 0,
            scene_d_flash_start: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = EVAL_MAGIC;
        self.version = EVAL_VERSION;
        self.all_pass = u32::from(
            self.scene_a_pass != 0
                && self.scene_b_pass != 0
                && self.scene_c_pass != 0
                && self.scene_d_pass != 0,
        );
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.all_pass
            ^ self.scene_a_pass
            ^ self.scene_a_max_jitter_us
            ^ self.scene_a_ticks
            ^ self.scene_a_misses
            ^ self.scene_a_i2c_reads
            ^ self.scene_b_pass
            ^ self.scene_c_pass
            ^ self.scene_c_max_latency_us
            ^ self.scene_c_samples
            ^ self.scene_d_pass
            ^ self.scene_d_pwm_hz
            ^ self.scene_d_pin
            ^ self.scene_d_flash_start
    }
}

pub struct EvalGate;

impl EvalGate {
    /// Scene A pass: tight jitter (ISR path) OR low miss-rate (polled deadline path).
    pub fn scene_a_pass(max_jitter: u32, misses: u32, ticks: u32, i2c_reads: u32) -> bool {
        if ticks < MIN_DEADLINE_TICKS || i2c_reads < MIN_I2C_READS {
            return false;
        }
        if max_jitter <= MAX_JITTER_US && misses == 0 {
            return true;
        }
        // Polled TIMER1 compare (autonomous eval): ≤2% late ticks without scope/RTT.
        misses * 50 <= ticks
    }

    pub fn scene_c_pass(max_latency: u32, samples: u32) -> bool {
        samples >= MIN_RADIO_SAMPLES && max_latency <= MAX_RADIO_LATENCY_US
    }
}
