//! Application graph builder (FLEX-01 / UX-01): declare tasks, channels, and
//! dependencies ONCE — by name — and derive every kernel input from that one
//! declaration: manifest, startup order, task metas, and labels.
//!
//! Identity: tasks get **opaque module identities** (`ModuleId::App(n)`,
//! allocated in declaration order) unless a declaration opts into a readable
//! well-known role. Names are labels carried alongside — diagnostics and hosts
//! read names, wire formats keep the stable numeric codes.
//!
//! Typed builders carry safe, reviewable defaults (UX-01):
//! - [`TaskDecl::periodic`] — a driver-criticality periodic task; jitter
//!   defaults to period/100 (minimum 1 µs when the period permits), execution
//!   budget to period/10, memory to 1 KiB flash / 256 B RAM.
//! - [`TaskDecl::control`] — hard-real-time: tighter jitter (period/200,
//!   minimum 1 µs when possible), same budget rule; put actuation here.
//! - [`TaskDecl::service`] — best-effort background work polled at a relaxed
//!   cadence with no deadline contract.
//!
//! Channels derive capabilities where unambiguous: `channel(from, to)` adds
//! the `Mailbox` capability requirement to both endpoints and pins ownership
//! on the kernel module — no duplicate declarations, no guessing beyond that.
//! Everything expands into the SAME low-level contract the kernel admits; the
//! expanded [`SystemManifest`] stays printable/inspectable, and admission
//! still validates it from scratch (the builder is convenience, not a bypass).
//!
//! Every failure is ONE attributed diagnostic naming the task label:
//! duplicates, unknown dependency names, over-capacity, dependency cycles,
//! and manifest conflicts all surface as [`GraphError`] values carrying the
//! offending name instead of a bare index.

use crate::{
    async_rt::{
        admit_reactor_domains, ReactorAdmissionError, ReactorAdmissionPlan, ReactorChannelContract,
        ReactorDomainContract,
    },
    kernel_owned_capabilities, Capability, CapabilitySet, ContainmentPolicy, Criticality,
    DeadlineContract, DependencySet, ExecError, ExecutorInitError, FaultThresholds, KernelExecutor,
    KernelExecutorCell, ManifestError, MemoryBudget, ModuleId, ModuleSpec, ObjectQuota,
    StartupError, StartupNode, StartupPlanner, SystemManifest, SystemProfile, TaskMeta,
};
use core::mem::MaybeUninit;

const MAX_DEPS: usize = 4;

const fn clamp_nonzero(value: u32, upper: u32) -> u32 {
    let value = if value < 1 { 1 } else { value };
    if value > upper {
        upper
    } else {
        value
    }
}

/// One task declaration; construct through the typed builders.
#[derive(Clone, Copy, Debug)]
pub struct TaskDecl {
    pub name: &'static str,
    pub criticality: Criticality,
    /// Offset of the first release from the executor epoch.
    pub phase_us: u32,
    pub period_us: u32,
    /// Relative deadline from each release.
    pub deadline_us: u32,
    pub max_jitter_us: u32,
    pub execution_budget_us: u32,
    pub blocking_us: u32,
    pub memory: MemoryBudget,
    pub objects: ObjectQuota,
    pub requires: CapabilitySet,
    pub owns: CapabilitySet,
    /// Optional readable well-known role instead of an opaque `App(n)` slot.
    pub role: Option<ModuleId>,
    /// Deadline contract participation (services opt out).
    pub has_deadline: bool,
    /// Optional explicit core placement. `None` = let the placement
    /// planner assign a core by balancing utilization (beginner-safe default).
    pub core_affinity: Option<u8>,
    /// Optional async reactor domain driven by this admitted kernel task.
    /// Exactly one graph task must drive each admitted reactor domain.
    pub reactor_domain: Option<u8>,
    after: [Option<&'static str>; MAX_DEPS],
}

impl TaskDecl {
    const EMPTY: Self = Self {
        name: "",
        criticality: Criticality::BestEffort,
        phase_us: 0,
        period_us: 0,
        deadline_us: 0,
        max_jitter_us: 0,
        execution_budget_us: 0,
        blocking_us: 0,
        memory: MemoryBudget::ZERO,
        objects: ObjectQuota::DEFAULT,
        requires: CapabilitySet::empty(),
        owns: CapabilitySet::empty(),
        role: None,
        has_deadline: false,
        core_affinity: None,
        reactor_domain: None,
        after: [None; MAX_DEPS],
    };

    const fn base(name: &'static str, criticality: Criticality, period_us: u32) -> Self {
        let max_jitter_us = if period_us <= 1 {
            0
        } else {
            clamp_nonzero(period_us / 100, period_us - 1)
        };
        let execution_budget_us = if period_us == 0 {
            1
        } else {
            clamp_nonzero(period_us / 10, period_us)
        };
        Self {
            name,
            criticality,
            phase_us: 0,
            period_us,
            deadline_us: period_us,
            max_jitter_us,
            execution_budget_us,
            blocking_us: 0,
            memory: MemoryBudget::new(1024, 256, 0),
            objects: ObjectQuota::DEFAULT,
            requires: CapabilitySet::empty(),
            owns: CapabilitySet::empty(),
            role: None,
            has_deadline: true,
            core_affinity: None,
            reactor_domain: None,
            after: [None; MAX_DEPS],
        }
    }

    /// A periodic worker with safe defaults (driver criticality).
    pub const fn periodic(name: &'static str, period_us: u32) -> Self {
        Self::base(name, Criticality::Driver, period_us)
    }

    /// A hard-real-time control task: tighter default jitter.
    pub const fn control(name: &'static str, period_us: u32) -> Self {
        let mut decl = Self::base(name, Criticality::HardRealtime, period_us);
        decl.max_jitter_us = if period_us <= 1 {
            0
        } else {
            clamp_nonzero(period_us / 200, period_us - 1)
        };
        decl
    }

    /// Best-effort background service polled at `poll_us`; no deadline contract.
    pub const fn service(name: &'static str, poll_us: u32) -> Self {
        let mut decl = Self::base(name, Criticality::BestEffort, poll_us);
        decl.has_deadline = false;
        decl
    }

    // -- explicit overrides (each default stays reviewable) -----------------

    pub const fn criticality(mut self, criticality: Criticality) -> Self {
        self.criticality = criticality;
        self
    }

    pub const fn jitter_us(mut self, max_jitter_us: u32) -> Self {
        self.max_jitter_us = max_jitter_us;
        self
    }

    /// Offset this task's first release to avoid unnecessary release bursts.
    pub const fn phase_us(mut self, phase_us: u32) -> Self {
        self.phase_us = phase_us;
        self
    }

    /// Set a constrained relative deadline. The default is the period.
    pub const fn deadline_us(mut self, deadline_us: u32) -> Self {
        self.deadline_us = deadline_us;
        self
    }

    pub const fn budget_us(mut self, execution_budget_us: u32) -> Self {
        self.execution_budget_us = execution_budget_us;
        self
    }

    /// Measured non-preemptible lower-priority/critical-section blocking bound.
    pub const fn blocking_us(mut self, blocking_us: u32) -> Self {
        self.blocking_us = blocking_us;
        self
    }

    pub const fn memory(mut self, memory: MemoryBudget) -> Self {
        self.memory = memory;
        self
    }

