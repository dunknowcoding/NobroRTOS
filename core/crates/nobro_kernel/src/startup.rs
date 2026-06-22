//! Static startup ordering for module dependencies.

use crate::ModuleId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DependencySet(u32);

impl DependencySet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn with_index(self, idx: usize) -> Self {
        Self(self.0 | (1u32 << idx))
    }

    pub const fn contains_index(self, idx: usize) -> bool {
        (self.0 & (1u32 << idx)) != 0
    }

    pub const fn without_index(self, idx: usize) -> Self {
        Self(self.0 & !(1u32 << idx))
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupNode {
    pub module: ModuleId,
    pub depends_on: DependencySet,
}

impl StartupNode {
    pub const EMPTY: Self = Self {
        module: ModuleId::Kernel,
        depends_on: DependencySet::empty(),
    };

    pub const fn new(module: ModuleId, depends_on: DependencySet) -> Self {
        Self { module, depends_on }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupGraphError {
    TooManyNodes,
    DuplicateModule(ModuleId),
    DuplicateDependency {
        module: ModuleId,
        depends_on: ModuleId,
    },
    UnknownModule(ModuleId),
    InvalidPlan(StartupError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupGraph<const N: usize> {
    nodes: [StartupNode; N],
    len: usize,
}

impl<const N: usize> StartupGraph<N> {
    pub const fn new() -> Self {
        Self {
            nodes: [StartupNode::EMPTY; N],
            len: 0,
        }
    }

    pub fn from_modules(modules: &[ModuleId]) -> Result<Self, StartupGraphError> {
        let mut graph = Self::new();
        for module in modules {
            graph.add(*module)?;
        }
        Ok(graph)
    }

    pub fn add(&mut self, module: ModuleId) -> Result<(), StartupGraphError> {
        if self.len == N || self.len == 32 {
            return Err(StartupGraphError::TooManyNodes);
        }
        if self.index_of(module).is_some() {
            return Err(StartupGraphError::DuplicateModule(module));
        }

        self.nodes[self.len] = StartupNode::new(module, DependencySet::empty());
        self.len += 1;
        Ok(())
    }

    pub fn add_dependency(
        &mut self,
        module: ModuleId,
        depends_on: ModuleId,
    ) -> Result<(), StartupGraphError> {
        let Some(module_idx) = self.index_of(module) else {
            return Err(StartupGraphError::UnknownModule(module));
        };
        let Some(dep_idx) = self.index_of(depends_on) else {
            return Err(StartupGraphError::UnknownModule(depends_on));
        };
        if self.nodes[module_idx].depends_on.contains_index(dep_idx) {
            return Err(StartupGraphError::DuplicateDependency { module, depends_on });
        }

        self.nodes[module_idx].depends_on = self.nodes[module_idx].depends_on.with_index(dep_idx);
        Ok(())
    }

    pub fn plan<const OUT: usize>(&self) -> Result<StartupPlan<OUT>, StartupError> {
        StartupPlanner::plan(self.as_slice())
    }

    pub fn dependency_impact<const OUT: usize>(
        &self,
        root: ModuleId,
    ) -> Result<DependencyImpact<OUT>, StartupGraphError> {
        let Some(root_idx) = self.index_of(root) else {
            return Err(StartupGraphError::UnknownModule(root));
        };
        let startup = self.plan::<N>().map_err(StartupGraphError::InvalidPlan)?;
        let mut affected = DependencySet::empty();
        let mut changed = true;

        while changed {
            changed = false;
            for idx in 0..self.len {
                if idx == root_idx || affected.contains_index(idx) {
                    continue;
                }
                let deps = self.nodes[idx].depends_on;
                if deps.contains_index(root_idx) || deps.intersects(affected) {
                    affected = affected.with_index(idx);
                    changed = true;
                }
            }
        }

        let mut impact = DependencyImpact::new(root);
        for module in startup
            .order
            .iter()
            .copied()
            .take(startup.len)
            .rev()
            .flatten()
        {
            let Some(idx) = self.index_of(module) else {
                continue;
            };
            if affected.contains_index(idx) {
                impact.push(module)?;
            }
        }
        Ok(impact)
    }

    pub fn as_slice(&self) -> &[StartupNode] {
        &self.nodes[..self.len]
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn index_of(&self, module: ModuleId) -> Option<usize> {
        self.nodes
            .iter()
            .take(self.len)
            .position(|node| node.module == module)
    }
}

impl<const N: usize> Default for StartupGraph<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DependencyImpact<const N: usize> {
    pub root: ModuleId,
    pub affected: [Option<ModuleId>; N],
    pub affected_count: usize,
}

impl<const N: usize> DependencyImpact<N> {
    pub const fn new(root: ModuleId) -> Self {
        Self {
            root,
            affected: [None; N],
            affected_count: 0,
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.affected_count == 0
    }

    pub fn contains(&self, module: ModuleId) -> bool {
        self.affected
            .iter()
            .take(self.affected_count)
            .any(|entry| *entry == Some(module))
    }

    fn push(&mut self, module: ModuleId) -> Result<(), StartupGraphError> {
        if self.affected_count == N {
            return Err(StartupGraphError::TooManyNodes);
        }
        self.affected[self.affected_count] = Some(module);
        self.affected_count += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupError {
    TooManyNodes,
    DuplicateModule(ModuleId),
    MissingDependencyBits(u32),
    Cycle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupPlan<const N: usize> {
    pub order: [Option<ModuleId>; N],
    pub len: usize,
}

pub struct StartupPlanner;

impl StartupPlanner {
    pub fn plan<const N: usize>(nodes: &[StartupNode]) -> Result<StartupPlan<N>, StartupError> {
        if nodes.len() > N || nodes.len() > 32 {
            return Err(StartupError::TooManyNodes);
        }

        for (idx, node) in nodes.iter().enumerate() {
            if nodes
                .iter()
                .skip(idx + 1)
                .any(|other| other.module == node.module)
            {
                return Err(StartupError::DuplicateModule(node.module));
            }
            let allowed = if nodes.len() == 32 {
                u32::MAX
            } else {
                (1u32 << nodes.len()) - 1
            };
            let missing = node.depends_on.bits() & !allowed;
            if missing != 0 {
                return Err(StartupError::MissingDependencyBits(missing));
            }
        }

        let mut remaining = [DependencySet::empty(); N];
        let mut emitted = [false; N];
        for (idx, node) in nodes.iter().enumerate() {
            remaining[idx] = node.depends_on;
        }

        let mut plan = StartupPlan {
            order: [None; N],
            len: 0,
        };

        while plan.len < nodes.len() {
            let mut progress = false;
            for idx in 0..nodes.len() {
                if emitted[idx] || !remaining[idx].is_empty() {
                    continue;
                }

                emitted[idx] = true;
                plan.order[plan.len] = Some(nodes[idx].module);
                plan.len += 1;
                progress = true;

                for deps in remaining.iter_mut().take(nodes.len()) {
                    *deps = deps.without_index(idx);
                }
            }

            if !progress {
                return Err(StartupError::Cycle);
            }
        }

        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_orders_dependencies_before_dependents() {
        let nodes = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Hal, DependencySet::empty().with_index(0)),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(1)),
            StartupNode::new(ModuleId::App(1), DependencySet::empty().with_index(2)),
        ];

        let plan = StartupPlanner::plan::<4>(&nodes).unwrap();

        assert_eq!(plan.len, 4);
        assert_eq!(plan.order[0], Some(ModuleId::Kernel));
        assert_eq!(plan.order[1], Some(ModuleId::Hal));
        assert_eq!(plan.order[2], Some(ModuleId::Sensor));
        assert_eq!(plan.order[3], Some(ModuleId::App(1)));
    }

    #[test]
    fn planner_detects_cycle() {
        let nodes = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty().with_index(1)),
            StartupNode::new(ModuleId::Hal, DependencySet::empty().with_index(0)),
        ];

        assert_eq!(StartupPlanner::plan::<2>(&nodes), Err(StartupError::Cycle));
    }

    #[test]
    fn planner_detects_missing_dependency_bit() {
        let nodes = [StartupNode::new(
            ModuleId::Kernel,
            DependencySet::empty().with_index(2),
        )];

        assert_eq!(
            StartupPlanner::plan::<1>(&nodes),
            Err(StartupError::MissingDependencyBits(0b100))
        );
    }

    #[test]
    fn graph_builds_startup_dependencies_by_module_id() {
        let mut graph = StartupGraph::<4>::from_modules(&[
            ModuleId::Kernel,
            ModuleId::Hal,
            ModuleId::Sensor,
            ModuleId::App(1),
        ])
        .unwrap();
        graph
            .add_dependency(ModuleId::Hal, ModuleId::Kernel)
            .unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Hal)
            .unwrap();
        graph
            .add_dependency(ModuleId::App(1), ModuleId::Sensor)
            .unwrap();

        let plan = graph.plan::<4>().unwrap();

        assert_eq!(graph.len(), 4);
        assert_eq!(plan.order[0], Some(ModuleId::Kernel));
        assert_eq!(plan.order[1], Some(ModuleId::Hal));
        assert_eq!(plan.order[2], Some(ModuleId::Sensor));
        assert_eq!(plan.order[3], Some(ModuleId::App(1)));
    }

    #[test]
    fn graph_rejects_duplicate_and_unknown_modules() {
        let mut graph = StartupGraph::<2>::new();
        graph.add(ModuleId::Kernel).unwrap();

        assert_eq!(
            graph.add(ModuleId::Kernel),
            Err(StartupGraphError::DuplicateModule(ModuleId::Kernel))
        );
        assert_eq!(
            graph.add_dependency(ModuleId::Sensor, ModuleId::Kernel),
            Err(StartupGraphError::UnknownModule(ModuleId::Sensor))
        );
        assert_eq!(
            graph.add_dependency(ModuleId::Kernel, ModuleId::Sensor),
            Err(StartupGraphError::UnknownModule(ModuleId::Sensor))
        );
    }

    #[test]
    fn graph_rejects_duplicate_dependencies() {
        let mut graph =
            StartupGraph::<2>::from_modules(&[ModuleId::Kernel, ModuleId::Sensor]).unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Kernel)
            .unwrap();

        assert_eq!(
            graph.add_dependency(ModuleId::Sensor, ModuleId::Kernel),
            Err(StartupGraphError::DuplicateDependency {
                module: ModuleId::Sensor,
                depends_on: ModuleId::Kernel,
            })
        );
    }

    #[test]
    fn graph_reports_transitive_fault_impact_in_reverse_startup_order() {
        let mut graph = StartupGraph::<5>::from_modules(&[
            ModuleId::Kernel,
            ModuleId::Hal,
            ModuleId::Sensor,
            ModuleId::Radio,
            ModuleId::App(1),
        ])
        .unwrap();
        graph
            .add_dependency(ModuleId::Hal, ModuleId::Kernel)
            .unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Hal)
            .unwrap();
        graph
            .add_dependency(ModuleId::Radio, ModuleId::Hal)
            .unwrap();
        graph
            .add_dependency(ModuleId::App(1), ModuleId::Sensor)
            .unwrap();

        let impact = graph.dependency_impact::<4>(ModuleId::Hal).unwrap();

        assert_eq!(impact.root, ModuleId::Hal);
        assert_eq!(impact.affected_count, 3);
        assert_eq!(impact.affected[0], Some(ModuleId::App(1)));
        assert_eq!(impact.affected[1], Some(ModuleId::Radio));
        assert_eq!(impact.affected[2], Some(ModuleId::Sensor));
        assert!(impact.contains(ModuleId::Sensor));
        assert!(!impact.contains(ModuleId::Kernel));
    }

    #[test]
    fn graph_fault_impact_reports_capacity_unknown_and_cycle_errors() {
        let mut graph = StartupGraph::<4>::from_modules(&[
            ModuleId::Kernel,
            ModuleId::Hal,
            ModuleId::Sensor,
            ModuleId::App(1),
        ])
        .unwrap();
        graph
            .add_dependency(ModuleId::Hal, ModuleId::Kernel)
            .unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Hal)
            .unwrap();
        graph
            .add_dependency(ModuleId::App(1), ModuleId::Sensor)
            .unwrap();

        assert_eq!(
            graph.dependency_impact::<1>(ModuleId::Hal),
            Err(StartupGraphError::TooManyNodes)
        );
        assert_eq!(
            graph.dependency_impact::<4>(ModuleId::Radio),
            Err(StartupGraphError::UnknownModule(ModuleId::Radio))
        );

        let mut cyclic =
            StartupGraph::<2>::from_modules(&[ModuleId::Kernel, ModuleId::Hal]).unwrap();
        cyclic
            .add_dependency(ModuleId::Kernel, ModuleId::Hal)
            .unwrap();
        cyclic
            .add_dependency(ModuleId::Hal, ModuleId::Kernel)
            .unwrap();
        assert_eq!(
            cyclic.dependency_impact::<2>(ModuleId::Kernel),
            Err(StartupGraphError::InvalidPlan(StartupError::Cycle))
        );
    }

    #[test]
    fn graph_preserves_cycle_detection() {
        let mut graph =
            StartupGraph::<2>::from_modules(&[ModuleId::Kernel, ModuleId::Hal]).expect("graph");
        graph
            .add_dependency(ModuleId::Kernel, ModuleId::Hal)
            .unwrap();
        graph
            .add_dependency(ModuleId::Hal, ModuleId::Kernel)
            .unwrap();

        assert_eq!(graph.plan::<2>(), Err(StartupError::Cycle));
    }
}
