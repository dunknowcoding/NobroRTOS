//! Heap-free Q4/Q2 dense inference with a stable row-aligned packed layout.

use crate::cmsis_requantize;

/// Stable packed-weight encodings. Rows start on byte boundaries and unused
/// high lanes in the final byte of a row are encoded as zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackedWeightFormat {
    Q4,
    Q2,
}

impl PackedWeightFormat {
    pub const fn bits(self) -> u8 {
        match self {
            Self::Q4 => 4,
            Self::Q2 => 2,
        }
    }

    pub const fn values_per_byte(self) -> usize {
        8 / self.bits() as usize
    }

    #[allow(clippy::manual_div_ceil)] // Keep older embedded compiler compatibility.
    pub const fn row_bytes(self, columns: usize) -> usize {
        let values_per_byte = self.values_per_byte();
        (columns + values_per_byte - 1) / values_per_byte
    }

    pub const fn packed_len(self, columns: usize, rows: usize) -> Option<usize> {
        match self.row_bytes(columns).checked_mul(rows) {
            Some(length) => Some(length),
            None => None,
        }
    }

    pub const fn quant_min(self) -> i8 {
        match self {
            Self::Q4 => -7,
            Self::Q2 => -1,
        }
    }

    pub const fn quant_max(self) -> i8 {
        match self {
            Self::Q4 => 7,
            Self::Q2 => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackedDenseBackendId {
    Scalar,
    NobroNative,
    CmsisNn,
    Vendor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackedDenseError {
    InvalidShape,
    InvalidQuantization,
    InvalidPackedLength,
    AccumulatorOverflow,
    Unsupported,
    ProviderFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackedDenseFallback {
    None,
    RequestedBackendUnsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedDenseReceipt {
    pub requested: PackedDenseBackendId,
    pub executed: PackedDenseBackendId,
    pub fallback: PackedDenseFallback,
    pub format: PackedWeightFormat,
    pub logical_weights: u32,
    pub packed_bytes: u32,
}

/// Per-tensor requantization for int8 activations and Q4/Q2 weights.
/// Packed weights are symmetric, so a weight offset is deliberately absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedDenseQuantization {
    pub input_offset: i32,
    pub output_offset: i32,
    pub multiplier: i32,
    pub shift: i32,
    pub activation_min: i8,
    pub activation_max: i8,
}

impl PackedDenseQuantization {
    pub const IDENTITY: Self = Self {
        input_offset: 0,
        output_offset: 0,
        multiplier: 1 << 30,
        shift: 1,
        activation_min: i8::MIN,
        activation_max: i8::MAX,
    };

    fn validate(self) -> Result<(), PackedDenseError> {
        if !(-127..=128).contains(&self.input_offset)
            || !(-127..=128).contains(&self.output_offset)
            || self.multiplier <= 0
            || !(-31..=30).contains(&self.shift)
            || self.activation_min > self.activation_max
        {
            return Err(PackedDenseError::InvalidQuantization);
        }
        Ok(())
    }
}

pub trait PackedDenseBackend {
    fn id(&self) -> PackedDenseBackendId;

    fn run(
        &mut self,
        format: PackedWeightFormat,
        input: &[i8],
        packed_weights: &[u8],
        bias: &[i32],
        quantization: PackedDenseQuantization,
        out: &mut [i8],
    ) -> Result<(), PackedDenseError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScalarPackedDense;

impl PackedDenseBackend for ScalarPackedDense {
    fn id(&self) -> PackedDenseBackendId {
        PackedDenseBackendId::Scalar
    }

    fn run(
        &mut self,
        format: PackedWeightFormat,
        input: &[i8],
        packed_weights: &[u8],
        bias: &[i32],
        quantization: PackedDenseQuantization,
        out: &mut [i8],
    ) -> Result<(), PackedDenseError> {
        packed_dense_scalar(format, input, packed_weights, bias, quantization, out)
    }
}

/// Nobro's portable byte-lane kernel. It consumes two Q4 or four Q2 values per
/// packed-byte iteration and is kept distinct from the scalar reference so an
/// exact target may admit it only after equivalence and timing gates.
#[derive(Clone, Copy, Debug, Default)]
pub struct NobroNativePackedDense;

impl PackedDenseBackend for NobroNativePackedDense {
    fn id(&self) -> PackedDenseBackendId {
        PackedDenseBackendId::NobroNative
    }

    fn run(
        &mut self,
        format: PackedWeightFormat,
        input: &[i8],
        packed_weights: &[u8],
        bias: &[i32],
        quantization: PackedDenseQuantization,
        out: &mut [i8],
    ) -> Result<(), PackedDenseError> {
        validate_packed_dense(format, input, packed_weights, bias, quantization, out)?;
        packed_dense_byte_lanes(format, input, packed_weights, bias, quantization, out)
    }
}

pub type PackedDenseFn = fn(
    PackedWeightFormat,
    &[i8],
    &[u8],
    &[i32],
    PackedDenseQuantization,
    &mut [i8],
) -> Result<(), PackedDenseError>;

macro_rules! callback_backend {
    ($name:ident, $id:expr) => {
        pub struct $name {
            run: PackedDenseFn,
        }

        impl $name {
            pub const fn new(run: PackedDenseFn) -> Self {
                Self { run }
            }
        }

        impl PackedDenseBackend for $name {
            fn id(&self) -> PackedDenseBackendId {
                $id
            }

            fn run(
                &mut self,
                format: PackedWeightFormat,
                input: &[i8],
                packed_weights: &[u8],
                bias: &[i32],
                quantization: PackedDenseQuantization,
                out: &mut [i8],
            ) -> Result<(), PackedDenseError> {
                validate_packed_dense(format, input, packed_weights, bias, quantization, out)?;
                (self.run)(format, input, packed_weights, bias, quantization, out)
            }
        }
    };
}

callback_backend!(CmsisNnPackedDense, PackedDenseBackendId::CmsisNn);
callback_backend!(VendorPackedDense, PackedDenseBackendId::Vendor);

/// Execute a packed dense operator. Only an explicit `Unsupported` response
/// enables scalar fallback; provider failures and malformed models remain visible.
pub fn packed_dense_with_fallback<B: PackedDenseBackend + ?Sized>(
    backend: &mut B,
    format: PackedWeightFormat,
    input: &[i8],
    packed_weights: &[u8],
    bias: &[i32],
    quantization: PackedDenseQuantization,
    out: &mut [i8],
) -> Result<PackedDenseReceipt, PackedDenseError> {
    let logical_weights =
        validate_packed_dense(format, input, packed_weights, bias, quantization, out)?;
    let logical_weights =
        u32::try_from(logical_weights).map_err(|_| PackedDenseError::InvalidShape)?;
    let packed_bytes =
        u32::try_from(packed_weights.len()).map_err(|_| PackedDenseError::InvalidPackedLength)?;
    let requested = backend.id();
    let (executed, fallback) =
        match backend.run(format, input, packed_weights, bias, quantization, out) {
            Ok(()) => (requested, PackedDenseFallback::None),
            Err(PackedDenseError::Unsupported) => {
                packed_dense_scalar(format, input, packed_weights, bias, quantization, out)?;
                (
                    PackedDenseBackendId::Scalar,
                    PackedDenseFallback::RequestedBackendUnsupported,
                )
            }
            Err(error) => return Err(error),
        };
    Ok(PackedDenseReceipt {
        requested,
        executed,
        fallback,
        format,
        logical_weights,
        packed_bytes,
    })
}

pub fn packed_dense_scalar(
    format: PackedWeightFormat,
    input: &[i8],
    packed_weights: &[u8],
    bias: &[i32],
    quantization: PackedDenseQuantization,
    out: &mut [i8],
) -> Result<(), PackedDenseError> {
    validate_packed_dense(format, input, packed_weights, bias, quantization, out)?;
    let row_bytes = format.row_bytes(input.len());
    for (row, output) in out.iter_mut().enumerate() {
        let packed_row = &packed_weights[row * row_bytes..(row + 1) * row_bytes];
        let mut accumulator = bias[row];
        for (column, value) in input.iter().enumerate() {
            let weight = unpack_lane(format, packed_row, column);
            accumulate(&mut accumulator, *value, weight, quantization.input_offset)?;
        }
        *output = requantize(accumulator, quantization);
    }
    Ok(())
}

fn packed_dense_byte_lanes(
    format: PackedWeightFormat,
    input: &[i8],
    packed_weights: &[u8],
    bias: &[i32],
    quantization: PackedDenseQuantization,
    out: &mut [i8],
) -> Result<(), PackedDenseError> {
    let row_bytes = format.row_bytes(input.len());
    for (row, output) in out.iter_mut().enumerate() {
        let packed_row = &packed_weights[row * row_bytes..(row + 1) * row_bytes];
        let mut accumulator = bias[row];
        let mut column = 0;
        for &packed in packed_row {
            let lanes = match format {
                PackedWeightFormat::Q4 => 2,
                PackedWeightFormat::Q2 => 4,
            };
            for lane in 0..lanes {
                if column == input.len() {
                    break;
                }
                let weight = sign_extend(packed >> (lane * format.bits()), format.bits());
                accumulate(
                    &mut accumulator,
                    input[column],
                    weight,
                    quantization.input_offset,
                )?;
                column += 1;
            }
        }
        *output = requantize(accumulator, quantization);
    }
    Ok(())
}

fn validate_packed_dense(
    format: PackedWeightFormat,
    input: &[i8],
    packed_weights: &[u8],
    bias: &[i32],
    quantization: PackedDenseQuantization,
    out: &[i8],
) -> Result<usize, PackedDenseError> {
    quantization.validate()?;
    if input.is_empty() || out.is_empty() || out.len() != bias.len() {
        return Err(PackedDenseError::InvalidShape);
    }
    let logical_weights = input
        .len()
        .checked_mul(out.len())
        .ok_or(PackedDenseError::InvalidShape)?;
    let expected = format
        .packed_len(input.len(), out.len())
        .ok_or(PackedDenseError::InvalidShape)?;
    if packed_weights.len() != expected {
        return Err(PackedDenseError::InvalidPackedLength);
    }
    Ok(logical_weights)
}

fn accumulate(
    accumulator: &mut i32,
    input: i8,
    weight: i8,
    input_offset: i32,
) -> Result<(), PackedDenseError> {
    let adjusted = i32::from(input)
        .checked_add(input_offset)
        .ok_or(PackedDenseError::AccumulatorOverflow)?;
    let product = adjusted
        .checked_mul(i32::from(weight))
        .ok_or(PackedDenseError::AccumulatorOverflow)?;
    *accumulator = accumulator
        .checked_add(product)
        .ok_or(PackedDenseError::AccumulatorOverflow)?;
    Ok(())
}

fn requantize(accumulator: i32, quantization: PackedDenseQuantization) -> i8 {
    cmsis_requantize(accumulator, quantization.multiplier, quantization.shift)
        .wrapping_add(quantization.output_offset)
        .clamp(
            i32::from(quantization.activation_min),
            i32::from(quantization.activation_max),
        ) as i8
}

fn sign_extend(value: u8, bits: u8) -> i8 {
    ((value << (8 - bits)) as i8) >> (8 - bits)
}

fn unpack_lane(format: PackedWeightFormat, row: &[u8], column: usize) -> i8 {
    let values_per_byte = format.values_per_byte();
    let packed = row[column / values_per_byte];
    let shift = (column % values_per_byte) as u8 * format.bits();
    sign_extend(packed >> shift, format.bits())
}

/// Pack already-quantized row-major weights into the stable byte-aligned layout.
pub fn pack_quantized_weights(
    format: PackedWeightFormat,
    values: &[i8],
    columns: usize,
    out: &mut [u8],
) -> Result<(), PackedDenseError> {
    #[allow(clippy::manual_is_multiple_of)]
    if columns == 0 || values.is_empty() || values.len() % columns != 0 {
        return Err(PackedDenseError::InvalidShape);
    }
    let rows = values.len() / columns;
    let row_bytes = format.row_bytes(columns);
    if out.len()
        != row_bytes
            .checked_mul(rows)
            .ok_or(PackedDenseError::InvalidShape)?
    {
        return Err(PackedDenseError::InvalidPackedLength);
    }
    out.fill(0);
    let values_per_byte = format.values_per_byte();
    for row in 0..rows {
        for column in 0..columns {
            let value = values[row * columns + column];
            if value < format.quant_min() || value > format.quant_max() {
                return Err(PackedDenseError::InvalidQuantization);
            }
            let byte = row * row_bytes + column / values_per_byte;
            let shift = (column % values_per_byte) as u8 * format.bits();
            let mask = (1_u8 << format.bits()) - 1;
            out[byte] |= ((value as u8) & mask) << shift;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(format: PackedWeightFormat, values: &[i8], columns: usize) -> [u8; 8] {
        let rows = values.len() / columns;
        let length = format.packed_len(columns, rows).unwrap();
        let mut output = [0_u8; 8];
        pack_quantized_weights(format, values, columns, &mut output[..length]).unwrap();
        output
    }

    #[test]
    fn row_aligned_q4_and_q2_layouts_are_stable() {
        let q4 = packed(PackedWeightFormat::Q4, &[-7, -1, 0, 1, 7, 1], 3);
        assert_eq!(&q4[..4], &[0xf9, 0x00, 0x71, 0x01]);
        let q2 = packed(PackedWeightFormat::Q2, &[-1, 0, 1, -1, 1, 0], 3);
        assert_eq!(&q2[..2], &[0x13, 0x07]);
    }

    #[test]
    fn scalar_and_nobro_byte_lane_kernels_are_equivalent() {
        for (format, values) in [
            (PackedWeightFormat::Q4, &[-7, -1, 0, 1, 7, 1][..]),
            (PackedWeightFormat::Q2, &[-1, 0, 1, -1, 1, 0][..]),
        ] {
            let weights = packed(format, values, 3);
            let length = format.packed_len(3, 2).unwrap();
            let input = [3_i8, -2, 5];
            let bias = [4_i32, -3];
            let mut scalar = [0_i8; 2];
            let mut native = [0_i8; 2];
            packed_dense_scalar(
                format,
                &input,
                &weights[..length],
                &bias,
                PackedDenseQuantization::IDENTITY,
                &mut scalar,
            )
            .unwrap();
            let receipt = packed_dense_with_fallback(
                &mut NobroNativePackedDense,
                format,
                &input,
                &weights[..length],
                &bias,
                PackedDenseQuantization::IDENTITY,
                &mut native,
            )
            .unwrap();
            assert_eq!(native, scalar);
            assert_eq!(receipt.executed, PackedDenseBackendId::NobroNative);
            assert_eq!(receipt.logical_weights, 6);
            assert_eq!(receipt.packed_bytes as usize, length);
        }
    }

    #[test]
    fn fallback_is_explicit_and_provider_failure_stays_visible() {
        fn unsupported(
            _: PackedWeightFormat,
            _: &[i8],
            _: &[u8],
            _: &[i32],
            _: PackedDenseQuantization,
            _: &mut [i8],
        ) -> Result<(), PackedDenseError> {
            Err(PackedDenseError::Unsupported)
        }
        let weights = packed(PackedWeightFormat::Q4, &[1, -1], 2);
        let mut output = [0_i8; 1];
        let receipt = packed_dense_with_fallback(
            &mut CmsisNnPackedDense::new(unsupported),
            PackedWeightFormat::Q4,
            &[4, 2],
            &weights[..1],
            &[0],
            PackedDenseQuantization::IDENTITY,
            &mut output,
        )
        .unwrap();
        assert_eq!(output, [2]);
        assert_eq!(receipt.executed, PackedDenseBackendId::Scalar);
        assert_eq!(
            receipt.fallback,
            PackedDenseFallback::RequestedBackendUnsupported
        );

        fn failure(
            _: PackedWeightFormat,
            _: &[i8],
            _: &[u8],
            _: &[i32],
            _: PackedDenseQuantization,
            _: &mut [i8],
        ) -> Result<(), PackedDenseError> {
            Err(PackedDenseError::ProviderFailure)
        }
        assert_eq!(
            packed_dense_with_fallback(
                &mut VendorPackedDense::new(failure),
                PackedWeightFormat::Q4,
                &[1],
                &[1],
                &[0],
                PackedDenseQuantization::IDENTITY,
                &mut [0],
            ),
            Err(PackedDenseError::ProviderFailure)
        );
    }

    #[test]
    fn malformed_models_and_overflow_fail_closed() {
        assert_eq!(
            packed_dense_scalar(
                PackedWeightFormat::Q2,
                &[1, 2],
                &[],
                &[0],
                PackedDenseQuantization::IDENTITY,
                &mut [0],
            ),
            Err(PackedDenseError::InvalidPackedLength)
        );
        assert_eq!(
            pack_quantized_weights(PackedWeightFormat::Q2, &[2], 1, &mut [0]),
            Err(PackedDenseError::InvalidQuantization)
        );
        assert_eq!(
            packed_dense_scalar(
                PackedWeightFormat::Q4,
                &[127],
                &[7],
                &[i32::MAX],
                PackedDenseQuantization::IDENTITY,
                &mut [0],
            ),
            Err(PackedDenseError::AccumulatorOverflow)
        );
    }
}
