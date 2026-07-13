//! No-heap control primitives (f32, FPU-friendly).
//! - [`Pid`] proportional-integral-derivative controller with anti-windup + output clamp
//! - [`ComplementaryFilter`] fuse an accel-derived angle with integrated gyro rate
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
        Self {
            angle: 0.0,
            alpha,
            primed: false,
        }
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

/// Differential-drive kinematics: convert body velocities to wheel speeds and
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

// ---- Trajectory, actuator mixing, safety envelope ----

/// A 1-D trapezoidal-velocity trajectory follower: given start, goal, cruise
/// velocity and acceleration, sample position/velocity along a smooth profile.
#[derive(Clone, Copy, Debug)]
pub struct Trajectory {
    start: f32,
    goal: f32,
    vmax: f32,
    accel: f32,
}

impl Trajectory {
    pub fn new(start: f32, goal: f32, vmax: f32, accel: f32) -> Self {
        Self {
            start,
            goal,
            vmax: vmax.abs().max(1e-3),
            accel: accel.abs().max(1e-3),
        }
    }

    fn dist(&self) -> f32 {
        (self.goal - self.start).abs()
    }

    /// Total motion duration (s), accounting for triangular profiles when the move is
    /// too short to reach cruise velocity.
    pub fn duration(&self) -> f32 {
        let d = self.dist();
        let t_ramp = self.vmax / self.accel;
        let d_ramp = 0.5 * self.accel * t_ramp * t_ramp;
        if 2.0 * d_ramp >= d {
            2.0 * sqrt_approx(d / self.accel) // triangular
        } else {
            2.0 * t_ramp + (d - 2.0 * d_ramp) / self.vmax // trapezoidal
        }
    }

    /// Sample commanded position at time `t` (s), clamped to [start, goal].
    pub fn position(&self, t: f32) -> f32 {
        let dir = if self.goal >= self.start { 1.0 } else { -1.0 };
        let d = self.dist();
        let dur = self.duration();
        if t <= 0.0 {
            return self.start;
        }
        if t >= dur {
            return self.goal;
        }
        let t_ramp = self.vmax / self.accel;
        let d_ramp = 0.5 * self.accel * t_ramp * t_ramp;
        let s = if 2.0 * d_ramp >= d {
            let t_peak = dur / 2.0;
            if t <= t_peak {
                0.5 * self.accel * t * t
            } else {
                let td = t - t_peak;
                d - 0.5 * self.accel * (t_peak - td).max(0.0) * (t_peak - td).max(0.0)
            }
        } else if t < t_ramp {
            0.5 * self.accel * t * t
        } else if t < dur - t_ramp {
            d_ramp + self.vmax * (t - t_ramp)
        } else {
            let td = dur - t;
            d - 0.5 * self.accel * td * td
        };
        self.start + dir * s.clamp(0.0, d)
    }
}

/// Differential-drive actuator mixer: map a (linear, angular) command to left and
/// right wheel efforts, saturating symmetrically so turning authority is preserved when
/// the linear term already saturates a side.
pub fn diff_drive_mix(linear: f32, angular: f32, limit: f32) -> (f32, f32) {
    let mut l = linear - angular;
    let mut r = linear + angular;
    let peak = l.abs().max(r.abs());
    if peak > limit && peak > 0.0 {
        let k = limit / peak;
        l *= k;
        r *= k;
    }
    (l, r)
}

/// Safety envelope / e-stop: latches a fault when any monitored signal leaves its
/// bound or a heartbeat goes stale, and forces the actuator command to the safe value
/// until explicitly reset.
#[derive(Clone, Copy, Debug)]
pub struct SafetyEnvelope {
    tripped: bool,
    tilt_limit: f32,
    heartbeat_timeout_us: u64,
    last_heartbeat_us: u64,
    safe_output: f32,
}

impl SafetyEnvelope {
    pub fn new(tilt_limit: f32, heartbeat_timeout_us: u64, safe_output: f32) -> Self {
        Self {
            tripped: false,
            tilt_limit: tilt_limit.abs(),
            heartbeat_timeout_us,
            last_heartbeat_us: 0,
            safe_output,
        }
    }

    pub fn heartbeat(&mut self, now_us: u64) {
        self.last_heartbeat_us = now_us;
    }

    /// Evaluate the guards; returns the command to actually apply (`safe_output` once
    /// tripped). Trips permanently until [`reset`] is called.
    pub fn guard(&mut self, tilt: f32, now_us: u64, desired: f32) -> f32 {
        if tilt.abs() > self.tilt_limit
            || now_us.saturating_sub(self.last_heartbeat_us) > self.heartbeat_timeout_us
        {
            self.tripped = true;
        }
        if self.tripped {
            self.safe_output
        } else {
            desired
        }
    }

    pub fn tripped(&self) -> bool {
        self.tripped
    }

    pub fn reset(&mut self, now_us: u64) {
        self.tripped = false;
        self.last_heartbeat_us = now_us;
    }
}

/// Wheel odometry integrator: accumulate pose (x, y, heading) from left/right
/// wheel travel over a differential-drive base of track width `w`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Odometry {
    pub x: f32,
    pub y: f32,
    pub heading: f32,
}

