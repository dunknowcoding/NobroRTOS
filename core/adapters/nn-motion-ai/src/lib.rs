//! Neural-network motion classifier: a bounded on-device inference adapter
//! (`AiInferenceSal`) backed by a trained **int8 MLP** (3 features -> 8 hidden -> 2
//! classes). Unlike the variance classifier, the decision boundary is *learned*: the
//! weights in `nn_weights.rs` are produced by `tools/train_motion_nn.py` and embedded
//! as const arrays. Inference is integer-only (no heap, no float), runs in bounded
//! time and a fixed arena, and depends only on the SAL traits - the "NN tools ->
//! embedded" pipeline behind NobroRTOS's bounded AI contract.
#![no_std]

use nobro_sal::{
    AiBackendKind, AiInferenceRequest, AiInferenceResult, AiInferenceSal, AiModelContract,
};

mod nn_weights;
use nn_weights::{B1, B2, FEAT, FEAT_MAX, HIDDEN, SHIFT1, W1, W2, WINDOW};
pub use nn_weights::TRAIN_ACC_MILLI;

/// Model identity ("NNM1").
pub const MODEL_ID: u32 = 0x4E4E_4D31;
pub const CLASS_IDLE: u8 = 0;
pub const CLASS_ACTIVE: u8 = 1;
const Q15_ONE: i32 = 32_767;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum NnError {
    /// Input is not a non-empty array of little-endian u16 samples, or output too small.
    BadBuffer,
}

#[derive(Default)]
pub struct NnMotionClassifier;

impl NnMotionClassifier {
    pub fn new() -> Self {
        NnMotionClassifier
    }
}

/// The 3 integer features the model was trained on (must match train_motion_nn.py).
fn features(samples: &[u16]) -> [i32; FEAT] {
    let n = samples.len() as i64;
    let mut sum: i64 = 0;
    let (mut mn, mut mx) = (4095u16, 0u16);
    for &s in samples {
        let v = s.min(4095);
        sum += i64::from(v);
        mn = mn.min(v);
        mx = mx.max(v);
    }
    let mean = sum / n;
    let mut var_acc: i64 = 0;
    for &s in samples {
        let d = i64::from(s.min(4095)) - mean;
        var_acc += d * d;
    }
    let var = (var_acc / n) as i32;
    let mut mad_acc: i64 = 0;
    for i in 1..samples.len() {
        mad_acc += (i64::from(samples[i].min(4095)) - i64::from(samples[i - 1].min(4095))).abs();
    }
    let mad = (mad_acc / (n - 1)) as i32;
    [
        (var >> 4).min(4095),
        (i32::from(mx) - i32::from(mn)).min(4095),
        mad.min(4095),
    ]
}

/// Normalize features to int8 [0,127] exactly as the trainer does.
fn normalize(f: [i32; FEAT]) -> [i32; FEAT] {
    let mut x = [0i32; FEAT];
    for i in 0..FEAT {
        x[i] = (f[i] * 127 / FEAT_MAX[i]).clamp(0, 127);
    }
    x
}

/// Integer MLP forward pass; returns (class, decision margin).
fn forward(x: [i32; FEAT]) -> (u8, i32) {
    let mut h = [0i32; HIDDEN];
    for j in 0..HIDDEN {
        let mut acc = B1[j];
        for i in 0..FEAT {
            acc += i32::from(W1[j][i]) * x[i];
        }
        h[j] = (acc >> SHIFT1).clamp(0, 127); // ReLU + requantize to int8
    }
    let mut o = [0i32; 2];
    for k in 0..2 {
        let mut acc = B2[k];
        for j in 0..HIDDEN {
            acc += i32::from(W2[k][j]) * h[j];
        }
        o[k] = acc;
    }
    let class = if o[1] > o[0] { CLASS_ACTIVE } else { CLASS_IDLE };
    let margin = (o[usize::from(class)] - o[1 - usize::from(class)]).max(1);
    (class, margin)
}

impl AiInferenceSal for NnMotionClassifier {
    type Error = NnError;

