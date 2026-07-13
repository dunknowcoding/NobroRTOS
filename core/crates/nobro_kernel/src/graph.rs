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
//!   defaults to period/100 (min 10 µs), execution budget to period/10,
//!   memory to 1 KiB flash / 256 B RAM.
//! - [`TaskDecl::control`] — hard-real-time: tighter jitter (period/200,
//!   min 5 µs), same budget rule; put actuation here.
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
    kernel_owned_capabilities, Capability, CapabilitySet, Criticality, DeadlineContract,
    DependencySet, ManifestError, MemoryBudget, ModuleId, ModuleSpec, ObjectQuota, StartupError,
    StartupNode, StartupPlanner, SystemManifest, SystemProfile, TaskMeta,
};

const MAX_DEPS: usize = 4;

/// One task declaration; construct through the typed builders.
#[derive(Clone, Copy, Debug)]
pub struct TaskDecl {
    pub name: &'static str,
    pub criticality: Criticality,
    pub period_us: u32,
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
    after: [Option<&'static str>; MAX_DEPS],
}

impl TaskDecl {
    fn base(name: &'static str, criticality: Criticality, period_us: u32) -> Self {
        Self {
            name,
            criticality,
            period_us,
            max_jitter_us: (period_us / 100).max(10),
            execution_budget_us: (period_us / 10).max(10),
            blocking_us: 0,
            memory: MemoryBudget::new(1024, 256, 0),
            objects: ObjectQuota::DEFAULT,
            requires: CapabilitySet::empty(),
            owns: CapabilitySet::empty(),
            role: None,
            has_deadline: true,
            core_affinity: None,
            after: [None; MAX_DEPS],
        }
    }

    /// A periodic worker with safe defaults (driver criticality).
    pub fn periodic(name: &'static str, period_us: u32) -> Self {
        Self::base(name, Criticality::Driver, period_us)
    }

    /// A hard-real-time control task: tighter default jitter.
    pub fn control(name: &'static str, period_us: u32) -> Self {
        let mut decl = Self::base(name, Criticality::HardRealtime, period_us);
        decl.max_jitter_us = (period_us / 200).max(5);
        decl
    }

