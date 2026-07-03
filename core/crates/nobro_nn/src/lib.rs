//! Neural-network building blocks from scratch, scoped for MCUs (inference side).
//!
//! Every block is `no_std`, heap-free, and shape-explicit: the caller owns all buffers
//! and the math is plain f32 loops - auditable, portable, and deterministic. Training,
//! word-embedding preparation, and evaluation belong on the host (bindings/python),
//! which exports flat weight arrays these blocks execute; quantization helpers live in
//! `nobro_ml`.
//!
//! Weight layouts (row-major): `dense` weights are `[out][in]`; recurrent cells take
//! separate input (`[out][in]`) and hidden (`[out][hidden]`) matrices; LSTM gate order
//! is `i, f, g, o` stacked along the output dimension.
#![cfg_attr(not(test), no_std)]

// ------------------------------------------------------------------ scalar math

/// exp(x) via range reduction to 2^k * exp(r), |r| <= 0.5 ln2, 6-term series.
pub fn exp_approx(x: f32) -> f32 {
    if x > 88.0 {
        return f32::MAX;
    }
    if x < -88.0 {
        return 0.0;
    }
    const LN2: f32 = core::f32::consts::LN_2;
    let k = (x / LN2 + if x >= 0.0 { 0.5 } else { -0.5 }) as i32;
    let r = x - k as f32 * LN2;
    // exp(r) for |r| <= ~0.35: 1 + r + r^2/2 + r^3/6 + r^4/24 + r^5/120
    let r2 = r * r;
    let er = 1.0 + r + r2 * 0.5 + r2 * r * (1.0 / 6.0) + r2 * r2 * (1.0 / 24.0)
        + r2 * r2 * r * (1.0 / 120.0);
    // scale by 2^k through the exponent bits
    let bits = ((k + 127) as u32) << 23;
    er * f32::from_bits(bits)
}

/// Newton-iteration square root (for attention scaling).
pub fn sqrt_approx(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FC0_0000); // seed
    for _ in 0..3 {
        y = 0.5 * (y + x / y);
    }
    y
}

// ------------------------------------------------------------------ activations

pub fn relu(x: f32) -> f32 {
    if x > 0.0 { x } else { 0.0 }
}

pub fn leaky_relu(x: f32, slope: f32) -> f32 {
    if x > 0.0 { x } else { slope * x }
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + exp_approx(-x))
}

pub fn tanh_approx(x: f32) -> f32 {
    2.0 * sigmoid(2.0 * x) - 1.0
}

/// Numerically-stable softmax over `x`, in place.
pub fn softmax(x: &mut [f32]) {
    let max = x.iter().copied().fold(f32::MIN, f32::max);
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = exp_approx(*v - max);
        sum += *v;
    }
    if sum > 0.0 {
        for v in x.iter_mut() {
            *v /= sum;
        }
    }
}

/// Index of the largest element (classification readout).
pub fn argmax(x: &[f32]) -> usize {
    let mut best = 0;
    for (i, &v) in x.iter().enumerate() {
        if v > x[best] {
            best = i;
        }
    }
    best
}

// ------------------------------------------------------------------ layers

/// Fully-connected layer: `out = W. input + b`. `weights` is `[OUT][IN]` row-major.
pub fn dense(input: &[f32], weights: &[f32], bias: &[f32], out: &mut [f32]) {
    let n_in = input.len();
    for (j, o) in out.iter_mut().enumerate() {
        let row = &weights[j * n_in..(j + 1) * n_in];
        let mut acc = bias[j];
        for (w, x) in row.iter().zip(input) {
            acc += w * x;
        }
        *o = acc;
    }
}

