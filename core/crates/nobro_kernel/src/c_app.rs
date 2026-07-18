//! Allocation-free task/wire declaration and dispatch for the plain-C Tier-C facade.
//!
//! This is intentionally a thin language bridge over [`AppGraph`]. It does not
//! maintain a second admission algorithm: [`CApp::run`] expands the registered
//! records into the same graph builder used by Rust applications, then starts
//! periodic callback dispatch only after that graph passes profile validation.

use crate::{AppGraph, Criticality, GraphError, SystemProfile, TaskDecl};

/// A C-compatible unit of periodic application work.
pub type CTaskStep = extern "C" fn() -> i32;

/// Beginner-facing role presets. Timing fields can still be overridden explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CTaskRole {
    /// Deadline-aware periodic driver work.
    Periodic,
    /// Tighter-jitter hard-real-time control work.
    Control,
    /// Best-effort background work without a deadline contract.
    Service,
}

/// Optional overrides for the defaults selected by [`CTaskRole`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CTaskOptions {
    pub role: CTaskRole,
    /// Zero keeps the role preset.
    pub budget_us: u32,
    /// Zero keeps the period-sized default deadline.
    pub deadline_us: u32,
    /// Zero keeps the role's derived jitter bound.
    pub jitter_us: u32,
    pub blocking_us: u32,
}

impl CTaskOptions {
    pub const DEFAULT: Self = Self {
        role: CTaskRole::Periodic,
        budget_us: 0,
        deadline_us: 0,
        jitter_us: 0,
        blocking_us: 0,
    };
}

impl Default for CTaskOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug)]
struct CTask {
    name: &'static str,
    period_us: u32,
    step: CTaskStep,
    options: CTaskOptions,
    next_release_us: u64,
}

#[derive(Clone, Copy, Debug)]
struct CWire {
    from: &'static str,
    to: &'static str,
    capacity: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CAppState {
    Configuring,
    Running,
    Faulted,
}

/// Stable failure classes exported by the C ABI as negative integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CAppError {
    InvalidState,
    InvalidName,
    InvalidPeriod,
    TaskCapacity,
    WireCapacity,
    UnknownEndpoint,
    DuplicateTask,
    InvalidOptions,
    Admission,
    StepFailed { task: &'static str, code: i32 },
}

impl CAppError {
    pub const fn status(self) -> i32 {
        match self {
            Self::InvalidState => -1,
            Self::InvalidName => -2,
            Self::InvalidPeriod => -3,
            Self::TaskCapacity => -4,
            Self::WireCapacity => -5,
            Self::UnknownEndpoint => -6,
            Self::DuplicateTask => -7,
            Self::InvalidOptions => -8,
            Self::Admission => -9,
            Self::StepFailed { .. } => -10,
        }
    }
}

/// Result of one bounded dispatch pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CDispatchReport {
    pub ran: u8,
    pub skipped: u32,
    pub next_release_us: Option<u64>,
}

/// Fixed-capacity C application declaration.
///
/// `TASKS` and `WIRES` are compile-time capacities; registration never allocates.
/// Names must be static lowercase ASCII labels because the admitted graph retains
/// them for diagnostics.
pub struct CApp<const TASKS: usize, const WIRES: usize> {
    tasks: [Option<CTask>; TASKS],
    task_len: usize,
    wires: [Option<CWire>; WIRES],
    wire_len: usize,
    state: CAppState,
    skipped_releases: u32,
}

impl<const TASKS: usize, const WIRES: usize> CApp<TASKS, WIRES> {
    pub const fn new() -> Self {
        Self {
            tasks: [None; TASKS],
            task_len: 0,
            wires: [None; WIRES],
            wire_len: 0,
            state: CAppState::Configuring,
            skipped_releases: 0,
        }
    }

