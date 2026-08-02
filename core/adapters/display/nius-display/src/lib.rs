#![no_std]

use nobro_display::{
    DisplayBackend, DisplayError, DisplayLimits, DisplayState, FrameReceipt, FrameStatus,
    PixelFormat, Region, RenderBufferOwnership, DISPLAY_RECEIPT_VERSION,
};

pub const BACKEND_ID: &str = "niusdisplay-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Begin,
    Render,
    Lifecycle,
}

pub trait NiusDisplayTransport {
    fn now_us(&self) -> u64;
    fn begin(&mut self) -> Result<(), TransportError>;
    fn render(
        &mut self,
        region: Region,
        format: PixelFormat,
        pixels: &[u8],
    ) -> Result<(), TransportError>;
    fn quiesce(&mut self) -> Result<(), TransportError>;
    fn recover(&mut self) -> Result<(), TransportError>;
    fn release(&mut self) -> Result<(), TransportError>;
}

pub struct NiusDisplayAdapter<T> {
    transport: T,
    limits: DisplayLimits,
    state: DisplayState,
    sequence: u32,
}

impl<T> NiusDisplayAdapter<T> {
    pub const fn new(transport: T, limits: DisplayLimits) -> Self {
        Self {
            transport,
            limits,
            state: DisplayState::Down,
            sequence: 0,
        }
    }

    pub const fn backend_id(&self) -> &'static str {
        BACKEND_ID
    }
    pub fn into_inner(self) -> T {
        self.transport
    }

    fn next_sequence(&mut self) -> u32 {
        self.sequence = self.sequence.wrapping_add(1);
        if self.sequence == 0 {
            self.sequence = 1;
        }
        self.sequence
    }

    fn expected_bytes(format: PixelFormat, region: Region) -> Option<u32> {
        let pixels = (region.width as u32).checked_mul(region.height as u32)?;
        match format {
            PixelFormat::Mono1 => pixels.checked_add(7).map(|value| value / 8),
            PixelFormat::Gray4 => pixels.checked_add(1).map(|value| value / 2),
            PixelFormat::Rgb565 => pixels.checked_mul(2),
            PixelFormat::Rgb666 | PixelFormat::Rgb888 => pixels.checked_mul(3),
        }
    }

    fn checksum(payload: &[u8]) -> u32 {
        let mut hash = 0x811c_9dc5u32;
        let mut index = 0;
        while index < payload.len() {
            hash ^= payload[index] as u32;
            hash = hash.wrapping_mul(0x0100_0193);
            index += 1;
        }
        hash
    }
}

impl<T: NiusDisplayTransport> NiusDisplayAdapter<T> {
    pub fn mount(&mut self) -> Result<(), DisplayError> {
        if self.state != DisplayState::Down {
            return Err(DisplayError::InvalidConfig);
        }
        if self.limits.width == 0
            || self.limits.height == 0
            || self.limits.max_transfer_bytes == 0
            || self.limits.max_pending != 1
        {
            return Err(DisplayError::InvalidConfig);
        }
        self.transport.begin().map_err(|_| {
            self.state = DisplayState::Faulted;
            DisplayError::Transport
        })?;
        self.state = DisplayState::Ready;
        Ok(())
    }
}

impl<T: NiusDisplayTransport> DisplayBackend for NiusDisplayAdapter<T> {
    type Receipt = FrameReceipt;

    fn state(&self) -> DisplayState {
        self.state
    }
    fn limits(&self) -> DisplayLimits {
        self.limits
    }
    fn pending(&self) -> u8 {
        0
    }
    fn render_buffer_ownership(&self) -> RenderBufferOwnership {
        RenderBufferOwnership::ConsumedDuringSubmit
    }

