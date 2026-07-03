//! Extensible device-module framework for NobroRTOS.
//!
//! A new servo, motor, or sensor **brand** is DATA - a `ServoProfile` / `MotorProfile` /
//! `SensorDescriptor` const - not code. Config-driven generic drivers turn that data into
//! working actuation/identification, so common hardware works out of the box and third
//! parties extend the supported-hardware list without touching the core. This is the
//! data-first extensibility of a devicetree with the safety of Rust traits and the
//! approachability of an Arduino library.
#![cfg_attr(not(test), no_std)]

// ---------------------------------------------------------------- taxonomy

/// Actuator category (extend as new kinds appear).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActuatorKind {
    /// Angular servo (position by pulse width).
    Servo,
    /// Continuous-rotation servo (speed by pulse width; center = stop).
    ContinuousServo,
    /// Brushless ESC (throttle by pulse width; needs arming).
    Esc,
    /// Brushed DC motor via an H-bridge (PWM duty + direction).
    DcMotor,
    /// Stepper (steps; profile carries steps/rev).
    Stepper,
}

/// Sensor category (extend as new categories appear).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    Imu,
    Accelerometer,
    Gyroscope,
    Magnetometer,
    Power,
    Pressure,
    Temperature,
    Humidity,
    Distance,
    Light,
    Sound,
}

/// Bus a device sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    I2c,
    Spi,
    Analog,
    Pwm,
}

// ---------------------------------------------------------------- actuators

/// A servo brand as data. `angle_to_pulse` is the generic driver every angular servo
/// shares - adding a brand is just another `ServoProfile` const.
#[derive(Clone, Copy, Debug)]
pub struct ServoProfile {
    pub name: &'static str,
    pub kind: ActuatorKind,
    pub min_us: u16,
    pub max_us: u16,
    pub center_us: u16,
    pub min_deg: i16,
    pub max_deg: i16,
    pub reversed: bool,
}

impl ServoProfile {
    /// Map an angle (deg) to a servo pulse (us), clamped to the profile's travel.
    pub fn angle_to_pulse(&self, deg: i16) -> u16 {
        let d = deg.clamp(self.min_deg, self.max_deg);
        let span_deg = (self.max_deg - self.min_deg).max(1) as i32;
        let span_us = self.max_us as i32 - self.min_us as i32;
        let frac = if self.reversed {
            (self.max_deg - d) as i32
        } else {
            (d - self.min_deg) as i32
        };
        (self.min_us as i32 + frac * span_us / span_deg) as u16
    }

    /// Continuous servo: map speed [-1000,1000] to a pulse around center.
    pub fn speed_to_pulse(&self, speed_milli: i32) -> u16 {
        let s = speed_milli.clamp(-1000, 1000);
        let s = if self.reversed { -s } else { s };
        let half = (self.max_us as i32 - self.center_us as i32).max(1);
        (self.center_us as i32 + s * half / 1000) as u16
    }
}

/// A motor / ESC brand as data. `throttle_to_pulse` is the shared driver.
#[derive(Clone, Copy, Debug)]
pub struct MotorProfile {
    pub name: &'static str,
    pub kind: ActuatorKind,
    pub min_us: u16,  // full reverse (bidir ESC) or idle (unidir)
    pub max_us: u16,  // full forward
    pub arm_us: u16,  // pulse to hold during the ESC arm delay
    pub bidirectional: bool,
    pub reversed: bool,
}

impl MotorProfile {
    /// Throttle in milli: unidirectional 0..1000, bidirectional -1000..1000.
    pub fn throttle_to_pulse(&self, throttle_milli: i32) -> u16 {
        if self.bidirectional {
            let t = throttle_milli.clamp(-1000, 1000);
            let t = if self.reversed { -t } else { t };
            let mid = (self.min_us as i32 + self.max_us as i32) / 2;
            let half = (self.max_us as i32 - mid).max(1);
            (mid + t * half / 1000) as u16
        } else {
            let t = throttle_milli.clamp(0, 1000);
            let t = if self.reversed { 1000 - t } else { t };
            let span = self.max_us as i32 - self.min_us as i32;
            (self.min_us as i32 + t * span / 1000) as u16
        }
    }
    /// Pulse to emit while arming (ESCs require a low/idle hold before they run).
    pub fn arm_pulse(&self) -> u16 {
        self.arm_us
    }
}

