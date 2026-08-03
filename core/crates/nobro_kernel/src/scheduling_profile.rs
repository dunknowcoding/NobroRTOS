//! Portable scheduling-profile admission.
//!
//! A priority-aware cooperative executor is not architectural preemption, and
//! detecting a late poll is not the same as containing it.  This module keeps
//! those promises distinct and rejects a request that the exact target port
//! cannot enforce.  The types contain no global state and are retained only
//! when an application uses them, so nano/cooperative builds pay no implicit
//! runtime or storage cost.

/// Execution semantics selected for one admitted application graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulingProfile {
    /// Work runs until it returns or yields. Priorities may choose the next
    /// task, but they cannot interrupt the current task.
    Cooperative,
    /// Ready futures/tasks are dispatched in priority order at bounded poll or
    /// step boundaries. A non-yielding poll is still not preemptible.
    AsyncPriority,
    /// An admitted budget interrupt may force an architectural context switch
    /// and suspend the offending task.
    ForcedPreemption,
}

/// Action promised when a task exceeds its deadline or execution budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineAction {
    /// No deadline promise is made by this scheduling profile.
    None,
    /// A qualified timer/sentinel detects and reports the violation; the
    /// current task remains cooperative.
    Observe,
    /// The exact port forcibly suspends the task and has a bounded independent
    /// watchdog fallback if the architectural switch cannot complete.
    ForceSuspend,
}

/// One graph's requested scheduling contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulingRequest {
    pub profile: SchedulingProfile,
    pub deadline_action: DeadlineAction,
    pub priority_levels: u8,
    pub deadline_us: u32,
    pub max_non_preemptible_us: u32,
    pub max_containment_us: u32,
    pub uses_fpu: bool,
}

impl SchedulingRequest {
    pub const fn cooperative() -> Self {
        Self {
            profile: SchedulingProfile::Cooperative,
            deadline_action: DeadlineAction::None,
            priority_levels: 1,
            deadline_us: 0,
            max_non_preemptible_us: 0,
            max_containment_us: 0,
            uses_fpu: false,
        }
    }

    pub const fn profile(mut self, profile: SchedulingProfile) -> Self {
        self.profile = profile;
        self
    }

    pub const fn priorities(mut self, levels: u8) -> Self {
        self.priority_levels = levels;
        self
    }

    pub const fn observe_deadline(mut self, deadline_us: u32) -> Self {
        self.deadline_action = DeadlineAction::Observe;
        self.deadline_us = deadline_us;
        self.max_non_preemptible_us = 0;
        self.max_containment_us = 0;
        self
    }

    pub const fn force_suspend(
        mut self,
        deadline_us: u32,
        max_non_preemptible_us: u32,
        max_containment_us: u32,
    ) -> Self {
        self.deadline_action = DeadlineAction::ForceSuspend;
        self.deadline_us = deadline_us;
        self.max_non_preemptible_us = max_non_preemptible_us;
        self.max_containment_us = max_containment_us;
        self
    }

    pub const fn uses_fpu(mut self, uses_fpu: bool) -> Self {
        self.uses_fpu = uses_fpu;
        self
    }
}

const PROFILE_COOPERATIVE: u8 = 1 << 0;
const PROFILE_ASYNC_PRIORITY: u8 = 1 << 1;
const PROFILE_FORCED_PREEMPTION: u8 = 1 << 2;

/// Capabilities of the exact architectural port and composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulingCapabilities {
    pub port_id: u32,
    pub generation: u32,
    profiles: u8,
    pub priority_levels: u8,
    pub timer_resolution_us: u32,
    /// Worst admitted delivery from a programmed timer deadline to the
    /// scheduler's budget/deadline handler, including wake and ISR entry.
    pub deadline_delivery_wcet_us: u32,
    pub switch_wcet_us: u32,
    pub watchdog_fallback_us: u32,
    pub preserves_fpu_context: bool,
}

impl SchedulingCapabilities {
    /// Cooperative-only port with no implied timer/deadline route.
    pub const fn cooperative(port_id: u32, generation: u32, priority_levels: u8) -> Self {
        Self {
            port_id,
            generation,
            profiles: PROFILE_COOPERATIVE,
            priority_levels,
            timer_resolution_us: 0,
            deadline_delivery_wcet_us: 0,
            switch_wcet_us: 0,
            watchdog_fallback_us: 0,
            preserves_fpu_context: false,
        }
    }

