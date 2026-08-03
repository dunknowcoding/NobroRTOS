//! Admission- and use-time freshness checks for AI fallback snapshots.

use crate::{AiInvocationLimits, AiModelContract, AiRoutePolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AiSnapshotExpiryAction {
    Degrade = 1,
    Recompute = 2,
    Fail = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiSnapshotFreshnessContract {
    pub max_age_us: u32,
    pub expiry_action: AiSnapshotExpiryAction,
}

impl AiSnapshotFreshnessContract {
    pub const fn new(max_age_us: u32, expiry_action: AiSnapshotExpiryAction) -> Self {
        Self {
            max_age_us,
            expiry_action,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiSnapshotStamp {
    pub model_id: u32,
    pub generation: u32,
    pub produced_at_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiFreshnessAdmission {
    model_id: u32,
    max_age_us: u32,
    expiry_action: AiSnapshotExpiryAction,
}

impl AiFreshnessAdmission {
    pub const fn model_id(&self) -> u32 {
        self.model_id
    }

    pub const fn max_age_us(&self) -> u32 {
        self.max_age_us
    }

    pub const fn expiry_action(&self) -> AiSnapshotExpiryAction {
        self.expiry_action
    }

    pub fn assess(
        self,
        snapshot: AiSnapshotStamp,
        now_us: u64,
    ) -> Result<AiSnapshotUseReceipt, AiFreshnessError> {
        if snapshot.model_id != self.model_id || snapshot.generation == 0 {
            return Err(AiFreshnessError::SnapshotIdentity);
        }
        let age_us = now_us
            .checked_sub(snapshot.produced_at_us)
            .ok_or(AiFreshnessError::ClockRegressed)?;
        let decision = if age_us <= u64::from(self.max_age_us) {
            AiSnapshotUseDecision::Use
        } else {
            match self.expiry_action {
                AiSnapshotExpiryAction::Degrade => AiSnapshotUseDecision::Degrade,
                AiSnapshotExpiryAction::Recompute => AiSnapshotUseDecision::Recompute,
                AiSnapshotExpiryAction::Fail => AiSnapshotUseDecision::Fail,
            }
        };
        Ok(AiSnapshotUseReceipt {
            model_id: snapshot.model_id,
            snapshot_generation: snapshot.generation,
            age_us,
            max_age_us: self.max_age_us,
            decision,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AiSnapshotUseDecision {
    Use = 1,
    Degrade = 2,
    Recompute = 3,
    Fail = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiSnapshotUseReceipt {
    pub model_id: u32,
    pub snapshot_generation: u32,
    pub age_us: u64,
    pub max_age_us: u32,
    pub decision: AiSnapshotUseDecision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiFreshnessError {
    InvalidContract,
    StaleRouteDisabled,
    InvocationBoundMissing,
    SnapshotIdentity,
    ClockRegressed,
}

/// Admit stale fallback only when all participating contracts carry a finite,
/// non-zero age bound. The strictest bound is rechecked immediately before use.
pub fn admit_ai_snapshot_freshness(
    model: AiModelContract,
    route: AiRoutePolicy,
    invocation: AiInvocationLimits,
    freshness: AiSnapshotFreshnessContract,
) -> Result<AiFreshnessAdmission, AiFreshnessError> {
    if model.model_id == 0 || freshness.max_age_us == 0 {
        return Err(AiFreshnessError::InvalidContract);
    }
    if !invocation.allow_stale_snapshot {
        return Err(AiFreshnessError::StaleRouteDisabled);
    }
    if invocation.max_stale_us == 0 {
        return Err(AiFreshnessError::InvocationBoundMissing);
    }
    let route_bound = route.effective_stale_after_us(model);
    if route_bound == 0 {
        return Err(AiFreshnessError::InvalidContract);
    }
    Ok(AiFreshnessAdmission {
        model_id: model.model_id,
        max_age_us: route_bound
            .min(invocation.max_stale_us)
            .min(freshness.max_age_us),
        expiry_action: freshness.expiry_action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AiBackendKind, AiRoutePreference};

    fn admitted(action: AiSnapshotExpiryAction) -> AiFreshnessAdmission {
        let model = AiModelContract::new(AiBackendKind::Hybrid, 9, 8, 8, 256, 10_000)
            .with_stale_after_us(80);
        let route = AiRoutePolicy::new(AiRoutePreference::HybridFallback, 70, 2);
        let invocation = AiInvocationLimits::new(8, 0, 512, 20_000)
            .with_max_stale_us(60)
            .allow_stale_snapshot();
        admit_ai_snapshot_freshness(
            model,
            route,
            invocation,
            AiSnapshotFreshnessContract::new(50, action),
        )
        .unwrap()
    }

    #[test]
    fn admission_uses_the_strictest_nonzero_bound() {
        let admission = admitted(AiSnapshotExpiryAction::Fail);
        assert_eq!(admission.model_id(), 9);
        assert_eq!(admission.max_age_us(), 50);
    }

    #[test]
    fn use_time_recheck_selects_each_explicit_expiry_action() {
        let stamp = AiSnapshotStamp {
            model_id: 9,
            generation: 3,
            produced_at_us: 100,
        };
        assert_eq!(
            admitted(AiSnapshotExpiryAction::Fail)
                .assess(stamp, 150)
                .unwrap()
                .decision,
            AiSnapshotUseDecision::Use
        );
        for (action, expected) in [
            (
                AiSnapshotExpiryAction::Degrade,
                AiSnapshotUseDecision::Degrade,
            ),
            (
                AiSnapshotExpiryAction::Recompute,
                AiSnapshotUseDecision::Recompute,
            ),
            (AiSnapshotExpiryAction::Fail, AiSnapshotUseDecision::Fail),
        ] {
            assert_eq!(
                admitted(action).assess(stamp, 151).unwrap().decision,
                expected
            );
        }
    }

    #[test]
    fn unbounded_disabled_and_bad_identity_paths_fail_closed() {
        let model = AiModelContract::new(AiBackendKind::Hybrid, 9, 8, 8, 256, 10_000)
            .with_stale_after_us(80);
        let route = AiRoutePolicy::new(AiRoutePreference::HybridFallback, 70, 2);
        let limits = AiInvocationLimits::new(8, 0, 512, 20_000);
        let freshness = AiSnapshotFreshnessContract::new(50, AiSnapshotExpiryAction::Fail);
        assert_eq!(
            admit_ai_snapshot_freshness(model, route, limits, freshness),
            Err(AiFreshnessError::StaleRouteDisabled)
        );
        assert_eq!(
            admit_ai_snapshot_freshness(model, route, limits.allow_stale_snapshot(), freshness),
            Err(AiFreshnessError::InvocationBoundMissing)
        );
        let admission = admitted(AiSnapshotExpiryAction::Fail);
        assert_eq!(
            admission.assess(
                AiSnapshotStamp {
                    model_id: 8,
                    generation: 1,
                    produced_at_us: 10,
                },
                20,
            ),
            Err(AiFreshnessError::SnapshotIdentity)
        );
        assert_eq!(
            admission.assess(
                AiSnapshotStamp {
                    model_id: 9,
                    generation: 1,
                    produced_at_us: 30,
                },
                20,
            ),
            Err(AiFreshnessError::ClockRegressed)
        );
    }
}