/// Built-in brand catalog. Third parties add a `pub const` of the same shape - that is
/// the entire "add a new servo/motor brand" workflow.
pub mod catalog {
    use super::*;

    pub const SERVO_GENERIC_180: ServoProfile = ServoProfile {
        name: "generic 180 servo",
        kind: ActuatorKind::Servo,
        min_us: 500,
        max_us: 2500,
        center_us: 1500,
        min_deg: 0,
        max_deg: 180,
        reversed: false,
    };
    pub const SERVO_SG90: ServoProfile = ServoProfile {
        name: "SG90-class 9g",
        kind: ActuatorKind::Servo,
        min_us: 500,
        max_us: 2400,
        center_us: 1450,
        min_deg: 0,
        max_deg: 180,
        reversed: false,
    };
    pub const SERVO_MG996R: ServoProfile = ServoProfile {
        name: "MG996R-class metal-gear",
        kind: ActuatorKind::Servo,
        min_us: 1000,
        max_us: 2000,
        center_us: 1500,
        min_deg: 0,
        max_deg: 120,
        reversed: false,
    };
    pub const SERVO_CONTINUOUS: ServoProfile = ServoProfile {
        name: "continuous-rotation servo",
        kind: ActuatorKind::ContinuousServo,
        min_us: 1000,
        max_us: 2000,
        center_us: 1500,
        min_deg: 0,
        max_deg: 0,
        reversed: false,
    };

    pub const ESC_UNIDIR: MotorProfile = MotorProfile {
        name: "generic unidirectional ESC",
        kind: ActuatorKind::Esc,
        min_us: 1000,
        max_us: 2000,
        arm_us: 1000,
        bidirectional: false,
        reversed: false,
    };
    pub const ESC_BIDIR: MotorProfile = MotorProfile {
        name: "bidirectional (3D) ESC",
        kind: ActuatorKind::Esc,
        min_us: 1000,
        max_us: 2000,
        arm_us: 1500,
        bidirectional: true,
        reversed: false,
    };
}

// ---------------------------------------------------------------- sensors

/// A sensor brand as data: category + bus + identity register so a probe can confirm the
/// right chip is present before a driver binds to it.
#[derive(Clone, Copy, Debug)]
pub struct SensorDescriptor {
    pub name: &'static str,
    pub kind: SensorKind,
    pub bus: Bus,
    /// I2C address (or SPI CS id).
    pub address: u8,
    /// Identity register + expected value (0/0 = no identity check).
    pub whoami_reg: u8,
    pub whoami_val: u8,
}

impl SensorDescriptor {
    /// Confirm identity: does `read_reg(whoami_reg)` return `whoami_val`?
    pub fn identify(&self, read_reg: impl Fn(u8) -> u8) -> bool {
        self.whoami_reg == 0 && self.whoami_val == 0 || read_reg(self.whoami_reg) == self.whoami_val
    }
}

/// Built-in sensor catalog (matches the drivers already in the tree). Extend by adding a
/// `pub const`.
pub mod sensor_catalog {
    use super::*;

    pub const MPU9250: SensorDescriptor = SensorDescriptor {
        name: "MPU-9250 9-axis IMU",
        kind: SensorKind::Imu,
        bus: Bus::Spi,
        address: 0,
        whoami_reg: 0x75,
        whoami_val: 0x71,
    };
    pub const INA3221: SensorDescriptor = SensorDescriptor {
        name: "INA3221 3-ch power monitor",
        kind: SensorKind::Power,
        bus: Bus::I2c,
        address: 0x40,
        whoami_reg: 0xFE,
        whoami_val: 0x49, // low byte of the TI manufacturer id
    };
    pub const BMP280: SensorDescriptor = SensorDescriptor {
        name: "BMP280 pressure/temp",
        kind: SensorKind::Pressure,
        bus: Bus::I2c,
        address: 0x76,
        whoami_reg: 0xD0,
        whoami_val: 0x58,
    };
    pub const ICM45686: SensorDescriptor = SensorDescriptor {
        name: "ICM-45686 6-axis IMU",
        kind: SensorKind::Imu,
        bus: Bus::I2c,
        address: 0x68,
        whoami_reg: 0x72,
        whoami_val: 0xE9,
    };
    pub const MPU6050: SensorDescriptor = SensorDescriptor {
        name: "MPU-6050 6-axis IMU",
        kind: SensorKind::Imu,
        bus: Bus::I2c,
        address: 0x68,
        whoami_reg: 0x75,
        whoami_val: 0x68,
    };
}

