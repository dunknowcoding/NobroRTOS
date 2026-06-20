"""Command-line helpers for NobroRTOS Python tooling."""

from __future__ import annotations

import argparse

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
    args = parser.parse_args()

    if args.command == "sample-ai-ros":
        print(_sample_ai_ros_bundle().to_json())
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