    /// Best-effort background service polled at `poll_us`; no deadline contract.
    pub fn service(name: &'static str, poll_us: u32) -> Self {
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

    /// Start this task after `name` (startup ordering, by label).
    pub fn after(mut self, name: &'static str) -> Self {
        for slot in self.after.iter_mut() {
            if slot.is_none() {
                *slot = Some(name);
                return self;
            }
        }
        // Too many edges is reported at build() with the task's name.
        self.after[MAX_DEPS - 1] = Some("\0too-many");
        self
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

/// Everything the kernel needs, derived from one declaration set.
pub struct BuiltGraph<const MODULES: usize, const TASKS: usize> {
    pub manifest: SystemManifest<MODULES>,
    pub startup: [StartupNode; MODULES],
    pub startup_len: usize,
    pub tasks: [Option<TaskMeta>; TASKS],
    pub task_len: usize,
    labels: [Option<(&'static str, ModuleId)>; TASKS],
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
}

pub struct AppGraph<const TASKS: usize> {
    tasks: [Option<TaskDecl>; TASKS],
    len: usize,
    channels: [Option<(&'static str, &'static str)>; TASKS],
    channel_len: usize,
    kernel_memory: MemoryBudget,
    kernel_deadline: DeadlineContract,
}

impl<const TASKS: usize> AppGraph<TASKS> {
    pub fn new() -> Self {
        Self {
            tasks: [None; TASKS],
            len: 0,
            channels: [None; TASKS],
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
        self.tasks.iter().flatten()
    }

    /// The declared channels as `(from, to)` label pairs.
    pub fn channel_pairs(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        self.channels.iter().flatten().copied()
    }

    pub fn task(mut self, decl: TaskDecl) -> Result<Self, GraphError> {
        if self
            .tasks
            .iter()
            .flatten()
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
        self.tasks[self.len] = Some(decl);
        self.len += 1;
        Ok(self)
    }

    /// A message channel between two declared tasks: derives the `Mailbox`
    /// capability requirement on both endpoints (ownership pinned on the
    /// kernel), instead of a second hand-written declaration.
    pub fn channel(mut self, from: &'static str, to: &'static str) -> Result<Self, GraphError> {
        for endpoint in [from, to] {
            if !self
                .tasks
                .iter()
                .flatten()
                .any(|task| task.name == endpoint)
            {
                return Err(GraphError::ChannelEndpointUnknown { endpoint });
            }
        }
        if self.channel_len == TASKS {
            return Err(GraphError::TooManyChannels);
        }
        self.channels[self.channel_len] = Some((from, to));
        self.channel_len += 1;
        Ok(self)
    }

    fn decl_index(&self, name: &str) -> Option<usize> {
        self.tasks[..self.len]
            .iter()
            .position(|task| task.map(|task| task.name == name).unwrap_or(false))
    }

    /// Expand the declarations into the real, still-validated contract set.
    pub fn build<const MODULES: usize>(&self) -> Result<BuiltGraph<MODULES, TASKS>, GraphError> {
        if self.len + 1 > MODULES {
            return Err(GraphError::TooManyTasks { capacity: MODULES });
        }

        // Identity allocation: explicit roles win; everything else gets the
        // next opaque App slot. Roles must be unique.
        let mut labels: [Option<(&'static str, ModuleId)>; TASKS] = [None; TASKS];
        let mut next_app: u8 = 0;
        for (index, decl) in self.tasks[..self.len].iter().flatten().enumerate() {
            let module = match decl.role {
                Some(role) => {
                    if labels.iter().flatten().any(|(_, used)| *used == role) {
                        return Err(GraphError::DuplicateRole { task: decl.name });
                    }
                    role
                }
                None => {
                    let module = ModuleId::App(next_app);
                    next_app += 1;
                    module
                }
            };
            labels[index] = Some((decl.name, module));
        }

        // Channel-derived capabilities.
        let mut mailbox_users = [false; TASKS];
        for (from, to) in self.channels[..self.channel_len].iter().flatten() {
            for endpoint in [*from, *to] {
                let index = self
                    .decl_index(endpoint)
                    .ok_or(GraphError::ChannelEndpointUnknown { endpoint })?;
                mailbox_users[index] = true;
            }
        }

        // Kernel spec: owns its usual set, plus Mailbox when channels exist.
        let mut kernel_owns = kernel_owned_capabilities();
        if self.channel_len > 0 {
            kernel_owns = kernel_owns.with(Capability::Mailbox);
        }
        let mut manifest = SystemManifest::<MODULES>::new();
        let kernel_spec = ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
            .owns(kernel_owns)
            .memory(self.kernel_memory)
            .deadline(self.kernel_deadline);
        manifest
            .add(kernel_spec)
            .map_err(|error| GraphError::Manifest {
                task: "kernel",
                error,
            })?;

        let mut startup = [StartupNode::EMPTY; MODULES];
        startup[0] = StartupNode::new(ModuleId::Kernel, DependencySet::empty());
        let mut tasks: [Option<TaskMeta>; TASKS] = [None; TASKS];

        for (index, decl) in self.tasks[..self.len].iter().flatten().enumerate() {
            if decl.blocking_us > decl.period_us.saturating_sub(decl.execution_budget_us) {
                return Err(GraphError::InvalidBlocking {
                    task: decl.name,
                    budget_us: decl.execution_budget_us,
                    blocking_us: decl.blocking_us,
                    period_us: decl.period_us,
                });
            }
            let (_, module) = labels[index].expect("allocated above");
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
                        .execution_budget(decl.execution_budget_us),
                );
            }
            manifest.add(spec).map_err(|error| GraphError::Manifest {
                task: decl.name,
                error,
            })?;

            // Startup edges by name -> node-index bits (kernel is node 0).
            let mut depends = DependencySet::empty().with_index(0);
            for dep_name in decl.after.iter().flatten() {
                let dep_index = self
                    .decl_index(dep_name)
                    .ok_or(GraphError::UnknownDependency {
                        task: decl.name,
                        depends_on: dep_name,
                    })?;
                depends = depends.with_index(dep_index + 1);
            }
            startup[index + 1] = StartupNode::new(module, depends);
            tasks[index] = Some(
                TaskMeta::new(
                    module,
                    decl.criticality,
                    decl.period_us,
                    decl.execution_budget_us,
                )
                .with_blocking_us(decl.blocking_us),
            );
        }

        // Cycle/consistency check with attribution: the planner sees the same
        // nodes admission will see.
        let startup_len = self.len + 1;
        if let Err(error) = StartupPlanner::plan::<MODULES>(&startup[..startup_len]) {
            let task = match error {
                StartupError::Cycle => self
                    .first_task_in_cycle(&startup[..startup_len])
                    .unwrap_or("<unknown>"),
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
                    labels
                        .iter()
                        .flatten()
                        .find(|(_, owner)| *owner == module)
                        .map(|(label, _)| *label)
                })
                .unwrap_or("<manifest>");
            return Err(GraphError::Manifest { task, error });
        }

        Ok(BuiltGraph {
            manifest,
            startup,
            startup_len,
            tasks,
            task_len: self.len,
            labels,
        })
    }

    /// Attribute a startup cycle to the first task on a back-edge.
    fn first_task_in_cycle(&self, nodes: &[StartupNode]) -> Option<&'static str> {
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
                return self.tasks[index.checked_sub(1)?].map(|task| task.name);
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
}

impl<const TASKS: usize> Default for AppGraph<TASKS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FaultThresholds;

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
}
