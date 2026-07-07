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
    let er = 1.0
        + r
        + r2 * 0.5
        + r2 * r * (1.0 / 6.0)
        + r2 * r2 * (1.0 / 24.0)
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
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

pub fn leaky_relu(x: f32, slope: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        slope * x
    }
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

/// Integer dense layer - our own CMSIS-NN-style int8 kernel (M137), no vendor lib.
/// `input` and `weights` (`[OUT][IN]` row-major) are int8; accumulation is int32 and
/// `bias` is already in accumulator units (`bias_real / (scale_in * scale_w)`).
///
/// For a softmax classifier the per-output scale is shared, so `argmax` over these int32
/// accumulators equals `argmax` over the dequantized f32 logits - exact classification
/// with no floating point in the hot loop (the point of an int8 kernel). Outputs are the
/// raw accumulators; requantize downstream only if you need the actual logit values.
pub fn dense_int8(input: &[i8], weights: &[i8], bias: &[i32], out: &mut [i32]) {
    let n_in = input.len();
    for (j, o) in out.iter_mut().enumerate() {
        let row = &weights[j * n_in..(j + 1) * n_in];
        let mut acc = bias[j];
        for (w, x) in row.iter().zip(input) {
            acc += i32::from(*w) * i32::from(*x);
        }
        *o = acc;
    }
}

/// Symmetric int8 quantization of an f32 slice into `out`; returns the scale (real =
/// q * scale). Mirrors the host nn_export quantizer so on-device and host agree.
pub fn quantize_i8(values: &[f32], out: &mut [i8]) -> f32 {
    let peak = values
        .iter()
        .fold(0.0f32, |m, &v| m.max(if v < 0.0 { -v } else { v }));
    let scale = if peak > 0.0 { peak / 127.0 } else { 1.0 };
    for (o, &v) in out.iter_mut().zip(values) {
        let q = (v / scale + if v >= 0.0 { 0.5 } else { -0.5 }) as i32;
        *o = q.clamp(-127, 127) as i8;
    }
    scale
}

/// One online SGD step for a dense + softmax classifier on a single labelled example -
/// the primitive for **on-device incremental learning** (M140). Runs a forward pass,
/// then applies the cross-entropy gradient `(p - onehot(label))` to `weights`/`bias`
/// in place. `scratch` is caller-owned output space of length = number of classes.
/// Returns the cross-entropy loss on this example *before* the update (so a caller can
/// watch it fall). All f32, no heap - it runs on the MCU, not just the host trainer.
pub fn sgd_update(
    input: &[f32],
    weights: &mut [f32],
    bias: &mut [f32],
    label: usize,
    lr: f32,
    scratch: &mut [f32],
) -> f32 {
    let n_in = input.len();
    let n_out = bias.len();
    dense(input, weights, bias, scratch);
    softmax(scratch);
    let p_label = scratch[label].max(1e-7);
    let loss = -log_approx(p_label);
    for j in 0..n_out {
        let grad = scratch[j] - if j == label { 1.0 } else { 0.0 };
        bias[j] -= lr * grad;
        let row = &mut weights[j * n_in..(j + 1) * n_in];
        for (w, &x) in row.iter_mut().zip(input) {
            *w -= lr * grad * x;
        }
    }
    loss
}

/// ln(x) for x in (0, 1] via range reduction to the mantissa - enough for a loss readout.
pub fn log_approx(x: f32) -> f32 {
    if x <= 0.0 {
        return f32::MIN;
    }
    // x = m * 2^e, m in [1,2): ln(x) = ln(m) + e*ln2
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xFF) as i32 - 127;
    let m = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000); // mantissa in [1,2)
                                                                // ln(m) for m in [1,2] via a 3-term atanh series around 1
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln_m = 2.0 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0)));
    ln_m + e as f32 * core::f32::consts::LN_2
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

/// Channel-aware 2-D valid convolution for tiny CNNs.
///
/// Layouts are NHWC without a batch dimension:
/// - `input`: `[H][W][C]`
/// - `kernel`: `[OUT][KH][KW][C]`
/// - `bias`: `[OUT]`
/// - `out`: `[H - KH + 1][W - KW + 1][OUT]`
///
/// The caller owns every buffer; there is no heap use and no hidden scratch space.
pub fn conv2d_valid(
    input: &[f32],
    in_h: usize,
    in_w: usize,
    in_ch: usize,
    kernel: &[f32],
    k_h: usize,
    k_w: usize,
    out_ch: usize,
    bias: &[f32],
    out: &mut [f32],
) {
    assert!(k_h > 0 && k_w > 0);
    assert!(in_h >= k_h && in_w >= k_w);
    assert_eq!(input.len(), in_h * in_w * in_ch);
    assert_eq!(kernel.len(), out_ch * k_h * k_w * in_ch);
    assert_eq!(bias.len(), out_ch);
    let out_h = in_h - k_h + 1;
    let out_w = in_w - k_w + 1;
    assert_eq!(out.len(), out_h * out_w * out_ch);

    for oy in 0..out_h {
        for ox in 0..out_w {
            for oc in 0..out_ch {
                let mut acc = bias[oc];
                for ky in 0..k_h {
                    for kx in 0..k_w {
                        for ic in 0..in_ch {
                            let x_i = ((oy + ky) * in_w + (ox + kx)) * in_ch + ic;
                            let k_i = (((oc * k_h + ky) * k_w + kx) * in_ch) + ic;
                            acc += input[x_i] * kernel[k_i];
                        }
                    }
                }
                out[(oy * out_w + ox) * out_ch + oc] = acc;
            }
        }
    }
}

