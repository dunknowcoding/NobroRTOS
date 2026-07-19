#![cfg_attr(not(test), no_std)]

use nobro_servo::{
    ProviderWorkload, PulseError, PulseResourcePrice, PulseState, PwmConfig, PwmEngineBackend,
};

pub const BACKEND_ID: &str = "esp32-arduino-ledc";

/// Build the runtime-price identity for one PWM configuration and admitted
/// duty-update rate.
pub const fn workload(config: PwmConfig, writes_per_second: u32) -> ProviderWorkload {
    ProviderWorkload::new(
        BACKEND_ID,
        &[config.frequency_hz, config.resolution_bits as u32],
        writes_per_second,
    )
}

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
    writes_per_second: u32,
    price: PulseResourcePrice,
    writes: u32,
    transport_errors: u32,
    recoveries: u32,
}

impl<T: Esp32LedcTransport> Esp32Ledc<T> {
    pub const fn new(transport: T, writes_per_second: u32, price: PulseResourcePrice) -> Self {
        Self {
            transport,
            state: PulseState::Down,
            config: None,
            attached: false,
            writes_per_second,
            price,
            writes: 0,
            transport_errors: 0,
            recoveries: 0,
        }
    }

    pub const fn admission_price(&self) -> PulseResourcePrice {
        self.price
    }

    pub const fn admitted_writes_per_second(&self) -> u32 {
        self.writes_per_second
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
        let fixed = self.price.fixed();
        let expected_workload = workload(config, self.writes_per_second);
        if !config.is_valid()
            || !self.price.is_complete_for(expected_workload)
            || fixed.peripheral_channels() == 0
            || fixed.interrupt_slots() != 0
            || fixed.dma_channels() != 0
            || self.state == PulseState::Busy
        {
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

    fn price(config: PwmConfig, writes_per_second: u32) -> PulseResourcePrice {
        let workload = workload(config, writes_per_second);
        PulseResourcePrice::new(
            nobro_servo::ProviderResourcePrice::known_zero().with_peripheral_channels(1),
            nobro_servo::ProviderRuntimePrice::known_zero(workload),
        )
    }

    #[test]
    fn duty_bounds_lifecycle_and_recovery_hold() {
        let config = PwmConfig {
            frequency_hz: 20_000,
            resolution_bits: 10,
        };
        let mut ledc = Esp32Ledc::new(Fake::default(), 100, price(config, 100));
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
        let mut ledc = Esp32Ledc::new(Fake::default(), 100, PulseResourcePrice::default());
        assert_eq!(
            ledc.configure(PwmConfig {
                frequency_hz: 20_000,
                resolution_bits: 10,
            }),
            Err(PulseError::InvalidConfig)
        );

        let config = PwmConfig {
            frequency_hz: 20_000,
            resolution_bits: 10,
        };
        let mut changed = config;
        changed.frequency_hz += 1;
        let mut ledc = Esp32Ledc::new(Fake::default(), 100, price(config, 100));
        assert_eq!(ledc.configure(changed), Err(PulseError::InvalidConfig));

        let zero_ownership = PulseResourcePrice::known_zero(workload(config, 100));
        let mut ledc = Esp32Ledc::new(Fake::default(), 100, zero_ownership);
        assert_eq!(ledc.configure(config), Err(PulseError::InvalidConfig));

        let mut ledc = Esp32Ledc::new(Fake::default(), 101, price(config, 100));
        assert_eq!(ledc.configure(config), Err(PulseError::InvalidConfig));
    }
}