/// 1-D valid convolution: `out[t] = sum_k kernel[k] * input[t+k] + bias`.
/// `out` must hold `input.len() - kernel.len() + 1` values.
pub fn conv1d_valid(input: &[f32], kernel: &[f32], bias: f32, out: &mut [f32]) {
    for (t, o) in out.iter_mut().enumerate() {
        let mut acc = bias;
        for (i, w) in kernel.iter().enumerate() {
            acc += w * input[t + i];
        }
        *o = acc;
    }
}

/// Vanilla RNN step: `h' = tanh(Wx.x + Wh.h + b)`, written back into `h`.
/// `wx` is `[H][IN]`, `wh` is `[H][H]`.
pub fn rnn_step(x: &[f32], wx: &[f32], wh: &[f32], b: &[f32], h: &mut [f32]) {
    let n_in = x.len();
    let n_h = h.len();
    let mut next = [0.0f32; 64];
    let next = &mut next[..n_h];
    for j in 0..n_h {
        let mut acc = b[j];
        for (w, xi) in wx[j * n_in..(j + 1) * n_in].iter().zip(x) {
            acc += w * xi;
        }
        for (w, hi) in wh[j * n_h..(j + 1) * n_h].iter().zip(h.iter()) {
            acc += w * hi;
        }
        next[j] = tanh_approx(acc);
    }
    h.copy_from_slice(next);
}

/// LSTM cell state (hidden + cell vectors), fixed hidden size `H`.
pub struct LstmState<const H: usize> {
    pub h: [f32; H],
    pub c: [f32; H],
}

impl<const H: usize> Default for LstmState<H> {
    fn default() -> Self {
        LstmState { h: [0.0; H], c: [0.0; H] }
    }
}

impl<const H: usize> LstmState<H> {
    /// One LSTM step. `wx` is `[4H][IN]`, `wh` is `[4H][H]`, `b` is `[4H]`; the 4H
    /// dimension stacks the gates in the order input, forget, cell(g), output.
    pub fn step(&mut self, x: &[f32], wx: &[f32], wh: &[f32], b: &[f32]) {
        let n_in = x.len();
        let mut gates = [0.0f32; 256];
        let gates = &mut gates[..4 * H];
        for (j, g) in gates.iter_mut().enumerate() {
            let mut acc = b[j];
            for (w, xi) in wx[j * n_in..(j + 1) * n_in].iter().zip(x) {
                acc += w * xi;
            }
            for (w, hi) in wh[j * H..(j + 1) * H].iter().zip(self.h.iter()) {
                acc += w * hi;
            }
            *g = acc;
        }
        for j in 0..H {
            let i = sigmoid(gates[j]);
            let f = sigmoid(gates[H + j]);
            let g = tanh_approx(gates[2 * H + j]);
            let o = sigmoid(gates[3 * H + j]);
            self.c[j] = f * self.c[j] + i * g;
            self.h[j] = o * tanh_approx(self.c[j]);
        }
    }
}