/// Int8 2-D valid convolution with int32 accumulation.
///
/// Shapes and layouts match [`conv2d_valid`]. `bias` is already in accumulator units,
/// typically `bias_real / (scale_input * scale_kernel)`. The output is intentionally
/// left as raw int32 accumulators so the caller can fuse activation or requantization.
pub fn conv2d_valid_i8(
    input: &[i8],
    in_h: usize,
    in_w: usize,
    in_ch: usize,
    kernel: &[i8],
    k_h: usize,
    k_w: usize,
    out_ch: usize,
    bias: &[i32],
    out: &mut [i32],
) {
    assert!(k_h > 0 && k_w > 0);
    assert!(in_h >= k_h && in_w >= k_w);
    assert_eq!(input.len(), in_h * in_w * in_ch);
    assert_eq!(kernel.len(), out_ch * k_h * k_w * in_ch);
    assert_eq!(bias.len(), out_ch);
    let out_h = in_h - k_h + 1;
    let out_w = in_w - k_w + 1;
    assert_eq!(out.len(), out_h * out_w * out_ch);

    for oy in 0..out_h {
        for ox in 0..out_w {
            for oc in 0..out_ch {
                let mut acc = bias[oc];
                for ky in 0..k_h {
                    for kx in 0..k_w {
                        for ic in 0..in_ch {
                            let x_i = ((oy + ky) * in_w + (ox + kx)) * in_ch + ic;
                            let k_i = (((oc * k_h + ky) * k_w + kx) * in_ch) + ic;
                            acc += i32::from(input[x_i]) * i32::from(kernel[k_i]);
                        }
                    }
                }
                out[(oy * out_w + ox) * out_ch + oc] = acc;
            }
        }
    }
}

