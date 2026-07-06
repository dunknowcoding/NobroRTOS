//! Edge-AI on hardware: read the IMU, run the bounded on-device motion classifier
//! (AiInferenceSal) over a sliding window, and record the class, confidence, and
//! measured latency in NOBRO_AI_EVAL_REPORT. At rest the board is classified IDLE
//! with high confidence, well inside the model's declared timeout - proving the AI
//! inference contract runs on a real board.
#![no_std]
#![no_main]

use cortex_m::asm;
use defmt_rtt as _;
use panic_halt as _;

use nobro_adapter_motion_ai::{MotionClassifier, CLASS_IDLE, MODEL_ID};
use nobro_adapter_mpu9250_imu::{accel_mag_mg, Mpu9250Imu};
use nobro_hal::{
    lease::Resource,
    traits::{HalClock, HalLease, PlatformHal},
    ActivePlatform as Hal,
};
use nobro_kernel::{pool::SamplePool, ImuPayload};
use nobro_sal::{AiInferenceRequest, AiInferenceSal, SensorSal};

#[repr(C)]
#[derive(Clone, Copy)]
struct AiEvalReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    model_id: u32,
    inferences: u32,
    last_class: u32,
    confidence_q15: u32,
    latency_us: u32,
    accel_mag_mg: u32,
    checksum: u32,
}

impl AiEvalReport {
    const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            all_pass: 0,
            model_id: 0,
            inferences: 0,
            last_class: 0,
            confidence_q15: 0,
            latency_us: 0,
            accel_mag_mg: 0,
            checksum: 0,
        }
    }
}

const AI_MAGIC: u32 = 0x4E42_4149; // "NBAI"
const OWNER_TWIM: u8 = 3;
const WIN: usize = 16;
const MIN_CONFIDENCE_Q15: u32 = 16_000;

#[no_mangle]
#[used]
static mut NOBRO_AI_EVAL_REPORT: AiEvalReport = AiEvalReport::zeroed();

fn idle() -> ! {
    loop {
        asm::delay(16_000_000);
    }
}

#[cortex_m_rt::entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, OWNER_TWIM).ok();
    let mut imu = match Mpu9250Imu::probe_and_init(OWNER_TWIM) {
        Ok(d) => d,
        Err(_) => idle(),
    };

    let mut clf = MotionClassifier::new();
    let contract = clf.contract();
    unsafe {
        NOBRO_AI_EVAL_REPORT.magic = AI_MAGIC;
        NOBRO_AI_EVAL_REPORT.version = 1;
        NOBRO_AI_EVAL_REPORT.model_id = MODEL_ID;
    }

    let mut win = [0u16; WIN];
    let mut widx = 0usize;
    let mut inferences = 0u32;
    let mut last_accel = 0u32;

    loop {
        if let Ok(Some(sample)) = imu.poll() {
            if let Some(p) = ImuPayload::read_from_handle(sample.handle) {
                let mg = accel_mag_mg(p.accel_g) as u16;
                win[widx] = mg;
                widx += 1;
                last_accel = u32::from(mg);
            }
            SamplePool::release(sample.handle);
        }

        if widx >= WIN {
            widx = 0;
            let mut input = [0u8; WIN * 2];
            for i in 0..WIN {
                let b = win[i].to_le_bytes();
                input[2 * i] = b[0];
                input[2 * i + 1] = b[1];
            }
            let mut out = [0u8; 4];
            let t0 = Hal::now_us();
            let req =
                AiInferenceRequest::new(MODEL_ID, &input, t0 + u64::from(contract.timeout_us));
            if let Ok(res) = clf.infer(req, &mut out) {
                let latency = (Hal::now_us() - t0) as u32;
                inferences += 1;
                // Build the report fields from locals (no reads of the mutable static).
                let class = u32::from(out[0]);
                let confidence = u32::from(res.confidence_q15);
                let completed = u32::from(inferences >= 4);
                let pass = inferences >= 4
                    && class == u32::from(CLASS_IDLE)
                    && confidence >= MIN_CONFIDENCE_Q15
                    && latency <= contract.timeout_us
                    && (800..1200).contains(&last_accel);
                let all_pass = u32::from(pass);
                let checksum = AI_MAGIC
                    ^ 1
                    ^ completed
                    ^ all_pass
                    ^ MODEL_ID
                    ^ inferences
                    ^ class
                    ^ confidence
                    ^ latency
                    ^ last_accel;
                unsafe {
                    NOBRO_AI_EVAL_REPORT.inferences = inferences;
                    NOBRO_AI_EVAL_REPORT.last_class = class;
                    NOBRO_AI_EVAL_REPORT.confidence_q15 = confidence;
                    NOBRO_AI_EVAL_REPORT.latency_us = latency;
                    NOBRO_AI_EVAL_REPORT.accel_mag_mg = last_accel;
                    NOBRO_AI_EVAL_REPORT.completed = completed;
                    NOBRO_AI_EVAL_REPORT.all_pass = all_pass;
                    NOBRO_AI_EVAL_REPORT.checksum = checksum;
                }
            }
        }
        asm::delay(150_000);
    }
}
