//! Per-core executor lifecycle coordination for multi-executor (dual-core and
//! beyond) runtimes.
//!
//! [`multicore`](crate::multicore) produces the *static* placement -- which
//! module runs on which core and whether each core fits its utilization bound.
//! This module owns the *runtime* residue Wave 88 left open (SCH-09 / MC-01):
//! bringing the independent per-core executors up and down in a defined order,
//! transferring a module's ownership from one core to another with preserved
//! accounting, and giving one core a fault/recovery authority that never
//! strands another core.
//!
//! It is deterministic and allocation-free. The actual executor start/stop is
//! the caller's (a real per-core `KernelExecutor`, or a test double): every
//! lifecycle method takes the action as an `FnMut` callback, so the coordinator
//! is fully host testable without real cores or threads. Startup is
//! transactional -- if any core fails to start, the cores already started are
//! stopped in reverse order and the system returns fully down, never partially
//! up.

use crate::ModuleId;

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
}

const UTIL_LIMIT: u32 = 10_000;

#[derive(Clone, Copy)]
struct Owned {
    module: ModuleId,
    util_permyriad: u32,
}

/// Coordinates `CORES` per-core executors, each retaining up to `SLOTS` owned
/// modules. Storage is fixed (`CORES * SLOTS` module slots); no allocation.
pub struct MulticoreExecutorLifecycle<const CORES: usize, const SLOTS: usize> {
    states: [CoreExecutorState; CORES],
    owned: [[Option<Owned>; SLOTS]; CORES],
    core_util: [u32; CORES],
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
        let mut core = 0usize;
        while core < CORES {
            if start(core as u8) {
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

    /// Move a module's ownership from one live core to another, preserving total
    /// utilization. Both cores must be `Up`, the source must own the module, and
    /// the destination must not overload; otherwise nothing changes (no partial
    /// transfer).
    pub fn transfer(&mut self, module: ModuleId, from: u8, to: u8) -> Result<(), MulticoreError> {
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
        let Some(slot_index) = self.find_on(from_index, module) else {
            return Err(MulticoreError::ModuleNotOnCore { core: from, module });
        };
        let util = self.owned[from_index][slot_index].unwrap().util_permyriad;
        let would_be = self.core_util[to_index].saturating_add(util);
        if would_be > UTIL_LIMIT {
            return Err(MulticoreError::CoreOverloaded {
                core: to,
                would_be,
                limit: UTIL_LIMIT,
            });
        }
        let Some(dest_slot) = self.owned[to_index].iter_mut().find(|s| s.is_none()) else {
            return Err(MulticoreError::CoreFull { core: to });
        };
        *dest_slot = Some(Owned {
            module,
            util_permyriad: util,
        });
        self.owned[from_index][slot_index] = None;
        self.core_util[from_index] -= util;
        self.core_util[to_index] = would_be;
        Ok(())
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
        if restart(core) {
            self.states[index] = CoreExecutorState::Up;
            Ok(())
        } else {
            Err(MulticoreError::StartFailed { core })
        }
    }

    pub fn state(&self, core: u8) -> Option<CoreExecutorState> {
        self.states.get(core as usize).copied()
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
}
