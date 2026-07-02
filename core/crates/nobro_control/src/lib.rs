//! No-heap control primitives (f32, FPU-friendly).
//! - [`Pid`] proportional-integral-derivative controller with anti-windup + output clamp (M148)
//! - [`ComplementaryFilter`] fuse an accel-derived angle with integrated gyro rate (M149)
#![cfg_attr(not(test), no_std)]

/// PID controller with integral anti-windup and output saturation.
#[derive(Clone, Copy, Debug)]
pub struct Pid {
    pub kp: f32,
    pub ki: f32,
    pub kd: f32,
    pub out_min: f32,
    pub out_max: f32,
    integral: f32,
    prev_error: f32,
    primed: bool,
}

impl Pid {
    pub fn new(kp: f32, ki: f32, kd: f32, out_min: f32, out_max: f32) -> Self {
        Self {
            kp,
            ki,
            kd,
            out_min,
            out_max,
            integral: 0.0,
            prev_error: 0.0,
            primed: false,
        }
    }

    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
        self.primed = false;
    }

    /// One control step; `dt` in seconds. The integral term is clamped to the output
    /// range (anti-windup) and the result is saturated to [out_min, out_max].
    pub fn update(&mut self, setpoint: f32, measurement: f32, dt: f32) -> f32 {
        let error = setpoint - measurement;
        if self.ki != 0.0 {
            self.integral =
                (self.integral + error * dt).clamp(self.out_min / self.ki, self.out_max / self.ki);
        }
        let deriv = if self.primed && dt > 0.0 {
            (error - self.prev_error) / dt
        } else {
            0.0
        };
        self.prev_error = error;
        self.primed = true;
        (self.kp * error + self.ki * self.integral + self.kd * deriv)
            .clamp(self.out_min, self.out_max)
    }
}

/// Complementary filter: fuse a (noisy but drift-free) accel-derived angle with a
/// (smooth but drifting) gyro rate. `alpha` weights the gyro-integrated estimate.
#[derive(Clone, Copy, Debug)]
pub struct ComplementaryFilter {
    angle: f32,
    alpha: f32,
    primed: bool,
}

impl ComplementaryFilter {
    pub const fn new(alpha: f32) -> Self {
        Self { angle: 0.0, alpha, primed: false }
    }

    /// `accel_angle` from the accelerometer, `gyro_rate` in same angle units per second.
    pub fn update(&mut self, accel_angle: f32, gyro_rate: f32, dt: f32) -> f32 {
        if !self.primed {
            self.angle = accel_angle;
            self.primed = true;
        } else {
            self.angle =
                self.alpha * (self.angle + gyro_rate * dt) + (1.0 - self.alpha) * accel_angle;
        }
        self.angle
    }

    pub fn angle(&self) -> f32 {
        self.angle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_proportional_and_saturates() {
        let mut pid = Pid::new(2.0, 0.0, 0.0, -50.0, 50.0);
        assert!((pid.update(10.0, 0.0, 0.1) - 20.0).abs() < 1e-3); // 2 * 10
        assert_eq!(pid.update(100.0, 0.0, 0.1), 50.0); // clamps to out_max
    }

    #[test]
    fn pid_integral_anti_windup() {
        let mut pid = Pid::new(0.0, 1.0, 0.0, -10.0, 10.0);
        for _ in 0..100 {
            pid.update(100.0, 0.0, 1.0); // integral would blow up without clamping
        }
        let out = pid.update(100.0, 0.0, 1.0);
        assert!(out <= 10.0 + 1e-3, "output {out} exceeded clamp");
    }

    #[test]
    fn complementary_primes_then_fuses() {
        let mut cf = ComplementaryFilter::new(0.98);
        assert_eq!(cf.update(5.0, 0.0, 0.01), 5.0); // primes to the accel angle
        // gyro +100 deg/s over 0.01 s = +1 deg; accel says 5 deg
        let a = cf.update(5.0, 100.0, 0.01);
        assert!((a - 5.98).abs() < 1e-3); // 0.98*(5+1) + 0.02*5
    }
}

/// Differential-drive kinematics (M151): convert body velocities to wheel speeds and
/// back. `wheel_base` is the distance between wheels; units are consistent (e.g. m, m/s,
/// rad/s).
#[derive(Clone, Copy, Debug)]
pub struct DiffDrive {
    pub wheel_base: f32,
}

impl DiffDrive {
    pub const fn new(wheel_base: f32) -> Self {
        Self { wheel_base }
    }

    /// (linear, angular) -> (left, right) wheel linear speeds.
    pub fn to_wheels(&self, linear: f32, angular: f32) -> (f32, f32) {
        let half = angular * self.wheel_base / 2.0;
        (linear - half, linear + half)
    }

    /// (left, right) wheel speeds -> (linear, angular) body velocities.
    pub fn to_body(&self, left: f32, right: f32) -> (f32, f32) {
        ((left + right) / 2.0, (right - left) / self.wheel_base)
    }
}

#[cfg(test)]
mod diff_drive_tests {
    use super::*;

    #[test]
    fn wheels_and_body_roundtrip() {
        let dd = DiffDrive::new(0.2);
        // pure rotation: wheels equal and opposite
        let (l, r) = dd.to_wheels(0.0, 1.0);
        assert!((l + 0.1).abs() < 1e-6 && (r - 0.1).abs() < 1e-6);
        // straight line: wheels equal
        let (l, r) = dd.to_wheels(0.5, 0.0);
        assert_eq!((l, r), (0.5, 0.5));
        // roundtrip
        let (lin, ang) = dd.to_body(0.4, 0.6);
        assert!((lin - 0.5).abs() < 1e-6 && (ang - 1.0).abs() < 1e-6);
    }
}
