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
    pub const fn is_valid(self) -> bool {
        self.channels > 0 && self.min_pulse_us > 0 && self.min_pulse_us <= self.max_pulse_us
    }

    pub const fn accepts(self, command: ServoCommand) -> bool {
        self.is_valid()
            && command.channel < self.channels
            && command.pulse_us >= self.min_pulse_us
            && command.pulse_us <= self.max_pulse_us
    }
}

pub trait ServoBackend {
    type Error;

    fn command(&mut self, command: ServoCommand) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServoState {
    Down,
    Ready,
    Suspended,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServoError<E> {
    InvalidConfig,
    InvalidCommand,
    NotReady,
    DeadlineMiss,
    Backend(E),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServoReceipt {
    pub sequence: u32,
    pub command: ServoCommand,
    pub submitted_us: u64,
    pub completed_us: u64,
}

/// Lifecycle-complete servo backend. Arduino libraries, native PWM providers,
/// and host simulations can implement the same bounded contract.
pub trait ManagedServoBackend: ServoBackend {
    fn state(&self) -> ServoState;
    fn bounds(&self) -> ServoBounds;
    fn mount(&mut self) -> Result<(), Self::Error>;
    fn quiesce(&mut self) -> Result<(), Self::Error>;
    fn recover(&mut self) -> Result<(), Self::Error>;
    fn release(&mut self) -> Result<(), Self::Error>;
}

pub struct MountedServo<B> {
    backend: B,
    bounds: ServoBounds,
    next_sequence: u32,
}

impl<B: ManagedServoBackend> MountedServo<B> {
    pub fn mount(mut backend: B) -> Result<Self, ServoError<B::Error>> {
        let bounds = backend.bounds();
        if !bounds.is_valid() || backend.state() != ServoState::Down {
            return Err(ServoError::InvalidConfig);
        }
        backend.mount().map_err(ServoError::Backend)?;
        if backend.state() != ServoState::Ready {
            return Err(ServoError::NotReady);
        }
        Ok(Self {
            backend,
            bounds,
            next_sequence: 1,
        })
    }

    pub fn command_at(
        &mut self,
        now_us: u64,
        command: ServoCommand,
    ) -> Result<ServoReceipt, ServoError<B::Error>> {
        if self.backend.state() != ServoState::Ready {
            return Err(ServoError::NotReady);
        }
        if now_us > command.deadline_us {
            return Err(ServoError::DeadlineMiss);
        }
        if !self.bounds.accepts(command) {
            return Err(ServoError::InvalidCommand);
        }
        self.backend.command(command).map_err(ServoError::Backend)?;
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
        Ok(ServoReceipt {
            sequence,
            command,
            submitted_us: now_us,
            completed_us: now_us.max(1),
        })
    }

    pub fn quiesce(&mut self) -> Result<(), ServoError<B::Error>> {
        self.backend.quiesce().map_err(ServoError::Backend)?;
        (self.backend.state() == ServoState::Suspended)
            .then_some(())
            .ok_or(ServoError::NotReady)
    }

    pub fn recover(&mut self) -> Result<(), ServoError<B::Error>> {
        self.backend.recover().map_err(ServoError::Backend)?;
        (self.backend.state() == ServoState::Ready)
            .then_some(())
            .ok_or(ServoError::NotReady)
    }

    pub fn release(mut self) -> Result<B, ServoError<B::Error>> {
        self.backend.release().map_err(ServoError::Backend)?;
        if self.backend.state() != ServoState::Down {
            return Err(ServoError::NotReady);
        }
        Ok(self.backend)
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
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

/// Complete admission price for one mounted pulse provider.
///
/// Fixed ownership is separate from runtime evidence for the exact configured
/// workload. Unknown fields cannot masquerade as zero.
pub use nobro_device::{
    ProviderAdmissionPrice as PulseResourcePrice, ProviderResourcePrice, ProviderRuntimePrice,
    ProviderWorkload,
};

/// Fixed-frequency duty engine such as ESP32 LEDC.
pub trait PwmEngineBackend {
    fn state(&self) -> PulseState;
    fn configure(&mut self, config: PwmConfig) -> Result<(), PulseError>;
    fn set_duty(&mut self, duty: u32) -> Result<(), PulseError>;
    fn quiesce(&mut self) -> Result<(), PulseError>;
    fn recover(&mut self) -> Result<(), PulseError>;
    /// Detach the engine and forget configuration. A released provider must
    /// be configured again before use.
    fn release(&mut self) -> Result<(), PulseError>;
}

/// Bounded symbol engine such as ESP32 RMT.
pub trait PulseEngineBackend {
    fn state(&self) -> PulseState;
    fn configure(&mut self, tick_hz: u32) -> Result<(), PulseError>;
    fn transmit(&mut self, symbols: &[PulseSymbol], max_block_us: u32) -> Result<(), PulseError>;
    fn quiesce(&mut self) -> Result<(), PulseError>;
    fn recover(&mut self) -> Result<(), PulseError>;
    /// Detach the engine and forget configuration. A released provider must
    /// be configured again before use.
    fn release(&mut self) -> Result<(), PulseError>;
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

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FakeError {
        Rejected,
    }

    struct FakeServo {
        state: ServoState,
        bounds: ServoBounds,
        last: Option<ServoCommand>,
        fail: bool,
    }

    impl ServoBackend for FakeServo {
        type Error = FakeError;

        fn command(&mut self, command: ServoCommand) -> Result<(), Self::Error> {
            if self.fail {
                return Err(FakeError::Rejected);
            }
            self.last = Some(command);
            Ok(())
        }
    }

    impl ManagedServoBackend for FakeServo {
        fn state(&self) -> ServoState {
            self.state
        }

        fn bounds(&self) -> ServoBounds {
            self.bounds
        }

        fn mount(&mut self) -> Result<(), Self::Error> {
            self.state = ServoState::Ready;
            Ok(())
        }

        fn quiesce(&mut self) -> Result<(), Self::Error> {
            self.state = ServoState::Suspended;
            Ok(())
        }

        fn recover(&mut self) -> Result<(), Self::Error> {
            self.state = ServoState::Ready;
            Ok(())
        }

        fn release(&mut self) -> Result<(), Self::Error> {
            self.state = ServoState::Down;
            Ok(())
        }
    }

    #[test]
    fn managed_servo_enforces_deadline_bounds_and_lifecycle() {
        let mut servo = MountedServo::mount(FakeServo {
            state: ServoState::Down,
            bounds: ServoBounds {
                min_pulse_us: 500,
                max_pulse_us: 2_500,
                channels: 2,
            },
            last: None,
            fail: false,
        })
        .unwrap();
        let command = ServoCommand::new(1, 1_500, 20);
        assert_eq!(servo.command_at(10, command).unwrap().sequence, 1);
        assert_eq!(servo.command_at(21, command), Err(ServoError::DeadlineMiss));
        assert_eq!(
            servo.command_at(10, ServoCommand::new(2, 1_500, 20)),
            Err(ServoError::InvalidCommand)
        );
        servo.quiesce().unwrap();
        assert_eq!(servo.command_at(10, command), Err(ServoError::NotReady));
        servo.recover().unwrap();
        assert!(servo.command_at(10, command).is_ok());
        assert_eq!(servo.release().unwrap().state(), ServoState::Down);
    }
}