    fn valid_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 48
            && bytes[0].is_ascii_lowercase()
            && bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_' || *byte == b'-'
            })
    }

    pub fn task(
        &mut self,
        name: &'static str,
        period_us: u32,
        step: CTaskStep,
        options: CTaskOptions,
    ) -> Result<(), CAppError> {
        if self.state != CAppState::Configuring {
            return Err(CAppError::InvalidState);
        }
        if !Self::valid_name(name) {
            return Err(CAppError::InvalidName);
        }
        if period_us == 0 {
            return Err(CAppError::InvalidPeriod);
        }
        if self.tasks[..self.task_len]
            .iter()
            .flatten()
            .any(|task| task.name == name)
        {
            return Err(CAppError::DuplicateTask);
        }
        if self.task_len == TASKS {
            return Err(CAppError::TaskCapacity);
        }
        let deadline_us = if options.deadline_us == 0 {
            period_us
        } else {
            options.deadline_us
        };
        let budget_us = if options.budget_us == 0 {
            (period_us / 10).max(1)
        } else {
            options.budget_us
        };
        if deadline_us > period_us
            || budget_us > deadline_us
            || options.blocking_us > deadline_us.saturating_sub(budget_us)
            || options.jitter_us >= period_us
        {
            return Err(CAppError::InvalidOptions);
        }
        self.tasks[self.task_len] = Some(CTask {
            name,
            period_us,
            step,
            options,
            next_release_us: 0,
        });
        self.task_len += 1;
        Ok(())
    }

    pub fn wire(
        &mut self,
        from: &'static str,
        to: &'static str,
        capacity: u8,
    ) -> Result<(), CAppError> {
        if self.state != CAppState::Configuring {
            return Err(CAppError::InvalidState);
        }
        if !Self::valid_name(from) || !Self::valid_name(to) {
            return Err(CAppError::InvalidName);
        }
        if capacity == 0 || capacity > 64 || self.wire_len == WIRES {
            return Err(CAppError::WireCapacity);
        }
        for endpoint in [from, to] {
            if !self.tasks[..self.task_len]
                .iter()
                .flatten()
                .any(|task| task.name == endpoint)
            {
                return Err(CAppError::UnknownEndpoint);
            }
        }
        if self.wires[..self.wire_len]
            .iter()
            .flatten()
            .any(|wire| wire.from == from && wire.to == to)
        {
            return Err(CAppError::WireCapacity);
        }
        self.wires[self.wire_len] = Some(CWire { from, to, capacity });
        self.wire_len += 1;
        Ok(())
    }

    fn declaration(task: CTask) -> TaskDecl {
        let mut declaration = match task.options.role {
            CTaskRole::Periodic => TaskDecl::periodic(task.name, task.period_us),
            CTaskRole::Control => TaskDecl::control(task.name, task.period_us),
            CTaskRole::Service => TaskDecl::service(task.name, task.period_us),
        };
        if task.options.budget_us != 0 {
            declaration = declaration.budget_us(task.options.budget_us);
        }
        if task.options.deadline_us != 0 {
            declaration = declaration.deadline_us(task.options.deadline_us);
        }
        if task.options.jitter_us != 0 {
            declaration = declaration.jitter_us(task.options.jitter_us);
        }
        declaration
            .blocking_us(task.options.blocking_us)
            .criticality(match task.options.role {
                CTaskRole::Periodic => Criticality::Driver,
                CTaskRole::Control => Criticality::HardRealtime,
                CTaskRole::Service => Criticality::BestEffort,
            })
    }

    fn graph_error(error: GraphError) -> CAppError {
        match error {
            GraphError::DuplicateName(_) => CAppError::DuplicateTask,
            GraphError::ChannelEndpointUnknown { .. } => CAppError::UnknownEndpoint,
            GraphError::TooManyTasks { .. } => CAppError::TaskCapacity,
            GraphError::TooManyChannels => CAppError::WireCapacity,
            _ => CAppError::Admission,
        }
    }

    /// Validate the declaration through the shared graph admission path and start it.
    ///
    /// `MODULES` must have room for the kernel plus every registered task.
    pub fn run<const MODULES: usize>(
        &mut self,
        profile: SystemProfile,
        now_us: u64,
    ) -> Result<(), CAppError> {
        if self.state != CAppState::Configuring || self.task_len == 0 {
            return Err(CAppError::InvalidState);
        }
        let mut graph = AppGraph::<TASKS>::new();
        for task in self.tasks[..self.task_len].iter().flatten().copied() {
            graph = graph
                .task(Self::declaration(task))
                .map_err(Self::graph_error)?;
        }
        for wire in self.wires[..self.wire_len].iter().flatten() {
            // Capacity is retained and bounded by this bridge. AppGraph owns the
            // relationship/capability admission; payload storage is a separate API.
            let _capacity = wire.capacity;
            graph = graph
                .channel(wire.from, wire.to)
                .map_err(Self::graph_error)?;
        }
        graph
            .build_for::<MODULES>(profile)
            .map_err(Self::graph_error)?;
        for task in self.tasks[..self.task_len].iter_mut().flatten() {
            task.next_release_us = now_us;
        }
        self.state = CAppState::Running;
        Ok(())
    }

    /// Run each due callback at most once and preserve its periodic phase.
    ///
    /// When polling is late by several periods, intermediate releases are counted
    /// as skipped rather than replayed in a burst.
    pub fn poll_at(&mut self, now_us: u64) -> Result<CDispatchReport, CAppError> {
        if self.state != CAppState::Running {
            return Err(CAppError::InvalidState);
        }
        let mut ran = 0u8;
        let mut skipped = 0u32;
        let mut next_release = None;
        for task in self.tasks[..self.task_len].iter_mut().flatten() {
            if now_us >= task.next_release_us {
                let late_periods =
                    u32::try_from((now_us - task.next_release_us) / u64::from(task.period_us))
                        .unwrap_or(u32::MAX);
                skipped = skipped.saturating_add(late_periods);
                self.skipped_releases = self.skipped_releases.saturating_add(late_periods);
                let advance = u64::from(task.period_us)
                    .saturating_mul(u64::from(late_periods).saturating_add(1));
                task.next_release_us = task.next_release_us.saturating_add(advance);
                let result = (task.step)();
                if result < 0 {
                    self.state = CAppState::Faulted;
                    return Err(CAppError::StepFailed {
                        task: task.name,
                        code: result,
                    });
                }
                ran = ran.saturating_add(1);
            }
            next_release = Some(match next_release {
                Some(current) => core::cmp::min(current, task.next_release_us),
                None => task.next_release_us,
            });
        }
        Ok(CDispatchReport {
            ran,
            skipped,
            next_release_us: next_release,
        })
    }

    pub const fn skipped_releases(&self) -> u32 {
        self.skipped_releases
    }
}

