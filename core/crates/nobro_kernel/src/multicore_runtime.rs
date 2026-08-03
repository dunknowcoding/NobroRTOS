//! Per-core executor lifecycle coordination for multi-executor (dual-core and
//! beyond) runtimes.
//!
//! [`multicore`](crate::multicore) produces the *static* placement -- which
//! module runs on which core and whether each core fits its utilization bound.
//! This module adds the *runtime* coordination on top:
//! bringing the independent per-core executors up and down in a defined order,
//! transferring a module's ownership metadata from one core to another with
//! preserved accounting, and giving one core a fault/recovery authority that
//! never strands another core.
//!
//! It is deterministic and allocation-free. Physical core start/stop remains
//! the board port's responsibility and is supplied through bounded callbacks.
//! Runtime ownership transfer uses [`MulticoreTaskExecutor`] so real
//! [`KernelExecutor`](crate::KernelExecutor) task state moves before placement
//! metadata commits. Startup is transactional -- if any core fails to start,
//! the cores already started are stopped in reverse order and the system
//! returns fully down, never partially up.

use crate::ModuleId;

/// Executor operation required for an ownership transfer between live cores.
///
/// Implementations must leave both executors unchanged on error. The kernel's
/// implementation moves the complete task scheduling state and reruns
/// destination response-time admission before changing either task table.
pub trait MulticoreTaskExecutor {
    type Error;

    fn transfer_task_to(
        &mut self,
        destination: &mut Self,
        module: ModuleId,
    ) -> Result<(), Self::Error>;
}

/// One core executor's lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreExecutorState {
    /// Not running; owns its placement but no executor is live.
    Down,
    /// Executor is live and running its owned modules.
    Up,
    /// Executor faulted; it owns its placement but is not running.
    Faulted,
}

/// Hardware/runtime role of one core. The primary bootstrap remains pinned to
/// core 0; application work may still run there when the exact port declares
/// that core application-capable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreRole {
    PrimaryBootstrap,
    Application,
}

/// Monotonic incarnation of one core executor. Zero means that the executor has
/// never started. Generations saturate rather than wrapping into an ABA match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreGeneration(u32);

