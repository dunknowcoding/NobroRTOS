//! Bounded non-servo actuator contracts.
//!
//! Servo pulse positioning remains in `nobro-servo`. This crate owns generic
//! motor, stepper, and binary actuator commands without assuming one driver,
//! transport, or board.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActuatorState {
    Down,
    Ready,
    Busy,
    Suspended,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActuatorError {
    InvalidConfig,
    InvalidCommand,
    NotReady,
    Backpressured,
    DeadlineMiss,
    Cancelled,
    Transport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Reverse,
    Brake,
    Coast,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotorCommand {
    pub channel: u8,
    pub direction: Direction,
    /// Magnitude in the inclusive range 0..=1000.
    pub effort_per_mille: u16,
    pub deadline_us: u64,
}

impl MotorCommand {
    pub const fn is_valid(self, channels: u8) -> bool {
        self.channel < channels && self.effort_per_mille <= 1_000 && self.deadline_us > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepperCommand {
    pub channel: u8,
    pub signed_steps: i32,
    pub rate_hz: u32,
    pub acceleration_steps_s2: u32,
    pub deadline_us: u64,
}

impl StepperCommand {
    pub const fn is_valid(self, channels: u8, max_rate_hz: u32) -> bool {
        self.channel < channels
            && self.signed_steps != 0
            && self.rate_hz > 0
            && self.rate_hz <= max_rate_hz
            && self.acceleration_steps_s2 > 0
            && self.deadline_us > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinaryCommand {
    pub channel: u8,
    pub energized: bool,
    /// Maximum continuously energized time; zero is rejected when energized.
    pub max_on_us: u32,
    pub deadline_us: u64,
}

impl BinaryCommand {
    pub const fn is_valid(self, channels: u8) -> bool {
        self.channel < channels && (!self.energized || self.max_on_us > 0) && self.deadline_us > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActuatorCommand {
    Motor(MotorCommand),
    Stepper(StepperCommand),
    Binary(BinaryCommand),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActuatorLimits {
    pub motor_channels: u8,
    pub stepper_channels: u8,
    pub binary_channels: u8,
    pub max_step_rate_hz: u32,
    pub max_pending: u8,
}

impl ActuatorCommand {
    pub const fn is_valid(self, limits: ActuatorLimits) -> bool {
        match self {
            Self::Motor(command) => command.is_valid(limits.motor_channels),
            Self::Stepper(command) => {
                command.is_valid(limits.stepper_channels, limits.max_step_rate_hz)
            }
            Self::Binary(command) => command.is_valid(limits.binary_channels),
        }
    }
}

/// One mounted actuator instance with bounded queueing owned by its backend.
pub trait ActuatorBackend {
    type Receipt: Copy;

    fn state(&self) -> ActuatorState;
    fn limits(&self) -> ActuatorLimits;
    fn pending(&self) -> u8;
    fn submit(&mut self, command: ActuatorCommand) -> Result<Self::Receipt, ActuatorError>;
    fn cancel(&mut self, receipt: Self::Receipt) -> Result<(), ActuatorError>;
    /// Enter the backend's safest electrically supported state immediately.
    fn emergency_stop(&mut self) -> Result<(), ActuatorError>;
    fn quiesce(&mut self) -> Result<(), ActuatorError>;
    fn recover(&mut self) -> Result<(), ActuatorError>;
    fn release(&mut self) -> Result<(), ActuatorError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motor_limits_are_not_servo_pulse_limits() {
        assert!(MotorCommand {
            channel: 1,
            direction: Direction::Forward,
            effort_per_mille: 1_000,
            deadline_us: 5_000,
        }
        .is_valid(2));
        assert!(!MotorCommand {
            channel: 0,
            direction: Direction::Forward,
            effort_per_mille: 1_001,
            deadline_us: 5_000,
        }
        .is_valid(2));
    }

    #[test]
    fn stepper_and_binary_commands_fail_closed() {
        assert!(StepperCommand {
            channel: 0,
            signed_steps: -200,
            rate_hz: 4_000,
            acceleration_steps_s2: 12_000,
            deadline_us: 50_000,
        }
        .is_valid(1, 5_000));
        assert!(!BinaryCommand {
            channel: 0,
            energized: true,
            max_on_us: 0,
            deadline_us: 1_000,
        }
        .is_valid(1));
        let limits = ActuatorLimits {
            motor_channels: 1,
            stepper_channels: 1,
            binary_channels: 1,
            max_step_rate_hz: 5_000,
            max_pending: 2,
        };
        assert!(!ActuatorCommand::Motor(MotorCommand {
            channel: 0,
            direction: Direction::Coast,
            effort_per_mille: 0,
            deadline_us: 0,
        })
        .is_valid(limits));
    }
}
