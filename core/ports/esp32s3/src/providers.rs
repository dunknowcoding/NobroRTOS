//! Reusable ESP32-S3 providers. Board pin selection stays with the application.

use core::{convert::Infallible, fmt};

use embedded_hal::{i2c::I2c, pwm::SetDutyCycle, spi::SpiBus};
use esp_hal::{
    time::Duration, timer::OneShotTimer, usb_serial_jtag::UsbSerialJtag, Blocking, DriverMode,
};
use nobro_hal::{
    HalAlarm, HalByteIo, HalClock, HalCompatibility, HalI2c, HalPwmChannel, HalSpi,
    HardwareCapability, HardwareCapabilitySet, TransferMode,
};

pub struct Esp32S3Providers;

impl HalCompatibility for Esp32S3Providers {
    const CAPABILITIES: HardwareCapabilitySet = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::DeadlineTimer)
        .with(HardwareCapability::ServoPwm)
        .with(HardwareCapability::Bus)
        .with(HardwareCapability::I2c)
        .with(HardwareCapability::Spi)
        .with(HardwareCapability::Usb);
}

pub struct Esp32S3Clock;

impl HalClock for Esp32S3Clock {
    fn now_us() -> u64 {
        esp_hal::time::now().ticks()
    }
}

pub struct Esp32S3Alarm<'d, Dm> {
    timer: OneShotTimer<'d, Dm>,
    deadline_us: Option<u64>,
}

impl<'d> Esp32S3Alarm<'d, Blocking> {
    pub fn new(timer: OneShotTimer<'d, Blocking>) -> Self {
        Self {
            timer,
            deadline_us: None,
        }
    }
}

impl<Dm: DriverMode> HalAlarm for Esp32S3Alarm<'_, Dm> {
    type Error = esp_hal::timer::Error;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        let delay_us = delay_us.max(1);
        self.timer.schedule(Duration::from_ticks(delay_us))?;
        let deadline = Esp32S3Clock::now_us().saturating_add(delay_us);
        self.deadline_us = Some(deadline);
        Ok(deadline)
    }

    fn cancel(&mut self) {
        self.timer.stop();
        self.timer.clear_interrupt();
        self.deadline_us = None;
    }

    fn deadline_us(&self) -> Option<u64> {
        self.deadline_us
    }

    fn poll_due(&mut self, now_us: u64) -> bool {
        if self.deadline_us.is_some_and(|deadline| now_us >= deadline) {
            self.cancel();
            true
        } else {
            false
        }
    }
}

pub struct I2cProvider<T>(pub T);

impl<T: I2c> HalI2c for I2cProvider<T> {
    type Error = T::Error;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write(address, bytes)
    }

    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read(address, bytes)
    }

    fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.0.write_read(address, write, read)
    }
}

pub struct SpiProvider<T>(pub T);

impl<T: SpiBus<u8>> HalSpi for SpiProvider<T> {
    type Error = T::Error;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
        self.0.transfer(read, write)
    }
}

pub struct PwmProvider<T>(pub T);

impl<T: SetDutyCycle> HalPwmChannel for PwmProvider<T> {
    type Error = T::Error;

    fn max_duty(&self) -> u16 {
        self.0.max_duty_cycle()
    }

    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
        self.0.set_duty_cycle(duty.min(self.0.max_duty_cycle()))
    }
}

pub struct Esp32S3Usb<'d>(UsbSerialJtag<'d, Blocking>);

impl<'d> Esp32S3Usb<'d> {
    pub fn new(usb: UsbSerialJtag<'d, Blocking>) -> Self {
        Self(usb)
    }
}

impl HalByteIo for Esp32S3Usb<'_> {
    type Error = Infallible;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        let mut count = 0;
        while count < bytes.len() {
            match self.0.read_byte() {
                Ok(byte) => {
                    bytes[count] = byte;
                    count += 1;
                }
                Err(nb::Error::WouldBlock) => break,
                Err(nb::Error::Other(error)) => return Err(error),
            }
        }
        Ok(count)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write_bytes(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush_tx()
    }
}

impl fmt::Write for Esp32S3Usb<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_all(value.as_bytes()).map_err(|_| fmt::Error)
    }
}