impl Odometry {
    pub fn update(&mut self, dl: f32, dr: f32, track_w: f32) {
        let dc = 0.5 * (dl + dr);
        let dtheta = (dr - dl) / track_w.max(1e-3);
        let mid = self.heading + 0.5 * dtheta;
        self.x += dc * cos_approx(mid);
        self.y += dc * sin_approx(mid);
        self.heading = wrap_pi(self.heading + dtheta);
    }
}

// libm-free helpers (no_std, adequate precision for control)
fn sqrt_approx(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = x;
    for _ in 0..12 {
        g = 0.5 * (g + x / g);
    }
    g
}

// small libm-free trig (adequate for odometry heading integration)
fn wrap_pi(a: f32) -> f32 {
    let two_pi = core::f32::consts::PI * 2.0;
    let mut x = a % two_pi;
    if x > core::f32::consts::PI {
        x -= two_pi;
    } else if x < -core::f32::consts::PI {
        x += two_pi;
    }
    x
}
fn sin_approx(x: f32) -> f32 {
    // Bhaskara-style minimax on [-pi, pi]
    let x = wrap_pi(x);
    let b = 4.0 / core::f32::consts::PI;
    let c = -4.0 / (core::f32::consts::PI * core::f32::consts::PI);
    let y = b * x + c * x * x.abs();
    0.775 * y + 0.225 * (y * y.abs())
}
fn cos_approx(x: f32) -> f32 {
    sin_approx(x + core::f32::consts::FRAC_PI_2)
}

/// Balance/inverted-pendulum controller: a PD law on tilt error that outputs a
/// wheel effort to keep the body upright. Thin wrapper over [`Pid`] with the derivative
/// term fed by the measured angular rate (cleaner than differencing angle).
#[derive(Clone, Copy, Debug)]
pub struct BalanceController {
    kp: f32,
    kd: f32,
    limit: f32,
}

impl BalanceController {
    pub fn new(kp: f32, kd: f32, limit: f32) -> Self {
        Self { kp, kd, limit }
    }
    /// `tilt` (rad from upright), `rate` (rad/s). Returns wheel effort in [-limit, limit].
    pub fn effort(&self, tilt: f32, rate: f32) -> f32 {
        (self.kp * tilt + self.kd * rate).clamp(-self.limit, self.limit)
    }
}

#[cfg(test)]
mod motion_tests {
    use super::*;

    #[test]
    fn trajectory_starts_at_start_reaches_goal_and_is_monotonic() {
        let tr = Trajectory::new(0.0, 100.0, 50.0, 25.0);
        let dur = tr.duration();
        assert!(dur > 0.0);
        assert!((tr.position(0.0) - 0.0).abs() < 1e-3);
        assert!((tr.position(dur * 2.0) - 100.0).abs() < 1e-2);
        let mut prev = -1.0;
        for i in 0..=20 {
            let p = tr.position(dur * i as f32 / 20.0);
            assert!(p >= prev - 1e-3, "not monotonic at {i}: {p} < {prev}");
            prev = p;
        }
    }

    #[test]
    fn diff_drive_mix_preserves_turn_under_saturation() {
        assert_eq!(diff_drive_mix(0.5, 0.2, 1.0), (0.3, 0.7));
        // linear already saturates; symmetric scale keeps the turn ratio
        let (l, r) = diff_drive_mix(1.0, 0.5, 1.0);
        assert!((r - 1.0).abs() < 1e-6 && (l - 1.0 / 3.0).abs() < 1e-6);
        assert!(l <= 1.0 && r <= 1.0);
    }

    #[test]
    fn safety_envelope_trips_on_tilt_and_stale_heartbeat() {
        let mut env = SafetyEnvelope::new(0.5, 100_000, 0.0);
        env.heartbeat(0);
        assert_eq!(env.guard(0.1, 1_000, 0.8), 0.8); // nominal
        assert_eq!(env.guard(0.9, 2_000, 0.8), 0.0); // tilt over limit -> safe
        assert!(env.tripped());
        env.reset(3_000);
        assert_eq!(env.guard(0.1, 3_500, 0.8), 0.8);
        // stale heartbeat trips too
        assert_eq!(env.guard(0.1, 3_500 + 200_000, 0.8), 0.0);
    }

    #[test]
    fn odometry_straight_line_and_turn() {
        let mut od = Odometry::default();
        od.update(1.0, 1.0, 0.2); // straight ahead 1 m
        assert!((od.x - 1.0).abs() < 1e-3 && od.y.abs() < 1e-3);
        let mut od2 = Odometry::default();
        od2.update(-0.1, 0.1, 0.2); // spin in place: +1 rad
        assert!(od2.x.abs() < 1e-3 && (od2.heading - 1.0).abs() < 1e-2);
    }

    #[test]
    fn balance_controller_pushes_against_tilt() {
        let bc = BalanceController::new(20.0, 2.0, 100.0);
        assert!(bc.effort(0.1, 0.0) > 0.0); // tilt forward -> forward effort
        assert!(bc.effort(-0.1, 0.0) < 0.0);
        assert_eq!(bc.effort(10.0, 0.0), 100.0); // saturates
    }
}