    pub const fn objects(mut self, objects: ObjectQuota) -> Self {
        self.objects = objects;
        self
    }

    pub const fn requires(mut self, capabilities: CapabilitySet) -> Self {
        self.requires = capabilities;
        self
    }

    pub const fn owns(mut self, capabilities: CapabilitySet) -> Self {
        self.owns = capabilities;
        self
    }

    /// Use a readable well-known role instead of an opaque slot.
    pub const fn role(mut self, role: ModuleId) -> Self {
        self.role = Some(role);
        self
    }

    /// Pin this task to a specific core. Omit to let the placement
    /// planner balance it automatically.
    pub const fn core(mut self, core: u8) -> Self {
        self.core_affinity = Some(core);
        self
    }

    /// Mark this admitted task as the driver for one async reactor domain.
    /// Futures in that domain still live inside the bounded reactor; this
    /// label connects the domain to manifest/admission budgets by name.
    pub const fn reactor_domain(mut self, domain: u8) -> Self {
        self.reactor_domain = Some(domain);
        self
    }

    /// Start this task after `name` (startup ordering, by label).
    pub const fn after(mut self, name: &'static str) -> Self {
        let mut index = 0;
        while index < MAX_DEPS {
            if self.after[index].is_none() {
                self.after[index] = Some(name);
                return self;
            }
            index += 1;
        }
        // Too many edges is reported at build() with the task's name.
        self.after[MAX_DEPS - 1] = Some("\0too-many");
        self
    }
}

/// One named graph channel. Keep these in a `const` slice with task
/// declarations when startup stack must stay independent of graph capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelDecl {
    pub from: &'static str,
    pub to: &'static str,
}

impl ChannelDecl {
    const EMPTY: Self = Self { from: "", to: "" };

