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
    pub const fn new(module: ModuleId, depends_on: DependencySet) -> Self {
        Self { module, depends_on }
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
}
