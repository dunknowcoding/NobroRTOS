"""Command-line helpers for NobroRTOS Python tooling."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile

from .contracts import (
    AiBackendKind,
    AiModelContract,
    AiRoutePolicy,
    AiRoutePreference,
    AiRuntimeState,
    Capability,
    Criticality,
    MemoryBudget,
    ModuleSpec,
    NobroContractBundle,
    RosBridgeDescriptor,
    RosParameter,
    RosService,
    RosTopic,
    StartupDependency,
    plan_startup,
    stable_hash32,
)
from .distribution import validate_distribution_metadata, validate_public_header_surface
from .host_contract import BootDiagnostic, load_repo_host_contract
from .reports import BootReportSummary, FixedReport, ReportKind, seal_report
from .templates import (
    ProjectTarget,
    build_project_template,
    materialize_project_template,
    repair_project_template,
    validate_project_template,
)
from .sim import (
    DegradePlannerSimulator,
    EventLogSimulator,
    KernelErrorKind,
    QuotaLedgerSimulator,
    RecoveryPolicySimulator,
    RecoverySummary,
    ResourceBudget,
    RuntimeDrillSimulator,
    SchedulerSimulator,
    SensorStubError,
    SensorStubMode,
    SensorStubSimulator,
    ServoSimulator,
    SystemProfile,
    WatchdogSimulator,
)


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="python -m nobro_rtos",
        description="Generate or inspect NobroRTOS host-side contracts.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser(
        "sample-ai-ros",
        help="print a sample AI and ROS bridge contract bundle as JSON",
    )
    subparsers.add_parser(
        "sample-ai-route",
        help="print a sample AI route policy decision as JSON",
    )
    check_ai_route = subparsers.add_parser(
        "check-ai-route",
        help="run an AI route decision gate and print pass/fail JSON",
    )
    check_ai_route.add_argument(
        "--backend",
        choices=("on_device", "remote_api", "edge_sidecar", "hybrid"),
        default="hybrid",
        help="AI backend contract to simulate",
    )
    check_ai_route.add_argument(
        "--preference",
        choices=("local_only", "prefer_local", "prefer_remote", "hybrid_fallback"),
        default="hybrid_fallback",
        help="AI route preference to simulate",
    )
    check_ai_route.add_argument("--budget-us", type=int, default=25_000)
    check_ai_route.add_argument("--timeout-us", type=int, default=20_000)
    check_ai_route.add_argument("--stale-after-us", type=int, default=50_000)
    check_ai_route.add_argument("--model-stale-after-us", type=int, default=100_000)
    check_ai_route.add_argument("--endpoint-failure-limit", type=int, default=2)
    check_ai_route.add_argument("--last-success-age-us", type=int, default=12_000)
    check_ai_route.add_argument("--endpoint-failures", type=int, default=1)
    check_ai_route.add_argument(
        "--local-ready",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="whether local inference is ready",
    )
    check_ai_route.add_argument(
        "--endpoint-ready",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="whether a remote or sidecar endpoint is ready",
    )
    check_ai_route.add_argument(
        "--require-target",
        choices=(
            "on_device",
            "remote_api",
            "edge_sidecar",
            "stale_snapshot",
            "degraded_fallback",
            "unavailable",
        ),
        default=None,
        help="require a specific route target",
    )
    check_ai_route.add_argument(
        "--allow-stale",
        action="store_true",
        help="allow stale snapshot reuse",
    )
    check_ai_route.add_argument(
        "--allow-degraded",
        action="store_true",
        help="allow degraded fallback routing",
    )
    check_ai_route.add_argument(
        "--allow-unavailable",
        action="store_true",
        help="allow unavailable route decisions",
    )
    check_ai_route.add_argument(
        "--allow-endpoint-circuit-open",
        action="store_true",
        help="allow an open remote endpoint circuit breaker",
    )
    subparsers.add_parser(
        "check-ai-route-matrix",
        help="run deterministic AI route checks across local, edge, remote, and fallback paths",
    )
    sample_report = subparsers.add_parser(
        "sample-report",
        help="print a sealed sample fixed report as JSON",
    )
    sample_report.add_argument(
        "kind",
        choices=(
            "admission",
            "runtime",
            "health",
            "event_log",
            "module_runtime",
            "degrade_application",
            "ai_model",
            "ros_bridge",
        ),
        help="sample report kind",
    )
    sample_sensor = subparsers.add_parser(
        "sample-sensor",
        help="run the deterministic sensor-stub simulator and print JSON",
    )
    sample_sensor.add_argument(
        "--mode",
        choices=("nominal", "silent", "error_every", "bad_data_every"),
        default="nominal",
        help="sensor fixture mode",
    )
    sample_sensor.add_argument(
        "--ticks",
        type=int,
        default=8,
        help="number of simulated polls",
    )
    sample_sensor.add_argument(
        "--period",
        type=int,
        default=2,
        help="sample period in ticks",
    )
    sample_sensor.add_argument(
        "--fault-period",
        type=int,
        default=2,
        help="fault period for error_every or bad_data_every modes",
    )
    sample_actuator = subparsers.add_parser(
        "sample-actuator",
        help="run the deterministic servo actuator simulator and print JSON",
    )
    sample_actuator.add_argument("--start-us", type=int, default=1200)
    sample_actuator.add_argument("--stop-us", type=int, default=1800)
    sample_actuator.add_argument("--step-us", type=int, default=300)
    sample_actuator.add_argument("--readback-offset-us", type=int, default=0)
    sample_actuator.add_argument("--tolerance-us", type=int, default=50)
    sample_recovery = subparsers.add_parser(
        "sample-recovery",
        help="run a deterministic recovery escalation simulation and print JSON",
    )
    sample_recovery.add_argument("--module", default="sensor")
    sample_recovery.add_argument(
        "--error",
        choices=tuple(item.value for item in KernelErrorKind),
        default=KernelErrorKind.SENSOR_READ_FAIL.value,
    )
    sample_recovery.add_argument("--events", type=int, default=4)
    sample_recovery.add_argument("--notify-after", type=int, default=2)
    sample_recovery.add_argument("--reboot-after", type=int, default=4)
    sample_recovery.add_argument(
        "--ok-after",
        type=int,
        default=0,
        help="insert an OK event after this many errors; 0 disables it",
    )
    subparsers.add_parser(
        "check-recovery-matrix",
        help="run deterministic self-healing recovery checks",
    )
    sample_watchdog = subparsers.add_parser(
        "sample-watchdog",
        help="run a deterministic watchdog heartbeat simulation and print JSON",
    )
    sample_watchdog.add_argument("--module", default="sensor")
    sample_watchdog.add_argument("--timeout-us", type=int, default=100)
    sample_watchdog.add_argument("--sweeps", type=int, default=3)
    sample_watchdog.add_argument("--step-us", type=int, default=75)
    sample_watchdog.add_argument(
        "--beat-at-sweep",
        type=int,
        default=0,
        help="insert a heartbeat before this sweep; 0 disables it",
    )
    subparsers.add_parser(
        "check-watchdog-matrix",
        help="run deterministic watchdog liveness checks",
    )
    subparsers.add_parser(
        "check-scheduler-matrix",
        help="run deterministic scheduler deadline checks",
    )
    sample_scheduler = subparsers.add_parser(
        "sample-scheduler",
        help="run a deterministic deadline tick simulation and print JSON",
    )
    sample_scheduler.add_argument(
        "--ticks",
        nargs="+",
        type=int,
        default=(1000, 21020, 41050),
        help="deadline tick timestamps in microseconds",
    )
    sample_scheduler.add_argument("--period-us", type=int, default=20_000)
    sample_scheduler.add_argument("--tolerance-us", type=int, default=10)
    sample_event_log = subparsers.add_parser(
        "sample-event-log",
        help="run a deterministic fixed-ring event log simulation and print JSON",
    )
    sample_event_log.add_argument("--capacity", type=int, default=3)
    sample_event_log.add_argument("--events", type=int, default=4)
    sample_event_log.add_argument("--recent", type=int, default=3)
    subparsers.add_parser(
        "check-event-log-matrix",
        help="run deterministic fixed-ring event log checks",
    )
    subparsers.add_parser(
        "sample-quota",
        help="run a deterministic quota ledger simulation and print JSON",
    )
    subparsers.add_parser(
        "check-quota-matrix",
        help="run deterministic fixed-capacity quota ledger checks",
    )
    sample_degrade = subparsers.add_parser(
        "sample-degrade",
        help="run a deterministic degraded-mode planning simulation and print JSON",
    )
    sample_degrade.add_argument("--flash-limit", type=int, default=72 * 1024)
    sample_degrade.add_argument("--ram-limit", type=int, default=16 * 1024)
    sample_degrade.add_argument("--pool-limit", type=int, default=5)
    sample_degrade.add_argument("--max-modules", type=int, default=4)
    subparsers.add_parser(
        "check-degrade-matrix",
        help="run deterministic degraded-mode planner checks",
    )
    sample_runtime_drill = subparsers.add_parser(
        "sample-runtime-drill",
        help="run a combined runtime pressure drill and print JSON",
    )
    sample_runtime_drill.add_argument("--flash-limit", type=int, default=72 * 1024)
    sample_runtime_drill.add_argument("--ram-limit", type=int, default=16 * 1024)
    sample_runtime_drill.add_argument("--pool-limit", type=int, default=5)
    sample_runtime_drill.add_argument("--max-modules", type=int, default=4)
    sample_runtime_drill.add_argument("--fault-module", default="sensor")
    sample_runtime_drill.add_argument(
        "--fault-error",
        choices=tuple(item.value for item in KernelErrorKind),
        default=KernelErrorKind.SENSOR_READ_FAIL.value,
    )
    sample_runtime_drill.add_argument("--fault-count", type=int, default=3)
    check_runtime_drill = subparsers.add_parser(
        "check-runtime-drill",
        help="run a runtime drill gate and print pass/fail JSON",
    )
    check_runtime_drill.add_argument("--flash-limit", type=int, default=72 * 1024)
    check_runtime_drill.add_argument("--ram-limit", type=int, default=16 * 1024)
    check_runtime_drill.add_argument("--pool-limit", type=int, default=5)
    check_runtime_drill.add_argument("--max-modules", type=int, default=4)
    check_runtime_drill.add_argument("--fault-module", default="sensor")
    check_runtime_drill.add_argument(
        "--fault-error",
        choices=tuple(item.value for item in KernelErrorKind),
        default=KernelErrorKind.SENSOR_READ_FAIL.value,
    )
    check_runtime_drill.add_argument("--fault-count", type=int, default=3)
    check_runtime_drill.add_argument("--max-disabled", type=int, default=1)
    check_runtime_drill.add_argument("--max-reboots", type=int, default=0)
    check_runtime_drill.add_argument("--max-dropped-events", type=int, default=1)
    subparsers.add_parser(
        "sample-startup",
        help="print a deterministic startup dependency plan as JSON",
    )
    subparsers.add_parser(
        "check-startup-matrix",
        help="run deterministic startup dependency planner checks",
    )
    subparsers.add_parser(
        "check-boot-summary-matrix",
        help="run deterministic boot report summary checks",
    )
    sample_project = subparsers.add_parser(
        "sample-project",
        help="print a starter project template as JSON without writing files",
    )
    sample_project.add_argument(
        "target",
        choices=tuple(item.value for item in ProjectTarget),
        help="starter project target",
    )
    sample_project.add_argument("--name", default="nobro_edge_app")
    sample_project.add_argument("--module", default="app")
    sample_project.add_argument("--author", default="dunknowcoding")
    write_project = subparsers.add_parser(
        "write-project",
        help="write a starter project template to an output directory",
    )
    write_project.add_argument(
        "target",
        choices=tuple(item.value for item in ProjectTarget),
        help="starter project target",
    )
    write_project.add_argument("--output", required=True, help="output directory")
    write_project.add_argument("--name", default="nobro_edge_app")
    write_project.add_argument("--module", default="app")
    write_project.add_argument("--author", default="dunknowcoding")
    write_project.add_argument(
        "--overwrite",
        action="store_true",
        help="allow generated files to overwrite existing files",
    )
    check_project = subparsers.add_parser(
        "check-project",
        help="validate a generated starter project directory",
    )
    check_project.add_argument("path", help="starter project directory")
    check_project.add_argument(
        "--target",
        choices=tuple(item.value for item in ProjectTarget),
        default=None,
        help="expected starter project target",
    )
    repair_project = subparsers.add_parser(
        "repair-project",
        help="repair generated starter project IDE metadata",
    )
    repair_project.add_argument("path", help="starter project directory")
    repair_project.add_argument(
        "--target",
        choices=tuple(item.value for item in ProjectTarget),
        default=None,
        help="expected starter project target",
    )
    subparsers.add_parser(
        "check-starter-templates",
        help="materialize and validate every starter project template",
    )
    subparsers.add_parser(
        "check-host-contract",
        help="validate host/nobro-host-contract.json against Python enums",
    )
    subparsers.add_parser(
        "check-distribution-metadata",
        help="validate SDK, Arduino, and PlatformIO package metadata",
    )
    subparsers.add_parser(
        "check-public-headers",
        help="validate public C/C++/Arduino/PlatformIO header surfaces",
    )
    subparsers.add_parser(
        "check-software-surface",
        help="run the host contract, package, header, AI route, and runtime gates",
    )
    subparsers.add_parser(
        "doctor",
        help="run host contract and package metadata checks and print JSON",
    )
    decode_boot = subparsers.add_parser(
        "decode-boot",
        help="decode a boot diagnostic code into stage, status, and error label",
    )
    decode_boot.add_argument("code", help="diagnostic code as decimal or 0x-prefixed hex")
    validate_bundle = subparsers.add_parser(
        "validate-bundle",
        help="load and validate a NobroRTOS contract bundle JSON file",
    )
    validate_bundle.add_argument("path", help="path to a contract bundle JSON file")
    decode_report = subparsers.add_parser(
        "decode-report",
        help="decode a board, manifest, or adapter report JSON file",
    )
    decode_report.add_argument(
        "kind",
        choices=(
            "board_profile",
            "board_package",
            "manifest",
            "adapter_compatibility",
            "admission",
            "runtime",
            "health",
            "event_log",
            "module_runtime",
            "degrade_application",
            "ai_model",
            "ros_bridge",
        ),
        help="report kind",
    )
    decode_report.add_argument("path", help="path to a report JSON file")
    summarize_boot = subparsers.add_parser(
        "summarize-boot",
        help="summarize boot report JSON by first non-passing stage",
    )
    summarize_boot.add_argument("path", help="path to a boot report bundle JSON file")
    args = parser.parse_args()

    if args.command == "sample-ai-ros":
        print(_sample_ai_ros_bundle().to_json())
        return 0
    if args.command == "sample-ai-route":
        print(json.dumps(_sample_ai_route(), indent=2, sort_keys=True))
        return 0
    if args.command == "check-ai-route":
        report = _check_ai_route(
            args.backend,
            args.preference,
            args.budget_us,
            args.timeout_us,
            args.stale_after_us,
            args.model_stale_after_us,
            args.endpoint_failure_limit,
            args.local_ready,
            args.endpoint_ready,
            args.last_success_age_us,
            args.endpoint_failures,
            args.require_target,
            args.allow_stale,
            args.allow_degraded,
            args.allow_unavailable,
            args.allow_endpoint_circuit_open,
        )
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "check-ai-route-matrix":
        report = _check_ai_route_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-report":
        print(json.dumps(_sample_report(args.kind), indent=2, sort_keys=True))
        return 0
    if args.command == "sample-sensor":
        print(
            json.dumps(
                _sample_sensor(args.mode, args.ticks, args.period, args.fault_period),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "sample-actuator":
        print(
            json.dumps(
                _sample_actuator(
                    args.start_us,
                    args.stop_us,
                    args.step_us,
                    args.readback_offset_us,
                    args.tolerance_us,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "sample-recovery":
        print(
            json.dumps(
                _sample_recovery(
                    args.module,
                    args.error,
                    args.events,
                    args.notify_after,
                    args.reboot_after,
                    args.ok_after,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "check-recovery-matrix":
        report = _check_recovery_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-watchdog":
        print(
            json.dumps(
                _sample_watchdog(
                    args.module,
                    args.timeout_us,
                    args.sweeps,
                    args.step_us,
                    args.beat_at_sweep,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "check-watchdog-matrix":
        report = _check_watchdog_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "check-scheduler-matrix":
        report = _check_scheduler_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-scheduler":
        print(
            json.dumps(
                _sample_scheduler(
                    args.ticks,
                    args.period_us,
                    args.tolerance_us,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "sample-event-log":
        print(
            json.dumps(
                _sample_event_log(args.capacity, args.events, args.recent),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "check-event-log-matrix":
        report = _check_event_log_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-quota":
        print(json.dumps(_sample_quota(), indent=2, sort_keys=True))
        return 0
    if args.command == "check-quota-matrix":
        report = _check_quota_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-degrade":
        print(
            json.dumps(
                _sample_degrade(
                    args.flash_limit,
                    args.ram_limit,
                    args.pool_limit,
                    args.max_modules,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "check-degrade-matrix":
        report = _check_degrade_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-runtime-drill":
        print(
            json.dumps(
                _sample_runtime_drill(
                    args.flash_limit,
                    args.ram_limit,
                    args.pool_limit,
                    args.max_modules,
                    args.fault_module,
                    args.fault_error,
                    args.fault_count,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "check-runtime-drill":
        report = _check_runtime_drill(
            args.flash_limit,
            args.ram_limit,
            args.pool_limit,
            args.max_modules,
            args.fault_module,
            args.fault_error,
            args.fault_count,
            args.max_disabled,
            args.max_reboots,
            args.max_dropped_events,
        )
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-startup":
        print(json.dumps(_sample_startup(), indent=2, sort_keys=True))
        return 0
    if args.command == "check-startup-matrix":
        report = _check_startup_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "check-boot-summary-matrix":
        report = _check_boot_summary_matrix()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "sample-project":
        print(
            json.dumps(
                _sample_project(
                    args.target,
                    args.name,
                    args.module,
                    args.author,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "write-project":
        print(
            json.dumps(
                _write_project(
                    args.target,
                    args.output,
                    args.name,
                    args.module,
                    args.author,
                    args.overwrite,
                ),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "check-project":
        report = _check_project(args.path, args.target)
        print(
            json.dumps(
                report,
                indent=2,
                sort_keys=True,
            )
        )
        return 0 if report["passing"] else 1
    if args.command == "repair-project":
        report = _repair_project(args.path, args.target)
        print(
            json.dumps(
                report,
                indent=2,
                sort_keys=True,
            )
        )
        return 0 if report["passing"] else 1
    if args.command == "check-starter-templates":
        report = _check_starter_templates()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "check-host-contract":
        contract = load_repo_host_contract()
        stages = ", ".join(contract.boot_stage_order())
        print(f"host contract ok: {stages}")
        return 0
    if args.command == "check-distribution-metadata":
        report = validate_distribution_metadata()
        print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
        return 0
    if args.command == "check-public-headers":
        report = validate_public_header_surface()
        print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
        return 0
    if args.command == "check-software-surface":
        report = _check_software_surface()
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["passing"] else 1
    if args.command == "doctor":
        print(json.dumps(_doctor(), indent=2, sort_keys=True))
        return 0
    if args.command == "decode-boot":
        code = int(args.code, 0)
        diagnostic = BootDiagnostic.decode(code)
        print(json.dumps(diagnostic.to_dict(), indent=2, sort_keys=True))
        return 0
    if args.command == "validate-bundle":
        bundle = NobroContractBundle.from_file(args.path)
        bundle.validate()
        print(f"bundle ok: {len(bundle.modules)} modules")
        return 0
    if args.command == "decode-report":
        report = FixedReport.from_json_file(args.kind, args.path)
        print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
        return 0
    if args.command == "summarize-boot":
        summary = BootReportSummary.from_json_file(args.path)
        print(json.dumps(summary.to_dict(), indent=2, sort_keys=True))
        return 0

    parser.error(f"unknown command: {args.command}")
    return 2


def _sample_ai_ros_bundle() -> NobroContractBundle:
    return NobroContractBundle(
        metadata={"profile": "sample-ai-ros"},
        modules=(
            ModuleSpec(
                module="ai",
                criticality=Criticality.USER,
                memory=MemoryBudget(16 * 1024, 6 * 1024, 1),
                requires=(
                    Capability.AI_INFERENCE,
                    Capability.AI_ENDPOINT,
                    Capability.STREAM,
                ),
                owns=(
                    Capability.AI_INFERENCE,
                    Capability.AI_ENDPOINT,
                    Capability.STREAM,
                ),
            ),
        ),
        ai_models=(
            AiModelContract(
                model_id=42,
                backend=AiBackendKind.ON_DEVICE,
                input_bytes_max=128,
                output_bytes_max=32,
                arena_bytes=4096,
                timeout_us=20_000,
                stale_after_us=100_000,
            ),
        ),
        ros_bridges=(
            RosBridgeDescriptor(
                bridge_id="robot_core",
                transport="serial",
                topics=(RosTopic("/imu", "sensor_msgs/Imu", 4, 128),),
                services=(RosService("/reset", 16, 16, 50_000),),
                parameters=(RosParameter("mode", 16),),
            ),
        ),
        startup_dependencies=(),
    )


def _doctor() -> dict[str, object]:
    contract = load_repo_host_contract()
    distribution = validate_distribution_metadata()
    headers = validate_public_header_surface()
    return {
        "status": "ok",
        "host_contract": {
            "boot_stages": list(contract.boot_stage_order()),
            "capability_count": len(contract.payload.get("capability_bits", {})),
            "ai_backend_count": len(
                contract.payload.get("ai_contracts", {}).get("backend_codes", {})
            ),
            "ros_transport_count": len(
                contract.payload.get("ros_bridge_contracts", {}).get(
                    "transport_codes",
                    {},
                )
            ),
        },
        "distribution": distribution.to_dict(),
        "public_headers": headers.to_dict(),
        "host_simulators": [
            "sensor",
            "actuator",
            "recovery",
            "recovery_matrix_gate",
            "watchdog",
            "watchdog_matrix_gate",
            "scheduler",
            "scheduler_matrix_gate",
            "event_log",
            "event_log_matrix_gate",
            "quota",
            "quota_matrix_gate",
            "degrade",
            "degrade_matrix_gate",
            "startup_matrix_gate",
            "boot_summary_matrix_gate",
            "runtime_drill",
            "runtime_drill_gate",
            "ai_route_gate",
            "ai_route_matrix_gate",
            "project_templates",
            "starter_template_gate",
        ],
    }


def _check_starter_templates() -> dict[str, object]:
    targets: list[dict[str, object]] = []
    errors: list[str] = []

    with tempfile.TemporaryDirectory(prefix="nobro-starters-") as tmp:
        temp_root = Path(tmp)
        for target in ProjectTarget:
            project_name = f"starter_{target.value}"
            output = temp_root / target.value
            try:
                template = build_project_template(
                    name=project_name,
                    target=target,
                    module_name="control",
                    author="dunknowcoding",
                )
                materialization = materialize_project_template(template, output)
                validation = validate_project_template(output, expected_target=target)
                target_report = {
                    "target": target.value,
                    "passing": validation.passing,
                    "file_count": len(validation.files),
                    "module_count": validation.module_count,
                    "written_count": len(materialization.written),
                    "errors": list(validation.errors),
                }
                targets.append(target_report)
                for error in validation.errors:
                    errors.append(f"{target.value}: {error}")
            except Exception as exc:
                errors.append(f"{target.value}: {exc}")
                targets.append(
                    {
                        "target": target.value,
                        "passing": False,
                        "file_count": 0,
                        "module_count": 0,
                        "written_count": 0,
                        "errors": [str(exc)],
                    }
                )

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "target_count": len(targets),
        "targets": targets,
    }


def _check_software_surface() -> dict[str, object]:
    checks: dict[str, object] = {}
    errors: list[str] = []

    def add_check(name: str, report: dict[str, object]) -> None:
        checks[name] = report
        if not bool(report.get("passing", True)):
            for error in report.get("errors", ()):
                errors.append(f"{name}: {error}")

    try:
        contract = load_repo_host_contract()
        add_check(
            "host_contract",
            {
                "passing": True,
                "boot_stages": list(contract.boot_stage_order()),
                "capability_count": len(contract.payload.get("capability_bits", {})),
            },
        )
    except Exception as exc:
        add_check("host_contract", {"passing": False, "errors": [str(exc)]})

    try:
        add_check(
            "distribution_metadata",
            {
                "passing": True,
                "report": validate_distribution_metadata().to_dict(),
            },
        )
    except Exception as exc:
        add_check("distribution_metadata", {"passing": False, "errors": [str(exc)]})

    try:
        add_check(
            "public_headers",
            {
                "passing": True,
                "report": validate_public_header_surface().to_dict(),
            },
        )
    except Exception as exc:
        add_check("public_headers", {"passing": False, "errors": [str(exc)]})

    add_check("starter_templates", _check_starter_templates())
    add_check("ai_route_matrix", _check_ai_route_matrix())
    add_check("recovery_matrix", _check_recovery_matrix())
    add_check("watchdog_matrix", _check_watchdog_matrix())
    add_check("scheduler_matrix", _check_scheduler_matrix())
    add_check("event_log_matrix", _check_event_log_matrix())
    add_check("quota_matrix", _check_quota_matrix())
    add_check("degrade_matrix", _check_degrade_matrix())
    add_check("startup_matrix", _check_startup_matrix())
    add_check("boot_summary_matrix", _check_boot_summary_matrix())
    add_check(
        "ai_route",
        _check_ai_route(
            backend="hybrid",
            preference="hybrid_fallback",
            budget_us=25_000,
            timeout_us=20_000,
            stale_after_us=50_000,
            model_stale_after_us=100_000,
            endpoint_failure_limit=2,
            local_ready=True,
            endpoint_ready=False,
            last_success_age_us=12_000,
            endpoint_failures=1,
            require_target="on_device",
            allow_stale=False,
            allow_degraded=False,
            allow_unavailable=False,
            allow_endpoint_circuit_open=False,
        ),
    )
    add_check(
        "runtime_drill",
        _check_runtime_drill(
            flash_limit=72 * 1024,
            ram_limit=16 * 1024,
            pool_limit=5,
            max_modules=4,
            fault_module="sensor",
            fault_error="sensor_read_fail",
            fault_count=3,
            max_disabled=1,
            max_reboots=0,
            max_dropped_events=1,
        ),
    )

    return {
        "passing": len(errors) == 0,
        "status": "ok" if len(errors) == 0 else "fail",
        "errors": errors,
        "checks": checks,
    }


def _sample_ai_route() -> dict[str, object]:
    return _build_ai_route(
        backend="hybrid",
        preference="hybrid_fallback",
        budget_us=25_000,
        timeout_us=20_000,
        stale_after_us=50_000,
        model_stale_after_us=100_000,
        endpoint_failure_limit=2,
        local_ready=True,
        endpoint_ready=False,
        last_success_age_us=12_000,
        endpoint_failures=1,
    )


def _build_ai_route(
    backend: str,
    preference: str,
    budget_us: int,
    timeout_us: int,
    stale_after_us: int,
    model_stale_after_us: int,
    endpoint_failure_limit: int,
    local_ready: bool,
    endpoint_ready: bool,
    last_success_age_us: int,
    endpoint_failures: int,
) -> dict[str, object]:
    backend_kind = _ai_backend_from_label(backend)
    route_preference = _ai_preference_from_label(preference)
    contract = AiModelContract(
        model_id=42,
        backend=backend_kind,
        input_bytes_max=128,
        output_bytes_max=32,
        arena_bytes=(
            4096
            if backend_kind in (AiBackendKind.ON_DEVICE, AiBackendKind.HYBRID)
            else 0
        ),
        timeout_us=timeout_us,
        stale_after_us=model_stale_after_us,
    )
    policy = AiRoutePolicy(
        preference=route_preference,
        stale_after_us=stale_after_us,
        endpoint_failure_limit=endpoint_failure_limit,
    )
    state = AiRuntimeState(
        local_ready=local_ready,
        endpoint_ready=endpoint_ready,
        last_success_age_us=last_success_age_us,
        consecutive_endpoint_failures=endpoint_failures,
    )
    decision = policy.decide(contract, state, budget_us=budget_us)
    return {
        "contract": contract.to_dict(),
        "policy": {
            "preference": policy.preference.name.lower(),
            "stale_after_us": policy.stale_after_us,
            "endpoint_failure_limit": policy.endpoint_failure_limit,
        },
        "budget_us": budget_us,
        "state": {
            "local_ready": state.local_ready,
            "endpoint_ready": state.endpoint_ready,
            "last_success_age_us": state.last_success_age_us,
            "consecutive_endpoint_failures": state.consecutive_endpoint_failures,
        },
        "decision": decision.to_dict(),
    }


def _check_ai_route_matrix() -> dict[str, object]:
    scenarios: tuple[dict[str, object], ...] = (
        {
            "name": "local_on_device",
            "backend": "on_device",
            "preference": "local_only",
            "local_ready": True,
            "endpoint_ready": False,
            "last_success_age_us": 80_000,
            "endpoint_failures": 0,
            "require_target": "on_device",
        },
        {
            "name": "remote_api_ready",
            "backend": "remote_api",
            "preference": "prefer_remote",
            "local_ready": False,
            "endpoint_ready": True,
            "last_success_age_us": 80_000,
            "endpoint_failures": 0,
            "require_target": "remote_api",
        },
        {
            "name": "edge_sidecar_ready",
            "backend": "edge_sidecar",
            "preference": "prefer_remote",
            "local_ready": False,
            "endpoint_ready": True,
            "last_success_age_us": 80_000,
            "endpoint_failures": 0,
            "require_target": "edge_sidecar",
        },
        {
            "name": "hybrid_prefers_local",
            "backend": "hybrid",
            "preference": "prefer_local",
            "local_ready": True,
            "endpoint_ready": True,
            "last_success_age_us": 20_000,
            "endpoint_failures": 0,
            "require_target": "on_device",
        },
        {
            "name": "remote_circuit_uses_stale_snapshot",
            "backend": "remote_api",
            "preference": "prefer_remote",
            "local_ready": False,
            "endpoint_ready": True,
            "last_success_age_us": 10_000,
            "endpoint_failures": 2,
            "require_target": "stale_snapshot",
            "allow_stale": True,
            "allow_endpoint_circuit_open": True,
        },
        {
            "name": "hybrid_budget_degrades",
            "backend": "hybrid",
            "preference": "hybrid_fallback",
            "budget_us": 5_000,
            "local_ready": False,
            "endpoint_ready": False,
            "last_success_age_us": 100_000,
            "endpoint_failures": 0,
            "stale_after_us": 10_000,
            "require_target": "degraded_fallback",
            "allow_degraded": True,
        },
        {
            "name": "local_only_unavailable",
            "backend": "remote_api",
            "preference": "local_only",
            "local_ready": False,
            "endpoint_ready": False,
            "last_success_age_us": 100_000,
            "endpoint_failures": 0,
            "stale_after_us": 10_000,
            "require_target": "unavailable",
            "allow_unavailable": True,
        },
    )
    reports: list[dict[str, object]] = []
    errors: list[str] = []

    for scenario in scenarios:
        name = str(scenario["name"])
        report = _check_ai_route(
            backend=str(scenario["backend"]),
            preference=str(scenario["preference"]),
            budget_us=int(scenario.get("budget_us", 25_000)),
            timeout_us=int(scenario.get("timeout_us", 20_000)),
            stale_after_us=int(scenario.get("stale_after_us", 50_000)),
            model_stale_after_us=int(scenario.get("model_stale_after_us", 100_000)),
            endpoint_failure_limit=int(scenario.get("endpoint_failure_limit", 2)),
            local_ready=bool(scenario["local_ready"]),
            endpoint_ready=bool(scenario["endpoint_ready"]),
            last_success_age_us=int(scenario["last_success_age_us"]),
            endpoint_failures=int(scenario["endpoint_failures"]),
            require_target=str(scenario["require_target"]),
            allow_stale=bool(scenario.get("allow_stale", False)),
            allow_degraded=bool(scenario.get("allow_degraded", False)),
            allow_unavailable=bool(scenario.get("allow_unavailable", False)),
            allow_endpoint_circuit_open=bool(
                scenario.get("allow_endpoint_circuit_open", False)
            ),
        )
        entry = {
            "name": name,
            "passing": report["passing"],
            "expected_target": scenario["require_target"],
            "summary": report["summary"],
            "errors": report["errors"],
            "route": report["route"],
        }
        reports.append(entry)
        for error in report["errors"]:
            errors.append(f"{name}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(reports),
        "scenarios": reports,
    }


def _check_ai_route(
    backend: str,
    preference: str,
    budget_us: int,
    timeout_us: int,
    stale_after_us: int,
    model_stale_after_us: int,
    endpoint_failure_limit: int,
    local_ready: bool,
    endpoint_ready: bool,
    last_success_age_us: int,
    endpoint_failures: int,
    require_target: str | None,
    allow_stale: bool,
    allow_degraded: bool,
    allow_unavailable: bool,
    allow_endpoint_circuit_open: bool,
) -> dict[str, object]:
    route = _build_ai_route(
        backend,
        preference,
        budget_us,
        timeout_us,
        stale_after_us,
        model_stale_after_us,
        endpoint_failure_limit,
        local_ready,
        endpoint_ready,
        last_success_age_us,
        endpoint_failures,
    )
    decision = route["decision"]
    target = str(decision["target"])
    errors: list[str] = []

    if require_target is not None and target != require_target:
        errors.append(f"AI route target mismatch: {target} != {require_target}")
    if target == "unavailable" and not allow_unavailable:
        errors.append("AI route is unavailable")
    if target == "degraded_fallback" and not allow_degraded:
        errors.append("AI route used degraded fallback")
    if bool(decision["uses_stale_snapshot"]) and not allow_stale:
        errors.append("AI route used a stale snapshot")
    if bool(decision["endpoint_circuit_open"]) and not allow_endpoint_circuit_open:
        errors.append("AI endpoint circuit is open")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "limits": {
            "backend": backend,
            "preference": preference,
            "budget_us": budget_us,
            "timeout_us": timeout_us,
            "require_target": require_target,
            "allow_stale": allow_stale,
            "allow_degraded": allow_degraded,
            "allow_unavailable": allow_unavailable,
            "allow_endpoint_circuit_open": allow_endpoint_circuit_open,
        },
        "summary": {
            "target": target,
            "endpoint_circuit_open": decision["endpoint_circuit_open"],
            "uses_stale_snapshot": decision["uses_stale_snapshot"],
        },
        "route": route,
    }


def _ai_backend_from_label(label: str) -> AiBackendKind:
    return {
        "on_device": AiBackendKind.ON_DEVICE,
        "remote_api": AiBackendKind.REMOTE_API,
        "edge_sidecar": AiBackendKind.EDGE_SIDECAR,
        "hybrid": AiBackendKind.HYBRID,
    }[label]


def _ai_preference_from_label(label: str) -> AiRoutePreference:
    return {
        "local_only": AiRoutePreference.LOCAL_ONLY,
        "prefer_local": AiRoutePreference.PREFER_LOCAL,
        "prefer_remote": AiRoutePreference.PREFER_REMOTE,
        "hybrid_fallback": AiRoutePreference.HYBRID_FALLBACK,
    }[label]


def _sample_report(kind: str) -> dict[str, int]:
    if kind == "admission":
        return seal_report(
            ReportKind.ADMISSION,
            {
                "admitted": 1,
                "module_count": 2,
                "startup_len": 2,
                "flash_used_bytes": 24 * 1024,
                "flash_limit_bytes": 64 * 1024,
                "ram_used_bytes": 6 * 1024,
                "ram_limit_bytes": 16 * 1024,
                "pool_used_slots": 6,
                "pool_limit_slots": 8,
            },
        )
    if kind == "runtime":
        return seal_report(
            ReportKind.RUNTIME,
            {
                "state": 3,
                "module_count": 2,
                "mailbox_len": 1,
                "alarm_len": 1,
                "next_alarm_due_us_lo": 0x5678_9ABC,
                "next_alarm_due_us_hi": 0x1234,
                "kv_len": 1,
                "quota_flash_used_bytes": 4096,
                "quota_ram_used_bytes": 1024,
                "quota_pool_used_slots": 1,
                "event_count": 3,
            },
        )
    if kind == "health":
        return seal_report(
            ReportKind.HEALTH,
            {
                "module_tag": 5,
                "total_errors": 2,
                "consecutive_errors": 1,
                "last_error": 4,
                "last_action": 2,
                "event_count": 5,
                "error_events": 1,
                "last_seen_us_lo": 0x40,
                "last_seen_us_hi": 0x1,
            },
        )
    if kind == "event_log":
        return seal_report(
            ReportKind.EVENT_LOG,
            {
                "event_count": 3,
                "capacity": 8,
                "latest_seq": 3,
                "latest_at_us_lo": 0x80,
                "latest_at_us_hi": 0x1,
                "latest_module_tag": 5,
                "latest_severity": 2,
                "latest_kind": 3,
                "latest_payload_kind": 2,
                "latest_payload0": 1,
            },
        )
    if kind == "module_runtime":
        return seal_report(
            ReportKind.MODULE_RUNTIME,
            {
                "module_count": 2,
                "capacity": 4,
                "active_count": 1,
                "faulted_count": 1,
                "latest_module_tag": 5,
                "latest_state": 4,
                "latest_fault_count": 1,
                "latest_change_us_lo": 0xC0,
                "latest_change_us_hi": 0x1,
            },
        )
    if kind == "degrade_application":
        return seal_report(
            ReportKind.DEGRADE_APPLICATION,
            {
                "requested_count": 2,
                "disabled_count": 1,
                "already_disabled_count": 1,
                "reason": 2,
                "applied_at_us_lo": 0x20,
                "applied_at_us_hi": 0x1,
            },
        )
    if kind == "ai_model":
        return seal_report(
            ReportKind.AI_MODEL,
            {
                "backend": int(AiBackendKind.HYBRID),
                "model_id": 42,
                "input_bytes_max": 128,
                "output_bytes_max": 32,
                "arena_bytes": 4096,
                "timeout_us": 20_000,
                "route_preference": int(AiRoutePreference.HYBRID_FALLBACK),
                "stale_after_us": 50_000,
                "endpoint_failure_limit": 2,
            },
        )
    if kind == "ros_bridge":
        return seal_report(
            ReportKind.ROS_BRIDGE,
            {
                "transport": 1,
                "bridge_id_hash": stable_hash32("robot_core"),
                "topic_count": 1,
                "service_count": 1,
                "action_count": 0,
                "parameter_count": 1,
                "total_buffer_bytes": 544,
                "max_timeout_us": 50_000,
            },
        )
    raise ValueError(f"unsupported sample report kind: {kind}")


def _check_boot_summary_matrix() -> dict[str, object]:
    scenarios = (
        _boot_all_pass_scenario(),
        _boot_missing_profile_scenario(),
        _boot_manifest_corrupt_scenario(),
        _boot_adapter_failure_scenario(),
        _boot_admission_in_progress_scenario(),
    )
    errors: list[str] = []
    for scenario in scenarios:
        for error in scenario["errors"]:
            errors.append(f"{scenario['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(scenarios),
        "scenarios": list(scenarios),
    }


def _boot_summary_scenario(
    name: str,
    reports: dict[str, dict[str, int]],
    expected_passing: bool,
    expected_stage: str,
    expected_status: str,
    expected_code: int,
    expected_counts: dict[str, int],
) -> dict[str, object]:
    summary = BootReportSummary.from_dict({"reports": reports}).to_dict()
    errors: list[str] = []
    _expect_equal(summary["passing"], expected_passing, "passing", errors)
    _expect_equal(summary["first_stage"], expected_stage, "first_stage", errors)
    _expect_equal(summary["first_status"], expected_status, "first_status", errors)
    _expect_equal(summary["diagnostic_code"], expected_code, "diagnostic_code", errors)
    for key, expected in expected_counts.items():
        _expect_equal(summary["status_counts"][key], expected, f"{key}_count", errors)
    return {
        "name": name,
        "passing": len(errors) == 0,
        "errors": errors,
        "summary": summary,
    }


def _boot_pass_reports() -> dict[str, dict[str, int]]:
    return {
        "board_profile": seal_report(
            ReportKind.BOARD_PROFILE,
            {
                "platform_hash": 0x1111,
                "board_hash": 0x2222,
                "app_flash_start": 0x1000,
                "flash_budget_bytes": 80 * 1024,
                "ram_budget_bytes": 32 * 1024,
                "sample_pool_slots": 8,
                "max_modules": 16,
                "servo_pin": 24,
                "servo_center_us": 1500,
                "led_pin": 15,
                "mvk_trigger_pin": 17,
            },
        ),
        "board_package": seal_report(
            ReportKind.BOARD_PACKAGE,
            {
                "valid": 1,
                "platform_hash": 0x1111,
                "board_hash": 0x2222,
                "boot_layout": 1,
                "app_flash_start": 0x1000,
                "app_flash_len_bytes": 1020 * 1024,
                "ram_start": 0x2000_0000,
                "ram_len_bytes": 256 * 1024,
                "flash_budget_bytes": 80 * 1024,
                "ram_budget_bytes": 32 * 1024,
                "sample_pool_slots": 8,
                "max_modules": 16,
                "led_pin": 15,
                "servo_pin": 24,
                "mvk_trigger_pin": 17,
            },
        ),
        "manifest": seal_report(
            ReportKind.MANIFEST,
            {
                "valid": 1,
                "module_count": 2,
                "fingerprint": 0x1234,
            },
        ),
        "adapter_compatibility": seal_report(
            ReportKind.ADAPTER_COMPAT,
            {
                "compatible": 1,
                "adapter_count": 2,
            },
        ),
        "admission": _sample_report("admission"),
        "runtime": _sample_report("runtime"),
    }


def _boot_all_pass_scenario() -> dict[str, object]:
    return _boot_summary_scenario(
        "all_pass_reports_runtime_pass",
        _boot_pass_reports(),
        expected_passing=True,
        expected_stage="runtime",
        expected_status="pass",
        expected_code=0x0600_0000,
        expected_counts={
            "pass": 6,
            "missing": 0,
            "in_progress": 0,
            "fail": 0,
            "corrupt": 0,
        },
    )


def _boot_missing_profile_scenario() -> dict[str, object]:
    reports = {"manifest": _boot_pass_reports()["manifest"]}
    return _boot_summary_scenario(
        "missing_profile_takes_priority",
        reports,
        expected_passing=False,
        expected_stage="board_profile",
        expected_status="missing",
        expected_code=0x0101_0000,
        expected_counts={
            "pass": 1,
            "missing": 5,
            "in_progress": 0,
            "fail": 0,
            "corrupt": 0,
        },
    )


def _boot_manifest_corrupt_scenario() -> dict[str, object]:
    reports = _boot_pass_reports()
    reports["manifest"] = dict(reports["manifest"])
    reports["manifest"]["module_count"] = 3
    return _boot_summary_scenario(
        "manifest_checksum_corruption_stops_boot",
        reports,
        expected_passing=False,
        expected_stage="manifest",
        expected_status="corrupt",
        expected_code=0x0303_0000,
        expected_counts={
            "pass": 5,
            "missing": 0,
            "in_progress": 0,
            "fail": 0,
            "corrupt": 1,
        },
    )


def _boot_adapter_failure_scenario() -> dict[str, object]:
    reports = _boot_pass_reports()
    reports["adapter_compatibility"] = seal_report(
        ReportKind.ADAPTER_COMPAT,
        {
            "compatible": 0,
            "adapter_count": 2,
            "error_code": 3,
            "error_module_tag": 3,
            "error_capability_bits": Capability.BUS0.bit,
        },
    )
    scenario = _boot_summary_scenario(
        "adapter_failure_preserves_error_label",
        reports,
        expected_passing=False,
        expected_stage="adapter_compatibility",
        expected_status="fail",
        expected_code=0x0404_0003,
        expected_counts={
            "pass": 5,
            "missing": 0,
            "in_progress": 0,
            "fail": 1,
            "corrupt": 0,
        },
    )
    _expect_equal(
        scenario["summary"]["first_error_label"],
        "capability_ownership_conflict",
        "first_error_label",
        scenario["errors"],
    )
    scenario["passing"] = len(scenario["errors"]) == 0
    return scenario


def _boot_admission_in_progress_scenario() -> dict[str, object]:
    reports = _boot_pass_reports()
    reports["admission"] = dict(reports["admission"])
    reports["admission"]["completed"] = 0
    return _boot_summary_scenario(
        "admission_in_progress_blocks_runtime",
        reports,
        expected_passing=False,
        expected_stage="admission",
        expected_status="in_progress",
        expected_code=0x0502_0000,
        expected_counts={
            "pass": 5,
            "missing": 0,
            "in_progress": 1,
            "fail": 0,
            "corrupt": 0,
        },
    )


def _sample_sensor(
    mode: str,
    ticks: int,
    sample_period_ticks: int,
    fault_period: int,
) -> dict[str, object]:
    if ticks < 0:
        raise ValueError("ticks must be non-negative")

    simulator = SensorStubSimulator(
        sample_period_ticks=sample_period_ticks,
        mode=SensorStubMode(mode),
        fault_period=fault_period,
    )
    samples: list[dict[str, object]] = []
    errors: list[dict[str, object]] = []

    for index in range(ticks):
        try:
            sample = simulator.poll(index)
        except SensorStubError as error:
            errors.append({"tick": simulator.tick, "error": str(error)})
            continue
        if sample is not None:
            samples.append(sample.to_dict())

    return {
        "mode": mode,
        "ticks": ticks,
        "sample_period_ticks": sample_period_ticks,
        "fault_period": fault_period,
        "sample_count": len(samples),
        "error_count": len(errors),
        "samples": samples,
        "errors": errors,
    }


def _sample_actuator(
    start_us: int,
    stop_us: int,
    step_us: int,
    readback_offset_us: int,
    tolerance_us: int,
) -> dict[str, object]:
    simulator = ServoSimulator(
        readback_offset_us=readback_offset_us,
        readback_tolerance_us=tolerance_us,
    )
    commands = simulator.sweep(start_us=start_us, stop_us=stop_us, step_us=step_us)
    command_dicts = [command.to_dict() for command in commands]
    return {
        "start_us": start_us,
        "stop_us": stop_us,
        "step_us": step_us,
        "readback_offset_us": readback_offset_us,
        "tolerance_us": tolerance_us,
        "command_count": len(command_dicts),
        "accepted_count": sum(1 for command in command_dicts if command["accepted"]),
        "deadline_miss_count": sum(
            1 for command in command_dicts if not command["deadline_met"]
        ),
        "readback_fail_count": sum(
            1 for command in command_dicts if not command["readback_ok"]
        ),
        "commands": command_dicts,
    }


def _sample_recovery(
    module: str,
    error: str,
    events: int,
    notify_after: int,
    reboot_after: int,
    ok_after: int,
) -> dict[str, object]:
    if events < 0:
        raise ValueError("events must be non-negative")
    if ok_after < 0:
        raise ValueError("ok_after must be non-negative")

    simulator = RecoveryPolicySimulator(
        notify_after=notify_after,
        reboot_after=reboot_after,
    )
    timeline: list[dict[str, object]] = []

    for index in range(events):
        now_us = (index + 1) * 10
        decision = simulator.record_error(module, error, now_us)
        timeline.append({"event": "error", **decision.to_dict()})
        if ok_after != 0 and index + 1 == ok_after:
            timeline.append(simulator.record_ok(now_us + 1))

    return {
        "module": module,
        "error": error,
        "notify_after": notify_after,
        "reboot_after": reboot_after,
        "ok_after": ok_after,
        "event_count": len(timeline),
        "timeline": timeline,
    }


def _check_recovery_matrix() -> dict[str, object]:
    scenarios: tuple[dict[str, object], ...] = (
        {
            "name": "sensor_ignore_first_error",
            "module": "sensor",
            "error": "sensor_read_fail",
            "events": 1,
            "notify_after": 2,
            "reboot_after": 4,
            "expected_final_state": "running",
            "expected_last_action": "ignore",
            "expected_notifications": 0,
            "expected_reboots": 0,
            "expected_retry_count": 0,
            "expected_max_consecutive": 1,
        },
        {
            "name": "bus_timeout_retry_delay",
            "module": "bus",
            "error": "bus_timeout",
            "events": 1,
            "notify_after": 3,
            "reboot_after": 4,
            "expected_final_state": "running",
            "expected_last_action": "retry_delay",
            "expected_notifications": 0,
            "expected_reboots": 0,
            "expected_retry_count": 1,
            "expected_max_consecutive": 1,
        },
        {
            "name": "sensor_notify_threshold",
            "module": "sensor",
            "error": "sensor_read_fail",
            "events": 2,
            "notify_after": 2,
            "reboot_after": 4,
            "expected_final_state": "degraded",
            "expected_last_action": "notify_user_task",
            "expected_notifications": 1,
            "expected_reboots": 0,
            "expected_retry_count": 0,
            "expected_max_consecutive": 2,
        },
        {
            "name": "sensor_reboot_threshold",
            "module": "sensor",
            "error": "sensor_read_fail",
            "events": 4,
            "notify_after": 2,
            "reboot_after": 4,
            "expected_final_state": "recovering",
            "expected_last_action": "reboot_module",
            "expected_notifications": 2,
            "expected_reboots": 1,
            "expected_retry_count": 0,
            "expected_max_consecutive": 4,
        },
        {
            "name": "ok_reset_breaks_error_streak",
            "module": "bus",
            "error": "bus_timeout",
            "events": 2,
            "notify_after": 2,
            "reboot_after": 3,
            "ok_after": 1,
            "expected_final_state": "running",
            "expected_last_action": "retry_delay",
            "expected_notifications": 0,
            "expected_reboots": 0,
            "expected_retry_count": 2,
            "expected_max_consecutive": 1,
        },
    )
    reports: list[dict[str, object]] = []
    errors: list[str] = []

    for scenario in scenarios:
        report = _run_recovery_matrix_scenario(scenario)
        reports.append(report)
        for error in report["errors"]:
            errors.append(f"{report['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(reports),
        "scenarios": reports,
    }


def _run_recovery_matrix_scenario(scenario: dict[str, object]) -> dict[str, object]:
    module = str(scenario["module"])
    error = str(scenario["error"])
    events = int(scenario["events"])
    notify_after = int(scenario["notify_after"])
    reboot_after = int(scenario["reboot_after"])
    ok_after = int(scenario.get("ok_after", 0))
    simulator = RecoveryPolicySimulator(
        notify_after=notify_after,
        reboot_after=reboot_after,
    )
    timeline: list[dict[str, object]] = []
    decisions = []

    for index in range(events):
        now_us = (index + 1) * 10
        decision = simulator.record_error(module, error, now_us)
        decisions.append(decision)
        timeline.append({"event": "error", **decision.to_dict()})
        if ok_after != 0 and index + 1 == ok_after:
            timeline.append(simulator.record_ok(now_us + 1))

    summary = RecoverySummary.from_decisions(module, decisions).to_dict()
    max_consecutive = max(
        (int(entry.get("consecutive_errors", 0)) for entry in timeline),
        default=0,
    )
    action_sequence = [
        str(entry["action"])
        for entry in timeline
        if entry.get("event") == "error"
    ]
    errors: list[str] = []
    _expect_equal(
        summary["final_state"],
        scenario["expected_final_state"],
        "final_state",
        errors,
    )
    _expect_equal(
        summary["last_action"],
        scenario["expected_last_action"],
        "last_action",
        errors,
    )
    _expect_equal(
        summary["notification_count"],
        scenario["expected_notifications"],
        "notification_count",
        errors,
    )
    _expect_equal(
        summary["reboot_count"],
        scenario["expected_reboots"],
        "reboot_count",
        errors,
    )
    _expect_equal(
        summary["retry_count"],
        scenario["expected_retry_count"],
        "retry_count",
        errors,
    )
    _expect_equal(
        max_consecutive,
        scenario["expected_max_consecutive"],
        "max_consecutive_errors",
        errors,
    )

    return {
        "name": scenario["name"],
        "passing": len(errors) == 0,
        "errors": errors,
        "summary": summary,
        "max_consecutive_errors": max_consecutive,
        "action_sequence": action_sequence,
        "timeline": timeline,
    }


def _expect_equal(
    actual: object,
    expected: object,
    label: str,
    errors: list[str],
) -> None:
    if actual != expected:
        errors.append(f"{label} mismatch: {actual!r} != {expected!r}")


def _sample_watchdog(
    module: str,
    timeout_us: int,
    sweeps: int,
    step_us: int,
    beat_at_sweep: int,
) -> dict[str, object]:
    if sweeps < 0:
        raise ValueError("sweeps must be non-negative")
    if step_us <= 0:
        raise ValueError("step_us must be positive")
    if beat_at_sweep < 0:
        raise ValueError("beat_at_sweep must be non-negative")

    simulator = WatchdogSimulator(capacity=1)
    simulator.register(module, timeout_us, now_us=0)
    timeline: list[dict[str, object]] = [
        {"event": "register", **simulator.get(module).to_dict()},
    ]

    for index in range(1, sweeps + 1):
        now_us = index * step_us
        if beat_at_sweep == index:
            simulator.beat(module, now_us)
            timeline.append({"event": "beat", **simulator.get(module).to_dict()})
        expired = simulator.expired(now_us)
        timeline.append(
            {
                "event": "sweep",
                "now_us": now_us,
                "expired_count": len(expired),
                "expired": [entry.to_dict() for entry in expired],
                "entry": simulator.get(module).to_dict(),
            }
        )

    entry = simulator.get(module)
    return {
        "module": module,
        "timeout_us": timeout_us,
        "sweeps": sweeps,
        "step_us": step_us,
        "beat_at_sweep": beat_at_sweep,
        "missed": 0 if entry is None else entry.missed,
        "event_count": len(timeline),
        "timeline": timeline,
    }


def _check_watchdog_matrix() -> dict[str, object]:
    scenarios = (
        _watchdog_precheck_scenario(),
        _watchdog_expiry_scenario(),
        _watchdog_heartbeat_reset_scenario(),
        _watchdog_multi_module_scenario(),
        _watchdog_capacity_scenario(),
    )
    errors: list[str] = []
    for scenario in scenarios:
        for error in scenario["errors"]:
            errors.append(f"{scenario['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(scenarios),
        "scenarios": list(scenarios),
    }


def _watchdog_precheck_scenario() -> dict[str, object]:
    watchdog = WatchdogSimulator(capacity=1)
    watchdog.register("sensor", timeout_us=100, now_us=0)
    before = watchdog.expired_count(150)
    missed_after_count = watchdog.get("sensor").missed
    errors: list[str] = []
    _expect_equal(before, 1, "expired_count", errors)
    _expect_equal(missed_after_count, 0, "missed_after_count", errors)
    return {
        "name": "non_mutating_precheck",
        "passing": len(errors) == 0,
        "errors": errors,
        "expired_count": before,
        "entry": watchdog.get("sensor").to_dict(),
    }


def _watchdog_expiry_scenario() -> dict[str, object]:
    watchdog = WatchdogSimulator(capacity=1)
    watchdog.register("sensor", timeout_us=100, now_us=0)
    expired = watchdog.expired(150)
    entry = watchdog.get("sensor")
    errors: list[str] = []
    _expect_equal(len(expired), 1, "expired_len", errors)
    _expect_equal(entry.missed, 1, "missed", errors)
    _expect_equal(expired[0].overdue_us(150), 50, "overdue_us", errors)
    return {
        "name": "expiry_updates_missed",
        "passing": len(errors) == 0,
        "errors": errors,
        "expired": [item.to_dict() for item in expired],
        "entry": entry.to_dict(),
    }


def _watchdog_heartbeat_reset_scenario() -> dict[str, object]:
    watchdog = WatchdogSimulator(capacity=1)
    watchdog.register("bus", timeout_us=100, now_us=0)
    watchdog.expired(150)
    watchdog.beat("bus", 160)
    expired_after_beat = watchdog.expired(200)
    entry = watchdog.get("bus")
    errors: list[str] = []
    _expect_equal(entry.missed, 0, "missed_after_beat", errors)
    _expect_equal(entry.last_beat_us, 160, "last_beat_us", errors)
    _expect_equal(len(expired_after_beat), 0, "expired_after_beat", errors)
    return {
        "name": "heartbeat_resets_missed",
        "passing": len(errors) == 0,
        "errors": errors,
        "expired_after_beat": [item.to_dict() for item in expired_after_beat],
        "entry": entry.to_dict(),
    }


def _watchdog_multi_module_scenario() -> dict[str, object]:
    watchdog = WatchdogSimulator(capacity=2)
    watchdog.register("sensor", timeout_us=100, now_us=0)
    watchdog.register("radio", timeout_us=500, now_us=0)
    expired = watchdog.expired(150)
    sensor = watchdog.get("sensor")
    radio = watchdog.get("radio")
    errors: list[str] = []
    _expect_equal([entry.module for entry in expired], ["sensor"], "expired_modules", errors)
    _expect_equal(sensor.missed, 1, "sensor_missed", errors)
    _expect_equal(radio.missed, 0, "radio_missed", errors)
    return {
        "name": "multi_module_selective_expiry",
        "passing": len(errors) == 0,
        "errors": errors,
        "expired": [item.to_dict() for item in expired],
        "entries": [entry.to_dict() for entry in watchdog.entries()],
    }


def _watchdog_capacity_scenario() -> dict[str, object]:
    watchdog = WatchdogSimulator(capacity=1)
    watchdog.register("sensor", timeout_us=100, now_us=0)
    error_label = None
    try:
        watchdog.register("radio", timeout_us=100, now_us=0)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        error_label = str(exc)

    errors: list[str] = []
    _expect_equal(error_label, "watchdog capacity exhausted", "capacity_error", errors)
    return {
        "name": "capacity_exhaustion_is_reported",
        "passing": len(errors) == 0,
        "errors": errors,
        "error_label": error_label,
        "entries": [entry.to_dict() for entry in watchdog.entries()],
    }


def _sample_scheduler(
    ticks: list[int] | tuple[int, ...],
    period_us: int,
    tolerance_us: int,
) -> dict[str, object]:
    if period_us <= 0:
        raise ValueError("period_us must be positive")
    if tolerance_us < 0:
        raise ValueError("tolerance_us must be non-negative")

    simulator = SchedulerSimulator(
        deadline_period_us=period_us,
        jitter_tolerance_us=tolerance_us,
    )
    timeline: list[dict[str, object]] = []
    for now_us in ticks:
        stats = simulator.on_deadline_tick(now_us)
        timeline.append({"now_us": int(now_us), **stats.to_dict()})

    return {
        "period_us": period_us,
        "tolerance_us": tolerance_us,
        "tick_count": simulator.tick_count,
        "max_jitter_us": simulator.max_jitter_us,
        "deadline_misses": simulator.deadline_misses,
        "timeline": timeline,
    }


def _check_scheduler_matrix() -> dict[str, object]:
    scenarios = (
        _scheduler_nominal_scenario(),
        _scheduler_within_tolerance_scenario(),
        _scheduler_late_miss_scenario(),
        _scheduler_wraparound_scenario(),
        _scheduler_reset_scenario(),
        _scheduler_invalid_config_scenario(),
    )
    errors: list[str] = []
    for scenario in scenarios:
        for error in scenario["errors"]:
            errors.append(f"{scenario['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(scenarios),
        "scenarios": list(scenarios),
    }


def _scheduler_scenario(
    name: str,
    ticks: tuple[int, ...],
    period_us: int,
    tolerance_us: int,
    expected_tick_count: int,
    expected_max_jitter_us: int,
    expected_deadline_misses: int,
) -> dict[str, object]:
    report = _sample_scheduler(ticks, period_us, tolerance_us)
    errors: list[str] = []
    _expect_equal(report["tick_count"], expected_tick_count, "tick_count", errors)
    _expect_equal(
        report["max_jitter_us"],
        expected_max_jitter_us,
        "max_jitter_us",
        errors,
    )
    _expect_equal(
        report["deadline_misses"],
        expected_deadline_misses,
        "deadline_misses",
        errors,
    )
    return {
        "name": name,
        "passing": len(errors) == 0,
        "errors": errors,
        **report,
    }


def _scheduler_nominal_scenario() -> dict[str, object]:
    return _scheduler_scenario(
        name="on_time_ticks",
        ticks=(1_000, 21_000, 41_000),
        period_us=20_000,
        tolerance_us=0,
        expected_tick_count=3,
        expected_max_jitter_us=0,
        expected_deadline_misses=0,
    )


def _scheduler_within_tolerance_scenario() -> dict[str, object]:
    return _scheduler_scenario(
        name="early_late_within_tolerance",
        ticks=(1_000, 21_020, 41_010),
        period_us=20_000,
        tolerance_us=25,
        expected_tick_count=3,
        expected_max_jitter_us=20,
        expected_deadline_misses=0,
    )


def _scheduler_late_miss_scenario() -> dict[str, object]:
    return _scheduler_scenario(
        name="late_ticks_miss_deadline",
        ticks=(1_000, 21_030, 41_080),
        period_us=20_000,
        tolerance_us=25,
        expected_tick_count=3,
        expected_max_jitter_us=50,
        expected_deadline_misses=2,
    )


def _scheduler_wraparound_scenario() -> dict[str, object]:
    first = 0xFFFF_FFFF - 5
    return _scheduler_scenario(
        name="u32_wraparound",
        ticks=(first, first + 20_003),
        period_us=20_000,
        tolerance_us=5,
        expected_tick_count=2,
        expected_max_jitter_us=3,
        expected_deadline_misses=0,
    )


def _scheduler_reset_scenario() -> dict[str, object]:
    simulator = SchedulerSimulator(jitter_tolerance_us=5)
    simulator.on_deadline_tick(1_000)
    simulator.on_deadline_tick(21_010)
    before = simulator.stats().to_dict()
    simulator.reset_stats()
    after = simulator.stats().to_dict()
    errors: list[str] = []
    _expect_equal(before["deadline_misses"], 1, "before_deadline_misses", errors)
    _expect_equal(after["tick_count"], 0, "after_tick_count", errors)
    _expect_equal(after["max_jitter_us"], 0, "after_max_jitter_us", errors)
    _expect_equal(after["deadline_misses"], 0, "after_deadline_misses", errors)
    _expect_equal(after["jitter_tolerance_us"], 10, "after_jitter_tolerance_us", errors)
    return {
        "name": "reset_clears_counters",
        "passing": len(errors) == 0,
        "errors": errors,
        "before": before,
        "after": after,
    }


def _scheduler_invalid_config_scenario() -> dict[str, object]:
    period_error = None
    tolerance_error = None
    try:
        SchedulerSimulator(deadline_period_us=0)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        period_error = str(exc)

    try:
        SchedulerSimulator().set_jitter_tolerance_us(-1)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        tolerance_error = str(exc)

    errors: list[str] = []
    _expect_equal(
        period_error,
        "deadline_period_us must be positive",
        "period_error",
        errors,
    )
    _expect_equal(
        tolerance_error,
        "tolerance_us must be non-negative",
        "tolerance_error",
        errors,
    )
    return {
        "name": "invalid_config_is_rejected",
        "passing": len(errors) == 0,
        "errors": errors,
        "period_error": period_error,
        "tolerance_error": tolerance_error,
    }


def _sample_event_log(capacity: int, events: int, recent: int) -> dict[str, object]:
    if events < 0:
        raise ValueError("events must be non-negative")
    if recent < 0:
        raise ValueError("recent must be non-negative")

    simulator = EventLogSimulator(capacity=capacity)
    severities = ("info", "warn", "error", "fatal")
    kinds = ("boot", "health", "recovery", "host")
    overwritten: list[dict[str, object]] = []

    for index in range(events):
        replaced = simulator.push(
            at_us=(index + 1) * 10,
            module="sensor" if index % 2 else "kernel",
            severity=severities[index % len(severities)],
            kind=kinds[index % len(kinds)],
            payload_kind="counter",
            payload0=index + 1,
        )
        if replaced is not None:
            overwritten.append(replaced.to_dict())

    return {
        **simulator.summary(),
        "warn_or_higher": simulator.count_at_or_above("warn"),
        "error_or_higher": simulator.count_at_or_above("error"),
        "latest": None if simulator.latest() is None else simulator.latest().to_dict(),
        "recent": [record.to_dict() for record in simulator.copy_recent(recent)],
        "overwritten": overwritten,
    }


def _check_event_log_matrix() -> dict[str, object]:
    scenarios = (
        _event_log_empty_scenario(),
        _event_log_ring_overwrite_scenario(),
        _event_log_zero_capacity_scenario(),
        _event_log_severity_count_scenario(),
        _event_log_invalid_config_scenario(),
    )
    errors: list[str] = []
    for scenario in scenarios:
        for error in scenario["errors"]:
            errors.append(f"{scenario['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(scenarios),
        "scenarios": list(scenarios),
    }


def _event_log_empty_scenario() -> dict[str, object]:
    log = EventLogSimulator(capacity=3)
    errors: list[str] = []
    _expect_equal(log.len, 0, "len", errors)
    _expect_equal(log.remaining_capacity, 3, "remaining_capacity", errors)
    _expect_equal(log.latest_sequence, 0, "latest_sequence", errors)
    _expect_equal(log.has_dropped_events, False, "has_dropped_events", errors)
    _expect_equal(log.latest(), None, "latest", errors)
    _expect_equal(log.copy_recent(3), [], "recent", errors)
    return {
        "name": "empty_log_reports_capacity",
        "passing": len(errors) == 0,
        "errors": errors,
        "summary": log.summary(),
        "recent": [],
    }


def _event_log_ring_overwrite_scenario() -> dict[str, object]:
    report = _sample_event_log(capacity=3, events=4, recent=3)
    errors: list[str] = []
    _expect_equal(report["len"], 3, "len", errors)
    _expect_equal(report["dropped"], 1, "dropped", errors)
    _expect_equal(report["latest_sequence"], 4, "latest_sequence", errors)
    _expect_equal(report["remaining_capacity"], 0, "remaining_capacity", errors)
    _expect_equal(
        [record["at_us"] for record in report["recent"]],
        [20, 30, 40],
        "recent_order",
        errors,
    )
    _expect_equal(
        [record["seq"] for record in report["overwritten"]],
        [1],
        "overwritten_sequence",
        errors,
    )
    return {
        "name": "ring_overwrite_preserves_recent_order",
        "passing": len(errors) == 0,
        "errors": errors,
        **report,
    }


def _event_log_zero_capacity_scenario() -> dict[str, object]:
    log = EventLogSimulator(capacity=0)
    returned = [
        log.push(10, "kernel", "warn", "boot", payload_kind="counter", payload0=1),
        log.push(20, "sensor", "error", "health", payload_kind="counter", payload0=2),
    ]
    errors: list[str] = []
    _expect_equal(log.len, 0, "len", errors)
    _expect_equal(log.dropped, 2, "dropped", errors)
    _expect_equal(log.latest_sequence, 0, "latest_sequence", errors)
    _expect_equal(log.latest(), None, "latest", errors)
    _expect_equal(log.copy_recent(1), [], "recent", errors)
    _expect_equal(
        [record.seq for record in returned if record is not None],
        [0, 0],
        "returned_sequences",
        errors,
    )
    return {
        "name": "zero_capacity_counts_drops_without_storing",
        "passing": len(errors) == 0,
        "errors": errors,
        "summary": log.summary(),
        "returned": [
            None if record is None else record.to_dict() for record in returned
        ],
    }


def _event_log_severity_count_scenario() -> dict[str, object]:
    log = EventLogSimulator(capacity=5)
    for index, severity in enumerate(("trace", "info", "warn", "error", "fatal")):
        log.push(
            at_us=(index + 1) * 10,
            module="kernel",
            severity=severity,
            kind="host",
            payload_kind="counter",
            payload0=index,
        )

    errors: list[str] = []
    _expect_equal(log.count_at_or_above("warn"), 3, "warn_or_higher", errors)
    _expect_equal(log.count_at_or_above("error"), 2, "error_or_higher", errors)
    _expect_equal(log.count_at_or_above("fatal"), 1, "fatal_or_higher", errors)
    _expect_equal(
        log.latest().severity.value if log.latest() else None,
        "fatal",
        "latest",
        errors,
    )
    return {
        "name": "severity_threshold_counts_are_stable",
        "passing": len(errors) == 0,
        "errors": errors,
        "summary": log.summary(),
        "warn_or_higher": log.count_at_or_above("warn"),
        "error_or_higher": log.count_at_or_above("error"),
        "fatal_or_higher": log.count_at_or_above("fatal"),
    }


def _event_log_invalid_config_scenario() -> dict[str, object]:
    capacity_error = None
    recent_error = None
    severity_error = None
    try:
        EventLogSimulator(capacity=-1)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        capacity_error = str(exc)

    try:
        EventLogSimulator(capacity=1).copy_recent(-1)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        recent_error = str(exc)

    try:
        EventLogSimulator(capacity=1).push(10, "kernel", "invalid", "boot")
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        severity_error = str(exc)

    errors: list[str] = []
    _expect_equal(
        capacity_error,
        "capacity must be non-negative",
        "capacity_error",
        errors,
    )
    _expect_equal(recent_error, "count must be non-negative", "recent_error", errors)
    _expect_equal(
        severity_error,
        "'invalid' is not a valid EventSeverity",
        "severity_error",
        errors,
    )
    return {
        "name": "invalid_config_is_rejected",
        "passing": len(errors) == 0,
        "errors": errors,
        "capacity_error": capacity_error,
        "recent_error": recent_error,
        "severity_error": severity_error,
    }


def _sample_quota() -> dict[str, object]:
    modules = _sample_runtime_modules()
    ledger = QuotaLedgerSimulator(capacity=8)
    ledger.register_modules(modules)
    timeline: list[dict[str, object]] = []

    for module, amount in (
        ("sensor", ResourceBudget(4096, 512, 1)),
        ("ai", ResourceBudget(12 * 1024, 4 * 1024, 1)),
        ("radio", ResourceBudget(2048, 512, 0)),
    ):
        ledger.reserve(module, amount)
        timeline.append(
            {
                "event": "reserve",
                "module": module,
                "amount": amount.to_dict(),
                "used": ledger.usage(module).to_dict(),
                "available": ledger.available(module).to_dict(),
            }
        )

    released = ResourceBudget(1024, 128, 0)
    ledger.release("sensor", released)
    timeline.append(
        {
            "event": "release",
            "module": "sensor",
            "amount": released.to_dict(),
            "used": ledger.usage("sensor").to_dict(),
            "available": ledger.available("sensor").to_dict(),
        }
    )

    return {
        **ledger.to_dict(),
        "timeline": timeline,
    }


def _check_quota_matrix() -> dict[str, object]:
    scenarios = (
        _quota_reserve_release_scenario(),
        _quota_capacity_scenario(),
        _quota_module_identity_scenario(),
        _quota_limit_and_underflow_scenario(),
        _quota_invalid_and_overflow_scenario(),
    )
    errors: list[str] = []
    for scenario in scenarios:
        for error in scenario["errors"]:
            errors.append(f"{scenario['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(scenarios),
        "scenarios": list(scenarios),
    }


def _quota_reserve_release_scenario() -> dict[str, object]:
    report = _sample_quota()
    entries = {entry["module"]: entry for entry in report["entries"]}
    errors: list[str] = []
    _expect_equal(report["len"], 5, "len", errors)
    _expect_equal(
        report["total_used"],
        {"flash_bytes": 17_408, "ram_bytes": 4_992, "pool_slots": 2},
        "total_used",
        errors,
    )
    _expect_equal(
        entries["sensor"]["used"],
        {"flash_bytes": 3_072, "ram_bytes": 384, "pool_slots": 1},
        "sensor_used",
        errors,
    )
    _expect_equal(
        entries["ai"]["available"],
        {"flash_bytes": 16_384, "ram_bytes": 4_096, "pool_slots": 1},
        "ai_available",
        errors,
    )
    return {
        "name": "reserve_release_totals",
        "passing": len(errors) == 0,
        "errors": errors,
        **report,
    }


def _quota_capacity_scenario() -> dict[str, object]:
    ledger = QuotaLedgerSimulator(capacity=1)
    ledger.register("sensor", ResourceBudget(1024, 256, 1))
    capacity_error = None
    try:
        ledger.register("radio", ResourceBudget(1024, 256, 1))
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        capacity_error = str(exc)

    errors: list[str] = []
    _expect_equal(
        capacity_error,
        "quota capacity exhausted",
        "capacity_error",
        errors,
    )
    _expect_equal(ledger.len, 1, "len", errors)
    return {
        "name": "capacity_exhaustion_is_reported",
        "passing": len(errors) == 0,
        "errors": errors,
        "capacity_error": capacity_error,
        "ledger": ledger.to_dict(),
    }


def _quota_module_identity_scenario() -> dict[str, object]:
    ledger = QuotaLedgerSimulator(capacity=2)
    ledger.register("sensor", ResourceBudget(1024, 256, 1))
    duplicate_error = None
    missing_error = None
    try:
        ledger.register("sensor", ResourceBudget(1024, 256, 1))
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        duplicate_error = str(exc)

    try:
        ledger.reserve("radio", ResourceBudget(1, 1, 0))
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        missing_error = str(exc)

    errors: list[str] = []
    _expect_equal(
        duplicate_error,
        "duplicate quota module",
        "duplicate_error",
        errors,
    )
    _expect_equal(missing_error, "missing quota module", "missing_error", errors)
    _expect_equal(ledger.usage("radio"), None, "unknown_usage", errors)
    return {
        "name": "module_identity_is_strict",
        "passing": len(errors) == 0,
        "errors": errors,
        "duplicate_error": duplicate_error,
        "missing_error": missing_error,
        "ledger": ledger.to_dict(),
    }


def _quota_limit_and_underflow_scenario() -> dict[str, object]:
    ledger = QuotaLedgerSimulator(capacity=1)
    ledger.register("sensor", ResourceBudget(1024, 256, 1))
    ledger.reserve("sensor", ResourceBudget(512, 128, 1))
    limit_error = None
    underflow_error = None
    try:
        ledger.reserve("sensor", ResourceBudget(600, 0, 0))
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        limit_error = str(exc)

    try:
        ledger.release("sensor", ResourceBudget(0, 0, 2))
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        underflow_error = str(exc)

    released = ledger.reset_usage("sensor")
    errors: list[str] = []
    _expect_equal(limit_error, "quota limit exceeded", "limit_error", errors)
    _expect_equal(
        underflow_error,
        "quota release underflow",
        "underflow_error",
        errors,
    )
    _expect_equal(released, ResourceBudget(512, 128, 1), "released", errors)
    _expect_equal(
        ledger.total_used(),
        ResourceBudget(),
        "total_used_after_reset",
        errors,
    )
    return {
        "name": "limit_and_underflow_are_rejected",
        "passing": len(errors) == 0,
        "errors": errors,
        "limit_error": limit_error,
        "underflow_error": underflow_error,
        "released": released.to_dict(),
        "ledger": ledger.to_dict(),
    }


def _quota_invalid_and_overflow_scenario() -> dict[str, object]:
    capacity_error = None
    negative_error = None
    flash_overflow_error = None
    pool_overflow_error = None
    try:
        QuotaLedgerSimulator(capacity=0)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        capacity_error = str(exc)

    try:
        ResourceBudget(flash_bytes=-1)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        negative_error = str(exc)

    try:
        ResourceBudget(0xFFFF_FFFF, 0, 0).checked_add(ResourceBudget(1, 0, 0))
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        flash_overflow_error = str(exc)

    try:
        ResourceBudget(0, 0, 0xFFFF).checked_add(ResourceBudget(0, 0, 1))
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        pool_overflow_error = str(exc)

    errors: list[str] = []
    _expect_equal(
        capacity_error,
        "capacity must be positive",
        "capacity_error",
        errors,
    )
    _expect_equal(
        negative_error,
        "flash_bytes must be non-negative",
        "negative_error",
        errors,
    )
    _expect_equal(
        flash_overflow_error,
        "flash quota overflow",
        "flash_overflow_error",
        errors,
    )
    _expect_equal(
        pool_overflow_error,
        "pool quota overflow",
        "pool_overflow_error",
        errors,
    )
    return {
        "name": "invalid_config_and_overflow_are_rejected",
        "passing": len(errors) == 0,
        "errors": errors,
        "capacity_error": capacity_error,
        "negative_error": negative_error,
        "flash_overflow_error": flash_overflow_error,
        "pool_overflow_error": pool_overflow_error,
    }


def _sample_degrade(
    flash_limit: int,
    ram_limit: int,
    pool_limit: int,
    max_modules: int,
) -> dict[str, object]:
    profile = SystemProfile(
        flash_limit_bytes=flash_limit,
        ram_limit_bytes=ram_limit,
        pool_slot_limit=pool_limit,
        max_modules=max_modules,
    )
    modules = _sample_runtime_modules()
    decision = DegradePlannerSimulator.fit(modules, profile, capacity=8)
    return {
        "profile": profile.to_dict(),
        "modules": [
            {
                "module": spec.module,
                "criticality": spec.criticality.name.lower(),
                "memory": spec.memory.to_dict(),
            }
            for spec in modules
        ],
        "decision": decision.to_dict(),
    }


def _check_degrade_matrix() -> dict[str, object]:
    scenarios = (
        _degrade_flash_pressure_scenario(),
        _degrade_ram_pressure_scenario(),
        _degrade_pool_pressure_scenario(),
        _degrade_module_limit_scenario(),
        _degrade_same_criticality_scenario(),
        _degrade_error_scenario(),
    )
    errors: list[str] = []
    for scenario in scenarios:
        for error in scenario["errors"]:
            errors.append(f"{scenario['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(scenarios),
        "scenarios": list(scenarios),
    }


def _degrade_scenario(
    name: str,
    modules: tuple[ModuleSpec, ...],
    profile: SystemProfile,
    expected_disabled: tuple[str, ...],
    expected_enabled: tuple[str, ...],
    expected_reason: str | None,
) -> dict[str, object]:
    decision = DegradePlannerSimulator.fit(modules, profile, capacity=8)
    errors: list[str] = []
    _expect_equal(decision.disabled, expected_disabled, "disabled", errors)
    _expect_equal(decision.enabled, expected_enabled, "enabled", errors)
    _expect_equal(
        None if decision.reason is None else decision.reason.value,
        expected_reason,
        "reason",
        errors,
    )
    return {
        "name": name,
        "passing": len(errors) == 0,
        "errors": errors,
        "profile": profile.to_dict(),
        "decision": decision.to_dict(),
    }


def _degrade_flash_pressure_scenario() -> dict[str, object]:
    modules = (
        _degrade_module("kernel", Criticality.HARD_REALTIME, 20, 4, 1),
        _degrade_module("sensor", Criticality.DRIVER, 20, 4, 1),
        _degrade_module("ai", Criticality.USER, 20, 4, 1),
        _degrade_module("telemetry", Criticality.BEST_EFFORT, 50, 4, 1),
    )
    return _degrade_scenario(
        "flash_pressure_drops_best_effort",
        modules,
        SystemProfile(70, 32, 8, 4),
        expected_disabled=("telemetry",),
        expected_enabled=("kernel", "sensor", "ai"),
        expected_reason="flash_budget",
    )


def _degrade_ram_pressure_scenario() -> dict[str, object]:
    modules = (
        _degrade_module("kernel", Criticality.HARD_REALTIME, 10, 8, 1),
        _degrade_module("sensor", Criticality.DRIVER, 10, 8, 1),
        _degrade_module("vision", Criticality.USER, 10, 16, 1),
        _degrade_module("telemetry", Criticality.BEST_EFFORT, 10, 4, 1),
    )
    return _degrade_scenario(
        "ram_pressure_drops_best_effort_then_user",
        modules,
        SystemProfile(64, 20, 8, 4),
        expected_disabled=("telemetry", "vision"),
        expected_enabled=("kernel", "sensor"),
        expected_reason="ram_budget",
    )


def _degrade_pool_pressure_scenario() -> dict[str, object]:
    modules = (
        _degrade_module("kernel", Criticality.HARD_REALTIME, 10, 4, 1),
        _degrade_module("sensor", Criticality.DRIVER, 10, 4, 1),
        _degrade_module("ai", Criticality.USER, 10, 4, 2),
        _degrade_module("telemetry", Criticality.BEST_EFFORT, 10, 4, 1),
    )
    return _degrade_scenario(
        "pool_pressure_drops_best_effort",
        modules,
        SystemProfile(64, 32, 4, 4),
        expected_disabled=("telemetry",),
        expected_enabled=("kernel", "sensor", "ai"),
        expected_reason="pool_budget",
    )


def _degrade_module_limit_scenario() -> dict[str, object]:
    modules = (
        _degrade_module("kernel", Criticality.HARD_REALTIME, 10, 4, 1),
        _degrade_module("sensor", Criticality.DRIVER, 10, 4, 1),
        _degrade_module("radio", Criticality.DRIVER, 10, 4, 1),
        _degrade_module("ai", Criticality.USER, 10, 4, 1),
        _degrade_module("telemetry", Criticality.BEST_EFFORT, 10, 4, 1),
    )
    return _degrade_scenario(
        "module_limit_drops_lowest_criticality",
        modules,
        SystemProfile(80, 40, 8, 4),
        expected_disabled=("telemetry",),
        expected_enabled=("kernel", "sensor", "radio", "ai"),
        expected_reason="module_limit",
    )


def _degrade_same_criticality_scenario() -> dict[str, object]:
    modules = (
        _degrade_module("kernel", Criticality.HARD_REALTIME, 10, 4, 1),
        _degrade_module("sensor", Criticality.DRIVER, 10, 4, 1),
        _degrade_module("ai_small", Criticality.USER, 10, 4, 1),
        _degrade_module("ai_large", Criticality.USER, 30, 4, 1),
    )
    return _degrade_scenario(
        "same_criticality_drops_larger_flash_first",
        modules,
        SystemProfile(40, 32, 8, 4),
        expected_disabled=("ai_large",),
        expected_enabled=("kernel", "sensor", "ai_small"),
        expected_reason="flash_budget",
    )


def _degrade_error_scenario() -> dict[str, object]:
    essential_error = None
    capacity_error = None
    profile_error = None
    try:
        DegradePlannerSimulator.fit(
            (
                _degrade_module("kernel", Criticality.HARD_REALTIME, 100, 4, 1),
                _degrade_module("hal", Criticality.SYSTEM, 100, 4, 1),
            ),
            SystemProfile(50, 16, 4, 2),
        )
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        essential_error = str(exc)

    try:
        DegradePlannerSimulator.fit(
            (
                _degrade_module("kernel", Criticality.HARD_REALTIME, 10, 4, 1),
                _degrade_module("sensor", Criticality.DRIVER, 10, 4, 1),
            ),
            SystemProfile(64, 32, 8, 2),
            capacity=1,
        )
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        capacity_error = str(exc)

    try:
        SystemProfile(-1, 16, 4, 2)
    except Exception as exc:  # noqa: BLE001 - report simulator gate context.
        profile_error = str(exc)

    errors: list[str] = []
    _expect_equal(
        essential_error,
        "essential modules exceed profile",
        "essential_error",
        errors,
    )
    _expect_equal(
        capacity_error,
        "too many modules for planner capacity",
        "capacity_error",
        errors,
    )
    _expect_equal(
        profile_error,
        "profile byte limits must be non-negative",
        "profile_error",
        errors,
    )
    return {
        "name": "planner_errors_are_reported",
        "passing": len(errors) == 0,
        "errors": errors,
        "essential_error": essential_error,
        "capacity_error": capacity_error,
        "profile_error": profile_error,
    }


def _degrade_module(
    name: str,
    criticality: Criticality,
    flash_bytes: int,
    ram_bytes: int,
    pool_slots: int,
) -> ModuleSpec:
    return ModuleSpec(
        name,
        criticality,
        MemoryBudget(flash_bytes, ram_bytes, pool_slots),
        period_us=20_000 if criticality == Criticality.HARD_REALTIME else None,
        max_jitter_us=10 if criticality == Criticality.HARD_REALTIME else None,
    )


def _sample_runtime_drill(
    flash_limit: int,
    ram_limit: int,
    pool_limit: int,
    max_modules: int,
    fault_module: str,
    fault_error: str,
    fault_count: int,
) -> dict[str, object]:
    profile = SystemProfile(
        flash_limit_bytes=flash_limit,
        ram_limit_bytes=ram_limit,
        pool_slot_limit=pool_limit,
        max_modules=max_modules,
    )
    drill = RuntimeDrillSimulator(
        modules=_sample_runtime_modules(),
        profile=profile,
        capacity=8,
        event_log_capacity=8,
    )
    return drill.run(
        fault_module=fault_module,
        fault_error=fault_error,
        fault_count=fault_count,
    ).to_dict()


def _check_runtime_drill(
    flash_limit: int,
    ram_limit: int,
    pool_limit: int,
    max_modules: int,
    fault_module: str,
    fault_error: str,
    fault_count: int,
    max_disabled: int,
    max_reboots: int,
    max_dropped_events: int,
) -> dict[str, object]:
    if max_disabled < 0:
        raise ValueError("max_disabled must be non-negative")
    if max_reboots < 0:
        raise ValueError("max_reboots must be non-negative")
    if max_dropped_events < 0:
        raise ValueError("max_dropped_events must be non-negative")

    drill = _sample_runtime_drill(
        flash_limit,
        ram_limit,
        pool_limit,
        max_modules,
        fault_module,
        fault_error,
        fault_count,
    )
    decision = drill["decision"]
    event_log = drill["event_log"]
    recovery_summary = drill["recovery_summary"]
    disabled_count = int(decision["disabled_count"])
    reboot_count = int(recovery_summary["reboot_count"])
    dropped_events = int(event_log["dropped"])

    errors: list[str] = []
    if disabled_count > max_disabled:
        errors.append(f"disabled modules exceeded limit: {disabled_count} > {max_disabled}")
    if reboot_count > max_reboots:
        errors.append(f"module reboots exceeded limit: {reboot_count} > {max_reboots}")
    if dropped_events > max_dropped_events:
        errors.append(
            f"dropped events exceeded limit: {dropped_events} > {max_dropped_events}"
        )

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "limits": {
            "max_disabled": max_disabled,
            "max_reboots": max_reboots,
            "max_dropped_events": max_dropped_events,
        },
        "summary": {
            "disabled_count": disabled_count,
            "reboot_count": reboot_count,
            "dropped_events": dropped_events,
            "final_state": recovery_summary["final_state"],
            "self_healing_required": recovery_summary["self_healing_required"],
        },
        "drill": drill,
    }


def _sample_startup() -> dict[str, object]:
    modules = _sample_runtime_modules()
    dependencies = (
        StartupDependency("sensor", "kernel"),
        StartupDependency("radio", "kernel"),
        StartupDependency("ai", "sensor"),
        StartupDependency("telemetry", "radio"),
    )
    plan = plan_startup(modules, dependencies)
    return {
        "dependencies": [dependency.to_dict() for dependency in dependencies],
        **plan.to_dict(),
    }


def _check_startup_matrix() -> dict[str, object]:
    scenarios = (
        _startup_no_dependencies_scenario(),
        _startup_chain_scenario(),
        _startup_fan_in_out_scenario(),
        _startup_error_scenario(),
    )
    errors: list[str] = []
    for scenario in scenarios:
        for error in scenario["errors"]:
            errors.append(f"{scenario['name']}: {error}")

    return {
        "passing": len(errors) == 0,
        "errors": errors,
        "scenario_count": len(scenarios),
        "scenarios": list(scenarios),
    }


def _startup_scenario(
    name: str,
    modules: tuple[ModuleSpec, ...],
    dependencies: tuple[StartupDependency, ...],
    expected_order: tuple[str, ...],
) -> dict[str, object]:
    plan = plan_startup(modules, dependencies)
    errors: list[str] = []
    _expect_equal(plan.order, expected_order, "order", errors)
    _expect_equal(
        plan.to_dict()["startup_len"],
        len(expected_order),
        "startup_len",
        errors,
    )
    return {
        "name": name,
        "passing": len(errors) == 0,
        "errors": errors,
        "dependencies": [dependency.to_dict() for dependency in dependencies],
        **plan.to_dict(),
    }


def _startup_modules(names: tuple[str, ...]) -> tuple[ModuleSpec, ...]:
    criticalities = {
        "kernel": Criticality.HARD_REALTIME,
        "bus": Criticality.SYSTEM,
        "hal": Criticality.SYSTEM,
        "sensor": Criticality.DRIVER,
        "radio": Criticality.DRIVER,
        "ai": Criticality.USER,
        "telemetry": Criticality.BEST_EFFORT,
    }
    return tuple(
        _degrade_module(name, criticalities.get(name, Criticality.USER), 16, 4, 1)
        for name in names
    )


def _startup_no_dependencies_scenario() -> dict[str, object]:
    return _startup_scenario(
        "no_dependencies_preserve_manifest_order",
        _startup_modules(("kernel", "sensor", "radio")),
        (),
        ("kernel", "sensor", "radio"),
    )


def _startup_chain_scenario() -> dict[str, object]:
    return _startup_scenario(
        "dependency_chain_orders_prerequisites_first",
        _startup_modules(("kernel", "sensor", "ai", "telemetry")),
        (
            StartupDependency("sensor", "kernel"),
            StartupDependency("ai", "sensor"),
            StartupDependency("telemetry", "ai"),
        ),
        ("kernel", "sensor", "ai", "telemetry"),
    )


def _startup_fan_in_out_scenario() -> dict[str, object]:
    return _startup_scenario(
        "fan_in_out_is_deterministic",
        _startup_modules(("kernel", "bus", "sensor", "radio", "ai", "telemetry")),
        (
            StartupDependency("bus", "kernel"),
            StartupDependency("sensor", "bus"),
            StartupDependency("radio", "bus"),
            StartupDependency("ai", "sensor"),
            StartupDependency("ai", "radio"),
            StartupDependency("telemetry", "radio"),
        ),
        ("kernel", "bus", "sensor", "radio", "ai", "telemetry"),
    )


def _startup_error_scenario() -> dict[str, object]:
    modules = _startup_modules(("kernel", "sensor"))
    unknown_dependency_error = None
    self_cycle_error = None
    duplicate_dependency_error = None
    cycle_error = None

    try:
        plan_startup(modules, (StartupDependency("sensor", "missing"),))
    except Exception as exc:  # noqa: BLE001 - report planner gate context.
        unknown_dependency_error = str(exc)

    try:
        plan_startup(modules, (StartupDependency("sensor", "sensor"),))
    except Exception as exc:  # noqa: BLE001 - report planner gate context.
        self_cycle_error = str(exc)

    try:
        plan_startup(
            modules,
            (
                StartupDependency("sensor", "kernel"),
                StartupDependency("sensor", "kernel"),
            ),
        )
    except Exception as exc:  # noqa: BLE001 - report planner gate context.
        duplicate_dependency_error = str(exc)

    try:
        plan_startup(
            modules,
            (
                StartupDependency("kernel", "sensor"),
                StartupDependency("sensor", "kernel"),
            ),
        )
    except Exception as exc:  # noqa: BLE001 - report planner gate context.
        cycle_error = str(exc)

    errors: list[str] = []
    _expect_equal(
        unknown_dependency_error,
        "startup dependency references unknown module: missing",
        "unknown_dependency_error",
        errors,
    )
    _expect_equal(
        self_cycle_error,
        "startup dependency self-cycle: sensor",
        "self_cycle_error",
        errors,
    )
    _expect_equal(
        duplicate_dependency_error,
        "duplicate startup dependency: sensor->kernel",
        "duplicate_dependency_error",
        errors,
    )
    _expect_equal(
        cycle_error,
        "startup dependency cycle: kernel, sensor",
        "cycle_error",
        errors,
    )
    return {
        "name": "planner_errors_are_reported",
        "passing": len(errors) == 0,
        "errors": errors,
        "unknown_dependency_error": unknown_dependency_error,
        "self_cycle_error": self_cycle_error,
        "duplicate_dependency_error": duplicate_dependency_error,
        "cycle_error": cycle_error,
    }


def _sample_project(
    target: str,
    name: str,
    module_name: str,
    author: str,
) -> dict[str, object]:
    return build_project_template(
        name=name,
        target=target,
        module_name=module_name,
        author=author,
    ).to_dict()


def _write_project(
    target: str,
    output: str,
    name: str,
    module_name: str,
    author: str,
    overwrite: bool,
) -> dict[str, object]:
    template = build_project_template(
        name=name,
        target=target,
        module_name=module_name,
        author=author,
    )
    return materialize_project_template(
        template,
        output_dir=output,
        overwrite=overwrite,
    ).to_dict()


def _check_project(path: str, target: str | None) -> dict[str, object]:
    return validate_project_template(path, expected_target=target).to_dict()


def _repair_project(path: str, target: str | None) -> dict[str, object]:
    return repair_project_template(path, expected_target=target).to_dict()


def _sample_runtime_modules() -> tuple[ModuleSpec, ...]:
    return (
        ModuleSpec(
            "kernel",
            Criticality.HARD_REALTIME,
            MemoryBudget(18 * 1024, 4 * 1024, 1),
            owns=(Capability.TIMEBASE, Capability.DEADLINE_TIMER),
            period_us=20_000,
            max_jitter_us=10,
        ),
        ModuleSpec(
            "sensor",
            Criticality.DRIVER,
            MemoryBudget(12 * 1024, 2 * 1024, 1),
            requires=(Capability.BUS0, Capability.SAMPLE_POOL),
            owns=(Capability.BUS0,),
        ),
        ModuleSpec(
            "radio",
            Criticality.DRIVER,
            MemoryBudget(14 * 1024, 2 * 1024, 1),
            requires=(Capability.RADIO,),
            owns=(Capability.RADIO,),
        ),
        ModuleSpec(
            "ai",
            Criticality.USER,
            MemoryBudget(28 * 1024, 8 * 1024, 2),
            requires=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
            owns=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
        ),
        ModuleSpec(
            "telemetry",
            Criticality.BEST_EFFORT,
            MemoryBudget(10 * 1024, 1024, 1),
            requires=(Capability.STREAM,),
            owns=(Capability.STREAM,),
        ),
    )