/// Single-head scaled-dot-product attention over a short sequence (the transformer
/// primitive). `q` is one query `[D]`; `keys`/`values` are `[S][D]` row-major; the
/// attended output (weighted sum of values) lands in `out[D]`. `scores` is caller
/// scratch of length >= S.
pub fn attention(
    q: &[f32],
    keys: &[f32],
    values: &[f32],
    scores: &mut [f32],
    out: &mut [f32],
) {
    let d = q.len();
    let s = scores.len();
    let scale = 1.0 / sqrt_approx(d as f32);
    for (t, sc) in scores.iter_mut().enumerate() {
        let mut acc = 0.0;
        for (qi, ki) in q.iter().zip(&keys[t * d..(t + 1) * d]) {
            acc += qi * ki;
        }
        *sc = acc * scale;
    }
    softmax(scores);
    for o in out.iter_mut() {
        *o = 0.0;
    }
    for t in 0..s {
        let w = scores[t];
        for (o, v) in out.iter_mut().zip(&values[t * d..(t + 1) * d]) {
            *o += w * v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn scalar_math_is_accurate_enough() {
        assert!(close(exp_approx(0.0), 1.0, 1e-6));
        assert!(close(exp_approx(1.0), core::f32::consts::E, 2e-4));
        assert!(close(exp_approx(-3.0), 0.049787, 2e-4));
        assert!(close(sqrt_approx(64.0), 8.0, 1e-3));
        assert!(close(sigmoid(0.0), 0.5, 1e-6));
        assert!(close(tanh_approx(0.0), 0.0, 1e-6));
        assert!(tanh_approx(10.0) > 0.999);
    }

    #[test]
    fn dense_matches_hand_math() {
        // 2-in 2-out: W = [[1,2],[3,4]], b = [0.5, -1]
        let w = [1.0, 2.0, 3.0, 4.0];
        let b = [0.5, -1.0];
        let mut out = [0.0; 2];
        dense(&[10.0, 20.0], &w, &b, &mut out);
        assert!(close(out[0], 50.5, 1e-4)); // 10+40+0.5
        assert!(close(out[1], 109.0, 1e-4)); // 30+80-1
    }

    #[test]
    fn conv1d_valid_slides_kernel() {
        let mut out = [0.0; 3];
        conv1d_valid(&[1.0, 2.0, 3.0, 4.0], &[1.0, -1.0], 0.0, &mut out);
        assert_eq!(out, [-1.0, -1.0, -1.0]); // x[t]-x[t+1]
    }

    #[test]
    fn softmax_normalizes_and_orders() {
        let mut x = [1.0, 2.0, 3.0];
        softmax(&mut x);
        let sum: f32 = x.iter().sum();
        assert!(close(sum, 1.0, 1e-4));
        assert!(x[2] > x[1] && x[1] > x[0]);
        assert_eq!(argmax(&x), 2);
    }

    #[test]
    fn rnn_and_lstm_zero_weights_stay_zero() {
        let mut h = [0.5f32, -0.5];
        rnn_step(&[1.0], &[0.0, 0.0], &[0.0; 4], &[0.0, 0.0], &mut h);
        assert_eq!(h, [0.0, 0.0]); // tanh(0)

        let mut lstm: LstmState<2> = LstmState::default();
        lstm.step(&[1.0], &[0.0; 8], &[0.0; 16], &[0.0; 8]);
        // gates at sigmoid(0)=0.5, g=tanh(0)=0 -> c stays 0, h stays 0
        assert!(close(lstm.h[0], 0.0, 1e-6));
        assert!(close(lstm.c[0], 0.0, 1e-6));
    }

    #[test]
    fn lstm_remembers_through_the_cell() {
        // A forget-gate-open, input-gate-open cell accumulates the g signal.
        let mut lstm: LstmState<1> = LstmState::default();
        // wx rows (IN=1): i=10 (open), f=10 (keep), g=10 (push +1), o=10 (open)
        let wx = [10.0, 10.0, 10.0, 10.0];
        let wh = [0.0; 4];
        let b = [0.0; 4];
        lstm.step(&[1.0], &wx, &wh, &b);
        let h1 = lstm.h[0];
        lstm.step(&[1.0], &wx, &wh, &b);
        assert!(lstm.c[0] > 1.5); // ~1 accumulated twice
        assert!(lstm.h[0] >= h1); // saturating but non-decreasing
    }

    #[test]
    fn attention_prefers_the_matching_key() {
        // q matches key1 far more than key0 -> output ~= value1
        let q = [4.0, 0.0];
        let keys = [0.0, 4.0, 4.0, 0.0]; // key0=[0,4], key1=[4,0]
        let values = [1.0, 0.0, 0.0, 1.0]; // val0=[1,0], val1=[0,1]
        let mut scores = [0.0; 2];
        let mut out = [0.0; 2];
        attention(&q, &keys, &values, &mut scores, &mut out);
        assert!(scores[1] > 0.99);
        assert!(out[1] > 0.99 && out[0] < 0.01);
    }
}
