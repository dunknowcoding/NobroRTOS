//! Bounded ESP32-S3 + ES8311 bridge.
//!
//! Nobro owns format validation, frame bounds, lifecycle, backpressure, and
//! accounting. The mounted transport owns Arduino-ESP32 I2S/DMA and ES8311
//! control; this crate never pretends the vendor runtime is implemented in
//! portable Rust.
#![no_std]

use nobro_audio::{
    AudioBackend, AudioError, AudioResourcePrice, AudioState, CodecConfig, ProviderWorkload,
    SampleFormat,
};

pub const BACKEND_ID: &str = "esp32s3-es8311-arduino";

/// Build the runtime-price identity for one codec configuration, maximum
/// transfer frame, and admitted capture/playback call rate.
pub const fn workload(
    config: CodecConfig,
    max_frame_bytes: usize,
    transfers_per_second: u32,
) -> ProviderWorkload {
    let max_frame_bytes = max_frame_bytes as u64;
    ProviderWorkload::new(
        BACKEND_ID,
        &[
            config.sample_rate_hz,
            config.channels as u32,
            config.format as u32,
            max_frame_bytes as u32,
            (max_frame_bytes >> 32) as u32,
        ],
        transfers_per_second,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    CodecControl,
    I2sStart,
    Io,
    Recovery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioDiagnostics {
    pub playback_frames: u32,
    pub capture_frames: u32,
    pub partial_transfers: u32,
    pub transport_errors: u32,
    pub recoveries: u32,
}

pub trait Esp32s3Es8311Transport {
    fn configure(&mut self, config: CodecConfig) -> Result<(), TransportError>;
    fn capture(&mut self, output: &mut [u8]) -> Result<usize, TransportError>;
    fn playback(&mut self, frame: &[u8]) -> Result<(), TransportError>;
    fn quiesce(&mut self) -> Result<(), TransportError>;
    fn recover(&mut self) -> Result<(), TransportError>;
}

pub struct Esp32s3Es8311<T> {
    transport: T,
    state: AudioState,
    config: Option<CodecConfig>,
    max_frame_bytes: usize,
    transfers_per_second: u32,
    price: AudioResourcePrice,
    diagnostics: AudioDiagnostics,
}

impl<T> Esp32s3Es8311<T> {
    pub const fn new(
        transport: T,
        max_frame_bytes: usize,
        transfers_per_second: u32,
        price: AudioResourcePrice,
    ) -> Self {
        Self {
            transport,
            state: AudioState::Down,
            config: None,
            max_frame_bytes,
            transfers_per_second,
            price,
            diagnostics: AudioDiagnostics {
                playback_frames: 0,
                capture_frames: 0,
                partial_transfers: 0,
                transport_errors: 0,
                recoveries: 0,
            },
        }
    }

    pub const fn backend_id(&self) -> &'static str {
        BACKEND_ID
    }

    pub const fn admission_price(&self) -> AudioResourcePrice {
        self.price
    }

    pub const fn admitted_transfers_per_second(&self) -> u32 {
        self.transfers_per_second
    }

    pub const fn diagnostics(&self) -> AudioDiagnostics {
        self.diagnostics
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    fn valid_config(&self, config: CodecConfig) -> bool {
        let fixed = self.price.provider.fixed();
        let expected_workload = workload(config, self.max_frame_bytes, self.transfers_per_second);
        config.is_valid()
            && (8_000..=192_000).contains(&config.sample_rate_hz)
            && (config.channels == 1 || config.channels == 2)
            && config.format == SampleFormat::Signed16
            && self.max_frame_bytes != 0
            && self.price.frame_bytes == self.max_frame_bytes
            && self.price.is_complete_for(expected_workload)
            && fixed.peripheral_channels() != 0
            && fixed.interrupt_slots() != 0
            && fixed.dma_channels() != 0
    }

    fn transport_error(&mut self) -> AudioError {
        self.diagnostics.transport_errors = self.diagnostics.transport_errors.saturating_add(1);
        self.state = AudioState::Faulted;
        AudioError::Transport
    }
}

impl<T: Esp32s3Es8311Transport> AudioBackend for Esp32s3Es8311<T> {
    fn state(&self) -> AudioState {
        self.state
    }

    fn configure(&mut self, config: CodecConfig) -> Result<(), AudioError> {
        if !self.valid_config(config) {
            return Err(AudioError::InvalidConfig);
        }
        self.transport
            .configure(config)
            .map_err(|_| self.transport_error())?;
        self.config = Some(config);
        self.state = AudioState::Ready;
        Ok(())
    }

    fn capture(&mut self, output: &mut [u8]) -> Result<usize, AudioError> {
        if self.state != AudioState::Ready {
            return Err(AudioError::NotReady);
        }
        if output.is_empty() || output.len() > self.max_frame_bytes {
            return Err(AudioError::FrameTooLarge);
        }
        let received = self
            .transport
            .capture(output)
            .map_err(|_| self.transport_error())?;
        if received != output.len() {
            self.diagnostics.partial_transfers =
                self.diagnostics.partial_transfers.saturating_add(1);
            self.state = AudioState::Faulted;
            return Err(AudioError::PartialIo);
        }
        self.diagnostics.capture_frames = self.diagnostics.capture_frames.saturating_add(1);
        Ok(received)
    }

    fn playback(&mut self, frame: &[u8]) -> Result<(), AudioError> {
        if self.state != AudioState::Ready {
            return Err(AudioError::NotReady);
        }
        if frame.is_empty() || frame.len() > self.max_frame_bytes {
            return Err(AudioError::FrameTooLarge);
        }
        self.transport
            .playback(frame)
            .map_err(|_| self.transport_error())?;
        self.diagnostics.playback_frames = self.diagnostics.playback_frames.saturating_add(1);
        Ok(())
    }

    fn quiesce(&mut self) -> Result<(), AudioError> {
        if self.state == AudioState::Down {
            return Ok(());
        }
        self.transport
            .quiesce()
            .map_err(|_| self.transport_error())?;
        self.state = AudioState::Suspended;
        Ok(())
    }

    fn recover(&mut self) -> Result<(), AudioError> {
        if self.config.is_none() {
            return Err(AudioError::NotReady);
        }
        self.transport
            .recover()
            .map_err(|_| self.transport_error())?;
        self.state = AudioState::Ready;
        self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        configured: bool,
        fail: bool,
        short_read: bool,
    }

    impl Esp32s3Es8311Transport for Fake {
        fn configure(&mut self, _config: CodecConfig) -> Result<(), TransportError> {
            if self.fail {
                Err(TransportError::CodecControl)
            } else {
                self.configured = true;
                Ok(())
            }
        }

        fn capture(&mut self, output: &mut [u8]) -> Result<usize, TransportError> {
            if self.fail {
                return Err(TransportError::Io);
            }
            output.fill(7);
            Ok(output.len() - usize::from(self.short_read && !output.is_empty()))
        }

        fn playback(&mut self, _frame: &[u8]) -> Result<(), TransportError> {
            if self.fail {
                Err(TransportError::Io)
            } else {
                Ok(())
            }
        }

        fn quiesce(&mut self) -> Result<(), TransportError> {
            if self.fail {
                Err(TransportError::Io)
            } else {
                Ok(())
            }
        }

        fn recover(&mut self) -> Result<(), TransportError> {
            self.fail = false;
            Ok(())
        }
    }

    fn price() -> AudioResourcePrice {
        let workload = workload(config(), 192, 100);
        AudioResourcePrice {
            frame_bytes: 192,
            queue_slots: 2,
            provider: nobro_audio::ProviderAdmissionPrice::new(
                nobro_audio::ProviderResourcePrice::unknown()
                    .with_flash_bytes(4096)
                    .with_static_ram_bytes(512)
                    .with_retained_heap_bytes(8192)
                    .with_stack_bytes(512)
                    .with_vendor_reserved_ram_bytes(4096)
                    .with_worker_threads(1)
                    .with_interrupt_slots(1)
                    .with_dma_channels(1)
                    .with_controller_firmware_bytes(0)
                    .with_peripheral_channels(1),
                nobro_audio::ProviderRuntimePrice::unknown(workload)
                    .with_transient_heap_peak_bytes(192)
                    .with_stack_high_water_bytes(384)
                    .with_cpu_cycles_per_second(320_000)
                    .with_latency_p99_cycles(1_000)
                    .with_latency_max_cycles(2_000),
            ),
        }
    }

    fn config() -> CodecConfig {
        CodecConfig::new(16_000, 1, SampleFormat::Signed16)
    }

    #[test]
    fn workload_identity_matches_the_exact_registry_binding() {
        let workload = workload(config(), 192, 100);
        assert_eq!(workload.configuration_fingerprint(), 0xcc5c_58e7_6e50_5756);
        assert_eq!(workload.operations_per_second(), 100);
    }

    #[test]
    fn bounds_lifecycle_and_price_are_explicit() {
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, price());
        assert_eq!(adapter.playback(&[1]), Err(AudioError::NotReady));
        assert_eq!(adapter.configure(config()), Ok(()));
        assert_eq!(adapter.playback(&[1; 192]), Ok(()));
        assert_eq!(adapter.playback(&[1; 193]), Err(AudioError::FrameTooLarge));
        let mut capture = [0; 32];
        assert_eq!(adapter.capture(&mut capture), Ok(32));
        assert!(capture.iter().all(|value| *value == 7));
        assert_eq!(adapter.quiesce(), Ok(()));
        assert_eq!(adapter.state(), AudioState::Suspended);
        assert_eq!(adapter.recover(), Ok(()));
        assert_eq!(adapter.state(), AudioState::Ready);
        assert_eq!(adapter.admission_price().frame_storage_bytes(), Some(384));
        assert_eq!(
            adapter.diagnostics(),
            AudioDiagnostics {
                playback_frames: 1,
                capture_frames: 1,
                partial_transfers: 0,
                transport_errors: 0,
                recoveries: 1,
            }
        );
    }

    #[test]
    fn invalid_or_unpriced_formats_fail_closed() {
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, price());
        assert_eq!(
            adapter.configure(CodecConfig::new(16_000, 1, SampleFormat::Signed24In32)),
            Err(AudioError::InvalidConfig)
        );
        let mut unpriced = price();
        unpriced.provider = nobro_audio::ProviderAdmissionPrice::new(
            nobro_audio::ProviderResourcePrice::unknown(),
            unpriced.provider.runtime(),
        );
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, unpriced);
        assert_eq!(adapter.configure(config()), Err(AudioError::InvalidConfig));

        let mut unknown = price();
        unknown.provider = nobro_audio::ProviderAdmissionPrice::default();
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, unknown);
        assert_eq!(adapter.configure(config()), Err(AudioError::InvalidConfig));

        let mut changed = config();
        changed.sample_rate_hz = 48_000;
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, price());
        assert_eq!(adapter.configure(changed), Err(AudioError::InvalidConfig));

        let mut zero_ownership = price();
        zero_ownership.provider =
            nobro_audio::ProviderAdmissionPrice::known_zero(workload(config(), 192, 100));
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, zero_ownership);
        assert_eq!(adapter.configure(config()), Err(AudioError::InvalidConfig));

        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 101, price());
        assert_eq!(adapter.configure(config()), Err(AudioError::InvalidConfig));
    }

    #[test]
    fn transport_fault_requires_recovery() {
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, price());
        assert_eq!(adapter.configure(config()), Ok(()));
        adapter.transport.fail = true;
        assert_eq!(adapter.playback(&[1]), Err(AudioError::Transport));
        assert_eq!(adapter.state(), AudioState::Faulted);
        assert_eq!(adapter.recover(), Ok(()));
        assert_eq!(adapter.state(), AudioState::Ready);
    }

    #[test]
    fn partial_capture_faults_and_is_counted() {
        let mut adapter = Esp32s3Es8311::new(Fake::default(), 192, 100, price());
        assert_eq!(adapter.configure(config()), Ok(()));
        adapter.transport.short_read = true;
        let mut capture = [0; 8];
        assert_eq!(adapter.capture(&mut capture), Err(AudioError::PartialIo));
        assert_eq!(adapter.state(), AudioState::Faulted);
        assert_eq!(adapter.diagnostics().partial_transfers, 1);
    }
}