    pub const fn new(from: &'static str, to: &'static str) -> Self {
        Self { from, to }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    DuplicateName(&'static str),
    UnknownDependency {
        task: &'static str,
        depends_on: &'static str,
    },
    TooManyTasks {
        capacity: usize,
    },
    TooManyDependencies {
        task: &'static str,
    },
    DuplicateRole {
        task: &'static str,
    },
    /// The startup order contains a cycle reachable from this task.
    Cycle {
        task: &'static str,
    },
    ChannelEndpointUnknown {
        endpoint: &'static str,
    },
    TooManyChannels,
    InvalidBlocking {
        task: &'static str,
        budget_us: u32,
        blocking_us: u32,
        period_us: u32,
    },
    /// The derived manifest failed validation; the offending task is named.
    Manifest {
        task: &'static str,
        error: ManifestError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphStartError {
    Graph(GraphError),
    Executor(ExecutorInitError),
    Execution(ExecError),
}

impl From<GraphError> for GraphStartError {
    fn from(error: GraphError) -> Self {
        Self::Graph(error)
    }
}

impl From<ExecutorInitError> for GraphStartError {
    fn from(error: ExecutorInitError) -> Self {
        Self::Executor(error)
    }
}

impl From<ExecError> for GraphStartError {
    fn from(error: ExecError) -> Self {
        Self::Execution(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorTaskBinding {
    pub task: &'static str,
    pub module: ModuleId,
    pub domain: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphReactorError {
    ReactorAdmission(ReactorAdmissionError),
    UnknownDomain {
        task: &'static str,
        domain: u8,
    },
    DuplicateDomainDriver {
        domain: u8,
        first_task: &'static str,
        duplicate_task: &'static str,
    },
    MissingDomainDriver {
        domain: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphReactorAdmission<const TASKS: usize, const DOMAINS: usize, const CHANNELS: usize> {
    pub reactor: ReactorAdmissionPlan<DOMAINS, CHANNELS>,
    pub bindings: [Option<ReactorTaskBinding>; TASKS],
    pub binding_len: usize,
}

/// Everything the kernel needs, derived from one declaration set.
pub struct BuiltGraph<const MODULES: usize, const TASKS: usize> {
    pub manifest: SystemManifest<MODULES>,
    pub startup: [StartupNode; MODULES],
    pub startup_len: usize,
    pub tasks: [Option<TaskMeta>; TASKS],
    pub task_len: usize,
    labels: [Option<(&'static str, ModuleId)>; TASKS],
    reactor_bindings: [Option<ReactorTaskBinding>; TASKS],
    reactor_binding_len: usize,
}

impl<const MODULES: usize, const TASKS: usize> BuiltGraph<MODULES, TASKS> {
    pub fn startup_nodes(&self) -> &[StartupNode] {
        &self.startup[..self.startup_len]
    }

    /// The opaque identity allocated for a task label.
    pub fn module_of(&self, name: &str) -> Option<ModuleId> {
        self.labels
            .iter()
            .flatten()
            .find(|(label, _)| *label == name)
            .map(|(_, module)| *module)
    }

    /// The readable label for an identity (diagnostics, host output).
    pub fn label_of(&self, module: ModuleId) -> Option<&'static str> {
        self.labels
            .iter()
            .flatten()
            .find(|(_, owner)| *owner == module)
            .map(|(label, _)| *label)
    }

    pub fn reactor_bindings(&self) -> impl Iterator<Item = &ReactorTaskBinding> + '_ {
        self.reactor_bindings.iter().flatten()
    }

    pub fn reactor_domain_of(&self, task: &str) -> Option<u8> {
        self.reactor_bindings()
            .find(|binding| binding.task == task)
            .map(|binding| binding.domain)
    }

    /// Admit reactor-domain contracts and prove every domain is driven by
    /// exactly one already-admitted graph task. This links the async executor
    /// plan back to the manifest/startup graph before runtime wiring.
    pub fn admit_reactor_domains<const DOMAINS: usize, const CHANNELS: usize>(
        &self,
        domains: [Option<ReactorDomainContract>; DOMAINS],
        channels: [Option<ReactorChannelContract>; CHANNELS],
    ) -> Result<GraphReactorAdmission<TASKS, DOMAINS, CHANNELS>, GraphReactorError> {
        let reactor = admit_reactor_domains(domains, channels)
            .map_err(GraphReactorError::ReactorAdmission)?;
        self.link_reactor_plan(reactor)
    }

    /// Link a pre-admitted reactor-domain plan to this graph.
    pub fn link_reactor_plan<const DOMAINS: usize, const CHANNELS: usize>(
        &self,
        reactor: ReactorAdmissionPlan<DOMAINS, CHANNELS>,
    ) -> Result<GraphReactorAdmission<TASKS, DOMAINS, CHANNELS>, GraphReactorError> {
        for binding in self.reactor_bindings() {
            if reactor.domain(binding.domain).is_none() {
                return Err(GraphReactorError::UnknownDomain {
                    task: binding.task,
                    domain: binding.domain,
                });
            }
        }
        for (index, binding) in self.reactor_bindings().enumerate() {
            for previous in self.reactor_bindings().take(index) {
                if previous.domain == binding.domain {
                    return Err(GraphReactorError::DuplicateDomainDriver {
                        domain: binding.domain,
                        first_task: previous.task,
                        duplicate_task: binding.task,
                    });
                }
            }
        }
        for domain in reactor.domains() {
            if !self
                .reactor_bindings()
                .any(|binding| binding.domain == domain.id)
            {
                return Err(GraphReactorError::MissingDomainDriver { domain: domain.id });
            }
        }
        Ok(GraphReactorAdmission {
            reactor,
            bindings: self.reactor_bindings,
            binding_len: self.reactor_binding_len,
        })
    }
}

pub struct AppGraph<const TASKS: usize> {
    tasks: [TaskDecl; TASKS],
    len: usize,
    channels: [ChannelDecl; TASKS],
    channel_len: usize,
    kernel_memory: MemoryBudget,
    kernel_deadline: DeadlineContract,
}

impl<const TASKS: usize> AppGraph<TASKS> {
    pub fn new() -> Self {
        Self {
            tasks: [TaskDecl::EMPTY; TASKS],
            len: 0,
            channels: [ChannelDecl::EMPTY; TASKS],
            channel_len: 0,
            kernel_memory: MemoryBudget::new(8 * 1024, 2 * 1024, 1),
            kernel_deadline: DeadlineContract::new(20_000, 100),
        }
    }

    pub fn kernel(mut self, memory: MemoryBudget, deadline: DeadlineContract) -> Self {
        self.kernel_memory = memory;
        self.kernel_deadline = deadline;
        self
    }

    /// The declared tasks (for placement planning and inspection).
    pub fn task_decls(&self) -> impl Iterator<Item = &TaskDecl> + '_ {
        self.tasks[..self.len].iter()
    }

    /// The declared channels as `(from, to)` label pairs.
    pub fn channel_pairs(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        self.channels[..self.channel_len]
            .iter()
            .map(|channel| (channel.from, channel.to))
    }

    pub fn task(mut self, decl: TaskDecl) -> Result<Self, GraphError> {
        if self.tasks[..self.len]
            .iter()
            .any(|existing| existing.name == decl.name)
        {
            return Err(GraphError::DuplicateName(decl.name));
        }
        if decl.after.iter().flatten().any(|dep| *dep == "\0too-many") {
            return Err(GraphError::TooManyDependencies { task: decl.name });
        }
        if self.len == TASKS {
            return Err(GraphError::TooManyTasks { capacity: TASKS });
        }
        self.tasks[self.len] = decl;
        self.len += 1;
        Ok(self)
    }

    /// A message channel between two declared tasks: derives the `Mailbox`
    /// capability requirement on both endpoints (ownership pinned on the
    /// kernel), instead of a second hand-written declaration.
    pub fn channel(mut self, from: &'static str, to: &'static str) -> Result<Self, GraphError> {
        for endpoint in [from, to] {
            if !self.tasks[..self.len]
                .iter()
                .any(|task| task.name == endpoint)
            {
                return Err(GraphError::ChannelEndpointUnknown { endpoint });
            }
        }
        if self.channel_len == TASKS {
            return Err(GraphError::TooManyChannels);
        }
        self.channels[self.channel_len] = ChannelDecl::new(from, to);
        self.channel_len += 1;
        Ok(self)
    }

    fn decl_index_in(tasks: &[TaskDecl], name: &str) -> Option<usize> {
        tasks.iter().position(|task| task.name == name)
    }

    fn module_for_index(tasks: &[TaskDecl], index: usize) -> ModuleId {
        if let Some(role) = tasks[index].role {
            return role;
        }
        let app = tasks[..index]
            .iter()
            .filter(|task| task.role.is_none())
            .count();
        ModuleId::App(app as u8)
    }

    fn task_meta(decl: &TaskDecl, module: ModuleId) -> TaskMeta {
        TaskMeta::new(
            module,
            decl.criticality,
            decl.period_us,
            decl.execution_budget_us,
        )
        .with_phase_us(decl.phase_us)
        .with_deadline_us(decl.deadline_us)
        .with_blocking_us(decl.blocking_us)
    }

    fn build_core_into<const MODULES: usize>(
        tasks: &[TaskDecl],
        channels: &[ChannelDecl],
        kernel_memory: MemoryBudget,
        kernel_deadline: DeadlineContract,
        manifest: &mut SystemManifest<MODULES>,
        startup: &mut [StartupNode; MODULES],
    ) -> Result<usize, GraphError> {
        if tasks.len() > TASKS {
            return Err(GraphError::TooManyTasks { capacity: TASKS });
        }
        if tasks.len() + 1 > MODULES {
            return Err(GraphError::TooManyTasks { capacity: MODULES });
        }
        if channels.len() > TASKS {
            return Err(GraphError::TooManyChannels);
        }
        for (index, task) in tasks.iter().enumerate() {
            if task.after.iter().flatten().any(|dep| *dep == "\0too-many") {
                return Err(GraphError::TooManyDependencies { task: task.name });
            }
            if tasks[..index]
                .iter()
                .any(|existing| existing.name == task.name)
            {
                return Err(GraphError::DuplicateName(task.name));
            }
            if task.role.is_some()
                && (0..index)
                    .any(|previous| Self::module_for_index(tasks, previous) == task.role.unwrap())
            {
                return Err(GraphError::DuplicateRole { task: task.name });
            }
        }

        // Channel-derived capabilities.
        let mut mailbox_users = [false; TASKS];
        for channel in channels {
            for endpoint in [channel.from, channel.to] {
                let index = Self::decl_index_in(tasks, endpoint)
                    .ok_or(GraphError::ChannelEndpointUnknown { endpoint })?;
                mailbox_users[index] = true;
            }
        }

        // Kernel spec: owns its usual set, plus Mailbox when channels exist.
        let mut kernel_owns = kernel_owned_capabilities();
        if !channels.is_empty() {
            kernel_owns = kernel_owns.with(Capability::Mailbox);
        }
        let kernel_spec = ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
            .owns(kernel_owns)
            .memory(kernel_memory)
            .deadline(kernel_deadline);
        manifest
            .add(kernel_spec)
            .map_err(|error| GraphError::Manifest {
                task: "kernel",
                error,
            })?;
        startup[0] = StartupNode::new(ModuleId::Kernel, DependencySet::empty());

        for (index, decl) in tasks.iter().enumerate() {
            if decl.blocking_us > decl.deadline_us.saturating_sub(decl.execution_budget_us) {
                return Err(GraphError::InvalidBlocking {
                    task: decl.name,
                    budget_us: decl.execution_budget_us,
                    blocking_us: decl.blocking_us,
                    period_us: decl.period_us,
                });
            }
            let module = Self::module_for_index(tasks, index);
            let mut requires = decl.requires;
            if mailbox_users[index] {
                requires = requires.with(Capability::Mailbox);
            }
            let mut spec = ModuleSpec::new(module, decl.criticality)
                .requires(requires)
                .owns(decl.owns)
                .memory(decl.memory)
                .objects(decl.objects);
            if decl.has_deadline {
                spec = spec.deadline(
                    DeadlineContract::new(decl.period_us, decl.max_jitter_us)
                        .phase_us(decl.phase_us)
                        .relative_deadline_us(decl.deadline_us)
                        .execution_budget(decl.execution_budget_us)
                        .blocking(decl.blocking_us),
                );
            }
            manifest.add(spec).map_err(|error| GraphError::Manifest {
                task: decl.name,
                error,
            })?;

            // Startup edges by name -> node-index bits (kernel is node 0).
            let mut depends = DependencySet::empty().with_index(0);
            for dep_name in decl.after.iter().flatten() {
                let dep_index =
                    Self::decl_index_in(tasks, dep_name).ok_or(GraphError::UnknownDependency {
                        task: decl.name,
                        depends_on: dep_name,
                    })?;
                depends = depends.with_index(dep_index + 1);
            }
            startup[index + 1] = StartupNode::new(module, depends);
        }

        // Cycle/consistency check with attribution: the planner sees the same
        // nodes admission will see.
        let startup_len = tasks.len() + 1;
        if let Err(error) = StartupPlanner::plan::<MODULES>(&startup[..startup_len]) {
            let task = match error {
                StartupError::Cycle => {
                    Self::first_task_in_cycle_for(tasks, &startup[..startup_len])
                        .unwrap_or("<unknown>")
                }
                _ => "<startup>",
            };
            return Err(GraphError::Cycle { task });
        }

        // The expanded contract is still validated exactly like a hand-written
        // one; attribute any failure back to a task label.
        if let Err(error) = manifest.validate() {
            let task = error
                .module()
                .and_then(|module| {
                    tasks
                        .iter()
                        .enumerate()
                        .find(|(index, _)| Self::module_for_index(tasks, *index) == module)
                        .map(|(_, task)| task.name)
                })
                .unwrap_or("<manifest>");
            return Err(GraphError::Manifest { task, error });
        }

        Ok(startup_len)
    }

    /// Expand the declarations into caller-provided storage.
    ///
    /// This is the stack-bounded form for small MCUs and generated firmware:
    /// the expanded [`BuiltGraph`] can live in a `static`/cell while still
    /// going through the same manifest, startup, and profile validation as the
    /// ergonomic by-value [`build`](Self::build) path. `BuiltGraph` contains no
    /// `Drop` fields; if validation fails after partial initialization, the
    /// caller may simply reuse or discard the `MaybeUninit` slot.
    pub fn build_into<'a, const MODULES: usize>(
        &self,
        destination: &'a mut MaybeUninit<BuiltGraph<MODULES, TASKS>>,
    ) -> Result<&'a mut BuiltGraph<MODULES, TASKS>, GraphError> {
        Self::build_parts_into(
            &self.tasks[..self.len],
            &self.channels[..self.channel_len],
            self.kernel_memory,
            self.kernel_deadline,
            destination,
        )
    }

    fn build_parts_into<'a, const MODULES: usize>(
        tasks: &[TaskDecl],
        channels: &[ChannelDecl],
        kernel_memory: MemoryBudget,
        kernel_deadline: DeadlineContract,
        destination: &'a mut MaybeUninit<BuiltGraph<MODULES, TASKS>>,
    ) -> Result<&'a mut BuiltGraph<MODULES, TASKS>, GraphError> {
        let out = destination.as_mut_ptr();
        unsafe {
            SystemManifest::<MODULES>::init_in_place(core::ptr::addr_of_mut!((*out).manifest));
            core::ptr::addr_of_mut!((*out).startup).write([StartupNode::EMPTY; MODULES]);
            core::ptr::addr_of_mut!((*out).startup_len).write(0);
            core::ptr::addr_of_mut!((*out).tasks).write([None; TASKS]);
            core::ptr::addr_of_mut!((*out).task_len).write(0);
            core::ptr::addr_of_mut!((*out).labels).write([None; TASKS]);
            core::ptr::addr_of_mut!((*out).reactor_bindings).write([None; TASKS]);
            core::ptr::addr_of_mut!((*out).reactor_binding_len).write(0);
        }
        let built = unsafe { &mut *out };

        let startup_len = Self::build_core_into(
            tasks,
            channels,
            kernel_memory,
            kernel_deadline,
            &mut built.manifest,
            &mut built.startup,
        )?;
        let mut reactor_binding_len = 0usize;

        for (index, decl) in tasks.iter().enumerate() {
            let module = Self::module_for_index(tasks, index);
            built.labels[index] = Some((decl.name, module));
            built.tasks[index] = Some(Self::task_meta(decl, module));
            if let Some(domain) = decl.reactor_domain {
                built.reactor_bindings[reactor_binding_len] = Some(ReactorTaskBinding {
                    task: decl.name,
                    module,
                    domain,
                });
                reactor_binding_len += 1;
            }
        }

        built.startup_len = startup_len;
        built.task_len = tasks.len();
        built.reactor_binding_len = reactor_binding_len;
        Ok(built)
    }

    /// Expand the declarations into the real, still-validated contract set.
    pub fn build<const MODULES: usize>(&self) -> Result<BuiltGraph<MODULES, TASKS>, GraphError> {
        let mut destination = MaybeUninit::uninit();
        self.build_into::<MODULES>(&mut destination)?;
        Ok(unsafe { destination.assume_init() })
    }

    fn first_task_in_cycle_for(tasks: &[TaskDecl], nodes: &[StartupNode]) -> Option<&'static str> {
        // Kahn-style elimination; anything left is on a cycle.
        let mut remaining: u32 = (1u32 << nodes.len()) - 1;
        loop {
            let mut progressed = false;
            for (index, node) in nodes.iter().enumerate() {
                if remaining & (1 << index) == 0 {
                    continue;
                }
                if DependencySet::from_bits(node.depends_on.bits() & remaining).is_empty() {
                    remaining &= !(1 << index);
                    progressed = true;
                }
            }
            if remaining == 0 {
                return None;
            }
            if !progressed {
                let index = remaining.trailing_zeros() as usize;
                // Node 0 is the kernel; tasks start at 1.
                return Some(tasks[index.checked_sub(1)?].name);
            }
        }
    }

    /// Validate the derived contract against a platform profile too.
    pub fn build_for<const MODULES: usize>(
        &self,
        profile: SystemProfile,
    ) -> Result<BuiltGraph<MODULES, TASKS>, GraphError> {
        let built = self.build::<MODULES>()?;
        built
            .manifest
            .validate_profile(profile)
            .map_err(|error| GraphError::Manifest {
                task: "<profile>",
                error,
            })?;
        Ok(built)
    }

    /// Profile-validated in-place form of [`build_for`](Self::build_for).
    pub fn build_for_into<'a, const MODULES: usize>(
        &self,
        profile: SystemProfile,
        destination: &'a mut MaybeUninit<BuiltGraph<MODULES, TASKS>>,
    ) -> Result<&'a mut BuiltGraph<MODULES, TASKS>, GraphError> {
        let built = self.build_into::<MODULES>(destination)?;
        built
            .manifest
            .validate_profile(profile)
            .map_err(|error| GraphError::Manifest {
                task: "<profile>",
                error,
            })?;
        Ok(built)
    }
}

impl<const TASKS: usize> Default for AppGraph<TASKS> {
    fn default() -> Self {
        Self::new()
    }
}

/// Borrowed, allocation-free graph declaration for startup-stack-sensitive
/// firmware. Put the task/channel arrays in `const` or `static` storage and
/// build the same validated [`BuiltGraph`] without first copying an
/// [`AppGraph`] builder onto the entry stack.
#[derive(Clone, Copy, Debug)]
pub struct GraphSpec<'a> {
    tasks: &'a [TaskDecl],
    channels: &'a [ChannelDecl],
    kernel_memory: MemoryBudget,
    kernel_deadline: DeadlineContract,
}

impl<'a> GraphSpec<'a> {
    pub const fn new(tasks: &'a [TaskDecl], channels: &'a [ChannelDecl]) -> Self {
        Self {
            tasks,
            channels,
            kernel_memory: MemoryBudget::new(8 * 1024, 2 * 1024, 1),
            kernel_deadline: DeadlineContract::new(20_000, 100),
        }
    }

