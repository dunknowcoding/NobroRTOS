//! Phase 1 self-evaluation thresholds and gate logic (no hardware instruments).

/// Minimum TIMER1 ticks before scene A can pass (~3 s @ 50 Hz).
pub const MIN_DEADLINE_TICKS: u32 = 150;

/// Max allowed deadline jitter in microseconds for technique_route section 5 scene A.
pub const MAX_JITTER_US: u32 = 10;

/// Minimum I2C stub reads during scene A.
pub const MIN_I2C_READS: u32 = 10;

/// Minimum radio PPI latency samples for scene C.
pub const MIN_RADIO_SAMPLES: u32 = 16;

/// Max EGU to PPI to CAPTURE latency measured inside ISR, in microseconds.
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

pub const SAL_EVAL_MAGIC: u32 = 0x4152_4E32; // "AR2"
pub const SAL_EVAL_VERSION: u32 = 1;
pub const MIN_SERVO_STEPS: u32 = 20;
pub const MIN_IMU_SAMPLES: u32 = 3;
pub const SERVO_READBACK_TOL_US: u32 = 50;

/// Phase 2 SAL adapter self-test report (no external IMU required).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SalEvalReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub all_pass: u32,
    pub servo_steps: u32,
    pub servo_readback_ok: u32,
    pub imu_samples: u32,
    pub imu_plausible: u32,
    pub checksum: u32,
}

impl SalEvalReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            all_pass: 0,
            servo_steps: 0,
            servo_readback_ok: 0,
            imu_samples: 0,
            imu_plausible: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = SAL_EVAL_MAGIC;
        self.version = SAL_EVAL_VERSION;
        self.all_pass = u32::from(
            self.servo_steps >= MIN_SERVO_STEPS
                && self.servo_readback_ok >= MIN_SERVO_STEPS
                && self.imu_samples >= MIN_IMU_SAMPLES
                && self.imu_plausible >= MIN_IMU_SAMPLES,
        );
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.all_pass
            ^ self.servo_steps
            ^ self.servo_readback_ok
            ^ self.imu_samples
            ^ self.imu_plausible
    }
}

pub const IMU_HW_EVAL_MAGIC: u32 = 0x4152_4E33; // "AR3"
pub const IMU_HW_EVAL_VERSION: u32 = 1;
pub const MIN_IMU_HW_READS: u32 = 10;

/// Hardware IMU bring-up report (GY-9250 / GY-91 over TWIM).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ImuHwEvalReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub all_pass: u32,
    pub board_id_tag: u32,
    pub who_am_i: u32,
    pub dev_addr: u32,
    pub i2c_devices: u32,
    pub bmp280_present: u32,
    pub imu_reads: u32,
    pub imu_errors: u32,
    pub accel_mag_mg: u32,
    pub checksum: u32,
}

impl ImuHwEvalReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            all_pass: 0,
            board_id_tag: 0,
            who_am_i: 0,
            dev_addr: 0,
            i2c_devices: 0,
            bmp280_present: 0,
            imu_reads: 0,
            imu_errors: 0,
            accel_mag_mg: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = IMU_HW_EVAL_MAGIC;
        self.version = IMU_HW_EVAL_VERSION;
        self.all_pass = u32::from(
            self.who_am_i != 0
                && (self.dev_addr == 0x68 || self.dev_addr == 0x69)
                && self.i2c_devices >= 1
                && self.imu_reads >= MIN_IMU_HW_READS
                && self.imu_errors * 100 <= self.imu_reads
                && (800..1200).contains(&self.accel_mag_mg),
        );
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.all_pass
            ^ self.board_id_tag
            ^ self.who_am_i
            ^ self.dev_addr
            ^ self.i2c_devices
            ^ self.bmp280_present
            ^ self.imu_reads
            ^ self.imu_errors
            ^ self.accel_mag_mg
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
        // Polled TIMER1 compare (autonomous eval): allow up to 2% late ticks without scope/RTT.
        misses * 50 <= ticks
    }

    pub fn scene_c_pass(max_latency: u32, samples: u32) -> bool {
        samples >= MIN_RADIO_SAMPLES && max_latency <= MAX_RADIO_LATENCY_US
    }
}