/// Average-pool an NHWC tensor. Useful for camera thumbnails and tiny CNN downsampling.
pub fn avg_pool2d(
    input: &[f32],
    in_h: usize,
    in_w: usize,
    in_ch: usize,
    pool_h: usize,
    pool_w: usize,
    stride_h: usize,
    stride_w: usize,
    out: &mut [f32],
) {
    assert!(pool_h > 0 && pool_w > 0 && stride_h > 0 && stride_w > 0);
    assert!(in_h >= pool_h && in_w >= pool_w);
    assert_eq!(input.len(), in_h * in_w * in_ch);
    let out_h = (in_h - pool_h) / stride_h + 1;
    let out_w = (in_w - pool_w) / stride_w + 1;
    assert_eq!(out.len(), out_h * out_w * in_ch);

    let denom = (pool_h * pool_w) as f32;
    for oy in 0..out_h {
        for ox in 0..out_w {
            for ic in 0..in_ch {
                let mut acc = 0.0;
                for ky in 0..pool_h {
                    for kx in 0..pool_w {
                        let y = oy * stride_h + ky;
                        let x = ox * stride_w + kx;
                        acc += input[(y * in_w + x) * in_ch + ic];
                    }
                }
                out[(oy * out_w + ox) * in_ch + ic] = acc / denom;
            }
        }
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
        LstmState {
            h: [0.0; H],
            c: [0.0; H],
        }
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
pub fn attention(q: &[f32], keys: &[f32], values: &[f32], scores: &mut [f32], out: &mut [f32]) {
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
    fn conv2d_valid_filters_nhwc_images() {
        // A 3x3 single-channel image with a 2x2 edge-like kernel.
        let input = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let kernel = [1.0, 0.0, 0.0, -1.0];
        let mut out = [0.0; 4];
        conv2d_valid(&input, 3, 3, 1, &kernel, 2, 2, 1, &[0.5], &mut out);
        assert_eq!(out, [-3.5, -3.5, -3.5, -3.5]);
    }

    #[test]
    fn conv2d_valid_i8_accumulates_like_the_float_path() {
        let input_f = [1.0, -1.0, 2.0, -2.0, 3.0, -3.0, 4.0, -4.0]; // 2x2x2 NHWC
        let kernel_f = [2.0, -1.0, 1.0, 0.0, -1.0, 2.0, 0.0, 1.0]; // 1x2x2x2
        let mut out_f = [0.0; 1];
        conv2d_valid(&input_f, 2, 2, 2, &kernel_f, 2, 2, 1, &[0.0], &mut out_f);

        let input_i = [1i8, -1, 2, -2, 3, -3, 4, -4];
        let kernel_i = [2i8, -1, 1, 0, -1, 2, 0, 1];
        let mut out_i = [0i32; 1];
        conv2d_valid_i8(&input_i, 2, 2, 2, &kernel_i, 2, 2, 1, &[0], &mut out_i);
        assert_eq!(out_f[0] as i32, out_i[0]);
        assert_eq!(out_i[0], -8);
    }

    #[test]
    fn avg_pool2d_downsamples_each_channel() {
        let input = [1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0]; // 2x2x2 NHWC
        let mut out = [0.0; 2];
        avg_pool2d(&input, 2, 2, 2, 2, 2, 2, 2, &mut out);
        assert!(close(out[0], 2.5, 1e-6));
        assert!(close(out[1], 25.0, 1e-6));
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
    fn int8_kernel_matches_the_float_path_argmax() {
        // A 3-class classifier over 8 inputs: run f32 dense and our int8 kernel on the
        // same quantized data, and confirm they pick the same class (the int8 kernel is
        // exact for argmax). Weights chosen so class 2 wins for this input.
        let w_f = [
            0.1, -0.2, 0.3, 0.0, 0.1, -0.1, 0.2, 0.0, // class 0
            -0.3, 0.1, 0.0, 0.2, -0.1, 0.1, 0.0, 0.1, // class 1
            0.4, 0.3, -0.1, 0.2, 0.3, 0.2, 0.1, 0.3, // class 2 (strong)
        ];
        let x_f = [0.9, 0.8, 0.1, 0.7, 0.6, 0.5, 0.2, 0.9];

        // float reference
        let mut out_f = [0.0f32; 3];
        dense(&x_f, &w_f, &[0.0; 3], &mut out_f);
        let want = argmax(&out_f);

        // quantize + int8 kernel (bias 0)
        let mut w_q = [0i8; 24];
        let mut x_q = [0i8; 8];
        quantize_i8(&w_f, &mut w_q);
        quantize_i8(&x_f, &mut x_q);
        let mut acc = [0i32; 3];
        dense_int8(&x_q, &w_q, &[0; 3], &mut acc);
        let got = acc.iter().enumerate().max_by_key(|(_, &v)| v).unwrap().0;

        assert_eq!(want, 2);
        assert_eq!(got, want); // int8 kernel agrees with the float path
    }

    #[test]
    fn quantize_i8_is_symmetric_and_bounded() {
        let vals = [0.0f32, 1.0, -1.0, 0.5, -0.5];
        let mut q = [0i8; 5];
        let scale = quantize_i8(&vals, &mut q);
        assert_eq!(q[0], 0);
        assert_eq!(q[1], 127); // peak maps to full scale
        assert_eq!(q[2], -127);
        assert!((q[3] as f32 * scale - 0.5).abs() <= scale);
    }

    #[test]
    fn log_approx_is_accurate() {
        assert!(close(log_approx(1.0), 0.0, 1e-5));
        assert!(close(log_approx(core::f32::consts::E), 1.0, 1e-3));
        assert!(close(log_approx(0.5), -0.693_147, 1e-3));
    }

    #[test]
    fn sgd_learns_a_linear_boundary_on_device_style() {
        // 2-in, 2-out classifier learns sign(x0 - x1) by online SGD - the exact loop an
        // MCU would run over a labelled stream (M140). Start from zero weights.
        let mut w = [0.0f32; 4];
        let mut b = [0.0f32; 2];
        let mut scratch = [0.0f32; 2];
        // a small deterministic training stream
        let stream = [
            ([2.0f32, -1.0], 0usize),
            ([-1.0, 2.0], 1),
            ([3.0, 0.0], 0),
            ([0.0, 3.0], 1),
            ([1.0, -2.0], 0),
            ([-2.0, 1.0], 1),
        ];
        let mut first_loss = 0.0;
        let mut last_loss = 0.0;
        for epoch in 0..200 {
            for (x, y) in stream.iter() {
                let l = sgd_update(x, &mut w, &mut b, *y, 0.05, &mut scratch);
                if epoch == 0 {
                    first_loss = l;
                }
                last_loss = l;
            }
        }
        // loss fell as the device learned
        assert!(last_loss < first_loss * 0.2);
        // and it now classifies held-out points correctly
        let mut out = [0.0f32; 2];
        dense(&[5.0, 1.0], &w, &b, &mut out);
        assert_eq!(argmax(&out), 0); // x0 > x1
        dense(&[1.0, 5.0], &w, &b, &mut out);
        assert_eq!(argmax(&out), 1); // x1 > x0
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
