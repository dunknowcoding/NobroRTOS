"""Host-side helpers for NobroRTOS contracts."""

from .contracts import (
    AiBackendKind,
    AiModelContract,
    Capability,
    CONTRACT_SCHEMA_VERSION,
    Criticality,
    MemoryBudget,
    ModuleSpec,
    NobroContractBundle,
    RosAction,
    RosBridgeDescriptor,
    RosParameter,
    RosService,
    RosTopic,
    capability_mask,
    capabilities_from_mask,
)
from .host_contract import (
    BootDiagnostic,
    HostContract,
    ReportStatusClass,
    find_repo_root,
    load_repo_host_contract,
)
from .reports import FixedReport, ReportKind, ReportStatus, seal_report

__all__ = [
    "AiBackendKind",
    "AiModelContract",
    "Capability",
    "CONTRACT_SCHEMA_VERSION",
    "Criticality",
    "MemoryBudget",
    "ModuleSpec",
    "NobroContractBundle",
    "RosAction",
    "RosBridgeDescriptor",
    "RosParameter",
    "RosService",
    "RosTopic",
    "capability_mask",
    "capabilities_from_mask",
    "find_repo_root",
    "BootDiagnostic",
    "HostContract",
    "load_repo_host_contract",
    "ReportStatusClass",
    "FixedReport",
    "ReportKind",
    "ReportStatus",
    "seal_report",
]