// ---------------------------------------------------------------- registry

/// A fixed-capacity registry of sensor descriptors: enumerate what a build supports and
/// look one up by category. Third parties push their own descriptors in.
pub struct SensorRegistry<const N: usize> {
    items: [Option<SensorDescriptor>; N],
    len: usize,
}

impl<const N: usize> Default for SensorRegistry<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> SensorRegistry<N> {
    pub const fn new() -> Self {
        Self { items: [None; N], len: 0 }
    }
    pub fn register(&mut self, d: SensorDescriptor) -> bool {
        if self.len >= N {
            return false;
        }
        self.items[self.len] = Some(d);
        self.len += 1;
        true
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// First registered sensor of a category.
    pub fn find_kind(&self, kind: SensorKind) -> Option<SensorDescriptor> {
        self.items
            .iter()
            .flatten()
            .copied()
            .find(|d| d.kind == kind)
    }
    /// Identify which registered sensor is actually on the bus at `address`.
    pub fn identify_at(
        &self,
        address: u8,
        read_reg: impl Fn(u8) -> u8 + Copy,
    ) -> Option<SensorDescriptor> {
        self.items
            .iter()
            .flatten()
            .copied()
            .find(|d| d.address == address && d.identify(read_reg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servo_angle_maps_and_clamps_per_brand() {
        let sg = catalog::SERVO_SG90;
        assert_eq!(sg.angle_to_pulse(0), 500);
        assert_eq!(sg.angle_to_pulse(180), 2400);
        assert_eq!(sg.angle_to_pulse(90), 1450); // midpoint
        assert_eq!(sg.angle_to_pulse(999), 2400); // clamp high
        // a different brand yields different pulses from the SAME angle - data-driven
        let mg = catalog::SERVO_MG996R;
        assert_eq!(mg.angle_to_pulse(0), 1000);
        assert_eq!(mg.angle_to_pulse(120), 2000);
    }

    #[test]
    fn reversed_servo_inverts() {
        let mut rev = catalog::SERVO_GENERIC_180;
        rev.reversed = true;
        assert_eq!(rev.angle_to_pulse(0), 2500);
        assert_eq!(rev.angle_to_pulse(180), 500);
    }

    #[test]
    fn continuous_servo_speed_center_is_stop() {
        let c = catalog::SERVO_CONTINUOUS;
        assert_eq!(c.speed_to_pulse(0), 1500);
        assert_eq!(c.speed_to_pulse(1000), 2000);
        assert_eq!(c.speed_to_pulse(-1000), 1000);
    }

    #[test]
    fn esc_throttle_uni_and_bidir() {
        let uni = catalog::ESC_UNIDIR;
        assert_eq!(uni.arm_pulse(), 1000);
        assert_eq!(uni.throttle_to_pulse(0), 1000);
        assert_eq!(uni.throttle_to_pulse(1000), 2000);
        assert_eq!(uni.throttle_to_pulse(500), 1500);
        let bi = catalog::ESC_BIDIR;
        assert_eq!(bi.throttle_to_pulse(0), 1500); // center = stop
        assert_eq!(bi.throttle_to_pulse(-1000), 1000);
        assert_eq!(bi.throttle_to_pulse(1000), 2000);
    }

    #[test]
    fn sensor_registry_finds_and_identifies() {
        let mut reg = SensorRegistry::<8>::new();
        reg.register(sensor_catalog::INA3221);
        reg.register(sensor_catalog::BMP280);
        reg.register(sensor_catalog::MPU6050);
        assert_eq!(reg.len(), 3);
        assert_eq!(
            reg.find_kind(SensorKind::Power).map(|d| d.name),
            Some("INA3221 3-ch power monitor")
        );
        // two IMUs share address 0x68; identity picks the right one from WHO_AM_I
        let is_mpu6050 = |r: u8| if r == 0x75 { 0x68 } else { 0 };
        assert_eq!(
            reg.identify_at(0x68, is_mpu6050).map(|d| d.name),
            Some("MPU-6050 6-axis IMU")
        );
        // no chip answering -> None
        assert!(reg.identify_at(0x76, |_| 0x00).is_none());
    }
}