    fn submit(
        &mut self,
        region: Region,
        pixels: &[u8],
        deadline_us: u64,
    ) -> Result<Self::Receipt, DisplayError> {
        if self.state != DisplayState::Ready {
            return Err(DisplayError::NotReady);
        }
        let expected =
            Self::expected_bytes(self.limits.format, region).ok_or(DisplayError::InvalidPayload)?;
        if !region.is_valid(self.limits)
            || pixels.len() as u32 != expected
            || expected > self.limits.max_transfer_bytes
        {
            return Err(DisplayError::InvalidPayload);
        }
        let submitted_us = self.transport.now_us();
        if deadline_us != 0 && submitted_us > deadline_us {
            return Err(DisplayError::DeadlineMiss);
        }
        self.state = DisplayState::Busy;
        if self
            .transport
            .render(region, self.limits.format, pixels)
            .is_err()
        {
            self.state = DisplayState::Faulted;
            return Err(DisplayError::Transport);
        }
        let completed_us = self.transport.now_us();
        self.state = DisplayState::Ready;
        let status = if deadline_us != 0 && completed_us > deadline_us {
            FrameStatus::DeadlineMiss
        } else {
            FrameStatus::Complete
        };
        Ok(FrameReceipt {
            version: DISPLAY_RECEIPT_VERSION,
            sequence: self.next_sequence(),
            region,
            format: self.limits.format,
            payload_bytes: expected,
            payload_checksum: Self::checksum(pixels),
            submitted_us,
            deadline_us,
            completed_us,
            status,
        })
    }

    fn cancel(&mut self, receipt: Self::Receipt) -> Result<(), DisplayError> {
        if receipt.version != DISPLAY_RECEIPT_VERSION {
            return Err(DisplayError::InvalidPayload);
        }
        Err(DisplayError::Cancelled)
    }

    fn quiesce(&mut self) -> Result<(), DisplayError> {
        if self.state == DisplayState::Down || self.state == DisplayState::Suspended {
            return Ok(());
        }
        self.transport
            .quiesce()
            .map_err(|_| DisplayError::Transport)?;
        self.state = DisplayState::Suspended;
        Ok(())
    }

    fn recover(&mut self) -> Result<(), DisplayError> {
        self.transport.recover().map_err(|_| {
            self.state = DisplayState::Faulted;
            DisplayError::Transport
        })?;
        self.state = DisplayState::Ready;
        Ok(())
    }

    fn release(&mut self) -> Result<(), DisplayError> {
        self.transport
            .release()
            .map_err(|_| DisplayError::Transport)?;
        self.state = DisplayState::Down;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        now: u64,
        renders: u8,
    }
    impl NiusDisplayTransport for Fake {
        fn now_us(&self) -> u64 {
            self.now
        }
        fn begin(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
        fn render(&mut self, _: Region, _: PixelFormat, _: &[u8]) -> Result<(), TransportError> {
            self.renders += 1;
            self.now += 10;
            Ok(())
        }
        fn quiesce(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
        fn recover(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
        fn release(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
    }

    fn adapter() -> NiusDisplayAdapter<Fake> {
        NiusDisplayAdapter::new(
            Fake {
                now: 100,
                renders: 0,
            },
            DisplayLimits {
                width: 2,
                height: 2,
                format: PixelFormat::Rgb565,
                max_transfer_bytes: 8,
                max_pending: 1,
            },
        )
    }

    #[test]
    fn renders_exact_bounded_payload_and_returns_v1_receipt() {
        let mut display = adapter();
        display.mount().unwrap();
        let receipt = display
            .submit(
                Region {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
                &[0; 8],
                120,
            )
            .unwrap();
        assert_eq!(receipt.status, FrameStatus::Complete);
        assert!(receipt.is_valid(display.limits()));
        assert_eq!(
            display.render_buffer_ownership(),
            RenderBufferOwnership::ConsumedDuringSubmit
        );
    }

    #[test]
    fn rejects_wrong_payload_and_reports_completed_deadline_miss() {
        let mut display = adapter();
        display.mount().unwrap();
        let region = Region {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        assert_eq!(
            display.submit(region, &[0; 7], 120),
            Err(DisplayError::InvalidPayload)
        );
        let receipt = display.submit(region, &[0; 8], 105).unwrap();
        assert_eq!(receipt.status, FrameStatus::DeadlineMiss);
    }

    #[test]
    fn lifecycle_releases_and_recovers_without_hidden_ownership() {
        let mut display = adapter();
        display.mount().unwrap();
        display.quiesce().unwrap();
        assert_eq!(display.state(), DisplayState::Suspended);
        display.recover().unwrap();
        display.release().unwrap();
        assert_eq!(display.state(), DisplayState::Down);
    }
}
