"""Command-line helpers for NobroRTOS Python tooling."""

from __future__ import annotations

import argparse
import json

from .contracts import (
    AiBackendKind,
    AiModelContract,
    Capability,
    Criticality,
    MemoryBudget,
    ModuleSpec,
    NobroContractBundle,
    RosBridgeDescriptor,
    RosParameter,
    RosService,
    RosTopic,
)
from .host_contract import BootDiagnostic, load_repo_host_contract
from .reports import BootReportSummary, FixedReport


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
        "check-host-contract",
        help="validate host/nobro-host-contract.json against Python enums",
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
        help="decode a manifest or adapter compatibility report JSON file",
    )
    decode_report.add_argument(
        "kind", choices=("manifest", "adapter_compatibility"), help="report kind"
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
    if args.command == "check-host-contract":
        contract = load_repo_host_contract()
        stages = ", ".join(contract.boot_stage_order())
        print(f"host contract ok: {stages}")
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
