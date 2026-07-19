#![cfg_attr(not(test), no_std)]

use nobro_servo::{PulseError, PulseResourcePrice, PulseState, PwmConfig, PwmEngineBackend};

pub const BACKEND_ID: &str = "esp32-arduino-ledc";

pub trait Esp32LedcTransport {
    fn attach(&mut self, config: PwmConfig) -> bool;
    fn write(&mut self, duty: u32) -> bool;
    fn detach(&mut self) -> bool;
}

pub struct Esp32Ledc<T> {
    transport: T,
    state: PulseState,
    config: Option<PwmConfig>,
    attached: bool,
    price: PulseResourcePrice,
    writes: u32,
    transport_errors: u32,
    recoveries: u32,
}

impl<T: Esp32LedcTransport> Esp32Ledc<T> {
    pub const fn new(transport: T, price: PulseResourcePrice) -> Self {
        Self {
            transport,
            state: PulseState::Down,
            config: None,
            attached: false,
            price,
            writes: 0,
            transport_errors: 0,
            recoveries: 0,
        }
    }

    pub const fn admission_price(&self) -> PulseResourcePrice {
        self.price
    }

    pub const fn writes(&self) -> u32 {
        self.writes
    }

    pub const fn transport_errors(&self) -> u32 {
        self.transport_errors
    }

    pub const fn recoveries(&self) -> u32 {
        self.recoveries
    }

    fn transport_failure(&mut self) -> PulseError {
        self.state = PulseState::Faulted;
        self.transport_errors = self.transport_errors.saturating_add(1);
        PulseError::Transport
    }
}

impl<T: Esp32LedcTransport> PwmEngineBackend for Esp32Ledc<T> {
    fn state(&self) -> PulseState {
        self.state
    }

    fn configure(&mut self, config: PwmConfig) -> Result<(), PulseError> {
        if !config.is_valid() || !self.price.is_complete() || self.state == PulseState::Busy {
            return Err(PulseError::InvalidConfig);
        }
        if self.attached && !self.transport.detach() {
            return Err(self.transport_failure());
        }
        self.attached = false;
        if !self.transport.attach(config) {
            return Err(self.transport_failure());
        }
        self.config = Some(config);
        self.attached = true;
        self.state = PulseState::Ready;
        Ok(())
    }

    fn set_duty(&mut self, duty: u32) -> Result<(), PulseError> {
        let config = self.config.ok_or(PulseError::NotReady)?;
        if self.state != PulseState::Ready {
            return Err(PulseError::NotReady);
        }
        if duty > config.max_duty() {
            return Err(PulseError::InvalidConfig);
        }
        if !self.transport.write(duty) {
            return Err(self.transport_failure());
        }
        self.writes = self.writes.saturating_add(1);
        Ok(())
    }

    fn quiesce(&mut self) -> Result<(), PulseError> {
        if self.attached && !self.transport.detach() {
            return Err(self.transport_failure());
        }
        self.attached = false;
        if self.config.is_some() {
            self.state = PulseState::Suspended;
        }
        Ok(())
    }

    fn recover(&mut self) -> Result<(), PulseError> {
        let config = self.config.ok_or(PulseError::NotReady)?;
        if self.attached && !self.transport.detach() {
            return Err(self.transport_failure());
        }
        self.attached = false;
        if !self.transport.attach(config) {
            return Err(self.transport_failure());
        }
        self.attached = true;
        self.state = PulseState::Ready;
        self.recoveries = self.recoveries.saturating_add(1);
        Ok(())
    }

    fn release(&mut self) -> Result<(), PulseError> {
        if self.attached && !self.transport.detach() {
            return Err(self.transport_failure());
        }
        self.attached = false;
        self.config = None;
        self.state = PulseState::Down;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        fail: bool,
        duty: u32,
        attaches: u8,
        attached: bool,
    }

    impl Esp32LedcTransport for Fake {
        fn attach(&mut self, _: PwmConfig) -> bool {
            if self.fail || self.attached {
                return false;
            }
            self.attaches += 1;
            self.attached = true;
            true
        }
        fn write(&mut self, duty: u32) -> bool {
            self.duty = duty;
            !self.fail
        }
        fn detach(&mut self) -> bool {
            if self.fail || !self.attached {
                return false;
            }
            self.attached = false;
            true
        }
    }

    #[test]
    fn duty_bounds_lifecycle_and_recovery_hold() {
        let mut ledc = Esp32Ledc::new(Fake::default(), PulseResourcePrice::known_zero());
        let config = PwmConfig {
            frequency_hz: 20_000,
            resolution_bits: 10,
        };
        assert_eq!(ledc.configure(config), Ok(()));
        assert_eq!(ledc.set_duty(1023), Ok(()));
        assert_eq!(ledc.set_duty(1024), Err(PulseError::InvalidConfig));
        assert_eq!(ledc.quiesce(), Ok(()));
        assert_eq!(ledc.recover(), Ok(()));
        assert_eq!(ledc.recoveries(), 1);
        assert_eq!(ledc.release(), Ok(()));
        assert_eq!(ledc.state(), PulseState::Down);
        assert_eq!(ledc.release(), Ok(()));
        assert_eq!(ledc.set_duty(1), Err(PulseError::NotReady));
        assert_eq!(ledc.configure(config), Ok(()));
    }

    #[test]
    fn unknown_price_cannot_mount_as_zero_cost() {
        let mut ledc = Esp32Ledc::new(Fake::default(), PulseResourcePrice::default());
        assert_eq!(
            ledc.configure(PwmConfig {
                frequency_hz: 20_000,
                resolution_bits: 10,
            }),
            Err(PulseError::InvalidConfig)
        );
    }
}
