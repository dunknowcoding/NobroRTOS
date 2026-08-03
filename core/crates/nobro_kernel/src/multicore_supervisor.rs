//! Bounded cross-core supervision commands layered over executor placement.
//!
//! Commands carry the exact core generation and, for module operations, the
//! exact placement owner. A release hook runs after the slot write but before
//! publication, and an acquire hook runs before the slot read, so ports can bind the queue to their cache and
//! memory-ordering mechanism. The queue itself is fixed-capacity and never
//! allocates.

use crate::{
    CoreExecutorState, CoreGeneration, CoreOwnership, ModuleId, MulticoreError,
    MulticoreExecutorLifecycle,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulticoreControlAction {
    CancelModule,
    RestartModule,
    FaultTimeout,
    RecoverCore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MulticoreControlCommand {
    pub sequence: u32,
    pub core: u8,
    pub generation: CoreGeneration,
    pub module: Option<ModuleId>,
    pub action: MulticoreControlAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulticoreControlOutcome {
    ModuleCancelled,
    ModuleRestarted,
    CoreFaulted,
    CoreRecovered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MulticoreControlReceipt {
    pub sequence: u32,
    pub core: u8,
    pub generation_before: CoreGeneration,
    pub generation_after: CoreGeneration,
    pub module: Option<ModuleId>,
    pub outcome: MulticoreControlOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulticoreQueueError {
    Coordination(MulticoreError),
    Full,
    SequenceExhausted,
    InvalidModuleScope,
    StaleGeneration {
        core: u8,
        expected: CoreGeneration,
        actual: CoreGeneration,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MulticoreProcessError<E> {
    Coordination(MulticoreError),
    InvalidModuleScope,
    StaleGeneration {
        core: u8,
        expected: CoreGeneration,
        actual: CoreGeneration,
    },
    OwnershipChanged {
        expected: CoreOwnership,
        actual: Option<CoreOwnership>,
    },
    PeripheralOwnershipRejected(CoreOwnership),
    Executor(E),
}

/// Port hook that binds queue publication/consumption to the exact cache and
/// memory-ordering mechanism. On cache-coherent MCUs these may be release and
/// acquire fences; on non-coherent MCUs they may additionally clean/invalidate
/// the command storage.
pub trait MulticoreCoherency {
    /// Called after command bytes are written and before queue publication.
    fn release_after_write(&mut self, sequence: u32);
    /// Called before the consumer reads the current head slot.
    fn acquire_before_read(&mut self, head: usize);
}

/// Operations performed by the executor/port after a command passes all
/// generation, placement, and peripheral-ownership checks.
pub trait MulticoreControlExecutor {
    type Error;

    fn cancel_module(&mut self, core: u8, module: ModuleId) -> Result<(), Self::Error>;
    fn restart_module(&mut self, core: u8, module: ModuleId) -> Result<(), Self::Error>;
    fn restart_core(&mut self, core: u8) -> Result<(), Self::Error>;
}

/// Checked peripheral-owner witness. A module command is not executed merely
/// because placement metadata matches: the port must also confirm that every
/// peripheral lease expected by that application belongs to this exact
/// core/generation or that the module owns no such lease.
pub trait MulticorePeripheralOwnership {
    fn verify(&mut self, ownership: CoreOwnership) -> bool;
}

/// Fixed-capacity FIFO of generation-tagged supervision commands.
pub struct MulticoreControlQueue<const N: usize> {
    slots: [Option<MulticoreControlCommand>; N],
    head: usize,
    len: usize,
    next_sequence: u32,
}

impl<const N: usize> Default for MulticoreControlQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> MulticoreControlQueue<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            head: 0,
            len: 0,
            next_sequence: 1,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn validate_scope<const CORES: usize, const SLOTS: usize>(
        lifecycle: &MulticoreExecutorLifecycle<CORES, SLOTS>,
        core: u8,
        generation: CoreGeneration,
        module: Option<ModuleId>,
        action: MulticoreControlAction,
    ) -> Result<(), MulticoreQueueError> {
        let state = lifecycle
            .state(core)
            .ok_or(MulticoreQueueError::Coordination(
                MulticoreError::UnknownCore { core },
            ))?;
        let actual = lifecycle
            .generation(core)
            .ok_or(MulticoreQueueError::Coordination(
                MulticoreError::UnknownCore { core },
            ))?;
        if generation != actual {
            return Err(MulticoreQueueError::StaleGeneration {
                core,
                expected: generation,
                actual,
            });
        }
        let required_state = match action {
            MulticoreControlAction::RecoverCore => CoreExecutorState::Faulted,
            _ => CoreExecutorState::Up,
        };
        if state != required_state {
            return Err(MulticoreQueueError::Coordination(
                MulticoreError::WrongCoreState { core, state },
            ));
        }
        match action {
            MulticoreControlAction::CancelModule | MulticoreControlAction::RestartModule => {
                let Some(module) = module else {
                    return Err(MulticoreQueueError::InvalidModuleScope);
                };
                if module == ModuleId::Kernel {
                    return Err(MulticoreQueueError::Coordination(
                        MulticoreError::BootstrapPinned { module, core },
                    ));
                }
                let actual_owner = lifecycle.ownership(module);
                if actual_owner
                    != Some(CoreOwnership {
                        core,
                        generation,
                        module,
                    })
                {
                    return Err(MulticoreQueueError::Coordination(
                        MulticoreError::ModuleNotOnCore { core, module },
                    ));
                }
            }
            MulticoreControlAction::FaultTimeout | MulticoreControlAction::RecoverCore => {
                if module.is_some() {
                    return Err(MulticoreQueueError::InvalidModuleScope);
                }
            }
        }
        Ok(())
    }

    pub fn enqueue<const CORES: usize, const SLOTS: usize>(
        &mut self,
        lifecycle: &MulticoreExecutorLifecycle<CORES, SLOTS>,
        core: u8,
        generation: CoreGeneration,
        module: Option<ModuleId>,
        action: MulticoreControlAction,
        coherency: &mut impl MulticoreCoherency,
    ) -> Result<u32, MulticoreQueueError> {
        Self::validate_scope(lifecycle, core, generation, module, action)?;
        if self.len == N {
            return Err(MulticoreQueueError::Full);
        }
        if self.next_sequence == u32::MAX {
            return Err(MulticoreQueueError::SequenceExhausted);
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let tail = (self.head + self.len) % N;
        let command = MulticoreControlCommand {
            sequence,
            core,
            generation,
            module,
            action,
        };
        self.slots[tail] = Some(command);
        // The command bytes exist before the port performs its release/cache
        // action; increasing `len` is the logical publication point.
        coherency.release_after_write(sequence);
        self.len += 1;
        Ok(sequence)
    }

    fn pop(&mut self, coherency: &mut impl MulticoreCoherency) -> Option<MulticoreControlCommand> {
        if self.len == 0 {
            return None;
        }
        coherency.acquire_before_read(self.head);
        let command = self.slots[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        command
    }

    pub fn process_one<const CORES: usize, const SLOTS: usize, E>(
        &mut self,
        lifecycle: &mut MulticoreExecutorLifecycle<CORES, SLOTS>,
        coherency: &mut impl MulticoreCoherency,
        peripheral_ownership: &mut impl MulticorePeripheralOwnership,
        executor: &mut impl MulticoreControlExecutor<Error = E>,
    ) -> Result<Option<MulticoreControlReceipt>, MulticoreProcessError<E>> {
        let Some(command) = self.pop(coherency) else {
            return Ok(None);
        };

        let actual_generation =
            lifecycle
                .generation(command.core)
                .ok_or(MulticoreProcessError::Coordination(
                    MulticoreError::UnknownCore { core: command.core },
                ))?;
        if actual_generation != command.generation {
            return Err(MulticoreProcessError::StaleGeneration {
                core: command.core,
                expected: command.generation,
                actual: actual_generation,
            });
        }

        let before = actual_generation;
        let outcome = match command.action {
            MulticoreControlAction::CancelModule | MulticoreControlAction::RestartModule => {
                let module = command
                    .module
                    .ok_or(MulticoreProcessError::InvalidModuleScope)?;
                let expected = CoreOwnership {
                    core: command.core,
                    generation: command.generation,
                    module,
                };
                let actual = lifecycle.ownership(module);
                if actual != Some(expected) {
                    return Err(MulticoreProcessError::OwnershipChanged { expected, actual });
                }
                if !peripheral_ownership.verify(expected) {
                    return Err(MulticoreProcessError::PeripheralOwnershipRejected(expected));
                }
                match command.action {
                    MulticoreControlAction::CancelModule => {
                        executor
                            .cancel_module(command.core, module)
                            .map_err(MulticoreProcessError::Executor)?;
                        MulticoreControlOutcome::ModuleCancelled
                    }
                    MulticoreControlAction::RestartModule => {
                        executor
                            .restart_module(command.core, module)
                            .map_err(MulticoreProcessError::Executor)?;
                        MulticoreControlOutcome::ModuleRestarted
                    }
                    _ => unreachable!(),
                }
            }
            MulticoreControlAction::FaultTimeout => {
                lifecycle
                    .fault(command.core)
                    .map_err(MulticoreProcessError::Coordination)?;
                MulticoreControlOutcome::CoreFaulted
            }
            MulticoreControlAction::RecoverCore => {
                let mut executor_error = None;
                let recovery =
                    lifecycle.recover(command.core, |core| match executor.restart_core(core) {
                        Ok(()) => true,
                        Err(error) => {
                            executor_error = Some(error);
                            false
                        }
                    });
                if let Some(error) = executor_error {
                    return Err(MulticoreProcessError::Executor(error));
                }
                recovery.map_err(MulticoreProcessError::Coordination)?;
                MulticoreControlOutcome::CoreRecovered
            }
        };
        let after = lifecycle.generation(command.core).unwrap_or(before);
        Ok(Some(MulticoreControlReceipt {
            sequence: command.sequence,
            core: command.core,
            generation_before: before,
            generation_after: after,
            module: command.module,
            outcome,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Trace {
        events: [u32; 16],
        len: usize,
    }

    impl Trace {
        fn push(&mut self, value: u32) {
            self.events[self.len] = value;
            self.len += 1;
        }
    }

    struct Fence<'a>(&'a core::cell::RefCell<Trace>);

    impl MulticoreCoherency for Fence<'_> {
        fn release_after_write(&mut self, sequence: u32) {
            self.0.borrow_mut().push(100 + sequence);
        }

        fn acquire_before_read(&mut self, head: usize) {
            self.0.borrow_mut().push(200 + head as u32);
        }
    }

    struct Owner<'a> {
        trace: &'a core::cell::RefCell<Trace>,
        accept: bool,
    }

    impl MulticorePeripheralOwnership for Owner<'_> {
        fn verify(&mut self, ownership: CoreOwnership) -> bool {
            self.trace
                .borrow_mut()
                .push(300 + u32::from(ownership.core));
            self.accept
        }
    }

    struct Exec<'a> {
        trace: &'a core::cell::RefCell<Trace>,
        fail: bool,
    }

    impl MulticoreControlExecutor for Exec<'_> {
        type Error = u8;

        fn cancel_module(&mut self, core: u8, _module: ModuleId) -> Result<(), Self::Error> {
            self.trace.borrow_mut().push(400 + u32::from(core));
            if self.fail {
                Err(1)
            } else {
                Ok(())
            }
        }

        fn restart_module(&mut self, core: u8, _module: ModuleId) -> Result<(), Self::Error> {
            self.trace.borrow_mut().push(500 + u32::from(core));
            if self.fail {
                Err(2)
            } else {
                Ok(())
            }
        }

        fn restart_core(&mut self, core: u8) -> Result<(), Self::Error> {
            self.trace.borrow_mut().push(600 + u32::from(core));
            if self.fail {
                Err(3)
            } else {
                Ok(())
            }
        }
    }

    fn lifecycle() -> MulticoreExecutorLifecycle<2, 2> {
        let mut lifecycle = MulticoreExecutorLifecycle::new();
        lifecycle.place(0, ModuleId::Kernel, 500).unwrap();
        lifecycle.place(0, ModuleId::App(1), 1_000).unwrap();
        lifecycle.place(1, ModuleId::App(2), 1_000).unwrap();
        lifecycle.start_all(|_| true, |_| {}).unwrap();
        lifecycle
    }

    #[test]
    fn queue_saturation_is_typed_and_release_precedes_acquire_and_execute() {
        let mut lifecycle = lifecycle();
        let trace = core::cell::RefCell::new(Trace::default());
        let mut fence = Fence(&trace);
        let mut owner = Owner {
            trace: &trace,
            accept: true,
        };
        let mut executor = Exec {
            trace: &trace,
            fail: false,
        };
        let mut queue = MulticoreControlQueue::<1>::new();
        let generation = lifecycle.generation(1).unwrap();
        assert_eq!(
            queue.enqueue(
                &lifecycle,
                1,
                generation,
                Some(ModuleId::App(2)),
                MulticoreControlAction::CancelModule,
                &mut fence,
            ),
            Ok(1)
        );
        assert_eq!(
            queue.enqueue(
                &lifecycle,
                1,
                generation,
                Some(ModuleId::App(2)),
                MulticoreControlAction::RestartModule,
                &mut fence,
            ),
            Err(MulticoreQueueError::Full)
        );
        let receipt = queue
            .process_one(&mut lifecycle, &mut fence, &mut owner, &mut executor)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.outcome, MulticoreControlOutcome::ModuleCancelled);
        assert_eq!(
            &trace.borrow().events[..trace.borrow().len],
            &[101, 200, 301, 401]
        );
    }

    #[test]
    fn exhausted_command_sequence_fails_closed_without_publication() {
        let lifecycle = lifecycle();
        let trace = core::cell::RefCell::new(Trace::default());
        let mut fence = Fence(&trace);
        let mut queue = MulticoreControlQueue::<1>::new();
        queue.next_sequence = u32::MAX;
        assert_eq!(
            queue.enqueue(
                &lifecycle,
                1,
                lifecycle.generation(1).unwrap(),
                None,
                MulticoreControlAction::FaultTimeout,
                &mut fence,
            ),
            Err(MulticoreQueueError::SequenceExhausted)
        );
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn transfer_or_recovery_invalidates_stale_commands() {
        let mut rt = lifecycle();
        let trace = core::cell::RefCell::new(Trace::default());
        let mut fence = Fence(&trace);
        let mut owner = Owner {
            trace: &trace,
            accept: true,
        };
        let mut executor = Exec {
            trace: &trace,
            fail: false,
        };
        let mut queue = MulticoreControlQueue::<2>::new();
        let old = rt.generation(1).unwrap();
        queue
            .enqueue(
                &rt,
                1,
                old,
                Some(ModuleId::App(2)),
                MulticoreControlAction::RestartModule,
                &mut fence,
            )
            .unwrap();
        rt.fault(1).unwrap();
        rt.recover(1, |_| true).unwrap();
        assert_eq!(
            queue.process_one(&mut rt, &mut fence, &mut owner, &mut executor),
            Err(MulticoreProcessError::StaleGeneration {
                core: 1,
                expected: old,
                actual: rt.generation(1).unwrap(),
            })
        );

        let mut transferred = lifecycle();
        let mut queue = MulticoreControlQueue::<1>::new();
        let source_generation = transferred.generation(0).unwrap();
        queue
            .enqueue(
                &transferred,
                0,
                source_generation,
                Some(ModuleId::App(1)),
                MulticoreControlAction::RestartModule,
                &mut fence,
            )
            .unwrap();
        transferred.transfer(ModuleId::App(1), 0, 1).unwrap();
        assert_eq!(
            queue.process_one(&mut transferred, &mut fence, &mut owner, &mut executor),
            Err(MulticoreProcessError::OwnershipChanged {
                expected: CoreOwnership {
                    core: 0,
                    generation: source_generation,
                    module: ModuleId::App(1),
                },
                actual: transferred.ownership(ModuleId::App(1)),
            })
        );
    }

    #[test]
    fn remote_timeout_and_recovery_bump_generation_without_losing_peer() {
        let mut lifecycle = lifecycle();
        let trace = core::cell::RefCell::new(Trace::default());
        let mut fence = Fence(&trace);
        let mut owner = Owner {
            trace: &trace,
            accept: true,
        };
        let mut executor = Exec {
            trace: &trace,
            fail: false,
        };
        let mut queue = MulticoreControlQueue::<2>::new();
        let before = lifecycle.generation(1).unwrap();
        queue
            .enqueue(
                &lifecycle,
                1,
                before,
                None,
                MulticoreControlAction::FaultTimeout,
                &mut fence,
            )
            .unwrap();
        queue
            .process_one(&mut lifecycle, &mut fence, &mut owner, &mut executor)
            .unwrap();
        assert_eq!(lifecycle.state(1), Some(CoreExecutorState::Faulted));
        assert_eq!(lifecycle.state(0), Some(CoreExecutorState::Up));
        queue
            .enqueue(
                &lifecycle,
                1,
                before,
                None,
                MulticoreControlAction::RecoverCore,
                &mut fence,
            )
            .unwrap();
        let receipt = queue
            .process_one(&mut lifecycle, &mut fence, &mut owner, &mut executor)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.outcome, MulticoreControlOutcome::CoreRecovered);
        assert_eq!(receipt.generation_before, before);
        assert!(receipt.generation_after.get() > before.get());
        assert!(lifecycle.all_up());
    }

    #[test]
    fn peripheral_owner_rejection_prevents_executor_call() {
        let mut lifecycle = lifecycle();
        let trace = core::cell::RefCell::new(Trace::default());
        let mut fence = Fence(&trace);
        let mut owner = Owner {
            trace: &trace,
            accept: false,
        };
        let mut executor = Exec {
            trace: &trace,
            fail: false,
        };
        let mut queue = MulticoreControlQueue::<1>::new();
        let generation = lifecycle.generation(0).unwrap();
        queue
            .enqueue(
                &lifecycle,
                0,
                generation,
                Some(ModuleId::App(1)),
                MulticoreControlAction::RestartModule,
                &mut fence,
            )
            .unwrap();
        assert_eq!(
            queue.process_one(&mut lifecycle, &mut fence, &mut owner, &mut executor),
            Err(MulticoreProcessError::PeripheralOwnershipRejected(
                CoreOwnership {
                    core: 0,
                    generation,
                    module: ModuleId::App(1),
                }
            ))
        );
        assert_eq!(
            &trace.borrow().events[..trace.borrow().len],
            &[101, 200, 300]
        );
    }

    #[test]
    fn failed_core_restart_stays_faulted_and_preserves_generation() {
        let mut lifecycle = lifecycle();
        lifecycle.fault(1).unwrap();
        let generation = lifecycle.generation(1).unwrap();
        let trace = core::cell::RefCell::new(Trace::default());
        let mut fence = Fence(&trace);
        let mut owner = Owner {
            trace: &trace,
            accept: true,
        };
        let mut executor = Exec {
            trace: &trace,
            fail: true,
        };
        let mut queue = MulticoreControlQueue::<1>::new();
        queue
            .enqueue(
                &lifecycle,
                1,
                generation,
                None,
                MulticoreControlAction::RecoverCore,
                &mut fence,
            )
            .unwrap();
        assert_eq!(
            queue.process_one(&mut lifecycle, &mut fence, &mut owner, &mut executor),
            Err(MulticoreProcessError::Executor(3))
        );
        assert_eq!(lifecycle.state(1), Some(CoreExecutorState::Faulted));
        assert_eq!(lifecycle.generation(1), Some(generation));
        assert_eq!(lifecycle.state(0), Some(CoreExecutorState::Up));
    }
}
