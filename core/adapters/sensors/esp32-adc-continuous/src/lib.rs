#![cfg_attr(not(test), no_std)]

use nobro_sensor::{
    AdcDmaBackend, AdcDmaConfig, AdcDmaError, AdcDmaResourcePrice, AdcDmaState, AdcSample,
};

pub const BACKEND_ID: &str = "esp32-arduino-continuous-adc";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Failed,
    Deadline,
}

pub trait Esp32AdcContinuousTransport {
    fn configure(&mut self, config: AdcDmaConfig) -> Result<(), TransportError>;
    fn start(&mut self) -> Result<(), TransportError>;
    fn read(
        &mut self,
        output: &mut [AdcSample],
        max_block_us: u32,
    ) -> Result<usize, TransportError>;
    fn stop(&mut self) -> Result<(), TransportError>;
    fn deinit(&mut self) -> Result<(), TransportError>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdcDiagnostics {
    pub frames: u32,
    pub samples: u32,
    pub partial_frames: u32,
    pub transport_errors: u32,
    pub deadline_misses: u32,
    pub recoveries: u32,
}

pub struct Esp32AdcContinuous<T> {
    transport: T,
    state: AdcDmaState,
    config: Option<AdcDmaConfig>,
    price: AdcDmaResourcePrice,
    diagnostics: AdcDiagnostics,
}

impl<T: Esp32AdcContinuousTransport> Esp32AdcContinuous<T> {
    pub const fn new(transport: T, price: AdcDmaResourcePrice) -> Self {
        Self {
            transport,
            state: AdcDmaState::Down,
            config: None,
            price,
            diagnostics: AdcDiagnostics {
                frames: 0,
                samples: 0,
                partial_frames: 0,
                transport_errors: 0,
                deadline_misses: 0,
                recoveries: 0,
            },
        }
    }

    pub const fn admission_price(&self) -> AdcDmaResourcePrice {
        self.price
    }

    pub const fn diagnostics(&self) -> AdcDiagnostics {
        self.diagnostics
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    fn transport_failure(&mut self, error: TransportError) -> AdcDmaError {
        self.state = AdcDmaState::Faulted;
        match error {
            TransportError::Deadline => {
                self.diagnostics.deadline_misses =
                    self.diagnostics.deadline_misses.saturating_add(1);
                AdcDmaError::DeadlineMiss
            }
            TransportError::Failed => {
                self.diagnostics.transport_errors =
                    self.diagnostics.transport_errors.saturating_add(1);
                AdcDmaError::Transport
            }
        }
    }
}

impl<T: Esp32AdcContinuousTransport> AdcDmaBackend for Esp32AdcContinuous<T> {
    fn state(&self) -> AdcDmaState {
        self.state
    }

    fn configure(&mut self, config: AdcDmaConfig) -> Result<(), AdcDmaError> {
        if !config.is_valid() || matches!(self.state, AdcDmaState::Running) {
            return Err(AdcDmaError::InvalidConfig);
        }
        self.transport
            .configure(config)
            .map_err(|error| self.transport_failure(error))?;
        self.config = Some(config);
        self.state = AdcDmaState::Ready;
        Ok(())
    }

    fn start(&mut self) -> Result<(), AdcDmaError> {
        if self.state != AdcDmaState::Ready {
            return Err(AdcDmaError::NotReady);
        }
        self.transport
            .start()
            .map_err(|error| self.transport_failure(error))?;
        self.state = AdcDmaState::Running;
        Ok(())
    }

    fn read_frame(
        &mut self,
        output: &mut [AdcSample],
        max_block_us: u32,
    ) -> Result<usize, AdcDmaError> {
        if self.state != AdcDmaState::Running {
            return Err(AdcDmaError::NotReady);
        }
        let expected = usize::from(self.config.ok_or(AdcDmaError::NotReady)?.channels);
        if output.len() < expected {
            return Err(AdcDmaError::OutputTooSmall);
        }
        let count = self
            .transport
            .read(&mut output[..expected], max_block_us)
            .map_err(|error| self.transport_failure(error))?;
        if count != expected {
            self.state = AdcDmaState::Faulted;
            self.diagnostics.partial_frames = self.diagnostics.partial_frames.saturating_add(1);
            return Err(AdcDmaError::PartialFrame);
        }
        self.diagnostics.frames = self.diagnostics.frames.saturating_add(1);
        self.diagnostics.samples = self
            .diagnostics
            .samples
            .saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
        Ok(count)
    }

