import json
from pathlib import Path
import re
import unittest

from nobro_rtos import (
    AiBackendKind,
    AiModelContract,
    AiRoutePolicy,
    AiRoutePreference,
    AiRouteTarget,
    AiRuntimeState,
    BootDiagnostic,
    BootReportSummary,
    Capability,
    Criticality,
    MemoryBudget,
    ModuleSpec,
    NobroContractBundle,
    FixedReport,
    ReportKind,
    ReportStatus,
    RosBridgeDescriptor,
    RosService,
    RosTopic,
    capabilities_from_mask,
    load_repo_host_contract,
    seal_report,
    stable_hash32,
    validate_distribution_metadata,
)


class ContractBuilderTests(unittest.TestCase):
    def test_distribution_metadata_points_to_canonical_contracts(self) -> None:
        repo_root = Path(__file__).resolve().parents[3]
        sdk_manifest = json.loads(
            (repo_root / "sdk" / "sdk-manifest.json").read_text(encoding="utf-8")
        )
        platformio = json.loads(
            (repo_root / "packages" / "platformio" / "library.json").read_text(
                encoding="utf-8"
            )
        )
        arduino_properties = dict(
            line.split("=", 1)
            for line in (
                repo_root / "packages" / "arduino" / "library.properties"
            ).read_text(encoding="utf-8").splitlines()
            if line and not line.startswith("#")
        )

        self.assertEqual(sdk_manifest["name"], "NobroRTOS Standalone SDK")
        self.assertEqual(sdk_manifest["license"], "Apache-2.0")
        self.assertEqual(
            sdk_manifest["canonical_contract"],
            "host/nobro-host-contract.json",
        )
        self.assertIn("bindings/c/include", sdk_manifest["include_roots"])
        self.assertIn("bindings/cpp/include", sdk_manifest["include_roots"])
        self.assertEqual(
            sdk_manifest["generated_output_policy"]["commit_cache_dirs"],
            False,
        )

        self.assertEqual(arduino_properties["name"], "NobroRTOS")
        self.assertEqual(
            arduino_properties["url"],
            "https://github.com/dunknowcoding/NobroRTOS",
        )
        self.assertEqual(arduino_properties["includes"], "NobroRTOS.h")

        self.assertEqual(platformio["name"], "NobroRTOS")
        self.assertEqual(platformio["license"], "Apache-2.0")
        self.assertEqual(
            platformio["repository"]["url"],
            "https://github.com/dunknowcoding/NobroRTOS.git",
        )
        self.assertEqual(platformio["headers"], ["NobroRTOS.h"])

        report = validate_distribution_metadata(repo_root)
        self.assertEqual(report.sdk_name, "NobroRTOS Standalone SDK")
        self.assertEqual(report.arduino_name, "NobroRTOS")
        self.assertEqual(report.platformio_name, "NobroRTOS")

    def test_c_header_report_constants_match_host_contract(self) -> None:
        repo_root = Path(__file__).resolve().parents[3]
        header = (repo_root / "bindings" / "c" / "include" / "nobro_rtos.h").read_text(
            encoding="utf-8"
        )
        cpp_header = (
            repo_root / "bindings" / "cpp" / "include" / "nobro_rtos.hpp"
        ).read_text(encoding="utf-8")
        contract = load_repo_host_contract()
        report_defines = {
            "board_profile_report": "NOBRO_BOARD_PROFILE_REPORT_MAGIC",
            "board_package_report": "NOBRO_BOARD_PACKAGE_REPORT_MAGIC",
            "manifest_report": "NOBRO_MANIFEST_REPORT_MAGIC",
            "adapter_compat_report": "NOBRO_ADAPTER_COMPAT_REPORT_MAGIC",
        }
        report_structs = {
            "board_profile_report": "nobro_board_profile_report_t",
            "board_package_report": "nobro_board_package_report_t",
            "manifest_report": "nobro_manifest_report_t",
            "adapter_compat_report": "nobro_adapter_compat_report_t",
        }

        for report_key, define in report_defines.items():
            match = re.search(rf"#define\s+{define}\s+(0x[0-9A-Fa-f]+)u", header)
            self.assertIsNotNone(match, define)
            self.assertEqual(
                int(match.group(1), 16),
                int(contract.payload[report_key]["magic"], 16),
            )
            self.assertIn(report_structs[report_key], header)

        for symbol in (
            "NOBRO_FNV1A32_OFFSET",
            "nobro_stable_hash32_cstr",
            "nobro_ai_model_contract_t",
            "nobro_ai_route_policy_t",
            "nobro_ai_route_decide",
            "nobro_ros_bridge_contract_t",
            "nobro_ros_topic_contract_t",
        ):
            self.assertIn(symbol, header)

        for symbol in (
            "stable_hash32",
            "AiRouteDecisionView",
            "decide_ai_route",
            "RosBridgeContractView",
        ):
            self.assertIn(symbol, cpp_header)

    def test_bundle_exports_stable_masks_and_schema_version(self) -> None:
        bundle = NobroContractBundle(
            modules=(
                ModuleSpec(
                    "ai",
                    Criticality.USER,
                    MemoryBudget(16 * 1024, 6 * 1024, 1),
                    requires=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
                    owns=(Capability.AI_ENDPOINT,),
                ),
            ),
            ai_models=(
                AiModelContract(
                    42,
                    AiBackendKind.ON_DEVICE,
                    128,
                    32,
                    4096,
                    20_000,
                    100_000,
                ),
            ),
        )

        payload = json.loads(bundle.to_json())

        self.assertEqual(payload["schema_version"], 1)
        self.assertEqual(
            payload["modules"][0]["requires_bits"],
            Capability.AI_INFERENCE.bit | Capability.AI_ENDPOINT.bit,
        )
        self.assertEqual(payload["modules"][0]["owns_bits"], Capability.AI_ENDPOINT.bit)
        self.assertEqual(payload["ai_models"][0]["backend"], "on_device")

    def test_bundle_round_trips_from_json(self) -> None:
        bundle = NobroContractBundle(
            metadata={"profile": "roundtrip"},
            modules=(
                ModuleSpec(
                    "ai",
                    Criticality.USER,
                    MemoryBudget(16 * 1024, 6 * 1024, 1),
                    requires=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
                    owns=(Capability.AI_ENDPOINT,),
                ),
            ),
            ai_models=(
                AiModelContract(
                    42,
                    AiBackendKind.ON_DEVICE,
                    128,
                    32,
                    4096,
                    20_000,
                    100_000,
                ),
            ),
            ros_bridges=(
                RosBridgeDescriptor(
                    "robot_core",
                    "serial",
                    topics=(RosTopic("/imu", "sensor_msgs/Imu", 4, 128),),
                    services=(RosService("/reset", 16, 16, 50_000),),
                ),
            ),
        )

        loaded = NobroContractBundle.from_json(bundle.to_json())

        self.assertEqual(loaded.to_dict(), bundle.to_dict())

    def test_ros_bridge_metadata_exports_stable_hashes(self) -> None:
        bridge = RosBridgeDescriptor(
            "robot_core",
            "serial",
            topics=(RosTopic("/imu", "sensor_msgs/Imu", 4, 128),),
            services=(RosService("/reset", 16, 16, 50_000),),
        )

        payload = bridge.to_dict()

        self.assertEqual(stable_hash32("/imu"), 0xB4CAA2A7)
        self.assertEqual(payload["bridge_id_hash"], stable_hash32("robot_core"))
        self.assertEqual(payload["transport_hash"], stable_hash32("serial"))
        self.assertEqual(payload["topics"][0]["name_hash"], stable_hash32("/imu"))
        self.assertEqual(
            payload["topics"][0]["message_type_hash"],
            stable_hash32("sensor_msgs/Imu"),
        )
        self.assertEqual(payload["services"][0]["name_hash"], stable_hash32("/reset"))

    def test_ai_route_policy_matches_runtime_decision_vectors(self) -> None:
        hybrid = AiModelContract(
            42,
            AiBackendKind.HYBRID,
            128,
            32,
            4096,
            20_000,
            100_000,
        )
        remote = AiModelContract(
            43,
            AiBackendKind.REMOTE_API,
            128,
            32,
            0,
            20_000,
            100_000,
        )

        local_only = AiRoutePolicy(AiRoutePreference.LOCAL_ONLY, 10_000, 3)
        self.assertEqual(
            local_only.decide(
                hybrid,
                AiRuntimeState(True, True, 1_000, 0),
                30_000,
            ).target,
            AiRouteTarget.ON_DEVICE,
        )

        prefer_remote = AiRoutePolicy(AiRoutePreference.PREFER_REMOTE, 50_000, 2)
        tripped = prefer_remote.decide(
            remote,
            AiRuntimeState(False, True, 10_000, 2),
            30_000,
        )
        self.assertEqual(tripped.target, AiRouteTarget.STALE_SNAPSHOT)
        self.assertTrue(tripped.endpoint_circuit_open)
        self.assertTrue(tripped.uses_stale_snapshot)

        fallback = AiRoutePolicy(AiRoutePreference.HYBRID_FALLBACK, 1_000, 3)
        self.assertEqual(
            fallback.decide(
                remote,
                AiRuntimeState(False, True, 5_000, 0),
                5_000,
            ).target,
            AiRouteTarget.DEGRADED_FALLBACK,
        )

        unavailable = local_only.decide(
            remote,
            AiRuntimeState(False, False, 50_000, 0),
            5_000,
        )
        self.assertEqual(unavailable.target, AiRouteTarget.UNAVAILABLE)

    def test_capability_masks_reject_unknown_bits(self) -> None:
        self.assertEqual(
            capabilities_from_mask(Capability.AI_ENDPOINT.bit),
            (Capability.AI_ENDPOINT,),
        )
        with self.assertRaisesRegex(ValueError, "unknown capability bits"):
            capabilities_from_mask(1 << 31)

    def test_hard_realtime_module_requires_deadline(self) -> None:
        bundle = NobroContractBundle(
            modules=(
                ModuleSpec(
                    "actuator",
                    Criticality.HARD_REALTIME,
                    MemoryBudget(4096, 512),
                ),
            ),
        )

        with self.assertRaisesRegex(ValueError, "deadline"):
            bundle.to_dict()

    def test_ros_bridge_rejects_duplicate_topics(self) -> None:
        bridge = RosBridgeDescriptor(
            "robot_core",
            "serial",
            topics=(
                RosTopic("/imu", "sensor_msgs/Imu", 4, 128),
                RosTopic("/imu", "sensor_msgs/Imu", 4, 128),
            ),
            services=(RosService("/reset", 16, 16, 50_000),),
        )

        with self.assertRaisesRegex(ValueError, "duplicate ROS topic"):
            bridge.to_dict()

    def test_bundle_rejects_duplicate_module_names(self) -> None:
        spec = ModuleSpec(
            "sensor",
            Criticality.DRIVER,
            MemoryBudget(8192, 1024),
            requires=(Capability.BUS0,),
        )
        bundle = NobroContractBundle(modules=(spec, spec))

        with self.assertRaisesRegex(ValueError, "duplicate module"):
            bundle.to_json()

    def test_repo_host_contract_matches_python_enums(self) -> None:
        contract = load_repo_host_contract()

        self.assertEqual(
            contract.boot_stage_order(),
            (
                "board_profile",
                "board_package",
                "manifest",
                "adapter_compatibility",
                "admission",
                "runtime",
            ),
        )
        self.assertEqual(contract.capability_label(Capability.AI_ENDPOINT), "ai_endpoint")

    def test_boot_diagnostic_decoder_preserves_error_label(self) -> None:
        contract = load_repo_host_contract()
        diagnostic = BootDiagnostic.decode(0x0404_0003, contract)

        self.assertFalse(diagnostic.passing)
        self.assertEqual(diagnostic.stage, "adapter_compatibility")
        self.assertEqual(diagnostic.status, "fail")
        self.assertEqual(diagnostic.error_code, 3)
        self.assertEqual(diagnostic.error_label, "capability_ownership_conflict")

    def test_boot_diagnostic_decoder_handles_pass(self) -> None:
        contract = load_repo_host_contract()
        diagnostic = BootDiagnostic.decode(0x0600_0000, contract)

        self.assertTrue(diagnostic.passing)
        self.assertEqual(diagnostic.stage, "runtime")
        self.assertEqual(diagnostic.status, "pass")
        self.assertIsNone(diagnostic.error_label)

    def test_manifest_report_decoder_accepts_sealed_pass(self) -> None:
        payload = seal_report(
            ReportKind.MANIFEST,
            {
                "valid": 1,
                "module_count": 2,
                "fingerprint": 0x1234,
                "required_bits": Capability.BUS0.bit,
                "owned_bits": Capability.HOST_REPORT.bit,
                "flash_used_bytes": 4096,
                "ram_used_bytes": 1024,
                "pool_used_slots": 2,
            },
        )

        report = FixedReport.from_dict(ReportKind.MANIFEST, payload)

        self.assertEqual(report.status, ReportStatus.PASS)
        self.assertTrue(report.passing)
        self.assertTrue(report.verify_checksum())
        self.assertEqual(report.to_dict()["count"], 2)

    def test_board_profile_report_decoder_accepts_sealed_pass(self) -> None:
        payload = seal_report(
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
        )

        report = FixedReport.from_dict(ReportKind.BOARD_PROFILE, payload)

        self.assertEqual(report.status, ReportStatus.PASS)
        self.assertTrue(report.verify_checksum())
        self.assertEqual(report.to_dict()["raw"]["servo_pin"], 24)

    def test_board_package_report_decoder_preserves_failure_context(self) -> None:
        payload = seal_report(
            ReportKind.BOARD_PACKAGE,
            {
                "valid": 0,
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
                "error_code": 7,
            },
        )

        report = FixedReport.from_dict(ReportKind.BOARD_PACKAGE, payload)

        self.assertEqual(report.status, ReportStatus.FAIL)
        self.assertEqual(report.error_label(), "duplicate_critical_pin")

    def test_adapter_report_decoder_preserves_failure_context(self) -> None:
        payload = seal_report(
            ReportKind.ADAPTER_COMPAT,
            {
                "compatible": 0,
                "adapter_count": 2,
                "error_code": 3,
                "error_module_tag": 3,
                "error_capability_bits": Capability.BUS0.bit,
            },
        )

        report = FixedReport.from_dict(ReportKind.ADAPTER_COMPAT, payload)
        summary = report.to_dict()

        self.assertEqual(report.status, ReportStatus.FAIL)
        self.assertFalse(report.passing)
        self.assertEqual(summary["error_label"], "capability_ownership_conflict")
        self.assertEqual(summary["error_module_label"], "bus")

    def test_report_decoder_marks_corrupt_checksum(self) -> None:
        payload = seal_report(
            ReportKind.MANIFEST,
            {
                "valid": 1,
                "module_count": 1,
                "fingerprint": 0xCAFE,
            },
        )
        payload["module_count"] = 2

        report = FixedReport.from_dict(ReportKind.MANIFEST, payload)

        self.assertEqual(report.status, ReportStatus.CORRUPT)

    def test_boot_summary_reports_first_missing_stage(self) -> None:
        manifest = seal_report(
            ReportKind.MANIFEST,
            {
                "valid": 1,
                "module_count": 2,
                "fingerprint": 0x1234,
            },
        )
        summary = BootReportSummary.from_dict({"reports": {"manifest": manifest}})

        self.assertFalse(summary.passing)
        self.assertEqual(summary.first_diagnostic.stage, "board_profile")
        self.assertEqual(summary.first_diagnostic.status, ReportStatus.MISSING)
        self.assertEqual(summary.status_counts()["missing"], 5)

    def test_boot_summary_reports_adapter_failure_after_passing_early_slots(self) -> None:
        board_profile = seal_report(
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
        )
        board_package = seal_report(
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
        )
        manifest = seal_report(
            ReportKind.MANIFEST,
            {
                "valid": 1,
                "module_count": 2,
                "fingerprint": 0x1234,
            },
        )
        adapter = seal_report(
            ReportKind.ADAPTER_COMPAT,
            {
                "compatible": 0,
                "adapter_count": 2,
                "error_code": 3,
                "error_module_tag": 3,
                "error_capability_bits": Capability.BUS0.bit,
            },
        )
        summary = BootReportSummary.from_dict(
            {
                "reports": {
                    "board_profile": board_profile,
                    "board_package": board_package,
                    "manifest": manifest,
                    "adapter_compatibility": adapter,
                }
            }
        )

        self.assertFalse(summary.passing)
        self.assertEqual(summary.first_diagnostic.stage, "adapter_compatibility")
        self.assertEqual(summary.first_diagnostic.status, ReportStatus.FAIL)
        self.assertEqual(
            summary.first_diagnostic.error_label, "capability_ownership_conflict"
        )


if __name__ == "__main__":
    unittest.main()