impl<const TASKS: usize, const WIRES: usize> Default for CApp<TASKS, WIRES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    static RUNS: AtomicU32 = AtomicU32::new(0);

    extern "C" fn pass() -> i32 {
        RUNS.fetch_add(1, Ordering::Relaxed);
        0
    }

    extern "C" fn fail() -> i32 {
        -23
    }

    extern "C" fn noop() -> i32 {
        0
    }

    fn profile() -> SystemProfile {
        SystemProfile::new(192 * 1024, 64 * 1024, 16, 9)
    }

    #[test]
    fn declaration_reuses_graph_admission_and_rejects_invalid_inputs() {
        let mut app = CApp::<2, 1>::new();
        assert_eq!(
            app.task("", 20_000, pass, CTaskOptions::DEFAULT),
            Err(CAppError::InvalidName)
        );
        assert_eq!(
            app.task("imu", 0, pass, CTaskOptions::DEFAULT),
            Err(CAppError::InvalidPeriod)
        );
        app.task("imu", 20_000, pass, CTaskOptions::DEFAULT)
            .unwrap();
        assert_eq!(
            app.task("imu", 20_000, pass, CTaskOptions::DEFAULT),
            Err(CAppError::DuplicateTask)
        );
        assert_eq!(
            app.wire("imu", "control", 8),
            Err(CAppError::UnknownEndpoint)
        );
        app.task(
            "control",
            20_000,
            pass,
            CTaskOptions {
                role: CTaskRole::Control,
                budget_us: 2_000,
                ..CTaskOptions::DEFAULT
            },
        )
        .unwrap();
        app.wire("imu", "control", 8).unwrap();
        app.run::<3>(profile(), 100).unwrap();
        assert_eq!(
            app.task("late", 20_000, pass, CTaskOptions::DEFAULT),
            Err(CAppError::InvalidState)
        );
    }

    #[test]
    fn dispatch_preserves_phase_and_counts_skipped_releases() {
        RUNS.store(0, Ordering::Relaxed);
        let mut app = CApp::<1, 0>::new();
        app.task("control", 10, pass, CTaskOptions::DEFAULT)
            .unwrap();
        app.run::<2>(profile(), 100).unwrap();
        assert_eq!(
            app.poll_at(100).unwrap(),
            CDispatchReport {
                ran: 1,
                skipped: 0,
                next_release_us: Some(110)
            }
        );
        assert_eq!(app.poll_at(109).unwrap().ran, 0);
        assert_eq!(
            app.poll_at(135).unwrap(),
            CDispatchReport {
                ran: 1,
                skipped: 2,
                next_release_us: Some(140)
            }
        );
        assert_eq!(app.skipped_releases(), 2);
        assert_eq!(RUNS.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn callback_failure_faults_the_application() {
        let mut app = CApp::<1, 0>::new();
        app.task("sensor", 1_000, fail, CTaskOptions::DEFAULT)
            .unwrap();
        app.run::<2>(profile(), 0).unwrap();
        assert_eq!(
            app.poll_at(0),
            Err(CAppError::StepFailed {
                task: "sensor",
                code: -23
            })
        );
        assert_eq!(app.poll_at(1), Err(CAppError::InvalidState));
    }

    #[test]
    fn option_and_capacity_errors_fail_before_admission() {
        let mut app = CApp::<1, 1>::new();
        assert_eq!(
            app.task(
                "control",
                100,
                pass,
                CTaskOptions {
                    budget_us: 90,
                    blocking_us: 11,
                    ..CTaskOptions::DEFAULT
                }
            ),
            Err(CAppError::InvalidOptions)
        );
        app.task("control", 100, pass, CTaskOptions::DEFAULT)
            .unwrap();
        assert_eq!(
            app.task("extra", 100, pass, CTaskOptions::DEFAULT),
            Err(CAppError::TaskCapacity)
        );
        assert_eq!(
            app.wire("control", "control", 0),
            Err(CAppError::WireCapacity)
        );
        assert_eq!(app.run::<1>(profile(), 0), Err(CAppError::TaskCapacity));
    }

    #[test]
    fn profile_admission_failure_does_not_start_dispatch() {
        let mut app = CApp::<1, 0>::new();
        app.task("control", 10_000, noop, CTaskOptions::DEFAULT)
            .unwrap();
        let too_small = SystemProfile::new(1, 1, 1, 2);
        assert_eq!(app.run::<2>(too_small, 0), Err(CAppError::Admission));
        assert_eq!(app.poll_at(0), Err(CAppError::InvalidState));
        app.run::<2>(profile(), 10).unwrap();
        assert_eq!(app.poll_at(10).unwrap().ran, 1);
    }
}