    fn quiesce(&mut self) -> Result<(), AdcDmaError> {
        if self.state == AdcDmaState::Running {
            self.transport
                .stop()
                .map_err(|error| self.transport_failure(error))?;
        }
        if self.config.is_some() {
            self.state = AdcDmaState::Suspended;
        }
        Ok(())
    }

    fn recover(&mut self) -> Result<(), AdcDmaError> {
        let config = self.config.ok_or(AdcDmaError::NotReady)?;
        self.transport
            .deinit()
            .and_then(|()| self.transport.configure(config))
            .and_then(|()| self.transport.start())
            .map_err(|error| self.transport_failure(error))?;
        self.state = AdcDmaState::Running;
        self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        partial: bool,
        deadline: bool,
        starts: u8,
        stops: u8,
        deinits: u8,
    }

    impl Esp32AdcContinuousTransport for Fake {
        fn configure(&mut self, _: AdcDmaConfig) -> Result<(), TransportError> {
            Ok(())
        }
        fn start(&mut self) -> Result<(), TransportError> {
            self.starts += 1;
            Ok(())
        }
        fn read(&mut self, output: &mut [AdcSample], _: u32) -> Result<usize, TransportError> {
            if self.deadline {
                return Err(TransportError::Deadline);
            }
            for (index, sample) in output.iter_mut().enumerate() {
                *sample = AdcSample {
                    channel: index as u8,
                    raw: 100 + index as u16,
                    millivolts: 500 + index as u16,
                };
            }
            Ok(output.len().saturating_sub(usize::from(self.partial)))
        }
        fn stop(&mut self) -> Result<(), TransportError> {
            self.stops += 1;
            Ok(())
        }
        fn deinit(&mut self) -> Result<(), TransportError> {
            self.deinits += 1;
            Ok(())
        }
    }

    fn config() -> AdcDmaConfig {
        AdcDmaConfig {
            channels: 2,
            resolution_bits: 12,
            conversions_per_channel: 16,
            sample_rate_hz: 20_000,
        }
    }

    #[test]
    fn lifecycle_and_frame_bounds_are_explicit() {
        let mut adc = Esp32AdcContinuous::new(Fake::default(), AdcDmaResourcePrice::default());
        assert_eq!(adc.configure(config()), Ok(()));
        assert_eq!(adc.start(), Ok(()));
        let mut short = [AdcSample::default(); 1];
        assert_eq!(
            adc.read_frame(&mut short, 100),
            Err(AdcDmaError::OutputTooSmall)
        );
        let mut frame = [AdcSample::default(); 2];
        assert_eq!(adc.read_frame(&mut frame, 100), Ok(2));
        assert_eq!(frame[1].raw, 101);
        assert_eq!(adc.quiesce(), Ok(()));
        assert_eq!(adc.state(), AdcDmaState::Suspended);
        assert_eq!(adc.recover(), Ok(()));
        assert_eq!(adc.diagnostics().recoveries, 1);
    }

    #[test]
    fn partial_and_deadline_paths_fault_and_attribute() {
        let mut adc = Esp32AdcContinuous::new(Fake::default(), AdcDmaResourcePrice::default());
        adc.configure(config()).unwrap();
        adc.start().unwrap();
        adc.transport.partial = true;
        let mut frame = [AdcSample::default(); 2];
        assert_eq!(
            adc.read_frame(&mut frame, 100),
            Err(AdcDmaError::PartialFrame)
        );
        assert_eq!(adc.diagnostics().partial_frames, 1);

        adc.transport.partial = false;
        adc.recover().unwrap();
        adc.transport.deadline = true;
        assert_eq!(
            adc.read_frame(&mut frame, 1),
            Err(AdcDmaError::DeadlineMiss)
        );
        assert_eq!(adc.diagnostics().deadline_misses, 1);
    }
}
