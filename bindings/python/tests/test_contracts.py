import json
import unittest

from nobro_rtos import (
    AiBackendKind,
    AiModelContract,
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
)


class ContractBuilderTests(unittest.TestCase):
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