impl CoreGeneration {
    pub const NEVER_STARTED: Self = Self(0);

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreOwnership {
    pub core: u8,
    pub generation: CoreGeneration,
    pub module: ModuleId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulticoreError {
    /// Core index is outside `0..CORES`.
    UnknownCore { core: u8 },
    /// A per-core module slot table is full.
    CoreFull { core: u8 },
    /// The same module was placed twice.
    DuplicateModule { module: ModuleId },
    /// Placing/transferring would push a core past 100% utilization.
    CoreOverloaded { core: u8, would_be: u32, limit: u32 },
    /// An operation needs a core to be `Up` (or `Faulted`) but it was not.
    WrongCoreState { core: u8, state: CoreExecutorState },
    /// A transfer named a module its source core does not own.
    ModuleNotOnCore { core: u8, module: ModuleId },
    /// A start callback reported failure; startup rolled back to fully down.
    StartFailed { core: u8 },
    /// The hardware-required primary bootstrap cannot be placed or migrated.
    BootstrapPinned { module: ModuleId, core: u8 },
    /// The exact port does not permit application execution on this core.
    CoreNotApplicationCapable { core: u8 },
    /// A core has exhausted its monotonic incarnation counter; accepting a
    /// wrapped generation would make stale commands valid again.
    GenerationExhausted { core: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulticoreTransferError<E> {
    Coordination(MulticoreError),
    Executor(E),
}

const UTIL_LIMIT: u32 = 10_000;

#[derive(Clone, Copy)]
struct Owned {
    module: ModuleId,
    util_permyriad: u32,
}

#[derive(Clone, Copy)]
struct TransferPlan {
    from_index: usize,
    to_index: usize,
    source_slot: usize,
    destination_slot: usize,
    util_permyriad: u32,
    destination_util: u32,
}

/// Coordinates `CORES` per-core executors, each retaining up to `SLOTS` owned
/// modules. Storage is fixed (`CORES * SLOTS` module slots); no allocation.
pub struct MulticoreExecutorLifecycle<const CORES: usize, const SLOTS: usize> {
    states: [CoreExecutorState; CORES],
    owned: [[Option<Owned>; SLOTS]; CORES],
    core_util: [u32; CORES],
    generations: [CoreGeneration; CORES],
    application_capable: [bool; CORES],
}

impl<const CORES: usize, const SLOTS: usize> Default for MulticoreExecutorLifecycle<CORES, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CORES: usize, const SLOTS: usize> MulticoreExecutorLifecycle<CORES, SLOTS> {
    pub const fn new() -> Self {
        Self {
            states: [CoreExecutorState::Down; CORES],
            owned: [[None; SLOTS]; CORES],
            core_util: [0; CORES],
            generations: [CoreGeneration::NEVER_STARTED; CORES],
            application_capable: [true; CORES],
        }
    }

    fn next_generation(&self, core: usize) -> Result<CoreGeneration, MulticoreError> {
        let current = self.generations[core].0;
        if current == u32::MAX {
            Err(MulticoreError::GenerationExhausted { core: core as u8 })
        } else {
            Ok(CoreGeneration(current + 1))
        }
    }

    fn check_core(&self, core: u8) -> Result<usize, MulticoreError> {
        let index = core as usize;
        if index >= CORES {
            return Err(MulticoreError::UnknownCore { core });
        }
        Ok(index)
    }

    /// Assign a module (with its utilization) to a core's placement. Only valid
    /// while the core is `Down` (placement is fixed before the executor runs).
    /// Rejects a duplicate module, a full core, or a core-overload -- accounting
    /// never admits more than one core of work.
    pub fn place(
        &mut self,
        core: u8,
        module: ModuleId,
        util_permyriad: u32,
    ) -> Result<(), MulticoreError> {
        let index = self.check_core(core)?;
        if self.states[index] != CoreExecutorState::Down {
            return Err(MulticoreError::WrongCoreState {
                core,
                state: self.states[index],
            });
        }
        if module == ModuleId::Kernel && index != 0 {
            return Err(MulticoreError::BootstrapPinned { module, core });
        }
        if module != ModuleId::Kernel && !self.application_capable[index] {
            return Err(MulticoreError::CoreNotApplicationCapable { core });
        }
        if self.find_any(module).is_some() {
            return Err(MulticoreError::DuplicateModule { module });
        }
        let would_be = self.core_util[index].saturating_add(util_permyriad);
        if would_be > UTIL_LIMIT {
            return Err(MulticoreError::CoreOverloaded {
                core,
                would_be,
                limit: UTIL_LIMIT,
            });
        }
        let Some(slot) = self.owned[index].iter_mut().find(|s| s.is_none()) else {
            return Err(MulticoreError::CoreFull { core });
        };
        *slot = Some(Owned {
            module,
            util_permyriad,
        });
        self.core_util[index] = would_be;
        Ok(())
    }

    /// Start every core's executor in ascending order. If any `start(core)`
    /// returns false, the cores already started are stopped in reverse order
    /// via `stop`, all cores are left `Down`, and `StartFailed` is returned --
    /// the system is either fully up or fully down, never partially up.
    pub fn start_all(
        &mut self,
        mut start: impl FnMut(u8) -> bool,
        mut stop: impl FnMut(u8),
    ) -> Result<(), MulticoreError> {
        // Validate the whole transaction before invoking any caller callback.
        // Re-entering startup while one executor is already live/faulted would
        // otherwise double-start early cores and make rollback ambiguous.
        for (core, state) in self.states.iter().copied().enumerate() {
            if state != CoreExecutorState::Down {
                return Err(MulticoreError::WrongCoreState {
                    core: core as u8,
                    state,
                });
            }
            self.next_generation(core)?;
        }
        let mut core = 0usize;
        while core < CORES {
            if start(core as u8) {
                self.generations[core] = self.next_generation(core)?;
                self.states[core] = CoreExecutorState::Up;
                core += 1;
            } else {
                // Roll back the ones already up, in reverse order.
                let mut back = core;
                while back > 0 {
                    back -= 1;
                    stop(back as u8);
                    self.states[back] = CoreExecutorState::Down;
                }
                return Err(MulticoreError::StartFailed { core: core as u8 });
            }
        }
        Ok(())
    }

    /// Stop every core's executor in descending order and leave all `Down`.
    pub fn shutdown_all(&mut self, mut stop: impl FnMut(u8)) {
        let mut core = CORES;
        while core > 0 {
            core -= 1;
            if self.states[core] != CoreExecutorState::Down {
                stop(core as u8);
                self.states[core] = CoreExecutorState::Down;
            }
        }
    }

    /// Move a module's ownership metadata from one live core to another,
    /// preserving total utilization. Both cores must be `Up`, the source must
    /// own the module, and the destination must not overload; otherwise nothing
    /// changes (no partial metadata transfer).
    ///
    /// This metadata-only variant is for callers that already moved or do not
    /// own executor work. Prefer [`transfer_executor`](Self::transfer_executor)
    /// when actual task ownership changes. A same-core transfer is an
    /// idempotent no-op after state and ownership validation.
    pub fn transfer(&mut self, module: ModuleId, from: u8, to: u8) -> Result<(), MulticoreError> {
        let plan = self.transfer_plan(module, from, to)?;
        self.commit_transfer(plan);
        Ok(())
    }

    /// Move real executor work first, then commit the matching ownership and
    /// utilization metadata. All coordination checks run before either
    /// executor is touched. A same-core move is an idempotent metadata check.
    pub fn transfer_executor<E: MulticoreTaskExecutor>(
        &mut self,
        module: ModuleId,
        from: u8,
        to: u8,
        source: &mut E,
        destination: &mut E,
    ) -> Result<(), MulticoreTransferError<E::Error>> {
        let plan = self
            .transfer_plan(module, from, to)
            .map_err(MulticoreTransferError::Coordination)?;
        if from != to {
            source
                .transfer_task_to(destination, module)
                .map_err(MulticoreTransferError::Executor)?;
        }
        self.commit_transfer(plan);
        Ok(())
    }

    fn transfer_plan(
        &self,
        module: ModuleId,
        from: u8,
        to: u8,
    ) -> Result<TransferPlan, MulticoreError> {
        let from_index = self.check_core(from)?;
        let to_index = self.check_core(to)?;
        if self.states[from_index] != CoreExecutorState::Up {
            return Err(MulticoreError::WrongCoreState {
                core: from,
                state: self.states[from_index],
            });
        }
        if self.states[to_index] != CoreExecutorState::Up {
            return Err(MulticoreError::WrongCoreState {
                core: to,
                state: self.states[to_index],
            });
        }
        if module == ModuleId::Kernel && from_index != to_index {
            return Err(MulticoreError::BootstrapPinned { module, core: to });
        }
        if from_index != to_index && !self.application_capable[to_index] {
            return Err(MulticoreError::CoreNotApplicationCapable { core: to });
        }
        let Some(slot_index) = self.find_on(from_index, module) else {
            return Err(MulticoreError::ModuleNotOnCore { core: from, module });
        };
        if from_index == to_index {
            return Ok(TransferPlan {
                from_index,
                to_index,
                source_slot: slot_index,
                destination_slot: slot_index,
                util_permyriad: 0,
                destination_util: self.core_util[to_index],
            });
        }
        let util = self.owned[from_index][slot_index].unwrap().util_permyriad;
        let would_be = self.core_util[to_index].saturating_add(util);
        if would_be > UTIL_LIMIT {
            return Err(MulticoreError::CoreOverloaded {
                core: to,
                would_be,
                limit: UTIL_LIMIT,
            });
        }
        let Some(destination_slot) = self.owned[to_index].iter().position(Option::is_none) else {
            return Err(MulticoreError::CoreFull { core: to });
        };
        Ok(TransferPlan {
            from_index,
            to_index,
            source_slot: slot_index,
            destination_slot,
            util_permyriad: util,
            destination_util: would_be,
        })
    }

    fn commit_transfer(&mut self, plan: TransferPlan) {
        if plan.from_index == plan.to_index {
            return;
        }
        let owned = self.owned[plan.from_index][plan.source_slot];
        self.owned[plan.to_index][plan.destination_slot] = owned;
        self.owned[plan.from_index][plan.source_slot] = None;
        self.core_util[plan.from_index] =
            self.core_util[plan.from_index].saturating_sub(plan.util_permyriad);
        self.core_util[plan.to_index] = plan.destination_util;
    }

    /// Record that a live core faulted. It retains its placement/accounting but
    /// stops running; peers are untouched (a fault does not cascade).
    pub fn fault(&mut self, core: u8) -> Result<(), MulticoreError> {
        let index = self.check_core(core)?;
        if self.states[index] != CoreExecutorState::Up {
            return Err(MulticoreError::WrongCoreState {
                core,
                state: self.states[index],
            });
        }
        self.states[index] = CoreExecutorState::Faulted;
        Ok(())
    }

    /// Restart one faulted core's executor. On a successful `restart(core)` the
    /// core returns `Up` with its original placement and accounting intact; on
    /// failure it stays `Faulted`. Only that core is touched.
    pub fn recover(
        &mut self,
        core: u8,
        mut restart: impl FnMut(u8) -> bool,
    ) -> Result<(), MulticoreError> {
        let index = self.check_core(core)?;
        if self.states[index] != CoreExecutorState::Faulted {
            return Err(MulticoreError::WrongCoreState {
                core,
                state: self.states[index],
            });
        }
        let next_generation = self.next_generation(index)?;
        if restart(core) {
            self.generations[index] = next_generation;
            self.states[index] = CoreExecutorState::Up;
            Ok(())
        } else {
            Err(MulticoreError::StartFailed { core })
        }
    }

    pub fn state(&self, core: u8) -> Option<CoreExecutorState> {
        self.states.get(core as usize).copied()
    }

    /// Configure whether the exact port permits ordinary application execution
    /// on this core. The primary bootstrap role remains core 0 regardless.
    pub fn set_application_capable(
        &mut self,
        core: u8,
        capable: bool,
    ) -> Result<(), MulticoreError> {
        let index = self.check_core(core)?;
        if self.states[index] != CoreExecutorState::Down {
            return Err(MulticoreError::WrongCoreState {
                core,
                state: self.states[index],
            });
        }
        self.application_capable[index] = capable;
        Ok(())
    }

    pub fn core_role(&self, core: u8) -> Option<CoreRole> {
        self.states.get(core as usize).map(|_| {
            if core == 0 {
                CoreRole::PrimaryBootstrap
            } else {
                CoreRole::Application
            }
        })
    }

    pub fn application_capable(&self, core: u8) -> Option<bool> {
        self.application_capable.get(core as usize).copied()
    }

    pub fn generation(&self, core: u8) -> Option<CoreGeneration> {
        self.generations.get(core as usize).copied()
    }

    pub fn ownership(&self, module: ModuleId) -> Option<CoreOwnership> {
        let (core, _) = self.find_any(module)?;
        Some(CoreOwnership {
            core: core as u8,
            generation: self.generations[core],
            module,
        })
    }

    pub fn all_up(&self) -> bool {
        self.states.iter().all(|s| *s == CoreExecutorState::Up)
    }

    pub fn all_down(&self) -> bool {
        self.states.iter().all(|s| *s == CoreExecutorState::Down)
    }

    pub fn core_utilization(&self, core: u8) -> Option<u32> {
        self.core_util.get(core as usize).copied()
    }

    /// Total placed utilization across all cores -- invariant across transfers.
    pub fn total_utilization(&self) -> u32 {
        self.core_util
            .iter()
            .copied()
            .fold(0u32, u32::saturating_add)
    }

    pub fn owns(&self, core: u8, module: ModuleId) -> bool {
        self.check_core(core)
            .ok()
            .and_then(|index| self.find_on(index, module))
            .is_some()
    }

    fn find_on(&self, core_index: usize, module: ModuleId) -> Option<usize> {
        self.owned[core_index]
            .iter()
            .position(|s| matches!(s, Some(o) if o.module == module))
    }

    fn find_any(&self, module: ModuleId) -> Option<(usize, usize)> {
        for (core_index, slots) in self.owned.iter().enumerate() {
            if let Some(slot_index) = slots
                .iter()
                .position(|s| matches!(s, Some(o) if o.module == module))
            {
                return Some((core_index, slot_index));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: u8) -> ModuleId {
        ModuleId::App(id)
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    struct ModelExecutor {
        owned: [Option<ModuleId>; 2],
        fail_transfer: bool,
    }

    impl MulticoreTaskExecutor for ModelExecutor {
        type Error = u8;

        fn transfer_task_to(
            &mut self,
            destination: &mut Self,
            module: ModuleId,
        ) -> Result<(), Self::Error> {
            if self.fail_transfer {
                return Err(7);
            }
            let source = self
                .owned
                .iter()
                .position(|entry| *entry == Some(module))
                .ok_or(8)?;
            let target = destination
                .owned
                .iter()
                .position(Option::is_none)
                .ok_or(9)?;
            destination.owned[target] = self.owned[source].take();
            Ok(())
        }
    }

    #[test]
    fn placement_admits_within_and_rejects_over_one_core() {
        let mut rt = MulticoreExecutorLifecycle::<2, 4>::new();
        rt.place(0, m(1), 6_000).unwrap();
        rt.place(0, m(2), 4_000).unwrap(); // exactly 100% on core 0
        assert_eq!(rt.core_utilization(0), Some(10_000));
        assert_eq!(
            rt.place(0, m(3), 1),
            Err(MulticoreError::CoreOverloaded {
                core: 0,
                would_be: 10_001,
                limit: 10_000
            })
        );
        assert_eq!(
            rt.place(1, m(1), 100),
            Err(MulticoreError::DuplicateModule { module: m(1) })
        );
        assert_eq!(
            rt.place(2, m(9), 100),
            Err(MulticoreError::UnknownCore { core: 2 })
        );
    }

    #[test]
    fn bootstrap_is_pinned_while_applications_may_use_either_capable_core() {
        let mut rt = MulticoreExecutorLifecycle::<2, 3>::new();
        assert_eq!(rt.core_role(0), Some(CoreRole::PrimaryBootstrap));
        assert_eq!(rt.core_role(1), Some(CoreRole::Application));
        rt.place(0, ModuleId::Kernel, 500).unwrap();
        rt.place(0, m(1), 1_000).unwrap();
        rt.place(1, m(2), 1_000).unwrap();
        assert_eq!(
            rt.place(1, ModuleId::Kernel, 500),
            Err(MulticoreError::BootstrapPinned {
                module: ModuleId::Kernel,
                core: 1,
            })
        );
        rt.start_all(|_| true, |_| {}).unwrap();
        assert_eq!(
            rt.transfer(ModuleId::Kernel, 0, 1),
            Err(MulticoreError::BootstrapPinned {
                module: ModuleId::Kernel,
                core: 1,
            })
        );
        assert!(rt.owns(0, ModuleId::Kernel));
        assert!(rt.owns(0, m(1)) && rt.owns(1, m(2)));
    }

    #[test]
    fn exact_port_can_reserve_a_core_from_application_placement() {
        let mut rt = MulticoreExecutorLifecycle::<2, 2>::new();
        rt.set_application_capable(1, false).unwrap();
        assert_eq!(
            rt.place(1, m(1), 100),
            Err(MulticoreError::CoreNotApplicationCapable { core: 1 })
        );
        rt.place(0, m(1), 100).unwrap();
        rt.start_all(|_| true, |_| {}).unwrap();
        assert_eq!(
            rt.set_application_capable(1, true),
            Err(MulticoreError::WrongCoreState {
                core: 1,
                state: CoreExecutorState::Up,
            })
        );
    }

    #[test]
    fn core_generation_advances_on_start_and_recovery_without_wrap() {
        let mut rt = MulticoreExecutorLifecycle::<2, 1>::new();
        assert_eq!(rt.generation(0), Some(CoreGeneration::NEVER_STARTED));
        rt.start_all(|_| true, |_| {}).unwrap();
        assert_eq!(rt.generation(0).unwrap().get(), 1);
        let owner = rt.ownership(m(1));
        assert_eq!(owner, None);
        rt.fault(0).unwrap();
        rt.recover(0, |_| true).unwrap();
        assert_eq!(rt.generation(0).unwrap().get(), 2);
    }

    #[test]
    fn exhausted_core_generation_rejects_start_before_any_callback() {
        let mut rt = MulticoreExecutorLifecycle::<2, 1>::new();
        rt.generations[1] = CoreGeneration(u32::MAX);
        let mut starts = 0;
        assert_eq!(
            rt.start_all(
                |_| {
                    starts += 1;
                    true
                },
                |_| {}
            ),
            Err(MulticoreError::GenerationExhausted { core: 1 })
        );
        assert_eq!(starts, 0);
        assert!(rt.all_down());
    }

    #[test]
    fn startup_is_ordered_and_rolls_back_transactionally_on_failure() {
        let mut rt = MulticoreExecutorLifecycle::<3, 2>::new();

        // Happy path: all three come up in ascending order.
        let mut started = [false; 3];
        rt.start_all(
            |c| {
                started[c as usize] = true;
                true
            },
            |_| {},
        )
        .unwrap();
        assert!(rt.all_up() && started == [true, true, true]);
        rt.shutdown_all(|_| {});
        assert!(rt.all_down());

        // Failure at core 2 must stop cores 1 then 0 (reverse) and leave all down.
        // Interior mutability lets both callbacks record into one shared trace.
        use core::cell::{Cell, RefCell};
        let order = RefCell::new([0u8; 8]);
        let n = Cell::new(0usize);
        let err = rt
            .start_all(
                |c| {
                    let i = n.get();
                    order.borrow_mut()[i] = c;
                    n.set(i + 1);
                    c != 2 // core 2 fails to start
                },
                |c| {
                    let i = n.get();
                    order.borrow_mut()[i] = 100 + c; // encode stops as 100 + core
                    n.set(i + 1);
                },
            )
            .unwrap_err();
        assert_eq!(err, MulticoreError::StartFailed { core: 2 });
        assert!(rt.all_down());
        // started 0,1,2 (2 failed) then stopped 1,0 in reverse.
        assert_eq!(&order.borrow()[..n.get()], &[0, 1, 2, 101, 100]);
    }

    #[test]
    fn startup_reentry_is_rejected_before_callbacks_run() {
        let mut rt = MulticoreExecutorLifecycle::<2, 1>::new();
        rt.start_all(|_| true, |_| {}).unwrap();

        let mut starts = 0;
        let mut stops = 0;
        assert_eq!(
            rt.start_all(
                |_| {
                    starts += 1;
                    true
                },
                |_| stops += 1,
            ),
            Err(MulticoreError::WrongCoreState {
                core: 0,
                state: CoreExecutorState::Up
            })
        );
        assert_eq!((starts, stops), (0, 0));
        assert!(rt.all_up());
    }

    #[test]
    fn ownership_transfer_preserves_total_utilization_and_rejects_overload() {
        let mut rt = MulticoreExecutorLifecycle::<2, 4>::new();
        rt.place(0, m(1), 5_000).unwrap();
        rt.place(0, m(2), 3_000).unwrap();
        rt.place(1, m(3), 4_000).unwrap();
        let total = rt.total_utilization();
        assert_eq!(total, 12_000);
        rt.start_all(|_| true, |_| {}).unwrap();

        // Move m(2) (3000) from core 0 to core 1: 4000 -> 7000, total preserved.
        rt.transfer(m(2), 0, 1).unwrap();
        assert_eq!(rt.core_utilization(0), Some(5_000));
        assert_eq!(rt.core_utilization(1), Some(7_000));
        assert_eq!(rt.total_utilization(), total);
        assert!(rt.owns(1, m(2)) && !rt.owns(0, m(2)));

        // Same-core transfer is an idempotent no-op, not a second utilization
        // charge or a duplicate ownership operation.
        rt.transfer(m(2), 1, 1).unwrap();
        assert_eq!(rt.core_utilization(1), Some(7_000));
        assert_eq!(rt.total_utilization(), total);
        assert!(rt.owns(1, m(2)));

        // m(1) is 5000; core 1 at 7000 would hit 12000 > 100% -> reject, unchanged.
        assert_eq!(
            rt.transfer(m(1), 0, 1),
            Err(MulticoreError::CoreOverloaded {
                core: 1,
                would_be: 12_000,
                limit: 10_000
            })
        );
        assert_eq!(rt.core_utilization(0), Some(5_000));
        assert_eq!(rt.core_utilization(1), Some(7_000));
        assert!(rt.owns(0, m(1)));

        // Transferring a module the source does not own fails cleanly.
        assert_eq!(
            rt.transfer(m(9), 0, 1),
            Err(MulticoreError::ModuleNotOnCore {
                core: 0,
                module: m(9)
            })
        );
    }

    #[test]
    fn executor_failure_does_not_commit_ownership_metadata() {
        let module = m(4);
        let mut rt = MulticoreExecutorLifecycle::<2, 2>::new();
        rt.place(0, module, 2_500).unwrap();
        rt.start_all(|_| true, |_| {}).unwrap();
        let mut source = ModelExecutor {
            owned: [Some(module), None],
            fail_transfer: true,
        };
        let mut destination = ModelExecutor::default();

        assert_eq!(
            rt.transfer_executor(module, 0, 1, &mut source, &mut destination),
            Err(MulticoreTransferError::Executor(7))
        );
        assert!(rt.owns(0, module));
        assert!(!rt.owns(1, module));
        assert_eq!(rt.core_utilization(0), Some(2_500));
        assert_eq!(rt.core_utilization(1), Some(0));
        assert_eq!(source.owned, [Some(module), None]);
        assert_eq!(destination.owned, [None, None]);

        source.fail_transfer = false;
        rt.transfer_executor(module, 0, 1, &mut source, &mut destination)
            .unwrap();
        assert!(!rt.owns(0, module));
        assert!(rt.owns(1, module));
        assert_eq!(source.owned, [None, None]);
        assert_eq!(destination.owned, [Some(module), None]);
    }

    #[test]
    fn fault_and_recovery_are_isolated_to_the_faulted_core() {
        let mut rt = MulticoreExecutorLifecycle::<2, 2>::new();
        rt.place(0, m(1), 5_000).unwrap();
        rt.place(1, m(2), 5_000).unwrap();
        rt.start_all(|_| true, |_| {}).unwrap();

        rt.fault(0).unwrap();
        assert_eq!(rt.state(0), Some(CoreExecutorState::Faulted));
        assert_eq!(rt.state(1), Some(CoreExecutorState::Up)); // peer untouched

        // A failed restart keeps the core faulted.
        assert_eq!(
            rt.recover(0, |_| false),
            Err(MulticoreError::StartFailed { core: 0 })
        );
        assert_eq!(rt.state(0), Some(CoreExecutorState::Faulted));

        // A successful restart returns it Up with its placement/accounting intact.
        rt.recover(0, |_| true).unwrap();
        assert_eq!(rt.state(0), Some(CoreExecutorState::Up));
        assert_eq!(rt.core_utilization(0), Some(5_000));
        assert!(rt.owns(0, m(1)));

        // Transfer requires both cores Up: a faulted destination is rejected.
        rt.fault(1).unwrap();
        assert_eq!(
            rt.transfer(m(1), 0, 1),
            Err(MulticoreError::WrongCoreState {
                core: 1,
                state: CoreExecutorState::Faulted
            })
        );
    }

    #[test]
    fn full_lifecycle_sequence_preserves_accounting_and_ownership() {
        // End-to-end: place -> start -> transfer -> fault -> recover -> transfer
        // back -> shutdown. Total utilization is invariant throughout, ownership
        // follows the transfers exactly, and each executor action fires once in
        // the right order.
        let mut rt = MulticoreExecutorLifecycle::<2, 3>::new();
        rt.place(0, m(1), 3_000).unwrap();
        rt.place(0, m(2), 2_000).unwrap();
        rt.place(1, m(3), 4_000).unwrap();
        let total = rt.total_utilization();
        assert_eq!(total, 9_000);

        let mut starts = 0u32;
        rt.start_all(
            |_| {
                starts += 1;
                true
            },
            |_| {},
        )
        .unwrap();
        assert!(rt.all_up() && starts == 2);

        // Move m(2) to core 1, then confirm ownership + preserved total.
        rt.transfer(m(2), 0, 1).unwrap();
        assert!(rt.owns(1, m(2)) && !rt.owns(0, m(2)));
        assert_eq!(rt.core_utilization(0), Some(3_000));
        assert_eq!(rt.core_utilization(1), Some(6_000));
        assert_eq!(rt.total_utilization(), total);

        // Core 1 faults and recovers; its accounting (now incl. m(2)) is intact.
        rt.fault(1).unwrap();
        assert_eq!(rt.state(1), Some(CoreExecutorState::Faulted));
        assert_eq!(rt.state(0), Some(CoreExecutorState::Up)); // peer unaffected
        let mut restarts = 0u32;
        rt.recover(1, |_| {
            restarts += 1;
            true
        })
        .unwrap();
        assert!(rt.all_up() && restarts == 1);
        assert_eq!(rt.core_utilization(1), Some(6_000));
        assert!(rt.owns(1, m(2)) && rt.owns(1, m(3)));

        // Move m(2) back; totals still invariant.
        rt.transfer(m(2), 1, 0).unwrap();
        assert_eq!(rt.core_utilization(0), Some(5_000));
        assert_eq!(rt.core_utilization(1), Some(4_000));
        assert_eq!(rt.total_utilization(), total);

        // Ordered shutdown stops both cores (descending) and leaves all down.
        let mut stop_order = [0u8; 2];
        let mut n = 0usize;
        rt.shutdown_all(|c| {
            stop_order[n] = c;
            n += 1;
        });
        assert!(rt.all_down());
        assert_eq!(&stop_order[..n], &[1, 0]);
    }
}