    fn contract(&self) -> AiModelContract {
        // On-device int8 MLP: <=64 input bytes (<=32 samples), 4 output bytes, 256-byte
        // arena (feature/activation scratch; weights are const in flash), 2 ms budget.
        AiModelContract::new(AiBackendKind::OnDevice, MODEL_ID, 64, 4, 256, 2_000)
    }

    fn infer(
        &mut self,
        request: AiInferenceRequest<'_>,
        output: &mut [u8],
    ) -> Result<AiInferenceResult, Self::Error> {
        let bytes = request.input;
        if bytes.len() < 4 || bytes.len() % 2 != 0 || output.is_empty() {
            return Err(NnError::BadBuffer);
        }
        let n = (bytes.len() / 2).min(WINDOW);
        let mut samples = [0u16; WINDOW];
        for i in 0..n {
            samples[i] = u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
        }
        let (class, margin) = forward(normalize(features(&samples[..n])));
        output[0] = class;
        Ok(AiInferenceResult::new(1, margin.clamp(1, Q15_ONE) as u16, 0))
    }
}

// ---- 3-class variant: idle / walk / shake (M33) ------------------------------------

mod nn3_weights;
pub use nn3_weights::TRAIN_ACC_MILLI as TRAIN_ACC_MILLI_3;

/// 3-class model identity ("NNM3").
pub const MODEL3_ID: u32 = 0x4E4E_4D33;
pub const CLASS3_IDLE: u8 = 0;
pub const CLASS3_WALK: u8 = 1;
pub const CLASS3_SHAKE: u8 = 2;

#[derive(Default)]
pub struct Nn3MotionClassifier;

impl Nn3MotionClassifier {
    pub fn new() -> Self {
        Nn3MotionClassifier
    }
}

fn normalize3(f: [i32; nn3_weights::FEAT]) -> [i32; nn3_weights::FEAT] {
    let mut x = [0i32; nn3_weights::FEAT];
    for i in 0..nn3_weights::FEAT {
        x[i] = (f[i] * 127 / nn3_weights::FEAT_MAX[i]).clamp(0, 127);
    }
    x
}

/// Integer MLP forward pass over 3 classes; returns (class, decision margin).
fn forward3(x: [i32; nn3_weights::FEAT]) -> (u8, i32) {
    use nn3_weights::{B1, B2, FEAT, HIDDEN, NUM_CLASSES, SHIFT1, W1, W2};
    let mut h = [0i32; HIDDEN];
    for j in 0..HIDDEN {
        let mut acc = B1[j];
        for i in 0..FEAT {
            acc += i32::from(W1[j][i]) * x[i];
        }
        h[j] = (acc >> SHIFT1).clamp(0, 127);
    }
    let mut o = [0i32; NUM_CLASSES];
    for k in 0..NUM_CLASSES {
        let mut acc = B2[k];
        for j in 0..HIDDEN {
            acc += i32::from(W2[k][j]) * h[j];
        }
        o[k] = acc;
    }
    let mut best = 0usize;
    for k in 1..NUM_CLASSES {
        if o[k] > o[best] {
            best = k;
        }
    }
    let mut second = i32::MIN;
    for (k, &v) in o.iter().enumerate() {
        if k != best && v > second {
            second = v;
        }
    }
    (best as u8, (o[best] - second).max(1))
}

impl AiInferenceSal for Nn3MotionClassifier {
    type Error = NnError;

    fn contract(&self) -> AiModelContract {
        AiModelContract::new(AiBackendKind::OnDevice, MODEL3_ID, 64, 4, 256, 2_000)
    }

    fn infer(
        &mut self,
        request: AiInferenceRequest<'_>,
        output: &mut [u8],
    ) -> Result<AiInferenceResult, Self::Error> {
        let bytes = request.input;
        if bytes.len() < 4 || bytes.len() % 2 != 0 || output.is_empty() {
            return Err(NnError::BadBuffer);
        }
        let n = (bytes.len() / 2).min(nn3_weights::WINDOW);
        let mut samples = [0u16; nn3_weights::WINDOW];
        for i in 0..n {
            samples[i] = u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
        }
        let (class, margin) = forward3(normalize3(features(&samples[..n])));
        output[0] = class;
        Ok(AiInferenceResult::new(1, margin.clamp(1, Q15_ONE) as u16, 0))
    }
}
