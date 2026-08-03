//! No-heap power management policy: pick a sleep mode from activity + a deadline,
//! and track an active-time duty budget. Pure policy; the HAL applies the mode.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMode {
    Active,   // CPU running
    Idle,     // WFE/WFI, peripherals on
    LowPower, // peripherals gated, RTC wake
    Off,      // deepest sleep until external wake
}

/// Lifecycle for a bounded electrical telemetry provider. This is separate
/// from CPU sleep policy even though both belong to the power domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMonitorState {
    Down,
    Ready,
    Suspended,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMonitorError {
    InvalidConfig,
    InvalidChannel,
    NotReady,
    Timeout,
    Transport,
    DeadlineMiss,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerChannelSample {
    pub channel: u8,
    pub bus_uv: i64,
    pub shunt_uv: i64,
    pub current_ua: i64,
    pub power_uw: i64,
    pub sequence: u32,
    pub timestamp_us: u64,
}

pub trait PowerMonitorBackend {
    type Error;

    fn state(&self) -> PowerMonitorState;
    fn channel_count(&self) -> u8;
    fn sample_channel(
        &mut self,
        channel: u8,
        deadline_us: u64,
    ) -> Result<PowerChannelSample, Self::Error>;
    fn sample_all(
        &mut self,
        output: &mut [PowerChannelSample],
        deadline_us: u64,
    ) -> Result<usize, Self::Error>;
    fn quiesce(&mut self) -> Result<(), Self::Error>;
    fn recover(&mut self) -> Result<(), Self::Error>;
    fn release(&mut self) -> Result<(), Self::Error>;
}

impl PowerMode {
    pub const fn depth(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Idle => 1,
            Self::LowPower => 2,
            Self::Off => 3,
        }
    }

    pub const fn shallower(self, other: Self) -> Self {
        if self.depth() <= other.depth() {
            self
        } else {
            other
        }
    }
}

/// Typed reasons why an otherwise requested power mode was not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PowerVetoReason {
    UsbActive = 0,
    RadioActive = 1,
    DmaActive = 2,
    StorageTransaction = 3,
    DebugSession = 4,
    RecoverySession = 5,
    RestorationUnproven = 6,
    SystemOffNotOptedIn = 7,
    WakeUnavailable = 8,
    PlatformLimited = 9,
    SleepUnqualified = 10,
    WakeLatencyExceeded = 11,
    RetentionUnavailable = 12,
    DomainUnavailable = 13,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerVetoMask(u16);

impl PowerVetoMask {
    pub const fn from_reason(reason: PowerVetoReason) -> Self {
        Self(1 << reason as u8)
    }

    pub const fn contains(self, reason: PowerVetoReason) -> bool {
        self.0 & (1 << reason as u8) != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    fn insert(&mut self, reason: PowerVetoReason) {
        self.0 |= 1 << reason as u8;
    }
}

/// An active peripheral operation and the deepest mode compatible with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerLeaseKind {
    UsbActive,
    RadioActive,
    DmaActive,
    StorageTransaction,
    DebugSession,
    RecoverySession,
    RestorationUnproven,
}

impl PowerLeaseKind {
    const fn limit(self) -> PowerMode {
        match self {
            Self::UsbActive
            | Self::RadioActive
            | Self::DmaActive
            | Self::StorageTransaction
            | Self::RestorationUnproven => PowerMode::Idle,
            Self::DebugSession | Self::RecoverySession => PowerMode::Active,
        }
    }