    /// Add a qualified timer/deadline delivery route. Resolution and delivery
    /// latency are separate: admitting only the counter quantum would
    /// under-price wake and interrupt entry.
    pub const fn deadline_observation(
        mut self,
        timer_resolution_us: u32,
        deadline_delivery_wcet_us: u32,
    ) -> Self {
        self.timer_resolution_us = timer_resolution_us;
        self.deadline_delivery_wcet_us = deadline_delivery_wcet_us;
        self
    }

    /// Cooperative plus priority-ordered async/task-boundary dispatch.
    pub const fn async_priority(mut self) -> Self {
        self.profiles |= PROFILE_ASYNC_PRIORITY;
        self
    }

    /// Add an exact forced-context-switch route.
    pub const fn forced_preemption(
        mut self,
        switch_wcet_us: u32,
        watchdog_fallback_us: u32,
        preserves_fpu_context: bool,
    ) -> Self {
        self.profiles |= PROFILE_FORCED_PREEMPTION;
        self.switch_wcet_us = switch_wcet_us;
        self.watchdog_fallback_us = watchdog_fallback_us;
        self.preserves_fpu_context = preserves_fpu_context;
        self
    }

    const fn supports(self, profile: SchedulingProfile) -> bool {
        let flag = match profile {
            SchedulingProfile::Cooperative => PROFILE_COOPERATIVE,
            SchedulingProfile::AsyncPriority => PROFILE_ASYNC_PRIORITY,
            SchedulingProfile::ForcedPreemption => PROFILE_FORCED_PREEMPTION,
        };
        self.profiles & flag != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulingAdmissionError {
    MissingPortIdentity,
    MissingCapabilityGeneration,
    UnsupportedProfile(SchedulingProfile),
    InvalidPriorityLevels,
    InconsistentDeadlineContract,
    PriorityLevelsUnsupported {
        requested: u8,
        available: u8,
    },
    DeadlineRequired,
    DeadlineTimerUnavailable,
    DeadlineDeliveryUnbounded,
    DeadlineBelowTimerResolution {
        deadline_us: u32,
        resolution_us: u32,
    },
    ForcedProfileRequired,
    ContextSwitchUnavailable,
    FpuContextUnsupported,
    ResponseBoundMissed {
        required_us: u32,
        deadline_us: u32,
    },
    WatchdogFallbackUnavailable,
    ContainmentBoundMissed {
        required_us: u32,
        admitted_us: u32,
    },
    CapabilityChanged,
}

/// Immutable proof that one request was checked against one exact port
/// generation. Revalidate it before starting after a provider reset/rebind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulingAdmission {
    pub request: SchedulingRequest,
    pub port_id: u32,
    pub generation: u32,
    pub response_bound_us: u32,
    pub containment_bound_us: u32,
}

impl SchedulingAdmission {
    pub fn revalidate(
        self,
        capabilities: SchedulingCapabilities,
    ) -> Result<Self, SchedulingAdmissionError> {
        if self.port_id != capabilities.port_id || self.generation != capabilities.generation {
            return Err(SchedulingAdmissionError::CapabilityChanged);
        }
        admit_scheduling(self.request, capabilities)
    }
}

/// Admit a graph against the exact port that will execute it.
pub fn admit_scheduling(
    request: SchedulingRequest,
    capabilities: SchedulingCapabilities,
) -> Result<SchedulingAdmission, SchedulingAdmissionError> {
    if capabilities.port_id == 0 {
        return Err(SchedulingAdmissionError::MissingPortIdentity);
    }
    if capabilities.generation == 0 {
        return Err(SchedulingAdmissionError::MissingCapabilityGeneration);
    }
    if !capabilities.supports(request.profile) {
        return Err(SchedulingAdmissionError::UnsupportedProfile(
            request.profile,
        ));
    }
    if request.priority_levels == 0 || capabilities.priority_levels == 0 {
        return Err(SchedulingAdmissionError::InvalidPriorityLevels);
    }
    if request.priority_levels > capabilities.priority_levels {
        return Err(SchedulingAdmissionError::PriorityLevelsUnsupported {
            requested: request.priority_levels,
            available: capabilities.priority_levels,
        });
    }

    match request.deadline_action {
        DeadlineAction::None
            if request.deadline_us != 0
                || request.max_non_preemptible_us != 0
                || request.max_containment_us != 0 =>
        {
            return Err(SchedulingAdmissionError::InconsistentDeadlineContract);
        }
        DeadlineAction::Observe
            if request.max_non_preemptible_us != 0 || request.max_containment_us != 0 =>
        {
            return Err(SchedulingAdmissionError::InconsistentDeadlineContract);
        }
        _ => {}
    }

    if request.profile == SchedulingProfile::ForcedPreemption {
        if capabilities.switch_wcet_us == 0 {
            return Err(SchedulingAdmissionError::ContextSwitchUnavailable);
        }
        if request.uses_fpu && !capabilities.preserves_fpu_context {
            return Err(SchedulingAdmissionError::FpuContextUnsupported);
        }
    }

    let mut response_bound_us = 0;
    let mut containment_bound_us = 0;
    if request.deadline_action != DeadlineAction::None {
        if request.deadline_us == 0 {
            return Err(SchedulingAdmissionError::DeadlineRequired);
        }
        if capabilities.timer_resolution_us == 0 {
            return Err(SchedulingAdmissionError::DeadlineTimerUnavailable);
        }
        if capabilities.deadline_delivery_wcet_us == 0 {
            return Err(SchedulingAdmissionError::DeadlineDeliveryUnbounded);
        }
        if request.deadline_us < capabilities.timer_resolution_us {
            return Err(SchedulingAdmissionError::DeadlineBelowTimerResolution {
                deadline_us: request.deadline_us,
                resolution_us: capabilities.timer_resolution_us,
            });
        }
        response_bound_us = capabilities
            .timer_resolution_us
            .saturating_add(capabilities.deadline_delivery_wcet_us);
        if response_bound_us > request.deadline_us {
            return Err(SchedulingAdmissionError::ResponseBoundMissed {
                required_us: response_bound_us,
                deadline_us: request.deadline_us,
            });
        }
    }

    if request.deadline_action == DeadlineAction::ForceSuspend {
        if request.profile != SchedulingProfile::ForcedPreemption {
            return Err(SchedulingAdmissionError::ForcedProfileRequired);
        }
        response_bound_us = response_bound_us
            .saturating_add(capabilities.switch_wcet_us)
            .saturating_add(request.max_non_preemptible_us);
        if response_bound_us > request.deadline_us {
            return Err(SchedulingAdmissionError::ResponseBoundMissed {
                required_us: response_bound_us,
                deadline_us: request.deadline_us,
            });
        }
        if capabilities.watchdog_fallback_us == 0 {
            return Err(SchedulingAdmissionError::WatchdogFallbackUnavailable);
        }
        containment_bound_us = capabilities.watchdog_fallback_us.max(response_bound_us);
        if request.max_containment_us < containment_bound_us {
            return Err(SchedulingAdmissionError::ContainmentBoundMissed {
                required_us: containment_bound_us,
                admitted_us: request.max_containment_us,
            });
        }
    }

    Ok(SchedulingAdmission {
        request,
        port_id: capabilities.port_id,
        generation: capabilities.generation,
        response_bound_us,
        containment_bound_us,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: SchedulingCapabilities = SchedulingCapabilities::cooperative(7, 3, 8)
        .deadline_observation(2, 3)
        .async_priority()
        .forced_preemption(4, 40, true);

    #[test]
    fn profiles_are_distinct_and_fail_closed() {
        let cooperative = SchedulingRequest::cooperative().observe_deadline(10);
        assert_eq!(
            admit_scheduling(cooperative, PORT)
                .unwrap()
                .response_bound_us,
            5
        );

        let async_request = SchedulingRequest::cooperative()
            .profile(SchedulingProfile::AsyncPriority)
            .priorities(4)
            .observe_deadline(10);
        assert!(admit_scheduling(async_request, PORT).is_ok());

        let forced = SchedulingRequest::cooperative()
            .profile(SchedulingProfile::ForcedPreemption)
            .priorities(8)
            .force_suspend(12, 3, 40)
            .uses_fpu(true);
        let receipt = admit_scheduling(forced, PORT).unwrap();
        assert_eq!(receipt.response_bound_us, 12);
        assert_eq!(receipt.containment_bound_us, 40);

        assert_eq!(
            admit_scheduling(
                forced,
                SchedulingCapabilities::cooperative(7, 3, 8)
                    .deadline_observation(2, 3)
                    .async_priority()
            ),
            Err(SchedulingAdmissionError::UnsupportedProfile(
                SchedulingProfile::ForcedPreemption
            ))
        );
    }

    #[test]
    fn force_suspend_never_degrades_to_observation() {
        let request = SchedulingRequest::cooperative().force_suspend(20, 0, 50);
        assert_eq!(
            admit_scheduling(request, PORT),
            Err(SchedulingAdmissionError::ForcedProfileRequired)
        );

        let forced = request.profile(SchedulingProfile::ForcedPreemption);
        let no_switch = SchedulingCapabilities::cooperative(1, 1, 1)
            .deadline_observation(1, 1)
            .forced_preemption(0, 10, false);
        assert_eq!(
            admit_scheduling(forced, no_switch),
            Err(SchedulingAdmissionError::ContextSwitchUnavailable)
        );
        let no_watchdog = SchedulingCapabilities::cooperative(1, 1, 1)
            .deadline_observation(1, 1)
            .forced_preemption(1, 0, false);
        assert_eq!(
            admit_scheduling(forced, no_watchdog),
            Err(SchedulingAdmissionError::WatchdogFallbackUnavailable)
        );
    }

    #[test]
    fn response_containment_fpu_and_generation_are_enforced() {
        let request = SchedulingRequest::cooperative()
            .profile(SchedulingProfile::ForcedPreemption)
            .force_suspend(8, 3, 40)
            .uses_fpu(true);
        assert_eq!(
            admit_scheduling(request, PORT),
            Err(SchedulingAdmissionError::ResponseBoundMissed {
                required_us: 12,
                deadline_us: 8
            })
        );

        let request = SchedulingRequest::cooperative()
            .profile(SchedulingProfile::ForcedPreemption)
            .force_suspend(12, 3, 39)
            .uses_fpu(true);
        assert_eq!(
            admit_scheduling(request, PORT),
            Err(SchedulingAdmissionError::ContainmentBoundMissed {
                required_us: 40,
                admitted_us: 39
            })
        );

        let receipt = admit_scheduling(request.force_suspend(12, 3, 40), PORT).unwrap();
        assert_eq!(
            receipt.revalidate(SchedulingCapabilities {
                generation: 4,
                ..PORT
            }),
            Err(SchedulingAdmissionError::CapabilityChanged)
        );

        let no_fpu = SchedulingCapabilities::cooperative(7, 3, 8)
            .deadline_observation(2, 3)
            .forced_preemption(4, 40, false);
        assert_eq!(
            admit_scheduling(request.force_suspend(12, 3, 40), no_fpu),
            Err(SchedulingAdmissionError::FpuContextUnsupported)
        );
    }

    #[test]
    fn invalid_identity_timer_priority_and_deadline_are_rejected() {
        let request = SchedulingRequest::cooperative().observe_deadline(2);
        assert_eq!(
            admit_scheduling(
                request,
                SchedulingCapabilities::cooperative(0, 1, 1).deadline_observation(1, 1)
            ),
            Err(SchedulingAdmissionError::MissingPortIdentity)
        );
        assert_eq!(
            admit_scheduling(
                request,
                SchedulingCapabilities::cooperative(1, 0, 1).deadline_observation(1, 1)
            ),
            Err(SchedulingAdmissionError::MissingCapabilityGeneration)
        );
        assert_eq!(
            admit_scheduling(
                request.priorities(2),
                SchedulingCapabilities::cooperative(1, 1, 1).deadline_observation(1, 1)
            ),
            Err(SchedulingAdmissionError::PriorityLevelsUnsupported {
                requested: 2,
                available: 1
            })
        );
        assert_eq!(
            admit_scheduling(request, SchedulingCapabilities::cooperative(1, 1, 1)),
            Err(SchedulingAdmissionError::DeadlineTimerUnavailable)
        );
        assert_eq!(
            admit_scheduling(
                request,
                SchedulingCapabilities::cooperative(1, 1, 1).deadline_observation(1, 0)
            ),
            Err(SchedulingAdmissionError::DeadlineDeliveryUnbounded)
        );
        assert_eq!(
            admit_scheduling(
                request,
                SchedulingCapabilities::cooperative(1, 1, 1).deadline_observation(3, 1)
            ),
            Err(SchedulingAdmissionError::DeadlineBelowTimerResolution {
                deadline_us: 2,
                resolution_us: 3
            })
        );
        assert_eq!(
            admit_scheduling(
                request,
                SchedulingCapabilities::cooperative(1, 1, 1).deadline_observation(1, 2)
            ),
            Err(SchedulingAdmissionError::ResponseBoundMissed {
                required_us: 3,
                deadline_us: 2
            })
        );
    }

    #[test]
    fn direct_struct_literals_cannot_smuggle_inactive_deadline_fields() {
        let no_action = SchedulingRequest {
            deadline_us: 1,
            ..SchedulingRequest::cooperative()
        };
        assert_eq!(
            admit_scheduling(no_action, PORT),
            Err(SchedulingAdmissionError::InconsistentDeadlineContract)
        );

        let observe_with_containment = SchedulingRequest {
            max_containment_us: 40,
            ..SchedulingRequest::cooperative().observe_deadline(10)
        };
        assert_eq!(
            admit_scheduling(observe_with_containment, PORT),
            Err(SchedulingAdmissionError::InconsistentDeadlineContract)
        );
    }
}
