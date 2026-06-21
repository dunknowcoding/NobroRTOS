"""Command-line helpers for NobroRTOS Python tooling."""

from __future__ import annotations

import argparse
import json

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
        "sample-quota",
        help="run a deterministic quota ledger simulation and print JSON",
    )
    sample_degrade = subparsers.add_parser(
        "sample-degrade",
        help="run a deterministic degraded-mode planning simulation and print JSON",
    )
    sample_degrade.add_argument("--flash-limit", type=int, default=72 * 1024)
    sample_degrade.add_argument("--ram-limit", type=int, default=16 * 1024)
    sample_degrade.add_argument("--pool-limit", type=int, default=5)
    sample_degrade.add_argument("--max-modules", type=int, default=4)
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
    if args.command == "sample-quota":
        print(json.dumps(_sample_quota(), indent=2, sort_keys=True))
        return 0
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
        print(
            json.dumps(
                _check_project(args.path, args.target),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    if args.command == "repair-project":
        print(
            json.dumps(
                _repair_project(args.path, args.target),
                indent=2,
                sort_keys=True,
            )
        )
        return 0
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
                owns=(Capability.AI_ENDPOINT,),
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
            "watchdog",
            "scheduler",
            "event_log",
            "quota",
            "degrade",
            "runtime_drill",
            "project_templates",
        ],
    }


def _sample_ai_route() -> dict[str, object]:
    contract = AiModelContract(
        model_id=42,
        backend=AiBackendKind.HYBRID,
        input_bytes_max=128,
        output_bytes_max=32,
        arena_bytes=4096,
        timeout_us=20_000,
        stale_after_us=100_000,
    )
    policy = AiRoutePolicy(
        preference=AiRoutePreference.HYBRID_FALLBACK,
        stale_after_us=50_000,
        endpoint_failure_limit=2,
    )
    state = AiRuntimeState(
        local_ready=True,
        endpoint_ready=False,
        last_success_age_us=12_000,
        consecutive_endpoint_failures=2,
    )
    decision = policy.decide(contract, state, budget_us=25_000)
    return {
        "contract": contract.to_dict(),
        "policy": {
            "preference": policy.preference.name.lower(),
            "stale_after_us": policy.stale_after_us,
            "endpoint_failure_limit": policy.endpoint_failure_limit,
        },
        "state": {
            "local_ready": state.local_ready,
            "endpoint_ready": state.endpoint_ready,
            "last_success_age_us": state.last_success_age_us,
            "consecutive_endpoint_failures": state.consecutive_endpoint_failures,
        },
        "decision": decision.to_dict(),
    }


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
        ),
        ModuleSpec(
            "radio",
            Criticality.DRIVER,
            MemoryBudget(14 * 1024, 2 * 1024, 1),
            requires=(Capability.RADIO,),
        ),
        ModuleSpec(
            "ai",
            Criticality.USER,
            MemoryBudget(28 * 1024, 8 * 1024, 2),
            requires=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
        ),
        ModuleSpec(
            "telemetry",
            Criticality.BEST_EFFORT,
            MemoryBudget(10 * 1024, 1024, 1),
            requires=(Capability.STREAM,),
        ),
    )
