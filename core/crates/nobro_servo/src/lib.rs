//! Allocation-free servo command contract.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServoCommand {
    pub channel: u8,
    pub pulse_us: u32,
    pub deadline_us: u64,
}

impl ServoCommand {
    pub const fn new(channel: u8, pulse_us: u32, deadline_us: u64) -> Self {
        Self {
            channel,
            pulse_us,
            deadline_us,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServoBounds {
    pub min_pulse_us: u32,
    pub max_pulse_us: u32,
    pub channels: u8,
}

impl ServoBounds {
    pub const fn accepts(self, command: ServoCommand) -> bool {
        command.channel < self.channels
            && command.pulse_us >= self.min_pulse_us
            && command.pulse_us <= self.max_pulse_us
    }
}

pub trait ServoBackend {
    type Error;

    fn command(&mut self, command: ServoCommand) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PulseState {
    Down,
    Ready,
    Busy,
    Suspended,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PulseError {
    InvalidConfig,
    NotReady,
    TooManySymbols,
    Backpressured,
    Transport,
    DeadlineMiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PwmConfig {
    pub frequency_hz: u32,
    pub resolution_bits: u8,
}

impl PwmConfig {
    pub const fn is_valid(self) -> bool {
        self.frequency_hz > 0 && self.resolution_bits > 0 && self.resolution_bits <= 31
    }

    pub const fn max_duty(self) -> u32 {
        if self.resolution_bits >= 31 {
            0x7fff_ffff
        } else {
            (1_u32 << self.resolution_bits) - 1
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PulseSymbol {
    pub high_ticks: u16,
    pub low_ticks: u16,
}

impl PulseSymbol {
    pub const fn is_valid(self) -> bool {
        self.high_ticks > 0 || self.low_ticks > 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PulseResourcePrice {
    pub flash_bytes: u32,
    pub static_ram_bytes: u32,
    pub heap_bytes: u32,
    pub stack_bytes: u32,
    pub vendor_reserved_ram_bytes: u32,
    pub worker_threads: u8,
    pub cpu_cycles_per_second: u64,
    pub interrupt_slots: u8,
    pub dma_channels: u8,
    pub controller_firmware_bytes: u32,
}

/// Fixed-frequency duty engine such as ESP32 LEDC.
pub trait PwmEngineBackend {
    fn state(&self) -> PulseState;
    fn configure(&mut self, config: PwmConfig) -> Result<(), PulseError>;
    fn set_duty(&mut self, duty: u32) -> Result<(), PulseError>;
    fn quiesce(&mut self) -> Result<(), PulseError>;
    fn recover(&mut self) -> Result<(), PulseError>;
}

/// Bounded symbol engine such as ESP32 RMT.
pub trait PulseEngineBackend {
    fn state(&self) -> PulseState;
    fn configure(&mut self, tick_hz: u32) -> Result<(), PulseError>;
    fn transmit(&mut self, symbols: &[PulseSymbol], max_block_us: u32) -> Result<(), PulseError>;
    fn quiesce(&mut self) -> Result<(), PulseError>;
    fn recover(&mut self) -> Result<(), PulseError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_are_channel_and_pulse_specific() {
        let bounds = ServoBounds {
            min_pulse_us: 500,
            max_pulse_us: 2_500,
            channels: 2,
        };
        assert!(bounds.accepts(ServoCommand::new(1, 1_500, 10_000)));
        assert!(!bounds.accepts(ServoCommand::new(2, 1_500, 10_000)));
        assert!(!bounds.accepts(ServoCommand::new(1, 3_000, 10_000)));
    }

    #[test]
    fn pwm_and_pulse_shapes_fail_closed() {
        let pwm = PwmConfig {
            frequency_hz: 20_000,
            resolution_bits: 10,
        };
        assert!(pwm.is_valid());
        assert_eq!(pwm.max_duty(), 1023);
        assert!(!PwmConfig {
            frequency_hz: 0,
            ..pwm
        }
        .is_valid());
        assert!(PulseSymbol {
            high_ticks: 4,
            low_ticks: 6,
        }
        .is_valid());
        assert!(!PulseSymbol::default().is_valid());
    }
}
