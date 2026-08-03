//! Bounded node-time discipline and distributed-deadline admission.
//!
//! Ordinary packet exchange does not create synchronized time. A caller must
//! provide an explicitly identified four-timestamp observation and accept the
//! resulting uncertainty, holdover, and correction policy.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeTimeCorrection {
    /// Accept bounded offset changes immediately.
    Step { max_step_us: u64 },
    /// Move the current offset toward a new observation at a bounded rate.
    Slew { max_slew_ppm: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeTimeContract {
    pub source_id: u32,
    pub max_uncertainty_us: u64,
    pub max_holdover_us: u64,
    pub oscillator_drift_ppm: u32,
    pub correction: NodeTimeCorrection,
}

impl NodeTimeContract {
    pub const fn valid(self) -> bool {
        if self.source_id == 0 || self.max_uncertainty_us == 0 || self.max_holdover_us == 0 {
            return false;
        }
        match self.correction {
            NodeTimeCorrection::Step { max_step_us } => max_step_us != 0,
            NodeTimeCorrection::Slew { max_slew_ppm } => max_slew_ppm != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeTimeObservation {
    pub source_id: u32,
    pub sequence: u32,
    pub local_send_us: u64,
    pub remote_receive_us: u64,
    pub remote_send_us: u64,
    pub local_receive_us: u64,
    /// Error bound declared by the remote clock source, excluding path delay.
    pub source_uncertainty_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeTimeReceipt {
    pub source_id: u32,
    pub source_sequence: u32,
    pub generation: u32,
    pub local_us: u64,
    pub synchronized_us: u64,
    pub offset_us: i64,
    pub uncertainty_us: u64,
    pub observation_age_us: u64,
    pub holdover: bool,
    pub correction_pending_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistributedDeadlineReceipt {
    pub time: NodeTimeReceipt,
    pub deadline_us: u64,
    pub execution_budget_us: u64,
    pub worst_case_finish_us: u64,
    pub slack_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeTimeError {
    InvalidContract,
    WrongSource,
    ReorderedObservation,
    InvalidExchange,
    ArithmeticOverflow,
    StepExceeded,
    ClockRegressed,
    HoldoverExpired,
    UncertaintyExceeded,
    DeadlineMiss,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NodeTimeState {
    sequence: u32,
    generation: u32,
    observed_local_us: u64,
    offset_us: i64,
    target_offset_us: i64,
    base_uncertainty_us: u64,
}

pub struct NodeTimeClock {
    contract: NodeTimeContract,
    state: Option<NodeTimeState>,
}

impl NodeTimeClock {
    pub fn new(contract: NodeTimeContract) -> Result<Self, NodeTimeError> {
        if !contract.valid() {
            return Err(NodeTimeError::InvalidContract);
        }
        Ok(Self {
            contract,
            state: None,
        })
    }

    pub const fn contract(&self) -> NodeTimeContract {
        self.contract
    }

    pub const fn synchronized(&self) -> bool {
        self.state.is_some()
    }

    pub fn observe(
        &mut self,
        observation: NodeTimeObservation,
    ) -> Result<NodeTimeReceipt, NodeTimeError> {
        if observation.source_id != self.contract.source_id {
            return Err(NodeTimeError::WrongSource);
        }
        if observation.local_receive_us < observation.local_send_us
            || observation.remote_send_us < observation.remote_receive_us
        {
            return Err(NodeTimeError::InvalidExchange);
        }
        if let Some(state) = self.state {
            if observation.local_receive_us < state.observed_local_us {
                return Err(NodeTimeError::ClockRegressed);
            }
            if !serial_is_newer(observation.sequence, state.sequence) {
                return Err(NodeTimeError::ReorderedObservation);
            }
        }

        let local_span = observation.local_receive_us - observation.local_send_us;
        let remote_span = observation.remote_send_us - observation.remote_receive_us;
        let round_trip_us = local_span
            .checked_sub(remote_span)
            .ok_or(NodeTimeError::InvalidExchange)?;
        let observed_offset = observed_offset_us(observation)?;
        let path_uncertainty = round_trip_us.div_ceil(2);
        let base_uncertainty_us = path_uncertainty
            .checked_add(observation.source_uncertainty_us)
            .ok_or(NodeTimeError::ArithmeticOverflow)?;
        if base_uncertainty_us > self.contract.max_uncertainty_us {
            return Err(NodeTimeError::UncertaintyExceeded);
        }

        let (offset_us, target_offset_us, generation) = match self.state {
            None => (observed_offset, observed_offset, 1),
            Some(state) => {
                let generation = state
                    .generation
                    .checked_add(1)
                    .ok_or(NodeTimeError::GenerationExhausted)?;
                let elapsed = observation.local_receive_us - state.observed_local_us;
                let current_offset = effective_offset(state, elapsed, self.contract.correction)?;
                let correction = signed_distance(current_offset, observed_offset)?;
                match self.contract.correction {
                    NodeTimeCorrection::Step { max_step_us } => {
                        if correction > max_step_us {
                            return Err(NodeTimeError::StepExceeded);
                        }
                        (observed_offset, observed_offset, generation)
                    }
                    NodeTimeCorrection::Slew { .. } => {
                        (current_offset, observed_offset, generation)
                    }
                }
            }
        };

        let previous = self.state;
        self.state = Some(NodeTimeState {
            sequence: observation.sequence,
            generation,
            observed_local_us: observation.local_receive_us,
            offset_us,
            target_offset_us,
            base_uncertainty_us,
        });
        match self.estimate(observation.local_receive_us) {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                // Observation admission is transactional: an uncertainty or
                // synchronized-time overflow cannot replace the last usable state.
                self.state = previous;
                Err(error)
            }
        }
    }

    pub fn estimate(&self, local_now_us: u64) -> Result<NodeTimeReceipt, NodeTimeError> {
        let state = self.state.ok_or(NodeTimeError::HoldoverExpired)?;
        let age = local_now_us
            .checked_sub(state.observed_local_us)
            .ok_or(NodeTimeError::ClockRegressed)?;
        if age > self.contract.max_holdover_us {
            return Err(NodeTimeError::HoldoverExpired);
        }
        let offset_us = effective_offset(state, age, self.contract.correction)?;
        let correction_pending_us = signed_distance(offset_us, state.target_offset_us)?;
        let drift = drift_growth(age, self.contract.oscillator_drift_ppm);
        let uncertainty_us = state
            .base_uncertainty_us
            .checked_add(drift)
            .and_then(|value| value.checked_add(correction_pending_us))
            .ok_or(NodeTimeError::ArithmeticOverflow)?;
        if uncertainty_us > self.contract.max_uncertainty_us {
            return Err(NodeTimeError::UncertaintyExceeded);
        }
        let synchronized_us = add_signed(local_now_us, offset_us)?;
        Ok(NodeTimeReceipt {
            source_id: self.contract.source_id,
            source_sequence: state.sequence,
            generation: state.generation,
            local_us: local_now_us,
            synchronized_us,
            offset_us,
            uncertainty_us,
            observation_age_us: age,
            holdover: age != 0,
            correction_pending_us,
        })
    }

    pub fn admit_deadline(
        &self,
        local_now_us: u64,
        synchronized_deadline_us: u64,
        execution_budget_us: u64,
    ) -> Result<DistributedDeadlineReceipt, NodeTimeError> {
        let time = self.estimate(local_now_us)?;
        let worst_case_finish_us = time
            .synchronized_us
            .checked_add(time.uncertainty_us)
            .and_then(|value| value.checked_add(execution_budget_us))
            .ok_or(NodeTimeError::ArithmeticOverflow)?;
        let slack_us = synchronized_deadline_us
            .checked_sub(worst_case_finish_us)
            .ok_or(NodeTimeError::DeadlineMiss)?;
        Ok(DistributedDeadlineReceipt {
            time,
            deadline_us: synchronized_deadline_us,
            execution_budget_us,
            worst_case_finish_us,
            slack_us,
        })
    }
}

fn observed_offset_us(observation: NodeTimeObservation) -> Result<i64, NodeTimeError> {
    let left = i128::from(observation.remote_receive_us) - i128::from(observation.local_send_us);
    let right = i128::from(observation.remote_send_us) - i128::from(observation.local_receive_us);
    let offset = (left + right) / 2;
    i64::try_from(offset).map_err(|_| NodeTimeError::ArithmeticOverflow)
}

fn add_signed(value: u64, delta: i64) -> Result<u64, NodeTimeError> {
    let result = i128::from(value) + i128::from(delta);
    u64::try_from(result).map_err(|_| NodeTimeError::ArithmeticOverflow)
}

fn signed_distance(left: i64, right: i64) -> Result<u64, NodeTimeError> {
    u64::try_from((i128::from(left) - i128::from(right)).abs())
        .map_err(|_| NodeTimeError::ArithmeticOverflow)
}

fn move_toward(current: i64, target: i64, amount: u64) -> Result<i64, NodeTimeError> {
    let current = i128::from(current);
    let target = i128::from(target);
    let amount = i128::from(amount);
    let next = if target >= current {
        (current + amount).min(target)
    } else {
        (current - amount).max(target)
    };
    i64::try_from(next).map_err(|_| NodeTimeError::ArithmeticOverflow)
}

fn effective_offset(
    state: NodeTimeState,
    elapsed_us: u64,
    correction: NodeTimeCorrection,
) -> Result<i64, NodeTimeError> {
    match correction {
        NodeTimeCorrection::Step { .. } => Ok(state.offset_us),
        NodeTimeCorrection::Slew { max_slew_ppm } => move_toward(
            state.offset_us,
            state.target_offset_us,
            drift_growth(elapsed_us, max_slew_ppm),
        ),
    }
}

fn drift_growth(elapsed_us: u64, ppm: u32) -> u64 {
    let product = u128::from(elapsed_us) * u128::from(ppm);
    u64::try_from(product.div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

fn serial_is_newer(candidate: u32, current: u32) -> bool {
    let delta = candidate.wrapping_sub(current);
    delta != 0 && delta < (1u32 << 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_contract() -> NodeTimeContract {
        NodeTimeContract {
            source_id: 7,
            max_uncertainty_us: 100,
            max_holdover_us: 1_000,
            oscillator_drift_ppm: 10_000,
            correction: NodeTimeCorrection::Step { max_step_us: 500 },
        }
    }

    fn observation(sequence: u32, local_base: u64, offset: u64) -> NodeTimeObservation {
        NodeTimeObservation {
            source_id: 7,
            sequence,
            local_send_us: local_base,
            remote_receive_us: local_base + 10 + offset,
            remote_send_us: local_base + 15 + offset,
            local_receive_us: local_base + 25,
            source_uncertainty_us: 2,
        }
    }

    #[test]
    fn identified_exchange_carries_uncertainty_into_deadline_admission() {
        let mut clock = NodeTimeClock::new(step_contract()).unwrap();
        let receipt = clock.observe(observation(1, 1_000, 200)).unwrap();
        assert_eq!(receipt.offset_us, 200);
        assert_eq!(receipt.uncertainty_us, 12);
        let deadline = clock.admit_deadline(1_030, 1_300, 40).unwrap();
        assert_eq!(deadline.worst_case_finish_us, 1_283);
        assert_eq!(deadline.slack_us, 17);
        assert_eq!(
            clock.admit_deadline(1_030, 1_282, 40),
            Err(NodeTimeError::DeadlineMiss)
        );
    }

    #[test]
    fn delay_reorder_source_step_and_invalid_exchange_fail_closed() {
        let mut clock = NodeTimeClock::new(step_contract()).unwrap();
        clock.observe(observation(10, 1_000, 200)).unwrap();
        assert_eq!(
            clock.observe(observation(10, 1_100, 200)),
            Err(NodeTimeError::ReorderedObservation)
        );
        let mut wrong = observation(11, 1_100, 200);
        wrong.source_id = 8;
        assert_eq!(clock.observe(wrong), Err(NodeTimeError::WrongSource));
        let mut invalid = observation(11, 1_100, 200);
        invalid.remote_send_us = invalid.remote_receive_us - 1;
        assert_eq!(clock.observe(invalid), Err(NodeTimeError::InvalidExchange));
        assert_eq!(
            clock.observe(observation(11, 1_100, 900)),
            Err(NodeTimeError::StepExceeded)
        );
    }

    #[test]
    fn low_power_holdover_grows_uncertainty_then_expires() {
        let mut clock = NodeTimeClock::new(step_contract()).unwrap();
        clock.observe(observation(1, 1_000, 200)).unwrap();
        let held = clock.estimate(1_525).unwrap();
        assert!(held.holdover);
        assert_eq!(held.observation_age_us, 500);
        assert_eq!(held.uncertainty_us, 17);
        assert_eq!(clock.estimate(2_026), Err(NodeTimeError::HoldoverExpired));
        assert_eq!(clock.estimate(900), Err(NodeTimeError::ClockRegressed));
    }

    #[test]
    fn slew_reports_pending_correction_instead_of_claiming_instant_sync() {
        let contract = NodeTimeContract {
            correction: NodeTimeCorrection::Slew {
                max_slew_ppm: 100_000,
            },
            max_uncertainty_us: 1_000,
            ..step_contract()
        };
        let mut clock = NodeTimeClock::new(contract).unwrap();
        clock.observe(observation(1, 1_000, 100)).unwrap();
        let corrected = clock.observe(observation(2, 1_100, 200)).unwrap();
        assert_eq!(corrected.offset_us, 100);
        assert_eq!(corrected.correction_pending_us, 100);
        assert!(corrected.uncertainty_us >= 102);
        let later = clock.estimate(1_225).unwrap();
        assert_eq!(later.offset_us, 110);
        assert_eq!(later.correction_pending_us, 90);
    }

    #[test]
    fn rejected_slew_observation_preserves_the_last_usable_generation() {
        let contract = NodeTimeContract {
            correction: NodeTimeCorrection::Slew { max_slew_ppm: 1 },
            max_uncertainty_us: 50,
            ..step_contract()
        };
        let mut clock = NodeTimeClock::new(contract).unwrap();
        let first = clock.observe(observation(1, 1_000, 100)).unwrap();
        assert_eq!(
            clock.observe(observation(2, 1_100, 500)),
            Err(NodeTimeError::UncertaintyExceeded)
        );
        let retained = clock.estimate(1_150).unwrap();
        assert_eq!(retained.generation, first.generation);
        assert_eq!(retained.source_sequence, first.source_sequence);
        assert_eq!(retained.offset_us, first.offset_us);
    }

    #[test]
    fn sequence_wrap_is_accepted_but_old_half_range_is_not() {
        let mut clock = NodeTimeClock::new(step_contract()).unwrap();
        clock.observe(observation(u32::MAX, 1_000, 200)).unwrap();
        clock.observe(observation(0, 1_100, 200)).unwrap();
        assert_eq!(
            clock.observe(observation(u32::MAX, 1_200, 200)),
            Err(NodeTimeError::ReorderedObservation)
        );
    }
}
