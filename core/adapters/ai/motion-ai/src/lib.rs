//! Motion classifier: a bounded on-device inference adapter (`AiInferenceSal`).
//!
//! It demonstrates NobroRTOS's "edge AI as a bounded RTOS contract" pillar with a
//! real, deterministic, no-heap, no-float model: given a short window of IMU
//! accel-magnitude samples it classifies motion as IDLE vs ACTIVE from the signal
//! variance and returns a Q15 confidence. The model is fixed-weight (the variance
//! threshold is the "trained" parameter), runs in bounded time and a fixed arena,
//! and only depends on the SAL traits - so it is portable across boards and could be
//! swapped for a generated TinyML kernel behind the same contract.
#![no_std]

use nobro_sal::{
    AiBackendKind, AiInferenceRequest, AiInferenceResult, AiInferenceSal, AiModelContract,
};

/// Model identity ("MOT1").
pub const MODEL_ID: u32 = 0x4D4F_5431;
pub const CLASS_IDLE: u8 = 0;
pub const CLASS_ACTIVE: u8 = 1;

/// Decision boundary on accel-magnitude variance (mg^2). A board at rest sits well
/// below this (steady ~1 g); deliberate motion pushes the windowed variance above it.
pub const VARIANCE_THRESHOLD: u32 = 2_500; // ~ (50 mg)^2

const Q15_ONE: u32 = 32_767;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MotionError {
    /// Input is not a non-empty array of little-endian u16 samples, or output too small.
    BadBuffer,
}

#[derive(Default)]
pub struct MotionClassifier;

impl MotionClassifier {
    pub fn new() -> Self {
        MotionClassifier
    }
}

impl AiInferenceSal for MotionClassifier {
    type Error = MotionError;

    fn contract(&self) -> AiModelContract {
        // On-device; <=64 input bytes (<=32 samples), 4 output bytes, 128-byte arena,
        // 2 ms timeout budget.
        AiModelContract::new(AiBackendKind::OnDevice, MODEL_ID, 64, 4, 128, 2_000)
    }

    fn infer(
        &mut self,
        request: AiInferenceRequest<'_>,
        output: &mut [u8],
    ) -> Result<AiInferenceResult, Self::Error> {
        let bytes = request.input;
        if bytes.len() < 4 || !bytes.len().is_multiple_of(2) || output.is_empty() {
            return Err(MotionError::BadBuffer);
        }
        let n = bytes.len() / 2;

        let mut sum: u64 = 0;
        for i in 0..n {
            sum += u64::from(u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]));
        }
        let mean = (sum / n as u64) as i64;

        let mut var_acc: u64 = 0;
        for i in 0..n {
            let v = i64::from(u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]));
            let d = v - mean;
            var_acc += (d * d) as u64;
        }
        let variance = (var_acc / n as u64) as u32;

        let (class, confidence_q15) = if variance > VARIANCE_THRESHOLD {
            let over = (variance - VARIANCE_THRESHOLD).min(VARIANCE_THRESHOLD) as u64;
            let c = (over * Q15_ONE as u64 / VARIANCE_THRESHOLD as u64) as u16;
            (CLASS_ACTIVE, c.max(1))
        } else {
            let under = (VARIANCE_THRESHOLD - variance) as u64;
            let c = (under * Q15_ONE as u64 / VARIANCE_THRESHOLD as u64) as u16;
            (CLASS_IDLE, c)
        };

        output[0] = class;
        // latency_us is filled by the caller (it owns the timebase); the model itself
        // is fixed-cost over the window.
        Ok(AiInferenceResult::new(1, confidence_q15, 0))
    }
}
