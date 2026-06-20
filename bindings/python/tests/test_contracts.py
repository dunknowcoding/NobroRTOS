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
    RecoveryAction,
    RecoveryPolicySimulator,
    SchedulerSimulator,
    SensorStubError,
    SensorStubSimulator,
    ServoSimulator,
    ServoSimulatorError,
    WatchdogSimulator,
    capabilities_from_mask,
    load_repo_host_contract,
    seal_report,
    stable_hash32,
    validate_distribution_metadata,
)
from nobro_rtos.cli import (
    _doctor,
    _sample_actuator,
    _sample_recovery,
    _sample_report,
    _sample_scheduler,
    _sample_sensor,
    _sample_watchdog,
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
        self.assertEqual(sdk_manifest["python_package"], "bindings/python")
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
        self.assertEqual(report.python_package_name, "nobro-rtos-tools")
        self.assertEqual(report.python_requires, ">=3.10")

    def test_doctor_summarizes_host_and_package_health(self) -> None:
        report = _doctor()

        self.assertEqual(report["status"], "ok")
        self.assertIn("runtime", report["host_contract"]["boot_stages"])
        self.assertGreater(report["host_contract"]["capability_count"], 0)
        self.assertEqual(
            report["distribution"]["python_package_name"],
            "nobro-rtos-tools",
        )
        self.assertIn("scheduler", report["host_simulators"])

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
            "admission_report": "NOBRO_ADMISSION_REPORT_MAGIC",
            "runtime_report": "NOBRO_RUNTIME_REPORT_MAGIC",
            "health_report": "NOBRO_HEALTH_REPORT_MAGIC",
            "event_log_report": "NOBRO_EVENT_LOG_REPORT_MAGIC",
            "module_runtime_report": "NOBRO_MODULE_RUNTIME_REPORT_MAGIC",
            "degrade_application_report": "NOBRO_DEGRADE_APPLICATION_REPORT_MAGIC",
            "ai_contracts.report": "NOBRO_AI_MODEL_REPORT_MAGIC",
            "ros_bridge_contracts.report": "NOBRO_ROS_BRIDGE_REPORT_MAGIC",
        }
        report_structs = {
            "board_profile_report": "nobro_board_profile_report_t",
            "board_package_report": "nobro_board_package_report_t",
            "manifest_report": "nobro_manifest_report_t",
            "adapter_compat_report": "nobro_adapter_compat_report_t",
            "admission_report": "nobro_admission_report_t",
            "runtime_report": "nobro_runtime_report_t",
            "health_report": "nobro_health_report_t",
            "event_log_report": "nobro_event_log_report_t",
            "module_runtime_report": "nobro_module_runtime_report_t",
            "degrade_application_report": "nobro_degrade_application_report_t",
            "ai_contracts.report": "nobro_ai_model_report_t",
            "ros_bridge_contracts.report": "nobro_ros_bridge_report_t",
        }

        for report_key, define in report_defines.items():
            match = re.search(rf"#define\s+{define}\s+(0x[0-9A-Fa-f]+)u", header)
            self.assertIsNotNone(match, define)
            report_contract = contract.payload
            for key in report_key.split("."):
                report_contract = report_contract[key]
            self.assertEqual(
                int(match.group(1), 16),
                int(report_contract["magic"], 16),
            )
            self.assertIn(report_structs[report_key], header)

        for symbol in (
            "NOBRO_FNV1A32_OFFSET",
            "nobro_stable_hash32_cstr",
            "nobro_ai_model_contract_t",
            "nobro_ai_route_policy_t",
            "nobro_ai_route_decide",
            "nobro_ai_model_report_status",
            "nobro_ros_bridge_contract_t",
            "nobro_ros_topic_contract_t",
            "nobro_ros_bridge_report_status",
            "nobro_admission_report_status",
            "nobro_runtime_report_status",
            "nobro_health_report_status",
            "nobro_event_log_report_status",
            "nobro_module_runtime_report_status",
            "nobro_degrade_application_report_status",
        ):
            self.assertIn(symbol, header)

        for symbol in (
            "stable_hash32",
            "AiRouteDecisionView",
            "AiModelReportView",
            "decide_ai_route",
            "RosBridgeContractView",
            "RosBridgeReportView",
            "AdmissionReportView",
            "RuntimeReportView",
            "HealthReportView",
            "EventLogReportView",
            "ModuleRuntimeReportView",
            "DegradeApplicationReportView",
        ):
            self.assertIn(symbol, cpp_header)

    def test_c_header_ai_and_ros_codes_match_host_contract(self) -> None:
        repo_root = Path(__file__).resolve().parents[3]
        header = (repo_root / "bindings" / "c" / "include" / "nobro_rtos.h").read_text(
            encoding="utf-8"
        )
        contract = load_repo_host_contract()

        c_symbols = {
            "ai_contracts.backend_codes": {
                "NOBRO_AI_BACKEND_ON_DEVICE": "on_device",
                "NOBRO_AI_BACKEND_REMOTE_API": "remote_api",
                "NOBRO_AI_BACKEND_EDGE_SIDECAR": "edge_sidecar",
                "NOBRO_AI_BACKEND_HYBRID": "hybrid",
            },
            "ai_contracts.route_preferences": {
                "NOBRO_AI_ROUTE_LOCAL_ONLY": "local_only",
                "NOBRO_AI_ROUTE_PREFER_LOCAL": "prefer_local",
                "NOBRO_AI_ROUTE_PREFER_REMOTE": "prefer_remote",
                "NOBRO_AI_ROUTE_HYBRID_FALLBACK": "hybrid_fallback",
            },
            "ai_contracts.route_targets": {
                "NOBRO_AI_TARGET_ON_DEVICE": "on_device",
                "NOBRO_AI_TARGET_REMOTE_API": "remote_api",
                "NOBRO_AI_TARGET_EDGE_SIDECAR": "edge_sidecar",
                "NOBRO_AI_TARGET_STALE_SNAPSHOT": "stale_snapshot",
                "NOBRO_AI_TARGET_DEGRADED_FALLBACK": "degraded_fallback",
                "NOBRO_AI_TARGET_UNAVAILABLE": "unavailable",
            },
            "ros_bridge_contracts.transport_codes": {
                "NOBRO_ROS_TRANSPORT_SERIAL": "serial",
                "NOBRO_ROS_TRANSPORT_UDP": "udp",
                "NOBRO_ROS_TRANSPORT_RADIO": "radio",
                "NOBRO_ROS_TRANSPORT_SHARED_MEMORY": "shared_memory",
                "NOBRO_ROS_TRANSPORT_CUSTOM": "custom",
            },
        }

        for table_path, symbols in c_symbols.items():
            table = contract.payload
            for key in table_path.split("."):
                table = table[key]
            for symbol, expected_label in symbols.items():
                match = re.search(rf"\b{symbol}\s*=\s*(\d+)", header)
                self.assertIsNotNone(match, symbol)
                self.assertEqual(table[match.group(1)], expected_label)

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
        self.assertEqual(
            contract.payload["ai_contracts"]["backend_codes"]["4"],
            "hybrid",
        )
        self.assertEqual(
            contract.payload["ai_contracts"]["route_targets"]["5"],
            "degraded_fallback",
        )
        self.assertEqual(
            contract.payload["ros_bridge_contracts"]["hash"],
            "fnv1a32_utf8",
        )
        self.assertEqual(
            contract.payload["ros_bridge_contracts"]["transport_codes"]["255"],
            "custom",
        )
        self.assertEqual(
            contract.payload["runtime_report"]["symbol"],
            "NOBRO_RUNTIME_REPORT",
        )
        self.assertEqual(contract.payload["health_report"]["magic"], "0x4E42484C")
        self.assertEqual(
            contract.payload["degrade_application_report"]["version"],
            1,
        )

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

    def test_ai_model_report_decoder_preserves_route_policy(self) -> None:
        payload = seal_report(
            ReportKind.AI_MODEL,
            {
                "backend": int(AiBackendKind.HYBRID),
                "model_id": 7,
                "input_bytes_max": 16,
                "output_bytes_max": 24,
                "arena_bytes": 4096,
                "timeout_us": 5_000,
                "route_preference": int(AiRoutePreference.HYBRID_FALLBACK),
                "stale_after_us": 30_000,
                "endpoint_failure_limit": 2,
            },
        )

        report = FixedReport.from_dict(ReportKind.AI_MODEL, payload)
        summary = report.to_dict()

        self.assertEqual(report.status, ReportStatus.PASS)
        self.assertTrue(report.verify_checksum())
        self.assertEqual(summary["backend"], "hybrid")
        self.assertEqual(summary["route_preference"], "hybrid_fallback")
        self.assertEqual(summary["raw"]["route_preference"], 4)
        self.assertEqual(summary["raw"]["stale_after_us"], 30_000)

    def test_ai_model_report_decoder_handles_missing_slot(self) -> None:
        report = FixedReport.from_dict(ReportKind.AI_MODEL, {})
        summary = report.to_dict()

        self.assertEqual(report.status, ReportStatus.MISSING)
        self.assertIsNone(summary["backend"])
        self.assertIsNone(summary["route_preference"])

    def test_ros_bridge_report_decoder_preserves_resource_summary(self) -> None:
        payload = seal_report(
            ReportKind.ROS_BRIDGE,
            {
                "transport": 1,
                "bridge_id_hash": stable_hash32("main"),
                "topic_count": 2,
                "service_count": 1,
                "action_count": 0,
                "parameter_count": 1,
                "total_buffer_bytes": 768,
                "max_timeout_us": 50_000,
            },
        )

        report = FixedReport.from_dict(ReportKind.ROS_BRIDGE, payload)
        summary = report.to_dict()

        self.assertEqual(report.status, ReportStatus.PASS)
        self.assertTrue(report.verify_checksum())
        self.assertEqual(summary["transport"], "serial")
        self.assertEqual(summary["raw"]["topic_count"], 2)
        self.assertEqual(summary["raw"]["total_buffer_bytes"], 768)

    def test_cli_sample_reports_are_decodable(self) -> None:
        ai = FixedReport.from_dict(ReportKind.AI_MODEL, _sample_report("ai_model"))
        ros = FixedReport.from_dict(ReportKind.ROS_BRIDGE, _sample_report("ros_bridge"))

        self.assertEqual(ai.status, ReportStatus.PASS)
        self.assertEqual(ai.to_dict()["backend"], "hybrid")
        self.assertEqual(ros.status, ReportStatus.PASS)
        self.assertEqual(ros.to_dict()["transport"], "serial")

    def test_cli_diagnostic_sample_reports_are_decodable(self) -> None:
        samples = {
            "admission": ReportKind.ADMISSION,
            "runtime": ReportKind.RUNTIME,
            "health": ReportKind.HEALTH,
            "event_log": ReportKind.EVENT_LOG,
            "module_runtime": ReportKind.MODULE_RUNTIME,
            "degrade_application": ReportKind.DEGRADE_APPLICATION,
        }

        decoded = {
            name: FixedReport.from_dict(kind, _sample_report(name)).to_dict()
            for name, kind in samples.items()
        }

        self.assertTrue(decoded["admission"]["admitted"])
        self.assertEqual(decoded["runtime"]["next_alarm_due_us"], 0x1234_5678_9ABC)
        self.assertEqual(decoded["health"]["module_label"], "sensor")
        self.assertEqual(decoded["event_log"]["latest_module_label"], "sensor")
        self.assertEqual(decoded["module_runtime"]["latest_change_us"], 0x1_0000_00C0)
        self.assertEqual(decoded["degrade_application"]["applied_at_us"], 0x1_0000_0020)

    def test_admission_report_decoder_preserves_failure_context(self) -> None:
        payload = seal_report(
            ReportKind.ADMISSION,
            {
                "admitted": 0,
                "error_code": 5,
            },
        )

        report = FixedReport.from_dict(ReportKind.ADMISSION, payload)

        self.assertEqual(report.status, ReportStatus.FAIL)
        self.assertEqual(report.error_label(), "missing_startup_node")

    def test_sensor_stub_simulator_matches_nominal_fixture_shape(self) -> None:
        simulator = SensorStubSimulator.nominal(sample_period_ticks=3)

        self.assertIsNone(simulator.poll(10))
        self.assertIsNone(simulator.poll(11))
        sample = simulator.poll(12)

        self.assertIsNotNone(sample)
        self.assertTrue(sample.plausible)
        self.assertEqual(sample.tick, 3)
        self.assertEqual(sample.to_dict()["captured_us"], 12)

    def test_sensor_stub_simulator_fault_modes_are_deterministic(self) -> None:
        silent = SensorStubSimulator.silent(sample_period_ticks=1)
        self.assertEqual(silent.run(3), [])

        erroring = SensorStubSimulator.error_every(3, sample_period_ticks=1)
        erroring.poll()
        erroring.poll()
        with self.assertRaises(SensorStubError):
            erroring.poll()

        bad = SensorStubSimulator.bad_data_every(2, sample_period_ticks=1)
        first = bad.poll()
        second = bad.poll()
        self.assertIsNotNone(first)
        self.assertIsNotNone(second)
        self.assertTrue(first.plausible)
        self.assertFalse(second.plausible)

    def test_cli_sensor_sample_summarizes_fixture_modes(self) -> None:
        bad = _sample_sensor("bad_data_every", 3, 1, 2)
        error = _sample_sensor("error_every", 3, 1, 2)

        self.assertEqual(bad["sample_count"], 3)
        self.assertEqual(bad["error_count"], 0)
        self.assertFalse(bad["samples"][1]["plausible"])
        self.assertEqual(error["sample_count"], 2)
        self.assertEqual(error["error_count"], 1)
        self.assertEqual(error["errors"][0]["tick"], 2)

    def test_servo_simulator_checks_channel_range_and_timing(self) -> None:
        servo = ServoSimulator(readback_offset_us=10, readback_tolerance_us=50)
        command = servo.set_duty_us(0, 1500, deadline_us=100, issued_at_us=90)
        late = servo.set_duty_us(0, 1600, deadline_us=100, issued_at_us=120)

        self.assertTrue(command.accepted)
        self.assertEqual(command.readback_delta_us, 10)
        self.assertFalse(late.deadline_met)
        self.assertFalse(late.accepted)
        with self.assertRaises(ServoSimulatorError):
            servo.set_duty_us(1, 1500, deadline_us=100)
        with self.assertRaises(ServoSimulatorError):
            servo.set_duty_us(0, 2600, deadline_us=100)

    def test_cli_actuator_sample_summarizes_sweep(self) -> None:
        report = _sample_actuator(1200, 1800, 300, readback_offset_us=60, tolerance_us=50)

        self.assertEqual(report["command_count"], 3)
        self.assertEqual(report["accepted_count"], 0)
        self.assertEqual(report["deadline_miss_count"], 0)
        self.assertEqual(report["readback_fail_count"], 3)
        self.assertEqual(report["commands"][0]["pulse_us"], 1200)

    def test_recovery_policy_simulator_escalates_like_kernel_thresholds(self) -> None:
        simulator = RecoveryPolicySimulator(notify_after=2, reboot_after=3)

        first = simulator.record_error("sensor", "sensor_read_fail", 10)
        second = simulator.record_error("sensor", "sensor_read_fail", 20)
        third = simulator.record_error("sensor", "sensor_read_fail", 30)

        self.assertEqual(first.action, RecoveryAction.IGNORE)
        self.assertEqual(second.action, RecoveryAction.NOTIFY_USER_TASK)
        self.assertEqual(second.state, "degraded")
        self.assertEqual(third.action, RecoveryAction.REBOOT_MODULE)
        self.assertEqual(third.state, "recovering")

    def test_recovery_policy_simulator_ok_resets_consecutive_errors(self) -> None:
        simulator = RecoveryPolicySimulator(notify_after=2, reboot_after=4)
        simulator.record_error("bus", "bus_timeout", 10)
        ok = simulator.record_ok(11)
        next_error = simulator.record_error("bus", "bus_timeout", 20)

        self.assertEqual(ok["consecutive_errors"], 0)
        self.assertEqual(next_error.action, RecoveryAction.RETRY_DELAY)
        self.assertEqual(next_error.delay_us, 1000)
        self.assertEqual(next_error.consecutive_errors, 1)

    def test_cli_recovery_sample_preserves_ok_reset(self) -> None:
        report = _sample_recovery(
            "sensor",
            "sensor_read_fail",
            events=3,
            notify_after=2,
            reboot_after=3,
            ok_after=1,
        )

        self.assertEqual(report["event_count"], 4)
        self.assertEqual(report["timeline"][0]["action"], "ignore")
        self.assertEqual(report["timeline"][1]["event"], "ok")
        self.assertEqual(report["timeline"][2]["action"], "ignore")
        self.assertEqual(report["timeline"][3]["action"], "notify_user_task")

    def test_watchdog_simulator_reports_expired_modules(self) -> None:
        watchdog = WatchdogSimulator(capacity=2)
        watchdog.register("sensor", timeout_us=100, now_us=0)
        watchdog.register("radio", timeout_us=500, now_us=0)

        expired = watchdog.expired(150)

        self.assertEqual([entry.module for entry in expired], ["sensor"])
        self.assertEqual(watchdog.get("sensor").missed, 1)
        self.assertEqual(watchdog.get("radio").missed, 0)

    def test_watchdog_simulator_heartbeat_resets_missed_count(self) -> None:
        watchdog = WatchdogSimulator(capacity=1)
        watchdog.register("bus", timeout_us=100, now_us=0)

        self.assertEqual(len(watchdog.expired(150)), 1)
        watchdog.beat("bus", 160)

        entry = watchdog.get("bus")
        self.assertEqual(entry.missed, 0)
        self.assertEqual(entry.last_beat_us, 160)
        self.assertEqual(watchdog.expired(200), [])

    def test_watchdog_simulator_expired_count_does_not_mutate(self) -> None:
        watchdog = WatchdogSimulator(capacity=2)
        watchdog.register("sensor", timeout_us=100, now_us=0)
        watchdog.register("radio", timeout_us=200, now_us=0)

        self.assertEqual(watchdog.expired_count(150), 1)
        self.assertEqual(watchdog.get("sensor").missed, 0)
        self.assertEqual(watchdog.get("sensor").overdue_us(150), 50)
        self.assertTrue(watchdog.get("sensor").is_expired(150))

        self.assertEqual(len(watchdog.expired(150)), 1)
        self.assertEqual(watchdog.get("sensor").missed, 1)

    def test_cli_watchdog_sample_summarizes_heartbeat_timeline(self) -> None:
        report = _sample_watchdog(
            "sensor",
            timeout_us=100,
            sweeps=3,
            step_us=75,
            beat_at_sweep=2,
        )

        self.assertEqual(report["event_count"], 5)
        self.assertEqual(report["timeline"][0]["event"], "register")
        self.assertEqual(report["timeline"][1]["expired_count"], 0)
        self.assertEqual(report["timeline"][2]["event"], "beat")
        self.assertEqual(report["timeline"][3]["expired_count"], 0)
        self.assertEqual(report["timeline"][4]["expired_count"], 0)

    def test_scheduler_simulator_tracks_configurable_jitter(self) -> None:
        scheduler = SchedulerSimulator(jitter_tolerance_us=25)
        scheduler.on_deadline_tick(1_000)
        scheduler.on_deadline_tick(21_020)
        scheduler.on_deadline_tick(41_050)

        stats = scheduler.stats()
        self.assertEqual(stats.tick_count, 3)
        self.assertEqual(stats.max_jitter_us, 30)
        self.assertEqual(stats.deadline_misses, 1)
        self.assertEqual(stats.jitter_tolerance_us, 25)

    def test_scheduler_simulator_handles_u32_wraparound(self) -> None:
        scheduler = SchedulerSimulator()
        first = 0xFFFF_FFFF - 5
        scheduler.on_deadline_tick(first)
        scheduler.on_deadline_tick(first + 20_000 + 3)

        self.assertEqual(scheduler.max_jitter_us, 3)

    def test_cli_scheduler_sample_summarizes_ticks(self) -> None:
        report = _sample_scheduler(
            (1_000, 21_020, 41_050),
            period_us=20_000,
            tolerance_us=25,
        )

        self.assertEqual(report["tick_count"], 3)
        self.assertEqual(report["max_jitter_us"], 30)
        self.assertEqual(report["deadline_misses"], 1)
        self.assertEqual(report["timeline"][2]["deadline_misses"], 1)

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
        self.assertEqual(summary.diagnostic_code(), 0x0101_0000)
        self.assertEqual(summary.status_counts()["missing"], 5)
        payload = summary.to_dict()
        self.assertEqual(payload["diagnostic_code"], 0x0101_0000)
        self.assertEqual(payload["missing_count"], 5)
        self.assertEqual(payload["observed_count"], 6)

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
        self.assertEqual(summary.diagnostic_code(), 0x0404_0003)
        self.assertEqual(
            summary.first_diagnostic.error_label, "capability_ownership_conflict"
        )
        payload = summary.to_dict()
        self.assertEqual(payload["fail_count"], 1)
        self.assertEqual(payload["pass_count"], 3)
        self.assertEqual(payload["diagnostic"]["symbol"], "NOBRO_ADAPTER_COMPAT_REPORT")


if __name__ == "__main__":
    unittest.main()