    const fn reason(self) -> PowerVetoReason {
        match self {
            Self::UsbActive => PowerVetoReason::UsbActive,
            Self::RadioActive => PowerVetoReason::RadioActive,
            Self::DmaActive => PowerVetoReason::DmaActive,
            Self::StorageTransaction => PowerVetoReason::StorageTransaction,
            Self::DebugSession => PowerVetoReason::DebugSession,
            Self::RecoverySession => PowerVetoReason::RecoverySession,
            Self::RestorationUnproven => PowerVetoReason::RestorationUnproven,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerLease {
    slot: u16,
    epoch: u32,
    generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerLeaseError {
    Full,
    Stale,
}

#[derive(Clone, Copy)]
struct LeaseSlot {
    owner: u16,
    generation: u32,
    kind: PowerLeaseKind,
    active: bool,
}

impl LeaseSlot {
    const EMPTY: Self = Self {
        owner: 0,
        generation: 0,
        kind: PowerLeaseKind::RestorationUnproven,
        active: false,
    };
}

/// Fixed-capacity, no-heap lease set. Every active lease composes into the
/// shallowest safe power limit and all applicable typed veto reasons.
pub struct PowerLeaseTable<const N: usize> {
    slots: [LeaseSlot; N],
    epoch: u32,
}

impl<const N: usize> PowerLeaseTable<N> {
    pub const fn new() -> Self {
        Self {
            slots: [LeaseSlot::EMPTY; N],
            epoch: 0,
        }
    }

    pub fn acquire(
        &mut self,
        owner: u16,
        kind: PowerLeaseKind,
    ) -> Result<PowerLease, PowerLeaseError> {
        self.prepare_generation_epoch()?;
        let Some((slot, entry)) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| !entry.active && entry.generation < u32::MAX)
        else {
            return Err(PowerLeaseError::Full);
        };
        let slot = u16::try_from(slot).map_err(|_| PowerLeaseError::Full)?;
        entry.generation += 1;
        entry.owner = owner;
        entry.kind = kind;
        entry.active = true;
        Ok(PowerLease {
            slot,
            epoch: self.epoch,
            generation: entry.generation,
        })
    }

    pub fn release(&mut self, lease: PowerLease) -> Result<(), PowerLeaseError> {
        let Some(entry) = self.slots.get_mut(usize::from(lease.slot)) else {
            return Err(PowerLeaseError::Stale);
        };
        if lease.epoch != self.epoch || !entry.active || entry.generation != lease.generation {
            return Err(PowerLeaseError::Stale);
        }
        entry.active = false;
        Ok(())
    }

    pub fn owner(&self, lease: PowerLease) -> Result<u16, PowerLeaseError> {
        let Some(entry) = self.slots.get(usize::from(lease.slot)) else {
            return Err(PowerLeaseError::Stale);
        };
        if lease.epoch != self.epoch || !entry.active || entry.generation != lease.generation {
            return Err(PowerLeaseError::Stale);
        }
        Ok(entry.owner)
    }

    fn prepare_generation_epoch(&mut self) -> Result<(), PowerLeaseError> {
        if self
            .slots
            .iter()
            .any(|entry| !entry.active && entry.generation < u32::MAX)
        {
            return Ok(());
        }
        if self.slots.is_empty() || self.slots.iter().any(|entry| entry.active) {
            return Err(PowerLeaseError::Full);
        }
        self.epoch = self.epoch.checked_add(1).ok_or(PowerLeaseError::Full)?;
        for entry in &mut self.slots {
            entry.generation = 0;
        }
        Ok(())
    }

    fn admit(&self, requested: PowerMode) -> (PowerMode, PowerVetoMask) {
        let mut selected = requested;
        let mut vetoes = PowerVetoMask::default();
        for lease in self.slots.iter().filter(|lease| lease.active) {
            let constrained = selected.shallower(lease.kind.limit());
            if constrained != selected {
                selected = constrained;
            }
            if requested.depth() > lease.kind.limit().depth() {
                vetoes.insert(lease.kind.reason());
            }
        }
        (selected, vetoes)
    }
}

impl<const N: usize> Default for PowerLeaseTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeStyle {
    Resume,
    Reset,
}

/// Proven retained wake route required before SYSTEMOFF can be selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemOffWake {
    source: u16,
    style: WakeStyle,
    ram_retained: bool,
    peripherals_retained: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemOffWakeError {
    InvalidSource,
}

impl SystemOffWake {
    pub const fn new(
        source: u16,
        style: WakeStyle,
        ram_retained: bool,
        peripherals_retained: bool,
    ) -> Result<Self, SystemOffWakeError> {
        if source == 0 {
            return Err(SystemOffWakeError::InvalidSource);
        }
        Ok(Self {
            source,
            style,
            ram_retained,
            peripherals_retained,
        })
    }

    pub const fn source(self) -> u16 {
        self.source
    }

    pub const fn style(self) -> WakeStyle {
        self.style
    }

    pub const fn ram_retained(self) -> bool {
        self.ram_retained
    }

    pub const fn peripherals_retained(self) -> bool {
        self.peripherals_retained
    }
}

/// Complete, honest result of one executor-owned power decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerTransition {
    pub requested: PowerMode,
    pub selected: PowerMode,
    pub effective: PowerMode,
    pub vetoes: PowerVetoMask,
    pub system_off_wake: Option<SystemOffWake>,
    /// Present only when the caller used the evidence-bearing admitted-sleep
    /// path. Compatibility sleep execution retains `None` and makes no timing
    /// or retention claim.
    pub sleep_admission: Option<SleepAdmission>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerHookError {
    pub source: u16,
    pub code: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SleepProfile {
    pub provider_id: u16,
    pub generation: u32,
    pub deepest_mode: PowerMode,
    pub wake_latency_us: u32,
    pub wake_sources: u32,
    pub retained_state: u32,
    pub retained_clock_domains: u32,
    pub retained_peripheral_domains: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SleepRequirements {
    pub max_wake_latency_us: u32,
    pub required_wake_sources: u32,
    pub required_retained_state: u32,
    pub required_clock_domains: u32,
    pub required_peripheral_domains: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SleepAdmission {
    pub provider_id: u16,
    pub generation: u32,
    pub mode: PowerMode,
    pub wake_latency_us: u32,
    pub wake_sources: u32,
    pub retained_state: u32,
    pub retained_clock_domains: u32,
    pub retained_peripheral_domains: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepAdmissionError {
    InvalidProfile,
    InvalidRequirements,
    ModeUnavailable,
    WakeLatencyExceeded { latency_us: u32, limit_us: u32 },
    WakeSourceUnavailable,
    RetentionUnavailable,
    ClockDomainUnavailable,
    PeripheralDomainUnavailable,
}

impl SleepProfile {
    pub fn admit(
        self,
        mode: PowerMode,
        requirements: SleepRequirements,
        time_until_deadline_us: Option<u64>,
    ) -> Result<SleepAdmission, SleepAdmissionError> {
        if self.provider_id == 0
            || self.generation == 0
            || self.wake_latency_us == 0
            || self.wake_sources == 0
        {
            return Err(SleepAdmissionError::InvalidProfile);
        }
        if requirements.max_wake_latency_us == 0 || requirements.required_wake_sources == 0 {
            return Err(SleepAdmissionError::InvalidRequirements);
        }
        if mode == PowerMode::Active || mode.depth() > self.deepest_mode.depth() {
            return Err(SleepAdmissionError::ModeUnavailable);
        }
        let deadline_limit = time_until_deadline_us
            .unwrap_or(u64::from(u32::MAX))
            .min(u64::from(u32::MAX)) as u32;
        let latency_limit = requirements.max_wake_latency_us.min(deadline_limit);
        if self.wake_latency_us > latency_limit {
            return Err(SleepAdmissionError::WakeLatencyExceeded {
                latency_us: self.wake_latency_us,
                limit_us: latency_limit,
            });
        }
        if self.wake_sources & requirements.required_wake_sources
            != requirements.required_wake_sources
        {
            return Err(SleepAdmissionError::WakeSourceUnavailable);
        }
        if self.retained_state & requirements.required_retained_state
            != requirements.required_retained_state
        {
            return Err(SleepAdmissionError::RetentionUnavailable);
        }
        if self.retained_clock_domains & requirements.required_clock_domains
            != requirements.required_clock_domains
        {
            return Err(SleepAdmissionError::ClockDomainUnavailable);
        }
        if self.retained_peripheral_domains & requirements.required_peripheral_domains
            != requirements.required_peripheral_domains
        {
            return Err(SleepAdmissionError::PeripheralDomainUnavailable);
        }
        Ok(SleepAdmission {
            provider_id: self.provider_id,
            generation: self.generation,
            mode,
            wake_latency_us: self.wake_latency_us,
            wake_sources: self.wake_sources,
            retained_state: self.retained_state,
            retained_clock_domains: self.retained_clock_domains,
            retained_peripheral_domains: self.retained_peripheral_domains,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerApplyError {
    Admission(SleepAdmissionError),
    Hook(PowerHookError),
}

impl From<PowerHookError> for PowerApplyError {
    fn from(error: PowerHookError) -> Self {
        Self::Hook(error)
    }
}

/// Qualified timing limits for one exact hardware deadline provider.
/// Generation changes whenever clocking, prescaling, or ISR routing changes,
/// so an admission receipt cannot survive a provider reconfiguration race.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineTimingProfile {
    pub provider_id: u16,
    pub generation: u32,
    pub minimum_period_us: u32,
    pub resolution_us: u32,
    pub programming_overhead_us: u32,
    pub interrupt_overhead_us: u32,
    pub wake_latency_us: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineTimingRequest {
    pub period_us: u32,
    pub overhead_slack_us: u32,
    pub require_exact_resolution: bool,
}

impl DeadlineTimingRequest {
    pub const fn exact(period_us: u32, overhead_slack_us: u32) -> Self {
        Self {
            period_us,
            overhead_slack_us,
            require_exact_resolution: true,
        }
    }

    /// Permit an earlier representable compare, never a later one.
    pub const fn early(period_us: u32, overhead_slack_us: u32) -> Self {
        Self {
            period_us,
            overhead_slack_us,
            require_exact_resolution: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineTimingAdmission {
    pub provider_id: u16,
    pub generation: u32,
    pub minimum_period_us: u32,
    pub requested_period_us: u32,
    pub programmed_period_us: u32,
    pub resolution_us: u32,
    pub total_overhead_us: u32,
    pub require_exact_resolution: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineTimingError {
    InvalidProfile,
    InvalidRequest,
    PeriodTooShort {
        requested_us: u32,
        minimum_us: u32,
    },
    ResolutionMismatch {
        requested_us: u32,
        resolution_us: u32,
    },
    OverheadExceedsSlack {
        overhead_us: u32,
        slack_us: u32,
    },
    ProviderChanged,
}

impl DeadlineTimingProfile {
    pub const fn is_valid(self) -> bool {
        self.provider_id != 0
            && self.generation != 0
            && self.minimum_period_us != 0
            && self.resolution_us != 0
    }

    pub fn admit(
        self,
        request: DeadlineTimingRequest,
    ) -> Result<DeadlineTimingAdmission, DeadlineTimingError> {
        if !self.is_valid() {
            return Err(DeadlineTimingError::InvalidProfile);
        }
        if request.period_us == 0 || request.overhead_slack_us >= request.period_us {
            return Err(DeadlineTimingError::InvalidRequest);
        }
        if request.period_us < self.minimum_period_us {
            return Err(DeadlineTimingError::PeriodTooShort {
                requested_us: request.period_us,
                minimum_us: self.minimum_period_us,
            });
        }
        let remainder = request.period_us % self.resolution_us;
        if request.require_exact_resolution && remainder != 0 {
            return Err(DeadlineTimingError::ResolutionMismatch {
                requested_us: request.period_us,
                resolution_us: self.resolution_us,
            });
        }
        let programmed_period_us = request.period_us - remainder;
        if programmed_period_us < self.minimum_period_us {
            return Err(DeadlineTimingError::PeriodTooShort {
                requested_us: programmed_period_us,
                minimum_us: self.minimum_period_us,
            });
        }
        let total_overhead_us = self
            .programming_overhead_us
            .checked_add(self.interrupt_overhead_us)
            .and_then(|value| value.checked_add(self.wake_latency_us))
            .ok_or(DeadlineTimingError::InvalidProfile)?;
        if total_overhead_us > request.overhead_slack_us {
            return Err(DeadlineTimingError::OverheadExceedsSlack {
                overhead_us: total_overhead_us,
                slack_us: request.overhead_slack_us,
            });
        }
        Ok(DeadlineTimingAdmission {
            provider_id: self.provider_id,
            generation: self.generation,
            minimum_period_us: self.minimum_period_us,
            requested_period_us: request.period_us,
            programmed_period_us,
            resolution_us: self.resolution_us,
            total_overhead_us,
            require_exact_resolution: request.require_exact_resolution,
        })
    }

    pub fn revalidate(self, admission: DeadlineTimingAdmission) -> Result<(), DeadlineTimingError> {
        let Some(total_overhead_us) = self
            .programming_overhead_us
            .checked_add(self.interrupt_overhead_us)
            .and_then(|value| value.checked_add(self.wake_latency_us))
        else {
            return Err(DeadlineTimingError::InvalidProfile);
        };
        if !self.is_valid()
            || self.provider_id != admission.provider_id
            || self.generation != admission.generation
            || self.minimum_period_us != admission.minimum_period_us
            || self.resolution_us != admission.resolution_us
            || total_overhead_us != admission.total_overhead_us
        {
            Err(DeadlineTimingError::ProviderChanged)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod deadline_timing_tests {
    use super::*;

    const PROFILE: DeadlineTimingProfile = DeadlineTimingProfile {
        provider_id: 7,
        generation: 3,
        minimum_period_us: 4,
        resolution_us: 2,
        programming_overhead_us: 1,
        interrupt_overhead_us: 2,
        wake_latency_us: 3,
    };

    #[test]
    fn exact_and_early_resolution_are_distinct_and_fail_closed() {
        let exact = PROFILE.admit(DeadlineTimingRequest::exact(10, 6)).unwrap();
        assert_eq!(exact.programmed_period_us, 10);
        assert_eq!(exact.total_overhead_us, 6);
        assert!(exact.require_exact_resolution);
        assert_eq!(
            PROFILE.admit(DeadlineTimingRequest::exact(9, 6)),
            Err(DeadlineTimingError::ResolutionMismatch {
                requested_us: 9,
                resolution_us: 2,
            })
        );
        let early = PROFILE.admit(DeadlineTimingRequest::early(9, 6)).unwrap();
        assert_eq!(early.programmed_period_us, 8);
        assert!(!early.require_exact_resolution);
        assert_eq!(
            PROFILE.admit(DeadlineTimingRequest::exact(2, 1)),
            Err(DeadlineTimingError::PeriodTooShort {
                requested_us: 2,
                minimum_us: 4,
            })
        );
        assert_eq!(
            PROFILE.admit(DeadlineTimingRequest::exact(10, 5)),
            Err(DeadlineTimingError::OverheadExceedsSlack {
                overhead_us: 6,
                slack_us: 5,
            })
        );
    }

    #[test]
    fn provider_reconfiguration_invalidates_the_old_admission() {
        let admission = PROFILE.admit(DeadlineTimingRequest::exact(10, 6)).unwrap();
        assert_eq!(PROFILE.revalidate(admission), Ok(()));
        assert_eq!(
            DeadlineTimingProfile {
                generation: PROFILE.generation + 1,
                ..PROFILE
            }
            .revalidate(admission),
            Err(DeadlineTimingError::ProviderChanged)
        );
        assert_eq!(
            DeadlineTimingProfile {
                wake_latency_us: PROFILE.wake_latency_us + 1,
                ..PROFILE
            }
            .revalidate(admission),
            Err(DeadlineTimingError::ProviderChanged)
        );
    }
}

/// Fallible board power operations owned by the authoritative executor.
pub trait PowerPlatform {
    fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError>;
    /// Arm a compare together with the admitted task-slot bits that its ISR
    /// must publish. Existing platforms retain wake-only behavior by default.
    fn program_deadline_release(
        &mut self,
        deadline_us: Option<u64>,
        _ready_mask: u32,
    ) -> Result<(), PowerHookError> {
        self.program_wake(deadline_us)
    }
    /// Atomically drain task bits published by the platform's compare ISR.
    /// Providers without an ISR handoff keep the default and the executor
    /// releases tasks from its ordered queue after wake.
    fn take_deadline_releases(&mut self, _now_us: u64) -> u32 {
        0
    }
    /// Largest compare-deadline-to-executor-entry delay observed by this
    /// provider. Qualification feeds a conservative bound back into admission;
    /// zero means no provider measurement is available.
    fn observed_wake_latency_us(&self) -> u32 {
        0
    }
    /// Return a qualified deadline profile only after this exact clock/compare/
    /// ISR/wake route has a conservative overhead bound. `None` keeps legacy
    /// wake behavior but cannot satisfy enforced high-resolution admission.
    fn deadline_timing_profile(&self) -> Option<DeadlineTimingProfile> {
        None
    }
    /// Exact sleep/wake/retention capability used only by the admitted-sleep
    /// path. `None` permits compatibility execution but never a sleep claim.
    fn sleep_profile(&self, _mode: PowerMode) -> Option<SleepProfile> {
        None
    }
    /// Constrain a policy choice to modes this backend actually implements.
    fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
        requested
    }
    fn vetoes(&self, _requested: PowerMode) -> PowerVetoMask {
        PowerVetoMask::default()
    }
    /// Prepare clocks and peripheral participants. No public/live state may be
    /// changed until every participant has prepared successfully.
    fn prepare_power(
        &mut self,
        _mode: PowerMode,
        _system_off_wake: Option<SystemOffWake>,
    ) -> Result<(), PowerHookError> {
        Ok(())
    }
    /// Undo a successful prepare after a later prepare/wake-arm step failed.
    fn rollback_power(&mut self, _mode: PowerMode) -> Result<(), PowerHookError> {
        Ok(())
    }
    /// Prove the armed wake route before entry. SYSTEMOFF implementations must
    /// reject wake contracts they cannot retain across reset-style entry.
    fn verify_wake(
        &mut self,
        mode: PowerMode,
        _deadline_us: Option<u64>,
        system_off_wake: Option<SystemOffWake>,
    ) -> Result<(), PowerHookError> {
        if mode == PowerMode::Off && system_off_wake.is_none() {
            return Err(PowerHookError {
                source: u16::MAX,
                code: 1,
            });
        }
        Ok(())
    }
    /// Enter the selected mode and return the mode hardware actually entered.
    fn enter(&mut self, mode: PowerMode) -> Result<PowerMode, PowerHookError>;
    /// Restore clocks/controllers before the executor publishes work as live.
    fn restore_power(&mut self, _effective: PowerMode) -> Result<(), PowerHookError> {
        Ok(())
    }
    fn suspend(&mut self, task_id: u16) -> Result<(), PowerHookError>;
    fn resume(&mut self, task_id: u16) -> Result<(), PowerHookError>;
}

/// A composable peripheral participant layered around the board's wake/entry
/// provider. Participants prepare in attachment order and roll back in reverse
/// order when [`PowerPlatformChain`] values are nested.
pub trait PowerParticipant {
    fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
        requested
    }

    fn vetoes(&self, _requested: PowerMode) -> PowerVetoMask {
        PowerVetoMask::default()
    }

    fn prepare_power(
        &mut self,
        _mode: PowerMode,
        _system_off_wake: Option<SystemOffWake>,
    ) -> Result<(), PowerHookError> {
        Ok(())
    }

    fn rollback_power(&mut self, _mode: PowerMode) -> Result<(), PowerHookError> {
        Ok(())
    }

    fn restore_power(&mut self, _effective: PowerMode) -> Result<(), PowerHookError> {
        Ok(())
    }

    fn suspend(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }

    fn resume(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }
}

/// Borrowed adapter that composes one peripheral participant with a platform.
/// Nest it to add more providers without allocation or a global registry.
pub struct PowerPlatformChain<'a, P, R> {
    platform: &'a mut P,
    participant: &'a mut R,
}

pub fn attach_participant<'a, P: PowerPlatform, R: PowerParticipant>(
    platform: &'a mut P,
    participant: &'a mut R,
) -> PowerPlatformChain<'a, P, R> {
    PowerPlatformChain {
        platform,
        participant,
    }
}

impl<P: PowerPlatform, R: PowerParticipant> PowerPlatform for PowerPlatformChain<'_, P, R> {
    fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
        self.platform.program_wake(deadline_us)
    }

    fn program_deadline_release(
        &mut self,
        deadline_us: Option<u64>,
        ready_mask: u32,
    ) -> Result<(), PowerHookError> {
        self.platform
            .program_deadline_release(deadline_us, ready_mask)
    }

    fn take_deadline_releases(&mut self, now_us: u64) -> u32 {
        self.platform.take_deadline_releases(now_us)
    }

    fn observed_wake_latency_us(&self) -> u32 {
        self.platform.observed_wake_latency_us()
    }

    fn deadline_timing_profile(&self) -> Option<DeadlineTimingProfile> {
        self.platform.deadline_timing_profile()
    }

    fn sleep_profile(&self, mode: PowerMode) -> Option<SleepProfile> {
        self.platform.sleep_profile(mode)
    }

    fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
        self.platform
            .constrain_mode(requested)
            .shallower(self.participant.constrain_mode(requested))
    }

    fn vetoes(&self, requested: PowerMode) -> PowerVetoMask {
        self.platform
            .vetoes(requested)
            .union(self.participant.vetoes(requested))
    }

    fn prepare_power(
        &mut self,
        mode: PowerMode,
        system_off_wake: Option<SystemOffWake>,
    ) -> Result<(), PowerHookError> {
        self.platform.prepare_power(mode, system_off_wake)?;
        self.participant.prepare_power(mode, system_off_wake)
    }

    fn rollback_power(&mut self, mode: PowerMode) -> Result<(), PowerHookError> {
        let participant = self.participant.rollback_power(mode);
        let platform = self.platform.rollback_power(mode);
        participant.and(platform)
    }

    fn verify_wake(
        &mut self,
        mode: PowerMode,
        deadline_us: Option<u64>,
        system_off_wake: Option<SystemOffWake>,
    ) -> Result<(), PowerHookError> {
        self.platform
            .verify_wake(mode, deadline_us, system_off_wake)
    }

    fn enter(&mut self, mode: PowerMode) -> Result<PowerMode, PowerHookError> {
        self.platform.enter(mode)
    }

    fn restore_power(&mut self, effective: PowerMode) -> Result<(), PowerHookError> {
        self.platform.restore_power(effective)?;
        self.participant.restore_power(effective)
    }

    fn suspend(&mut self, task_id: u16) -> Result<(), PowerHookError> {
        self.platform.suspend(task_id)?;
        if let Err(error) = self.participant.suspend(task_id) {
            let _ = self.participant.resume(task_id);
            let _ = self.platform.resume(task_id);
            return Err(error);
        }
        Ok(())
    }

    fn resume(&mut self, task_id: u16) -> Result<(), PowerHookError> {
        self.participant.resume(task_id)?;
        if let Err(error) = self.platform.resume(task_id) {
            let _ = self.participant.suspend(task_id);
            return Err(error);
        }
        Ok(())
    }
}

/// Chooses a power mode and enforces an active-time duty budget over a window.
pub struct PowerManager {
    active_us: u64,
    window_us: u64,
    budget_us: u64,
}

impl PowerManager {
    /// `budget_us` of active time allowed per `window_us`.
    pub const fn new(window_us: u64, budget_us: u64) -> Self {
        Self {
            active_us: 0,
            window_us,
            budget_us,
        }
    }

    /// Pick a mode: if work is pending choose Active; else sleep as deeply as the next
    /// deadline allows (short -> Idle, longer -> LowPower, none -> Off).
    pub fn select(&self, work_pending: bool, next_deadline_us: Option<u64>) -> PowerMode {
        if work_pending {
            return PowerMode::Active;
        }
        match next_deadline_us {
            None => PowerMode::Off,
            Some(d) if d < 2_000 => PowerMode::Idle,
            Some(_) => PowerMode::LowPower,
        }
    }

    /// Account active time; returns true if the duty budget for the window is exceeded
    /// (caller should back off / shed work).
    pub fn account_active(&mut self, dt_us: u64) -> bool {
        self.active_us = self.active_us.saturating_add(dt_us);
        self.active_us > self.budget_us
    }

    pub fn end_window(&mut self) {
        self.active_us = 0;
    }

    pub fn duty_milli(&self) -> u32 {
        self.active_us
            .saturating_mul(1000)
            .checked_div(self.window_us)
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_mode_by_activity_and_deadline() {
        let pm = PowerManager::new(1_000_000, 100_000);
        assert_eq!(pm.select(true, Some(500)), PowerMode::Active);
        assert_eq!(pm.select(false, Some(500)), PowerMode::Idle);
        assert_eq!(pm.select(false, Some(50_000)), PowerMode::LowPower);
        assert_eq!(pm.select(false, None), PowerMode::Off);
    }

    #[test]
    fn enforces_duty_budget() {
        let mut pm = PowerManager::new(1_000_000, 100_000); // 10% duty
        assert!(!pm.account_active(80_000));
        assert!(pm.account_active(30_000)); // 110k > 100k budget -> exceeded
        assert_eq!(pm.duty_milli(), 110); // 11.0%
    }
}

/// Per-task energy ledger: charge each task's active time at a measured power
/// draw (uW) and report energy in uJ. Fixed capacity, no heap.
pub struct EnergyLedger<const N: usize> {
    // Keep identifiers separate from 64-bit counters so every slot does not
    // retain alignment padding.
    energy_uj: [u64; N],
    task_ids: [u16; N],
    len: usize,
}

impl<const N: usize> Default for EnergyLedger<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> EnergyLedger<N> {
    pub const fn new() -> Self {
        Self {
            energy_uj: [0; N],
            task_ids: [0; N],
            len: 0,
        }
    }

    unsafe fn init_in_place(destination: *mut Self) {
        let energy_uj = core::ptr::addr_of_mut!((*destination).energy_uj).cast::<u64>();
        let task_ids = core::ptr::addr_of_mut!((*destination).task_ids).cast::<u16>();
        for index in 0..N {
            energy_uj.add(index).write(0);
            task_ids.add(index).write(0);
        }
        core::ptr::addr_of_mut!((*destination).len).write(0);
    }

    /// Charge `task` for `active_us` at `power_uw`. Returns false if the ledger is full.
    pub fn charge(&mut self, task: u16, active_us: u64, power_uw: u64) -> bool {
        if self.len > N {
            return false;
        }
        let energy_uj = active_us.saturating_mul(power_uw) / 1_000_000;
        for (task_id, recorded_uj) in self
            .task_ids
            .iter()
            .zip(self.energy_uj.iter_mut())
            .take(self.len)
        {
            if *task_id == task {
                *recorded_uj = recorded_uj.saturating_add(energy_uj);
                return true;
            }
        }
        if self.len >= N {
            return false;
        }
        let Some(task_id) = self.task_ids.get_mut(self.len) else {
            return false;
        };
        let Some(recorded_uj) = self.energy_uj.get_mut(self.len) else {
            return false;
        };
        *task_id = task;
        *recorded_uj = energy_uj;
        self.len += 1;
        true
    }

    pub fn energy_uj(&self, task: u16) -> Option<u64> {
        if self.len > N {
            return None;
        }
        self.task_ids
            .iter()
            .zip(self.energy_uj.iter())
            .take(self.len)
            .find(|(entry, _)| **entry == task)
            .map(|(_, energy_uj)| *energy_uj)
    }

    pub fn total_uj(&self) -> u64 {
        if self.len > N {
            return u64::MAX;
        }
        self.energy_uj.iter().take(self.len).sum()
    }

    /// The hungriest task (id, energy uJ).
    pub fn top(&self) -> Option<(u16, u64)> {
        if self.len > N {
            return None;
        }
        self.task_ids
            .iter()
            .copied()
            .zip(self.energy_uj.iter().copied())
            .take(self.len)
            .max_by_key(|entry| entry.1)
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<EnergyLedger<5>>() == 56);

/// Executor-owned power policy, task power profiles, and measured energy ledger.
pub struct ExecutorPower<const N: usize, const LEASES: usize = 8> {
    manager: PowerManager,
    ledger: EnergyLedger<N>,
    leases: PowerLeaseTable<LEASES>,
    profile_power_uw: [u64; N],
    profile_task_ids: [u16; N],
    profile_len: usize,
    default_power_uw: u64,
    system_off_wake: Option<SystemOffWake>,
}

#[derive(Clone, Copy)]
struct PowerSelection {
    requested: PowerMode,
    selected: PowerMode,
    vetoes: PowerVetoMask,
    system_off_wake: Option<SystemOffWake>,
}

impl<const N: usize, const LEASES: usize> ExecutorPower<N, LEASES> {
    pub const fn new(window_us: u64, budget_us: u64, default_power_uw: u64) -> Self {
        Self {
            manager: PowerManager::new(window_us, budget_us),
            ledger: EnergyLedger::new(),
            leases: PowerLeaseTable::new(),
            profile_power_uw: [0; N],
            profile_task_ids: [0; N],
            profile_len: 0,
            default_power_uw,
            system_off_wake: None,
        }
    }

    /// Initialize caller-owned static storage without a capacity-sized
    /// aggregate temporary.
    ///
    /// # Safety
    ///
    /// `destination` must be aligned, writable storage for one uninitialized
    /// `ExecutorPower<N>`.
    #[doc(hidden)]
    pub unsafe fn init_in_place(
        destination: *mut Self,
        window_us: u64,
        budget_us: u64,
        default_power_uw: u64,
    ) {
        core::ptr::addr_of_mut!((*destination).manager)
            .write(PowerManager::new(window_us, budget_us));
        EnergyLedger::init_in_place(core::ptr::addr_of_mut!((*destination).ledger));
        core::ptr::addr_of_mut!((*destination).leases).write(PowerLeaseTable::new());

        let profile_power_uw =
            core::ptr::addr_of_mut!((*destination).profile_power_uw).cast::<u64>();
        let profile_task_ids =
            core::ptr::addr_of_mut!((*destination).profile_task_ids).cast::<u16>();
        for index in 0..N {
            profile_power_uw.add(index).write(0);
            profile_task_ids.add(index).write(0);
        }
        core::ptr::addr_of_mut!((*destination).profile_len).write(0);
        core::ptr::addr_of_mut!((*destination).default_power_uw).write(default_power_uw);
        core::ptr::addr_of_mut!((*destination).system_off_wake).write(None);
    }

    pub fn set_task_power(&mut self, task_id: u16, power_uw: u64) -> bool {
        if self.profile_len > N {
            return false;
        }
        if let Some(index) = self
            .profile_task_ids
            .iter()
            .take(self.profile_len)
            .position(|profile| *profile == task_id)
        {
            let Some(profile_power_uw) = self.profile_power_uw.get_mut(index) else {
                return false;
            };
            *profile_power_uw = power_uw;
            return true;
        }
        if self.profile_len == N {
            return false;
        }
        let Some(profile_task_id) = self.profile_task_ids.get_mut(self.profile_len) else {
            return false;
        };
        let Some(profile_power_uw) = self.profile_power_uw.get_mut(self.profile_len) else {
            return false;
        };
        *profile_task_id = task_id;
        *profile_power_uw = power_uw;
        self.profile_len += 1;
        true
    }

    /// Return the power used for future accounting of `task_id`.
    ///
    /// This intentionally returns the default when no task-specific profile
    /// exists.  A multicore scheduler can therefore preserve attribution when
    /// ownership moves without exposing or copying the historical ledger.
    pub fn task_power_uw(&self, task_id: u16) -> u64 {
        self.profile_task_ids
            .iter()
            .zip(self.profile_power_uw.iter())
            .take(self.profile_len)
            .find(|(profile, _)| **profile == task_id)
            .map(|(_, power_uw)| *power_uw)
            .unwrap_or(self.default_power_uw)
    }

    pub fn account_task(&mut self, task_id: u16, active_us: u64) -> bool {
        if self.profile_len > N {
            return false;
        }
        let power_uw = self.task_power_uw(task_id);
        let _ = self.manager.account_active(active_us);
        self.ledger.charge(task_id, active_us, power_uw)
    }

    pub fn acquire_lease(
        &mut self,
        owner: u16,
        kind: PowerLeaseKind,
    ) -> Result<PowerLease, PowerLeaseError> {
        self.leases.acquire(owner, kind)
    }

    pub fn release_lease(&mut self, lease: PowerLease) -> Result<(), PowerLeaseError> {
        self.leases.release(lease)
    }

    /// Explicitly opt in to SYSTEMOFF using a retained, board-qualified wake
    /// route. Clearing this contract makes an `Off` request select `Idle`.
    pub fn set_system_off_wake(&mut self, wake: Option<SystemOffWake>) {
        self.system_off_wake = wake;
    }

    pub fn apply_idle(
        &self,
        now_us: u64,
        work_pending: bool,
        deadline_us: Option<u64>,
        platform: &mut impl PowerPlatform,
    ) -> Result<PowerTransition, PowerHookError> {
        self.apply_idle_release(now_us, work_pending, deadline_us, 0, platform)
    }

    pub fn apply_idle_admitted(
        &self,
        now_us: u64,
        work_pending: bool,
        deadline_us: Option<u64>,
        requirements: SleepRequirements,
        platform: &mut impl PowerPlatform,
    ) -> Result<PowerTransition, PowerApplyError> {
        self.apply_idle_release_admitted(
            now_us,
            work_pending,
            deadline_us,
            0,
            requirements,
            platform,
        )
    }

    pub fn apply_idle_release(
        &self,
        now_us: u64,
        work_pending: bool,
        deadline_us: Option<u64>,
        ready_mask: u32,
        platform: &mut impl PowerPlatform,
    ) -> Result<PowerTransition, PowerHookError> {
        let selection = self.select_idle(now_us, work_pending, deadline_us, platform);
        self.apply_selected(selection, deadline_us, ready_mask, None, platform)
    }

    /// Evidence-bearing sleep entry. Qualification happens before any prepare,
    /// wake-arm, or entry hook; a rejected admission cannot touch hardware.
    pub fn apply_idle_release_admitted(
        &self,
        now_us: u64,
        work_pending: bool,
        deadline_us: Option<u64>,
        ready_mask: u32,
        requirements: SleepRequirements,
        platform: &mut impl PowerPlatform,
    ) -> Result<PowerTransition, PowerApplyError> {
        let selection = self.select_idle(now_us, work_pending, deadline_us, platform);
        let admission =
            if selection.selected == PowerMode::Active {
                None
            } else {
                let profile = platform.sleep_profile(selection.selected).ok_or(
                    PowerApplyError::Admission(SleepAdmissionError::InvalidProfile),
                )?;
                Some(
                    profile
                        .admit(
                            selection.selected,
                            requirements,
                            deadline_us.map(|deadline| deadline.saturating_sub(now_us)),
                        )
                        .map_err(PowerApplyError::Admission)?,
                )
            };
        self.apply_selected(selection, deadline_us, ready_mask, admission, platform)
            .map_err(PowerApplyError::Hook)
    }

    fn select_idle(
        &self,
        now_us: u64,
        work_pending: bool,
        deadline_us: Option<u64>,
        platform: &impl PowerPlatform,
    ) -> PowerSelection {
        let relative = deadline_us.map(|deadline| deadline.saturating_sub(now_us));
        let requested = self.manager.select(work_pending, relative);
        let (mut selected, mut vetoes) = self.leases.admit(requested);
        let mut system_off_wake = None;
        if selected == PowerMode::Off {
            if let Some(wake) = self.system_off_wake {
                system_off_wake = Some(wake);
            } else {
                selected = PowerMode::Idle;
                vetoes.insert(PowerVetoReason::SystemOffNotOptedIn);
                vetoes.insert(PowerVetoReason::WakeUnavailable);
            }
        }
        vetoes = vetoes.union(platform.vetoes(selected));
        let platform_selected = platform.constrain_mode(selected).shallower(selected);
        if platform_selected != selected {
            selected = platform_selected;
            vetoes.insert(PowerVetoReason::PlatformLimited);
            system_off_wake = None;
        }
        PowerSelection {
            requested,
            selected,
            vetoes,
            system_off_wake,
        }
    }

    fn apply_selected(
        &self,
        selection: PowerSelection,
        deadline_us: Option<u64>,
        ready_mask: u32,
        sleep_admission: Option<SleepAdmission>,
        platform: &mut impl PowerPlatform,
    ) -> Result<PowerTransition, PowerHookError> {
        let PowerSelection {
            requested,
            selected,
            mut vetoes,
            system_off_wake,
        } = selection;
        if selected == PowerMode::Active {
            return Ok(PowerTransition {
                requested,
                selected,
                effective: PowerMode::Active,
                vetoes,
                system_off_wake,
                sleep_admission: None,
            });
        }

        if let Err(error) = platform.prepare_power(selected, system_off_wake) {
            let _ = platform.rollback_power(selected);
            return Err(error);
        }
        if let Err(error) = platform.program_deadline_release(deadline_us, ready_mask) {
            let _ = platform.rollback_power(selected);
            return Err(error);
        }
        if let Err(error) = platform.verify_wake(selected, deadline_us, system_off_wake) {
            let _ = platform.rollback_power(selected);
            return Err(error);
        }
        let effective = match platform.enter(selected) {
            Ok(effective) => effective.shallower(selected),
            Err(error) => {
                let _ = platform.rollback_power(selected);
                return Err(error);
            }
        };
        if effective != selected {
            vetoes.insert(PowerVetoReason::PlatformLimited);
        }
        if let Err(error) = platform.restore_power(effective) {
            let _ = platform.rollback_power(selected);
            return Err(error);
        }
        Ok(PowerTransition {
            requested,
            selected,
            effective,
            vetoes,
            system_off_wake,
            sleep_admission,
        })
    }

    pub const fn ledger(&self) -> &EnergyLedger<N> {
        &self.ledger
    }

    pub const fn manager(&self) -> &PowerManager {
        &self.manager
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<ExecutorPower<5>>() <= 224);

#[cfg(test)]
mod energy_tests {
    use super::*;

    #[derive(Default)]
    struct Hooks {
        wake: Option<u64>,
        mode: Option<PowerMode>,
        suspended: Option<u16>,
    }

    impl PowerPlatform for Hooks {
        fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
            self.wake = deadline_us;
            Ok(())
        }
        fn enter(&mut self, mode: PowerMode) -> Result<PowerMode, PowerHookError> {
            self.mode = Some(mode);
            Ok(mode)
        }
        fn sleep_profile(&self, _mode: PowerMode) -> Option<SleepProfile> {
            Some(SleepProfile {
                provider_id: 4,
                generation: 2,
                deepest_mode: PowerMode::LowPower,
                wake_latency_us: 250,
                wake_sources: 0b11,
                retained_state: 0b01,
                retained_clock_domains: 0b10,
                retained_peripheral_domains: 0b100,
            })
        }
        fn suspend(&mut self, task_id: u16) -> Result<(), PowerHookError> {
            self.suspended = Some(task_id);
            Ok(())
        }
        fn resume(&mut self, task_id: u16) -> Result<(), PowerHookError> {
            (self.suspended == Some(task_id))
                .then(|| self.suspended = None)
                .ok_or(PowerHookError { source: 1, code: 2 })
        }
    }

    #[test]
    fn ledger_charges_and_ranks_tasks() {
        let mut led = EnergyLedger::<4>::new();
        // sensor task: 200 ms at 5 mW -> 1000 uJ; radio: 50 ms at 40 mW -> 2000 uJ
        assert!(led.charge(1, 200_000, 5_000));
        assert!(led.charge(2, 50_000, 40_000));
        assert!(led.charge(1, 200_000, 5_000)); // accumulates
        assert_eq!(led.energy_uj(1), Some(2_000));
        assert_eq!(led.energy_uj(2), Some(2_000));
        assert_eq!(led.total_uj(), 4_000);
        assert_eq!(led.top(), Some((2, 2_000)));
        assert!(led.charge(2, 1_000_000, 40_000)); // radio burns 40 mJ more
        assert_eq!(led.top(), Some((2, 42_000)));
    }

    #[test]
    fn corrupted_length_invariants_fail_closed_without_indexing_panics() {
        let mut ledger = EnergyLedger::<2>::new();
        ledger.len = 3;
        assert!(!ledger.charge(7, 1, 1));
        assert_eq!(ledger.energy_uj(7), None);
        assert_eq!(ledger.total_uj(), u64::MAX);
        assert_eq!(ledger.top(), None);

        let mut power = ExecutorPower::<2>::new(1_000_000, 100_000, 1_000);
        power.profile_len = 3;
        assert!(!power.set_task_power(7, 5_000));
        assert!(!power.account_task(7, 1));
    }

    #[test]
    fn executor_power_accounts_and_programs_wake_before_sleep() {
        let mut power = ExecutorPower::<2>::new(1_000_000, 100_000, 1_000);
        assert!(power.set_task_power(7, 5_000));
        assert!(power.set_task_power(7, 10_000));
        assert!(power.set_task_power(8, 2_000));
        assert!(!power.set_task_power(9, 1_000));
        assert!(power.account_task(7, 200_000));
        assert_eq!(power.ledger().energy_uj(7), Some(2_000));

        let mut hooks = Hooks::default();
        assert_eq!(
            power.apply_idle(10_000, false, Some(20_000), &mut hooks),
            Ok(PowerTransition {
                requested: PowerMode::LowPower,
                selected: PowerMode::LowPower,
                effective: PowerMode::LowPower,
                vetoes: PowerVetoMask::default(),
                system_off_wake: None,
                sleep_admission: None,
            })
        );
        assert_eq!(hooks.wake, Some(20_000));
        assert_eq!(hooks.mode, Some(PowerMode::LowPower));
    }

    #[test]
    fn admitted_sleep_proves_latency_wake_retention_and_domains_before_entry() {
        let power = ExecutorPower::<1>::new(1_000_000, 100_000, 1_000);
        let requirements = SleepRequirements {
            max_wake_latency_us: 500,
            required_wake_sources: 0b01,
            required_retained_state: 0b01,
            required_clock_domains: 0b10,
            required_peripheral_domains: 0b100,
        };
        let mut hooks = Hooks::default();
        let report = power
            .apply_idle_admitted(1_000, false, Some(11_000), requirements, &mut hooks)
            .unwrap();
        assert_eq!(report.effective, PowerMode::LowPower);
        assert_eq!(
            report.sleep_admission,
            Some(SleepAdmission {
                provider_id: 4,
                generation: 2,
                mode: PowerMode::LowPower,
                wake_latency_us: 250,
                wake_sources: 0b11,
                retained_state: 0b01,
                retained_clock_domains: 0b10,
                retained_peripheral_domains: 0b100,
            })
        );

        hooks.wake = None;
        hooks.mode = None;
        let too_tight = SleepRequirements {
            max_wake_latency_us: 200,
            ..requirements
        };
        assert_eq!(
            power.apply_idle_admitted(1_000, false, Some(11_000), too_tight, &mut hooks),
            Err(PowerApplyError::Admission(
                SleepAdmissionError::WakeLatencyExceeded {
                    latency_us: 250,
                    limit_us: 200,
                }
            ))
        );
        assert_eq!(hooks.wake, None);
        assert_eq!(hooks.mode, None);
    }

    #[test]
    fn in_place_executor_power_matches_value_constructor() {
        let mut storage = core::mem::MaybeUninit::<ExecutorPower<2>>::uninit();
        unsafe {
            ExecutorPower::init_in_place(storage.as_mut_ptr(), 1_000_000, 100_000, 2_000);
        }
        let mut in_place = unsafe { storage.assume_init() };
        let mut by_value = ExecutorPower::<2>::new(1_000_000, 100_000, 2_000);

        for power in [&mut in_place, &mut by_value] {
            assert!(power.set_task_power(7, 10_000));
            assert!(power.account_task(7, 200_000));
            assert!(power.account_task(8, 1_000));
            assert_eq!(power.ledger().energy_uj(7), Some(2_000));
            assert_eq!(power.ledger().energy_uj(8), Some(2));
            assert_eq!(power.ledger().total_uj(), 2_002);
            assert_eq!(power.manager().duty_milli(), 201);
        }
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    struct TransactionHooks {
        log: [u8; 160],
        len: usize,
        fail_at: u8,
        limit: PowerMode,
        effective: PowerMode,
    }

    impl TransactionHooks {
        const fn new(limit: PowerMode, effective: PowerMode) -> Self {
            Self {
                log: [0; 160],
                len: 0,
                fail_at: 0,
                limit,
                effective,
            }
        }

        fn push(&mut self, step: u8) -> Result<(), PowerHookError> {
            self.log[self.len] = step;
            self.len += 1;
            if self.fail_at == step {
                Err(PowerHookError {
                    source: 91,
                    code: u16::from(step),
                })
            } else {
                Ok(())
            }
        }
    }

    impl PowerPlatform for TransactionHooks {
        fn program_wake(&mut self, _deadline_us: Option<u64>) -> Result<(), PowerHookError> {
            self.push(2)
        }

        fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
            requested.shallower(self.limit)
        }

        fn prepare_power(
            &mut self,
            _mode: PowerMode,
            _wake: Option<SystemOffWake>,
        ) -> Result<(), PowerHookError> {
            self.push(1)
        }

        fn rollback_power(&mut self, _mode: PowerMode) -> Result<(), PowerHookError> {
            self.push(9)
        }

        fn verify_wake(
            &mut self,
            _mode: PowerMode,
            _deadline_us: Option<u64>,
            _wake: Option<SystemOffWake>,
        ) -> Result<(), PowerHookError> {
            self.push(3)
        }

        fn enter(&mut self, _mode: PowerMode) -> Result<PowerMode, PowerHookError> {
            self.push(4)?;
            Ok(self.effective)
        }

        fn restore_power(&mut self, _effective: PowerMode) -> Result<(), PowerHookError> {
            self.push(5)
        }

        fn suspend(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
            Ok(())
        }

        fn resume(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
            Ok(())
        }
    }

    struct Participant {
        prepared: u8,
        rolled_back: u8,
        restored: u8,
        fail_prepare: bool,
    }

    impl PowerParticipant for Participant {
        fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
            requested.shallower(PowerMode::Idle)
        }

        fn vetoes(&self, requested: PowerMode) -> PowerVetoMask {
            if requested.depth() > PowerMode::Idle.depth() {
                PowerVetoMask::from_reason(PowerVetoReason::RestorationUnproven)
            } else {
                PowerVetoMask::default()
            }
        }

        fn prepare_power(
            &mut self,
            _mode: PowerMode,
            _wake: Option<SystemOffWake>,
        ) -> Result<(), PowerHookError> {
            self.prepared += 1;
            if self.fail_prepare {
                Err(PowerHookError {
                    source: 91,
                    code: 20,
                })
            } else {
                Ok(())
            }
        }

        fn rollback_power(&mut self, _mode: PowerMode) -> Result<(), PowerHookError> {
            self.rolled_back += 1;
            Ok(())
        }

        fn restore_power(&mut self, _effective: PowerMode) -> Result<(), PowerHookError> {
            self.restored += 1;
            Ok(())
        }
    }

    #[test]
    fn leases_compose_to_the_shallowest_mode_and_stale_handles_fail_closed() {
        let mut power = ExecutorPower::<3>::new(1_000_000, 100_000, 1_000);
        let usb = power.acquire_lease(7, PowerLeaseKind::UsbActive).unwrap();
        let debug = power
            .acquire_lease(8, PowerLeaseKind::DebugSession)
            .unwrap();
        let mut hooks = TransactionHooks::new(PowerMode::Off, PowerMode::Active);
        let report = power
            .apply_idle(0, false, Some(50_000), &mut hooks)
            .unwrap();
        assert_eq!(report.requested, PowerMode::LowPower);
        assert_eq!(report.selected, PowerMode::Active);
        assert_eq!(report.effective, PowerMode::Active);
        assert!(report.vetoes.contains(PowerVetoReason::UsbActive));
        assert!(report.vetoes.contains(PowerVetoReason::DebugSession));
        assert_eq!(hooks.len, 0);

        power.release_lease(debug).unwrap();
        assert_eq!(power.release_lease(debug), Err(PowerLeaseError::Stale));
        power.release_lease(usb).unwrap();
    }

    #[test]
    fn exhausted_slot_generation_advances_epoch_without_reviving_stale_lease() {
        let mut leases = PowerLeaseTable::<1>::new();
        leases.slots[0].generation = u32::MAX - 1;
        let last = leases
            .acquire(7, PowerLeaseKind::UsbActive)
            .expect("last unique generation");
        assert_eq!(leases.release(last), Ok(()));
        let next = leases
            .acquire(7, PowerLeaseKind::UsbActive)
            .expect("fresh epoch");
        assert_ne!(last, next);
        assert_eq!(leases.release(last), Err(PowerLeaseError::Stale));
        assert_eq!(leases.release(next), Ok(()));

        leases.epoch = u32::MAX;
        leases.slots[0].generation = u32::MAX;
        assert_eq!(
            leases.acquire(7, PowerLeaseKind::UsbActive),
            Err(PowerLeaseError::Full)
        );
    }

    #[test]
    fn veto_composition_property_grid_never_selects_below_any_active_limit() {
        let kinds = [
            PowerLeaseKind::UsbActive,
            PowerLeaseKind::RadioActive,
            PowerLeaseKind::DmaActive,
            PowerLeaseKind::StorageTransaction,
            PowerLeaseKind::DebugSession,
            PowerLeaseKind::RecoverySession,
            PowerLeaseKind::RestorationUnproven,
        ];
        let modes = [
            PowerMode::Active,
            PowerMode::Idle,
            PowerMode::LowPower,
            PowerMode::Off,
        ];
        for first in kinds {
            for second in kinds {
                let mut leases = PowerLeaseTable::<2>::new();
                leases.acquire(1, first).unwrap();
                leases.acquire(2, second).unwrap();
                for requested in modes {
                    let (selected, vetoes) = leases.admit(requested);
                    let expected = requested.shallower(first.limit()).shallower(second.limit());
                    assert_eq!(selected, expected);
                    assert_eq!(
                        vetoes.contains(first.reason()),
                        requested.depth() > first.limit().depth()
                    );
                    assert_eq!(
                        vetoes.contains(second.reason()),
                        requested.depth() > second.limit().depth()
                    );
                }
            }
        }
    }

    #[test]
    fn participant_chain_composes_veto_prepare_restore_and_reverse_rollback() {
        let power = ExecutorPower::<1>::new(1_000_000, 100_000, 1_000);
        let mut hooks = TransactionHooks::new(PowerMode::Off, PowerMode::Idle);
        let mut participant = Participant {
            prepared: 0,
            rolled_back: 0,
            restored: 0,
            fail_prepare: false,
        };
        let mut chain = attach_participant(&mut hooks, &mut participant);
        let report = power
            .apply_idle(0, false, Some(50_000), &mut chain)
            .unwrap();
        assert_eq!(report.selected, PowerMode::Idle);
        assert!(report.vetoes.contains(PowerVetoReason::RestorationUnproven));
        assert_eq!(participant.prepared, 1);
        assert_eq!(participant.restored, 1);
        assert_eq!(participant.rolled_back, 0);

        participant.fail_prepare = true;
        let mut chain = attach_participant(&mut hooks, &mut participant);
        assert!(power
            .apply_idle(0, false, Some(50_000), &mut chain)
            .is_err());
        assert_eq!(participant.rolled_back, 1);
        assert_eq!(hooks.log[hooks.len - 1], 9);
    }

    #[test]
    fn system_off_requires_explicit_retained_wake_and_records_reset_semantics() {
        assert_eq!(
            SystemOffWake::new(0, WakeStyle::Reset, true, false),
            Err(SystemOffWakeError::InvalidSource)
        );
        let mut power = ExecutorPower::<1>::new(1_000_000, 100_000, 1_000);
        let mut hooks = TransactionHooks::new(PowerMode::Off, PowerMode::Off);
        let rejected = power.apply_idle(0, false, None, &mut hooks).unwrap();
        assert_eq!(rejected.requested, PowerMode::Off);
        assert_eq!(rejected.selected, PowerMode::Idle);
        assert_eq!(rejected.effective, PowerMode::Idle);
        assert!(rejected
            .vetoes
            .contains(PowerVetoReason::SystemOffNotOptedIn));
        assert!(rejected.vetoes.contains(PowerVetoReason::WakeUnavailable));

        let wake = SystemOffWake::new(4, WakeStyle::Reset, true, false).unwrap();
        power.set_system_off_wake(Some(wake));
        let admitted = power.apply_idle(0, false, None, &mut hooks).unwrap();
        assert_eq!(admitted.requested, PowerMode::Off);
        assert_eq!(admitted.selected, PowerMode::Off);
        assert_eq!(admitted.effective, PowerMode::Off);
        assert_eq!(admitted.system_off_wake, Some(wake));
    }

    #[test]
    fn selected_and_effective_modes_are_distinct_and_never_overclaimed() {
        let power = ExecutorPower::<1>::new(1_000_000, 100_000, 1_000);
        let mut hooks = TransactionHooks::new(PowerMode::Off, PowerMode::Idle);
        let report = power
            .apply_idle(0, false, Some(50_000), &mut hooks)
            .unwrap();
        assert_eq!(report.requested, PowerMode::LowPower);
        assert_eq!(report.selected, PowerMode::LowPower);
        assert_eq!(report.effective, PowerMode::Idle);
        assert!(report.vetoes.contains(PowerVetoReason::PlatformLimited));
        assert_eq!(&hooks.log[..hooks.len], &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn wake_arm_failure_rolls_back_before_entry_or_publication() {
        let power = ExecutorPower::<1>::new(1_000_000, 100_000, 1_000);
        for fail_at in [1, 2, 3, 4, 5] {
            let mut hooks = TransactionHooks::new(PowerMode::Off, PowerMode::LowPower);
            hooks.fail_at = fail_at;
            assert_eq!(
                power.apply_idle(0, false, Some(50_000), &mut hooks),
                Err(PowerHookError {
                    source: 91,
                    code: u16::from(fail_at),
                })
            );
            assert_eq!(hooks.log[hooks.len - 1], 9);
            assert!(!hooks.log[..hooks.len].contains(&5) || fail_at == 5);
        }
    }

    #[test]
    fn repeated_suspend_wake_restore_cycles_preserve_order() {
        let power = ExecutorPower::<1>::new(1_000_000, 100_000, 1_000);
        let mut hooks = TransactionHooks::new(PowerMode::Off, PowerMode::LowPower);
        for cycle in 0..32 {
            let report = power
                .apply_idle(
                    cycle * 100_000,
                    false,
                    Some(cycle * 100_000 + 50_000),
                    &mut hooks,
                )
                .unwrap();
            assert_eq!(report.effective, PowerMode::LowPower);
        }
        assert_eq!(hooks.len, 160);
        for steps in hooks.log.chunks_exact(5) {
            assert_eq!(steps, &[1, 2, 3, 4, 5]);
        }
    }
}

/// Adaptive sampling by battery level: map a state-of-charge (percent) to a
/// decimation factor for the sensor pipeline - full rate when charged, progressively
/// heavier downsampling as the battery drains, minimum duty when critical.
///
/// Pairs with `nobro-sensor`'s `Decimator`: `Decimator::new(sampling_divisor(soc))`.
pub fn sampling_divisor(soc_percent: u8) -> u16 {
    match soc_percent {
        60..=u8::MAX => 1, // full rate
        30..=59 => 2,      // half rate
        15..=29 => 4,      // quarter rate
        5..=14 => 8,       // eighth rate
        _ => 16,           // critical: minimum duty
    }
}

#[cfg(test)]
mod adaptive_sampling_tests {
    use super::*;

    #[test]
    fn divisor_scales_with_soc_and_is_monotonic() {
        assert_eq!(sampling_divisor(100), 1);
        assert_eq!(sampling_divisor(60), 1);
        assert_eq!(sampling_divisor(45), 2);
        assert_eq!(sampling_divisor(20), 4);
        assert_eq!(sampling_divisor(10), 8);
        assert_eq!(sampling_divisor(2), 16);
        // monotonic: lower charge never samples faster
        let mut last = 0u16;
        for soc in (0..=100u8).rev() {
            let d = sampling_divisor(soc);
            assert!(d >= last, "soc {soc}: divisor {d} < {last}");
            last = d;
        }
    }
}

/// Energy-harvest-aware scheduling: decide the work budget for the next window
/// from harvested income vs. battery reserve. Energy-neutral operation: spend at most
/// (harvest income + an affordable battery draw that keeps SoC above the reserve floor).
pub fn harvest_work_budget_uj(
    harvest_uw: u32,
    window_ms: u32,
    soc_percent: u8,
    reserve_floor_percent: u8,
    battery_capacity_uj: u64,
) -> u64 {
    let income_uj = u64::from(harvest_uw) * u64::from(window_ms) / 1000;
    if soc_percent <= reserve_floor_percent {
        // At/below the reserve: strictly energy-neutral (spend only what is harvested).
        return income_uj;
    }
    // Above the reserve: may additionally draw down toward the floor, rate-limited to
    // 1% of capacity per window so a burst cannot crater the battery.
    let above = u64::from(soc_percent - reserve_floor_percent);
    let draw_cap = battery_capacity_uj / 100;
    let affordable = (battery_capacity_uj * above / 100).min(draw_cap);
    income_uj + affordable
}

#[cfg(test)]
mod harvest_tests {
    use super::*;

    #[test]
    fn harvest_budget_is_neutral_at_floor_and_generous_above() {
        let cap = 10_000_000u64; // 10 J battery
        assert_eq!(harvest_work_budget_uj(5_000, 1_000, 20, 20, cap), 5_000);
        let b = harvest_work_budget_uj(5_000, 1_000, 80, 20, cap);
        assert_eq!(b, 5_000 + cap / 100);
        assert_eq!(harvest_work_budget_uj(0, 1_000, 15, 20, cap), 0);
    }
}

/// Duty-cycle scheduler: drive a periodic task toward a target active fraction.
/// Each tick reports whether to run (active) or sleep, keeping the long-run active time
/// within the target duty using a leaky accumulator - robust to jittery tick spacing.
#[derive(Clone, Copy, Debug)]
pub struct DutyScheduler {
    target_milli: u32, // target duty in 1/1000
    credit: i64,       // accumulated "owed" active micros (signed)
    window_us: u64,
}

impl DutyScheduler {
    /// `target_duty_milli` in [0,1000]; `window_us` is the averaging horizon.
    pub const fn new(target_duty_milli: u32, window_us: u64) -> Self {
        Self {
            target_milli: target_duty_milli,
            credit: 0,
            window_us,
        }
    }

    /// Advance by `dt_us`; returns true if the task should be ACTIVE this interval.
    /// Accrues target active-time as credit, spends it when active, and leaks toward 0
    /// over the window so transient bursts do not bias the long-run duty.
    pub fn tick(&mut self, dt_us: u64, was_active: bool) -> bool {
        // accrue the target share of this interval
        self.credit += (dt_us as i64) * (self.target_milli as i64) / 1000;
        if was_active {
            self.credit -= dt_us as i64;
        }
        // leak toward zero across the window
        if self.window_us > 0 {
            self.credit -= self.credit * (dt_us as i64) / (self.window_us as i64) / 4;
        }
        // run when we owe active time
        self.credit > 0
    }
}

#[cfg(test)]
mod duty_tests {
    use super::*;

    #[test]
    fn duty_scheduler_converges_to_target() {
        // target 25% duty, 1 s window, 10 ms ticks over 10 s
        let mut ds = DutyScheduler::new(250, 1_000_000);
        let mut active_ticks = 0u32;
        let mut was_active = false;
        let total = 1000;
        for _ in 0..total {
            was_active = ds.tick(10_000, was_active);
            if was_active {
                active_ticks += 1;
            }
        }
        let duty = active_ticks * 1000 / total; // in milli
        assert!((200..=300).contains(&duty), "duty {duty} not near 250");
    }

    #[test]
    fn duty_zero_and_full() {
        let mut off = DutyScheduler::new(0, 1_000_000);
        let mut a = false;
        for _ in 0..100 {
            a = off.tick(10_000, a);
            assert!(!a);
        }
        let mut on = DutyScheduler::new(1000, 1_000_000);
        let mut a2 = false;
        let mut hi = 0;
        for _ in 0..100 {
            a2 = on.tick(10_000, a2);
            if a2 {
                hi += 1;
            }
        }
        assert!(hi > 90, "full-duty scheduler mostly active: {hi}/100");
    }
}
