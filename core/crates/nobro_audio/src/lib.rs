//! Allocation-free audio contracts with explicit format, lifecycle, and backpressure.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    Signed16,
    Signed24In32,
    Signed32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecConfig {
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub format: SampleFormat,
}

impl CodecConfig {
    pub const fn new(sample_rate_hz: u32, channels: u8, format: SampleFormat) -> Self {
        Self {
            sample_rate_hz,
            channels,
            format,
        }
    }

    pub const fn is_valid(self) -> bool {
        self.sample_rate_hz > 0 && self.channels > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioState {
    Down,
    Ready,
    Suspended,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioError {
    InvalidConfig,
    NotReady,
    FrameTooLarge,
    Backpressured,
    Empty,
    Transport,
}

/// One mountable codec/transport implementation.
///
/// Vendor DMA, heap, ISR, and worker-thread ownership is reported by the
/// concrete adapter; this portable trait does not pretend to bound it.
pub trait AudioBackend {
    fn state(&self) -> AudioState;
    fn configure(&mut self, config: CodecConfig) -> Result<(), AudioError>;
    fn capture(&mut self, output: &mut [u8]) -> Result<usize, AudioError>;
    fn playback(&mut self, frame: &[u8]) -> Result<(), AudioError>;
    fn quiesce(&mut self) -> Result<(), AudioError>;
    fn recover(&mut self) -> Result<(), AudioError>;
}

#[derive(Clone, Copy)]
struct Frame<const BYTES: usize> {
    bytes: [u8; BYTES],
    len: usize,
}

impl<const BYTES: usize> Frame<BYTES> {
    const EMPTY: Self = Self {
        bytes: [0; BYTES],
        len: 0,
    };
}

/// Fixed-capacity frame storage with deterministic RAM cost and backpressure.
pub struct AudioRing<const SLOTS: usize, const BYTES: usize> {
    frames: [Frame<BYTES>; SLOTS],
    read: usize,
    write: usize,
    len: usize,
    dropped: u32,
}

impl<const SLOTS: usize, const BYTES: usize> AudioRing<SLOTS, BYTES> {
    pub const fn new() -> Self {
        Self {
            frames: [Frame::EMPTY; SLOTS],
            read: 0,
            write: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        SLOTS
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub fn push(&mut self, frame: &[u8]) -> Result<(), AudioError> {
        if frame.len() > BYTES {
            return Err(AudioError::FrameTooLarge);
        }
        if SLOTS == 0 || self.len == SLOTS {
            self.dropped = self.dropped.saturating_add(1);
            return Err(AudioError::Backpressured);
        }
        let slot = &mut self.frames[self.write];
        slot.bytes[..frame.len()].copy_from_slice(frame);
        slot.len = frame.len();
        self.write = (self.write + 1) % SLOTS;
        self.len += 1;
        Ok(())
    }

    pub fn pop_into(&mut self, output: &mut [u8]) -> Result<usize, AudioError> {
        if self.len == 0 {
            return Err(AudioError::Empty);
        }
        let slot = &mut self.frames[self.read];
        if output.len() < slot.len {
            return Err(AudioError::FrameTooLarge);
        }
        output[..slot.len].copy_from_slice(&slot.bytes[..slot.len]);
        let copied = slot.len;
        slot.len = 0;
        self.read = (self.read + 1) % SLOTS;
        self.len -= 1;
        Ok(copied)
    }
}

impl<const SLOTS: usize, const BYTES: usize> Default for AudioRing<SLOTS, BYTES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_preserves_frames_and_backpressures() {
        let mut ring = AudioRing::<2, 4>::new();
        assert_eq!(ring.push(&[1, 2, 3]), Ok(()));
        assert_eq!(ring.push(&[4]), Ok(()));
        assert_eq!(ring.push(&[5]), Err(AudioError::Backpressured));
        assert_eq!(ring.dropped(), 1);
        let mut output = [0; 4];
        assert_eq!(ring.pop_into(&mut output), Ok(3));
        assert_eq!(&output[..3], &[1, 2, 3]);
        assert_eq!(ring.pop_into(&mut output), Ok(1));
        assert_eq!(output[0], 4);
        assert_eq!(ring.pop_into(&mut output), Err(AudioError::Empty));
    }

    #[test]
    fn frame_and_output_bounds_fail_without_consuming() {
        let mut ring = AudioRing::<1, 3>::new();
        assert_eq!(ring.push(&[1, 2, 3, 4]), Err(AudioError::FrameTooLarge));
        assert_eq!(ring.push(&[1, 2, 3]), Ok(()));
        let mut short = [0; 2];
        assert_eq!(ring.pop_into(&mut short), Err(AudioError::FrameTooLarge));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn zero_slot_ring_fails_closed() {
        let mut ring = AudioRing::<0, 4>::new();
        assert_eq!(ring.push(&[1]), Err(AudioError::Backpressured));
        assert_eq!(ring.dropped(), 1);
    }

    #[test]
    fn codec_config_rejects_zero_rate_or_channels() {
        assert!(CodecConfig::new(48_000, 2, SampleFormat::Signed16).is_valid());
        assert!(!CodecConfig::new(0, 2, SampleFormat::Signed16).is_valid());
        assert!(!CodecConfig::new(48_000, 0, SampleFormat::Signed16).is_valid());
    }
}
