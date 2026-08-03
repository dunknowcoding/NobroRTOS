//! Transactional dependencies established after static startup.

use crate::startup::{DependencyImpact, StartupGraph, StartupGraphError};
use crate::ModuleId;

/// A bounded, generation-tagged dependency graph for relationships established
/// after boot, such as a client bound to a newly mounted service.
///
/// Updates are transactional: a candidate graph must remain acyclic before its
/// generation is published. Recovery plans retain that generation and reject a
/// changed graph before executing any lifecycle hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeDependencyGraph<const N: usize> {
    graph: StartupGraph<N>,
    pub(crate) generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeDependencyError {
    Graph(StartupGraphError),
    GenerationExhausted,
    StaleGeneration { expected: u32, current: u32 },
}

impl RuntimeDependencyError {
    /// Stable machine-readable error category for logs that cannot retain the
    /// full enum payload. The enum remains the authoritative typed surface.
    pub const fn category(self) -> &'static str {
        match self {
            Self::Graph(_) => "graph",
            Self::GenerationExhausted => "generation-exhausted",
            Self::StaleGeneration { .. } => "stale-generation",
        }
    }
}

impl<const N: usize> RuntimeDependencyGraph<N> {
    pub fn from_startup(graph: StartupGraph<N>) -> Result<Self, RuntimeDependencyError> {
        graph.plan::<N>().map_err(|error| {
            RuntimeDependencyError::Graph(StartupGraphError::InvalidPlan(error))
        })?;
        Ok(Self {
            graph,
            generation: 1,
        })
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub const fn graph(&self) -> &StartupGraph<N> {
        &self.graph
    }

    pub fn bind(
        &mut self,
        module: ModuleId,
        depends_on: ModuleId,
    ) -> Result<u32, RuntimeDependencyError> {
        self.update(|candidate| candidate.add_dependency(module, depends_on))
    }

    pub fn unbind(
        &mut self,
        module: ModuleId,
        depends_on: ModuleId,
    ) -> Result<u32, RuntimeDependencyError> {
        self.update(|candidate| candidate.remove_dependency(module, depends_on))
    }

    pub fn dependency_impact<const OUT: usize>(
        &self,
        root: ModuleId,
    ) -> Result<RuntimeDependencyImpact<OUT>, RuntimeDependencyError> {
        Ok(RuntimeDependencyImpact {
            generation: self.generation,
            impact: self
                .graph
                .dependency_impact(root)
                .map_err(RuntimeDependencyError::Graph)?,
        })
    }

    pub const fn revalidate(&self, generation: u32) -> Result<(), RuntimeDependencyError> {
        if generation == self.generation {
            Ok(())
        } else {
            Err(RuntimeDependencyError::StaleGeneration {
                expected: generation,
                current: self.generation,
            })
        }
    }

    fn update(
        &mut self,
        mutate: impl FnOnce(&mut StartupGraph<N>) -> Result<(), StartupGraphError>,
    ) -> Result<u32, RuntimeDependencyError> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(RuntimeDependencyError::GenerationExhausted)?;
        let mut candidate = self.graph;
        mutate(&mut candidate).map_err(RuntimeDependencyError::Graph)?;
        candidate.plan::<N>().map_err(|error| {
            RuntimeDependencyError::Graph(StartupGraphError::InvalidPlan(error))
        })?;
        self.graph = candidate;
        self.generation = generation;
        Ok(generation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeDependencyImpact<const N: usize> {
    pub generation: u32,
    pub impact: DependencyImpact<N>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::DependencySet;

    fn graph() -> StartupGraph<3> {
        let mut graph = StartupGraph::new();
        graph.add(ModuleId::Kernel).unwrap();
        graph.add(ModuleId::Bus).unwrap();
        graph.add(ModuleId::Sensor).unwrap();
        graph
            .add_dependency(ModuleId::Bus, ModuleId::Kernel)
            .unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Bus)
            .unwrap();
        assert_eq!(graph.as_slice()[0].depends_on, DependencySet::empty());
        graph
    }

    #[test]
    fn error_categories_are_stable_and_payload_free() {
        assert_eq!(
            RuntimeDependencyError::GenerationExhausted.category(),
            "generation-exhausted"
        );
        assert_eq!(
            RuntimeDependencyError::StaleGeneration {
                expected: 1,
                current: 2,
            }
            .category(),
            "stale-generation"
        );
        assert_eq!(
            RuntimeDependencyError::Graph(StartupGraphError::TooManyNodes).category(),
            "graph"
        );
    }

    #[test]
    fn failed_cycle_update_is_transactional() {
        let mut runtime = RuntimeDependencyGraph::from_startup(graph()).unwrap();
        let generation = runtime.generation();
        assert!(matches!(
            runtime.bind(ModuleId::Kernel, ModuleId::Sensor),
            Err(RuntimeDependencyError::Graph(
                StartupGraphError::InvalidPlan(_)
            ))
        ));
        assert_eq!(runtime.generation(), generation);
        assert_eq!(runtime.graph(), &graph());
    }
}
