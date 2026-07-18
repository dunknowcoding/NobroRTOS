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
}