    pub const fn kernel(mut self, memory: MemoryBudget, deadline: DeadlineContract) -> Self {
        self.kernel_memory = memory;
        self.kernel_deadline = deadline;
        self
    }

    pub const fn task_decls(&self) -> &'a [TaskDecl] {
        self.tasks
    }

    pub const fn channel_decls(&self) -> &'a [ChannelDecl] {
        self.channels
    }

    /// Resolve the identity assigned to a declaration.
    ///
    /// After a successful build/start this is the same mapping exposed by
    /// [`BuiltGraph::module_of`], without retaining a capacity-sized expanded
    /// graph in RAM.
    pub fn module_of(&self, name: &str) -> Option<ModuleId> {
        let mut next_app = 0u8;
        for task in self.tasks {
            let module = match task.role {
                Some(role) => role,
                None => {
                    let module = ModuleId::App(next_app);
                    next_app = next_app.checked_add(1)?;
                    module
                }
            };
            if task.name == name {
                return Some(module);
            }
        }
        None
    }

    pub fn build_into<'b, const MODULES: usize, const TASKS: usize>(
        &self,
        destination: &'b mut MaybeUninit<BuiltGraph<MODULES, TASKS>>,
    ) -> Result<&'b mut BuiltGraph<MODULES, TASKS>, GraphError> {
        AppGraph::<TASKS>::build_parts_into(
            self.tasks,
            self.channels,
            self.kernel_memory,
            self.kernel_deadline,
            destination,
        )
    }

    pub fn build_for_into<'b, const MODULES: usize, const TASKS: usize>(
        &self,
        profile: SystemProfile,
        destination: &'b mut MaybeUninit<BuiltGraph<MODULES, TASKS>>,
    ) -> Result<&'b mut BuiltGraph<MODULES, TASKS>, GraphError> {
        let built = self.build_into::<MODULES, TASKS>(destination)?;
        built
            .manifest
            .validate_profile(profile)
            .map_err(|error| GraphError::Manifest {
                task: "<profile>",
                error,
            })?;
        Ok(built)
    }

    /// Build, admit, boot, register, and seal a graph in one call.
    ///
    /// The derived manifest and startup nodes temporarily occupy an
    /// initialization workspace overlaid with the still-uninitialized
    /// executor cell; admission consumes them before the final runtime
    /// overwrites that workspace. Labels, task metadata, and reactor bindings
    /// are regenerated directly from the declarations instead of retaining a
    /// capacity-sized [`BuiltGraph`]. The executor's `STARTUP` capacity is also
    /// used as the manifest capacity, which is the normal coherent runtime
    /// configuration. Advanced layouts can keep using [`build_for_into`].
    ///
    /// The destination cell is one-shot. It is claimed while its initialization
    /// workspace derives and validates the graph; any graph failure restores
    /// the empty state, while a successful initialization remains one-shot.
    #[allow(
        clippy::too_many_arguments,
        reason = "the const capacities are inferred from the cell; callers provide only startup policy"
    )]
    #[allow(
        clippy::mut_from_ref,
        reason = "the one-shot executor cell atomically proves unique mutable ownership"
    )]
    pub fn start_executor<
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    >(
        &self,
        cell: &'static KernelExecutorCell<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        profile: SystemProfile,
        thresholds: FaultThresholds,
        containment: ContainmentPolicy,
        now_us: u64,
    ) -> Result<
        &'static mut KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        GraphStartError,
    > {
        // SAFETY: the builder validates the exact manifest/profile pair
        // returned to the cell and bounds startup_len by the provided array.
        let executor = match unsafe {
            cell.init_prevalidated_with_graph_scratch(
                profile,
                thresholds,
                containment,
                |manifest, startup| {
                    let startup_len = AppGraph::<TASKS>::build_core_into(
                        self.tasks,
                        self.channels,
                        self.kernel_memory,
                        self.kernel_deadline,
                        manifest,
                        startup,
                    )?;
                    manifest
                        .validate_profile(profile)
                        .map_err(|error| GraphError::Manifest {
                            task: "<profile>",
                            error,
                        })?;
                    Ok::<usize, GraphError>(startup_len)
                },
            )
        } {
            Ok(executor) => executor,
            Err(crate::kernel_executor::ExecutorGraphInitError::Graph(error)) => {
                return Err(error.into());
            }
            Err(crate::kernel_executor::ExecutorGraphInitError::Executor(error)) => {
                return Err(error.into());
            }
        };
        for (index, decl) in self.tasks.iter().enumerate() {
            let module = AppGraph::<TASKS>::module_for_index(self.tasks, index);
            executor
                .runtime_mut()
                .configure_object_quota(module, decl.objects);
        }
        executor
            .runtime_mut()
            .boot_to_running(now_us)
            .map_err(ExecError::Runtime)?;
        for (index, decl) in self.tasks.iter().enumerate() {
            let module = AppGraph::<TASKS>::module_for_index(self.tasks, index);
            executor.add_task(AppGraph::<TASKS>::task_meta(decl, module), now_us)?;
        }
        executor.seal()?;
        Ok(executor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FaultThresholds;
    use std::boxed::Box;

    fn demo_graph() -> AppGraph<4> {
        AppGraph::<4>::new()
            .task(TaskDecl::control("motor", 20_000))
            .unwrap()
            .task(TaskDecl::periodic("imu", 100_000).after("motor"))
            .unwrap()
            .task(TaskDecl::service("telemetry", 500_000).after("imu"))
            .unwrap()
            .channel("imu", "motor")
            .unwrap()
    }

    #[test]
    fn one_declaration_derives_manifest_startup_tasks_and_labels() {
        let built = demo_graph().build::<4>().unwrap();
        assert_eq!(built.manifest.len(), 4); // kernel + 3 tasks
        assert_eq!(built.startup_len, 4);
        assert_eq!(built.task_len, 3);

        // Opaque identity + labels both ways.
        let motor = built.module_of("motor").unwrap();
        assert_eq!(motor, ModuleId::App(0));
        assert_eq!(built.label_of(motor), Some("motor"));

        // Channel-derived capability: both endpoints require Mailbox, the
        // kernel owns it — nobody wrote a capability line by hand.
        let imu = built.module_of("imu").unwrap();
        let imu_spec = built.manifest.iter().find(|spec| spec.id == imu).unwrap();
        assert!(imu_spec.requires.contains(Capability::Mailbox));
        let kernel = built
            .manifest
            .iter()
            .find(|spec| spec.id == ModuleId::Kernel)
            .unwrap();
        assert!(kernel.owns.contains(Capability::Mailbox));

        // Safe defaults expanded into the real contract: control jitter tighter
        // than periodic's 1% rule, budget = period/10.
        let motor_spec = built.manifest.iter().find(|spec| spec.id == motor).unwrap();
        let deadline = motor_spec.deadline.unwrap();
        assert_eq!(deadline.max_jitter_us, 100); // 20_000/200
        assert_eq!(deadline.execution_budget_us, 2_000);

        // The derived contract admits and boots exactly like a hand-written one.
        let mut runtime = crate::Runtime::<4, 4, 8, 4, 8, 4, 16>::admit(
            &built.manifest,
            built.startup_nodes(),
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
        )
        .unwrap();
        runtime.boot_to_running(0).unwrap();
    }

    #[test]
    fn graph_spec_starts_and_seals_executor_without_retained_built_graph() {
        const TASKS: [TaskDecl; 3] = [
            TaskDecl::control("motor", 20_000).objects(ObjectQuota::new(7, 6, 5)),
            TaskDecl::periodic("imu", 100_000).after("motor"),
            TaskDecl::service("telemetry", 500_000).after("imu"),
        ];
        const CHANNELS: [ChannelDecl; 1] = [ChannelDecl::new("imu", "motor")];

        type Cell = KernelExecutorCell<3, 4, 4, 4, 0, 0, 4, 0>;
        let cell: &'static Cell = Box::leak(Box::new(Cell::new()));
        let graph = GraphSpec::new(&TASKS, &CHANNELS);
        let executor = graph
            .start_executor(
                cell,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
                0,
            )
            .unwrap();

        let motor = graph.module_of("motor").unwrap();
        assert_eq!(motor, ModuleId::App(0));
        assert!(executor.tasks().get(motor).is_some());
        assert_eq!(executor.runtime().plan().module_count(), 4);
        assert_eq!(
            executor.runtime().object_quota(motor),
            Some(ObjectQuota::new(7, 6, 5))
        );
        assert_eq!(
            executor.add_task(TaskMeta::new(motor, Criticality::System, 20_000, 2_000), 0),
            Err(ExecError::Sealed)
        );
    }

    #[test]
    fn compact_startup_core_matches_the_full_built_graph() {
        const TASKS: [TaskDecl; 3] = [
            TaskDecl::control("motor", 20_000),
            TaskDecl::periodic("imu", 100_000).after("motor"),
            TaskDecl::service("telemetry", 500_000).after("imu"),
        ];
        const CHANNELS: [ChannelDecl; 1] = [ChannelDecl::new("imu", "motor")];

        let graph = GraphSpec::new(&TASKS, &CHANNELS);
        let mut full_slot = MaybeUninit::<BuiltGraph<4, 3>>::uninit();
        let full = graph
            .build_for_into::<4, 3>(SystemProfile::NRF52840_CORE, &mut full_slot)
            .unwrap();

        let mut manifest = SystemManifest::<4>::new();
        let mut startup = [StartupNode::EMPTY; 4];
        let startup_len = AppGraph::<3>::build_core_into(
            graph.tasks,
            graph.channels,
            graph.kernel_memory,
            graph.kernel_deadline,
            &mut manifest,
            &mut startup,
        )
        .unwrap();
        manifest
            .validate_profile(SystemProfile::NRF52840_CORE)
            .unwrap();

        assert_eq!(startup_len, full.startup_len);
        assert_eq!(&startup[..startup_len], full.startup_nodes());
        assert_eq!(manifest.len(), full.manifest.len());
        for (compact, retained) in manifest.iter().zip(full.manifest.iter()) {
            assert_eq!(compact, retained);
        }
    }

    #[test]
    fn graph_validation_failure_does_not_claim_the_executor_cell() {
        const INVALID: [TaskDecl; 1] = [TaskDecl::periodic("imu", 10_000).after("ghost")];
        const VALID: [TaskDecl; 1] = [TaskDecl::periodic("imu", 10_000)];

        type Cell = KernelExecutorCell<1, 2, 2, 1, 0, 0, 2, 0>;
        let cell: &'static Cell = Box::leak(Box::new(Cell::new()));
        let error = GraphSpec::new(&INVALID, &[])
            .start_executor(
                cell,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
                0,
            )
            .err();
        assert_eq!(
            error,
            Some(GraphStartError::Graph(GraphError::UnknownDependency {
                task: "imu",
                depends_on: "ghost",
            }))
        );

        GraphSpec::new(&VALID, &[])
            .start_executor(
                cell,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
                0,
            )
            .unwrap();
    }

    #[test]
    fn graph_executor_init_failure_restores_the_cell_for_retry() {
        const TASKS: [TaskDecl; 1] = [TaskDecl::periodic("imu", 10_000)];
        type Cell = KernelExecutorCell<1, 2, 2, 1, 0, 0, 2, 0>;
        let cell: &'static Cell = Box::leak(Box::new(Cell::new()));
        let graph = GraphSpec::new(&TASKS, &[]);

        let error = graph
            .start_executor(
                cell,
                SystemProfile::NRF52840_CORE,
                FaultThresholds {
                    notify_after: 3,
                    reboot_after: 2,
                },
                ContainmentPolicy::Cooperative,
                0,
            )
            .err()
            .unwrap();
        assert!(matches!(
            error,
            GraphStartError::Executor(ExecutorInitError::Runtime(_))
        ));

        graph
            .start_executor(
                cell,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
                0,
            )
            .unwrap();
    }

    #[test]
    fn in_place_build_matches_by_value_graph() {
        let graph = demo_graph();
        let by_value = graph.build_for::<4>(SystemProfile::NRF52840_CORE).unwrap();
        let mut slot = MaybeUninit::<BuiltGraph<4, 4>>::uninit();
        let in_place = graph
            .build_for_into::<4>(SystemProfile::NRF52840_CORE, &mut slot)
            .unwrap();

        assert_eq!(in_place.manifest.len(), by_value.manifest.len());
        assert_eq!(in_place.startup_nodes(), by_value.startup_nodes());
        assert_eq!(in_place.tasks, by_value.tasks);
        assert_eq!(in_place.task_len, by_value.task_len);
        for label in ["motor", "imu", "telemetry"] {
            assert_eq!(in_place.module_of(label), by_value.module_of(label));
        }
    }

    #[test]
    fn const_graph_spec_matches_the_builder_without_builder_storage() {
        const TASKS: [TaskDecl; 3] = [
            TaskDecl::control("motor", 20_000),
            TaskDecl::periodic("imu", 100_000).after("motor"),
            TaskDecl::service("telemetry", 500_000).after("imu"),
        ];
        const CHANNELS: [ChannelDecl; 2] = [
            ChannelDecl::new("motor", "imu"),
            ChannelDecl::new("imu", "telemetry"),
        ];

        let by_value = AppGraph::<3>::new()
            .task(TASKS[0])
            .unwrap()
            .task(TASKS[1])
            .unwrap()
            .task(TASKS[2])
            .unwrap()
            .channel("motor", "imu")
            .unwrap()
            .channel("imu", "telemetry")
            .unwrap()
            .build_for::<4>(SystemProfile::NRF52840_CORE)
            .unwrap();
        let mut slot = MaybeUninit::<BuiltGraph<4, 3>>::uninit();
        let borrowed = GraphSpec::new(&TASKS, &CHANNELS)
            .build_for_into::<4, 3>(SystemProfile::NRF52840_CORE, &mut slot)
            .unwrap();

        assert_eq!(borrowed.startup_nodes(), by_value.startup_nodes());
        assert_eq!(borrowed.tasks, by_value.tasks);
        assert_eq!(
            borrowed.manifest.fingerprint(),
            by_value.manifest.fingerprint()
        );
        for label in ["motor", "imu", "telemetry"] {
            assert_eq!(borrowed.module_of(label), by_value.module_of(label));
        }
    }

    #[test]
    fn const_graph_spec_rejects_duplicate_names_and_unknown_channels() {
        const DUPLICATE: [TaskDecl; 2] = [
            TaskDecl::periodic("imu", 10_000),
            TaskDecl::periodic("imu", 20_000),
        ];
        const UNKNOWN: [ChannelDecl; 1] = [ChannelDecl::new("imu", "ghost")];

        let mut duplicate_slot = MaybeUninit::<BuiltGraph<3, 2>>::uninit();
        let duplicate = GraphSpec::new(&DUPLICATE, &[])
            .build_into::<3, 2>(&mut duplicate_slot)
            .err();
        assert_eq!(duplicate, Some(GraphError::DuplicateName("imu")));

        let mut unknown_slot = MaybeUninit::<BuiltGraph<2, 1>>::uninit();
        let unknown = GraphSpec::new(&DUPLICATE[..1], &UNKNOWN)
            .build_into::<2, 1>(&mut unknown_slot)
            .err();
        assert_eq!(
            unknown,
            Some(GraphError::ChannelEndpointUnknown { endpoint: "ghost" })
        );
    }

    #[test]
    fn every_bad_graph_fails_with_one_attributed_diagnostic() {
        // duplicate name
        let err = AppGraph::<4>::new()
            .task(TaskDecl::periodic("imu", 1000))
            .unwrap()
            .task(TaskDecl::periodic("imu", 2000))
            .err()
            .unwrap();
        assert_eq!(err, GraphError::DuplicateName("imu"));

        // unknown dependency
        let err = AppGraph::<4>::new()
            .task(TaskDecl::periodic("imu", 1000).after("ghost"))
            .unwrap()
            .build::<4>()
            .err()
            .unwrap();
        assert_eq!(
            err,
            GraphError::UnknownDependency {
                task: "imu",
                depends_on: "ghost"
            }
        );

        // cycle, attributed to a task on it
        let err = AppGraph::<4>::new()
            .task(TaskDecl::periodic("a", 1000).after("b"))
            .unwrap()
            .task(TaskDecl::periodic("b", 1000).after("a"))
            .unwrap()
            .build::<4>()
            .err()
            .unwrap();
        assert!(matches!(err, GraphError::Cycle { task: "a" | "b" }));

        // over capacity
        let err = AppGraph::<1>::new()
            .task(TaskDecl::periodic("a", 1000))
            .unwrap()
            .task(TaskDecl::periodic("b", 1000))
            .err()
            .unwrap();
        assert_eq!(err, GraphError::TooManyTasks { capacity: 1 });

        // conflicting (duplicate) roles
        let err = AppGraph::<4>::new()
            .task(TaskDecl::periodic("x", 1000).role(ModuleId::Sensor))
            .unwrap()
            .task(TaskDecl::periodic("y", 1000).role(ModuleId::Sensor))
            .unwrap()
            .build::<4>()
            .err()
            .unwrap();
        assert_eq!(err, GraphError::DuplicateRole { task: "y" });

        // manifest conflict (utilization overrun), attributed by label
        let err = AppGraph::<4>::new()
            .task(TaskDecl::periodic("hog", 1000).budget_us(999))
            .unwrap()
            .task(TaskDecl::periodic("hog2", 1000).budget_us(999))
            .unwrap()
            .build_for::<4>(SystemProfile::NRF52840_CORE)
            .err()
            .unwrap();
        assert!(matches!(err, GraphError::Manifest { .. }));

        // unknown channel endpoint
        let err = AppGraph::<4>::new()
            .task(TaskDecl::periodic("imu", 1000))
            .unwrap()
            .channel("imu", "ghost")
            .err()
            .unwrap();
        assert_eq!(
            err,
            GraphError::ChannelEndpointUnknown { endpoint: "ghost" }
        );
    }

    #[test]
    fn services_have_no_deadline_and_roles_are_optional_labels() {
        let built = AppGraph::<4>::new()
            .task(TaskDecl::service("logger", 1_000_000))
            .unwrap()
            .task(TaskDecl::periodic("bus", 10_000).role(ModuleId::Bus))
            .unwrap()
            .build::<4>()
            .unwrap();
        let logger = built.module_of("logger").unwrap();
        let spec = built
            .manifest
            .iter()
            .find(|spec| spec.id == logger)
            .unwrap();
        assert!(spec.deadline.is_none());
        assert_eq!(built.module_of("bus"), Some(ModuleId::Bus));
        assert_eq!(built.label_of(ModuleId::Bus), Some("bus"));
    }

    #[test]
    fn blocking_override_is_attributed_to_task_label() {
        let err = AppGraph::<1>::new()
            .task(
                TaskDecl::control("motor", 1000)
                    .budget_us(600)
                    .blocking_us(401),
            )
            .unwrap()
            .build_for::<2>(SystemProfile::NRF52840_CORE)
            .err()
            .unwrap();
        assert_eq!(
            err,
            GraphError::InvalidBlocking {
                task: "motor",
                budget_us: 600,
                blocking_us: 401,
                period_us: 1000,
            }
        );
    }

    #[test]
    fn blocking_bound_reaches_the_shared_admission_contract() {
        let graph = AppGraph::<1>::new()
            .task(
                TaskDecl::control("motor", 5_000)
                    .budget_us(400)
                    .blocking_us(75),
            )
            .unwrap();
        let built = graph.build_for::<2>(SystemProfile::NRF52840_CORE).unwrap();
        let motor = built.module_of("motor").unwrap();
        let deadline = built
            .manifest
            .iter()
            .find(|spec| spec.id == motor)
            .unwrap()
            .deadline
            .unwrap();
        assert_eq!(deadline.blocking_us, 75);
    }

    #[test]
    fn phase_period_deadline_are_declared_once_and_reach_both_kernel_inputs() {
        let graph = AppGraph::<1>::new()
            .task(
                TaskDecl::control("motor", 5_000)
                    .phase_us(1_000)
                    .deadline_us(4_000)
                    .budget_us(400),
            )
            .unwrap();
        let built = graph.build_for::<2>(SystemProfile::NRF52840_CORE).unwrap();
        let contract = built
            .manifest
            .iter()
            .find(|spec| spec.id == ModuleId::App(0))
            .unwrap()
            .deadline
            .unwrap();
        assert_eq!(
            (contract.phase_us, contract.period_us, contract.deadline_us),
            (1_000, 5_000, 4_000)
        );
        let task = built.tasks[0].unwrap();
        assert_eq!(
            (task.phase_us, task.period_us, task.deadline_us),
            (1_000, 5_000, 4_000)
        );
    }

    #[test]
    fn reactor_domains_are_linked_to_admitted_graph_tasks() {
        let built = AppGraph::<2>::new()
            .task(TaskDecl::control("control-reactor", 5_000).reactor_domain(0))
            .unwrap()
            .task(
                TaskDecl::service("telemetry-reactor", 100_000)
                    .reactor_domain(1)
                    .after("control-reactor"),
            )
            .unwrap()
            .channel("control-reactor", "telemetry-reactor")
            .unwrap()
            .build::<3>()
            .unwrap();

        assert_eq!(built.reactor_domain_of("control-reactor"), Some(0));
        let admitted = built
            .admit_reactor_domains::<2, 1>(
                [
                    Some(ReactorDomainContract::new(0, 0).task_slots(4)),
                    Some(ReactorDomainContract::new(1, 3).task_slots(2)),
                ],
                [Some(ReactorChannelContract::new(0, 1, 4).waiter_slots(4))],
            )
            .unwrap();

        assert_eq!(admitted.binding_len, 2);
        assert_eq!(admitted.reactor.cross_domain_len, 1);
        assert_eq!(
            admitted.bindings[0],
            Some(ReactorTaskBinding {
                task: "control-reactor",
                module: ModuleId::App(0),
                domain: 0,
            })
        );
    }

    #[test]
    fn reactor_domain_linkage_rejects_missing_unknown_and_duplicate_drivers() {
        let missing = AppGraph::<1>::new()
            .task(TaskDecl::control("control-reactor", 5_000).reactor_domain(0))
            .unwrap()
            .build::<2>()
            .unwrap()
            .admit_reactor_domains::<2, 0>(
                [
                    Some(ReactorDomainContract::new(0, 0)),
                    Some(ReactorDomainContract::new(1, 1)),
                ],
                [],
            )
            .unwrap_err();
        assert_eq!(
            missing,
            GraphReactorError::MissingDomainDriver { domain: 1 }
        );

        let unknown = AppGraph::<1>::new()
            .task(TaskDecl::control("control-reactor", 5_000).reactor_domain(7))
            .unwrap()
            .build::<2>()
            .unwrap()
            .admit_reactor_domains::<1, 0>([Some(ReactorDomainContract::new(0, 0))], [])
            .unwrap_err();
        assert_eq!(
            unknown,
            GraphReactorError::UnknownDomain {
                task: "control-reactor",
                domain: 7,
            }
        );

        let duplicate = AppGraph::<2>::new()
            .task(TaskDecl::control("control-reactor", 5_000).reactor_domain(0))
            .unwrap()
            .task(TaskDecl::service("telemetry-reactor", 100_000).reactor_domain(0))
            .unwrap()
            .build::<3>()
            .unwrap()
            .admit_reactor_domains::<1, 0>([Some(ReactorDomainContract::new(0, 0))], [])
            .unwrap_err();
        assert_eq!(
            duplicate,
            GraphReactorError::DuplicateDomainDriver {
                domain: 0,
                first_task: "control-reactor",
                duplicate_task: "telemetry-reactor",
            }
        );

        let invalid_contract = AppGraph::<1>::new()
            .task(TaskDecl::control("control-reactor", 5_000).reactor_domain(0))
            .unwrap()
            .build::<2>()
            .unwrap()
            .admit_reactor_domains::<0, 0>([], [])
            .unwrap_err();
        assert_eq!(
            invalid_contract,
            GraphReactorError::ReactorAdmission(ReactorAdmissionError::EmptyDomains)
        );
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, n: u8) -> u8 {
            (self.next() % u64::from(n)) as u8
        }
    }

    #[test]
    fn reactor_domain_linkage_fuzz_matches_shadow_model() {
        let cases: u64 = if cfg!(miri) { 32 } else { 1024 };
        let names = ["reactor-a", "reactor-b", "reactor-c", "reactor-d"];

        for seed in 1..=cases {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let task_count = usize::from(rng.below(4) + 1);
            let mut graph = AppGraph::<4>::new();
            let mut binding_domains: [Option<u8>; 4] = [None; 4];

            for index in 0..task_count {
                let mut decl = if rng.below(2) == 0 {
                    TaskDecl::control(names[index], 5_000 + u32::from(rng.below(8)) * 1_000)
                } else {
                    TaskDecl::service(names[index], 50_000 + u32::from(rng.below(8)) * 10_000)
                };
                if rng.below(4) != 0 {
                    let domain = rng.below(5);
                    binding_domains[index] = Some(domain);
                    decl = decl.reactor_domain(domain);
                }
                graph = graph.task(decl).unwrap();
            }

            let built = graph.build::<5>().unwrap();
            let mut domain_present = [false; 4];
            let mut domains = [None; 4];
            for id in 0..4 {
                if rng.below(2) == 0 {
                    domain_present[id] = true;
                    domains[id] = Some(
                        ReactorDomainContract::new(id as u8, id as u8)
                            .task_slots(rng.below(31) + 1)
                            .fuel_per_cycle(u32::from(rng.below(8)) + 1),
                    );
                }
            }

            let result = built.admit_reactor_domains::<4, 0>(domains, []);
            let any_domain = domain_present.iter().any(|present| *present);
            let first_unknown =
                binding_domains[..task_count]
                    .iter()
                    .enumerate()
                    .find_map(|(index, domain)| {
                        let domain = (*domain)?;
                        let known = domain_present
                            .get(usize::from(domain))
                            .copied()
                            .unwrap_or(false);
                        (!known).then_some((names[index], domain))
                    });
            let duplicate =
                binding_domains[..task_count]
                    .iter()
                    .enumerate()
                    .find_map(|(index, domain)| {
                        let domain = (*domain)?;
                        binding_domains[..index].iter().enumerate().find_map(
                            |(previous_index, previous)| {
                                (*previous == Some(domain)).then_some((
                                    domain,
                                    names[previous_index],
                                    names[index],
                                ))
                            },
                        )
                    });
            let missing = domain_present
                .iter()
                .enumerate()
                .find_map(|(domain, present)| {
                    if !*present {
                        return None;
                    }
                    let driven = binding_domains[..task_count].contains(&Some(domain as u8));
                    (!driven).then_some(domain as u8)
                });

            match (any_domain, first_unknown, duplicate, missing) {
                (false, _, _, _) => assert_eq!(
                    result.unwrap_err(),
                    GraphReactorError::ReactorAdmission(ReactorAdmissionError::EmptyDomains),
                    "seed {seed}"
                ),
                (true, Some((task, domain)), _, _) => assert_eq!(
                    result.unwrap_err(),
                    GraphReactorError::UnknownDomain { task, domain },
                    "seed {seed}"
                ),
                (true, None, Some((domain, first_task, duplicate_task)), _) => assert_eq!(
                    result.unwrap_err(),
                    GraphReactorError::DuplicateDomainDriver {
                        domain,
                        first_task,
                        duplicate_task,
                    },
                    "seed {seed}"
                ),
                (true, None, None, Some(domain)) => assert_eq!(
                    result.unwrap_err(),
                    GraphReactorError::MissingDomainDriver { domain },
                    "seed {seed}"
                ),
                (true, None, None, None) => {
                    let admitted = result.unwrap();
                    assert_eq!(
                        admitted.binding_len,
                        binding_domains[..task_count]
                            .iter()
                            .filter(|domain| domain.is_some())
                            .count(),
                        "seed {seed}"
                    );
                }
            }
        }
    }

    #[test]
    fn shortest_period_defaults_remain_self_consistent() {
        let decl = TaskDecl::periodic("fast", 1);
        assert_eq!(decl.max_jitter_us, 0);
        assert_eq!(decl.execution_budget_us, 1);
        assert!(AppGraph::<1>::new()
            .task(decl)
            .unwrap()
            .build_for::<2>(SystemProfile::NRF52840_CORE)
            .is_ok());
        let control = TaskDecl::control("fast-control", 1);
        assert_eq!(control.max_jitter_us, 0);
        assert_eq!(control.execution_budget_us, 1);
    }
}
