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
from .distribution import validate_distribution_metadata
from .host_contract import BootDiagnostic, load_repo_host_contract
from .reports import BootReportSummary, FixedReport, ReportKind, seal_report
from .sim import SensorStubError, SensorStubMode, SensorStubSimulator


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
        choices=("ai_model", "ros_bridge"),
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
    subparsers.add_parser(
        "check-host-contract",
        help="validate host/nobro-host-contract.json against Python enums",
    )
    subparsers.add_parser(
        "check-distribution-metadata",
        help="validate SDK, Arduino, and PlatformIO package metadata",
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
    if args.command == "check-host-contract":
        contract = load_repo_host_contract()
        stages = ", ".join(contract.boot_stage_order())
        print(f"host contract ok: {stages}")
        return 0
    if args.command == "check-distribution-metadata":
        report = validate_distribution_metadata()
        print(json.dumps(report.to_dict(), indent=2, sort_keys=True))
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
