//! Host-side contract constants shared by scripts, tools, and documentation.

#![no_std]

pub const MAINTENANCE_CDC_MI: &str = "MI_00";
pub const USER_CDC_MI: &str = "MI_02";
pub const UPLOAD_TOUCH_BAUD: u32 = 1200;

pub const APP_START_NO_SOFTDEVICE: u32 = 0x1000;
pub const APP_START_S140_V6: u32 = 0x26000;

pub const PHASE1_EVAL_SYMBOL: &str = "NOBRO_EVAL_REPORT";
pub const PHASE1_EVAL_MAGIC: u32 = 0x4E42_4E31;
pub const PHASE2_EVAL_SYMBOL: &str = "NOBRO_SAL_EVAL_REPORT";
pub const PHASE2_EVAL_MAGIC: u32 = 0x4E42_4E32;
pub const HEALTH_REPORT_SYMBOL: &str = "NOBRO_HEALTH_REPORT";
pub const HEALTH_REPORT_MAGIC: u32 = 0x4E42_484C;
pub const EVENT_LOG_REPORT_SYMBOL: &str = "NOBRO_EVENT_LOG_REPORT";
pub const EVENT_LOG_REPORT_MAGIC: u32 = 0x4E42_454C;
pub const EVENT_LOG_REPORT_VERSION: u32 = 1;
pub const MODULE_RUNTIME_REPORT_SYMBOL: &str = "NOBRO_MODULE_RUNTIME_REPORT";
pub const MODULE_RUNTIME_REPORT_MAGIC: u32 = 0x4E42_4D52;
pub const MODULE_RUNTIME_REPORT_VERSION: u32 = 1;
pub const DEGRADE_APPLICATION_REPORT_SYMBOL: &str = "NOBRO_DEGRADE_APPLICATION_REPORT";
pub const DEGRADE_APPLICATION_REPORT_MAGIC: u32 = 0x4E42_4447;
pub const DEGRADE_APPLICATION_REPORT_VERSION: u32 = 1;
pub const RUNTIME_REPORT_SYMBOL: &str = "NOBRO_RUNTIME_REPORT";
pub const RUNTIME_REPORT_MAGIC: u32 = 0x4E42_5254;
pub const RUNTIME_REPORT_VERSION: u32 = 1;
pub const BOARD_PROFILE_REPORT_SYMBOL: &str = "NOBRO_BOARD_PROFILE_REPORT";
pub const BOARD_PROFILE_REPORT_MAGIC: u32 = 0x4E42_4250;
pub const BOARD_PROFILE_REPORT_VERSION: u32 = 1;
pub const BOARD_PACKAGE_REPORT_SYMBOL: &str = "NOBRO_BOARD_PACKAGE_REPORT";
pub const BOARD_PACKAGE_REPORT_MAGIC: u32 = 0x4E42_424B;
pub const BOARD_PACKAGE_REPORT_VERSION: u32 = 1;
pub const MANIFEST_REPORT_SYMBOL: &str = "NOBRO_MANIFEST_REPORT";
pub const MANIFEST_REPORT_MAGIC: u32 = 0x4E42_4D46;
pub const MANIFEST_REPORT_VERSION: u32 = 1;
pub const ADAPTER_COMPAT_REPORT_SYMBOL: &str = "NOBRO_ADAPTER_COMPAT_REPORT";
pub const ADAPTER_COMPAT_REPORT_MAGIC: u32 = 0x4E42_4143;
pub const ADAPTER_COMPAT_REPORT_VERSION: u32 = 1;
pub const AI_MODEL_REPORT_SYMBOL: &str = "NOBRO_AI_MODEL_REPORT";
pub const AI_MODEL_REPORT_MAGIC: u32 = 0x4E42_4149;
pub const AI_MODEL_REPORT_VERSION: u32 = 1;
pub const ROS_BRIDGE_REPORT_SYMBOL: &str = "NOBRO_ROS_BRIDGE_REPORT";
pub const ROS_BRIDGE_REPORT_MAGIC: u32 = 0x4E42_5253;
pub const ROS_BRIDGE_REPORT_VERSION: u32 = 1;
pub const ADMISSION_REPORT_SYMBOL: &str = "NOBRO_ADMISSION_REPORT";
pub const ADMISSION_REPORT_MAGIC: u32 = 0x4E42_4144;
pub const ADMISSION_REPORT_VERSION: u32 = 1;

pub const BOOT_REPORT_STAGE_COUNT: usize = 6;

pub const MAX_PHASE1_JITTER_US: u32 = 10;
pub const MIN_PHASE1_DEADLINE_TICKS: u32 = 150;
pub const MIN_PHASE1_I2C_READS: u32 = 10;
pub const MAX_PHASE1_RADIO_LATENCY_US: u32 = 10;
pub const MIN_PHASE1_RADIO_SAMPLES: u32 = 16;

pub const MIN_PHASE2_SERVO_STEPS: u32 = 20;
pub const MIN_PHASE2_IMU_SAMPLES: u32 = 3;
pub const PHASE2_SERVO_READBACK_TOL_US: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootLayout {
    NoSoftDevice,
    SoftDeviceS140V6,
    Custom,
}

impl BootLayout {
    pub const fn app_start(self) -> u32 {
        match self {
            Self::NoSoftDevice => APP_START_NO_SOFTDEVICE,
            Self::SoftDeviceS140V6 => APP_START_S140_V6,
            Self::Custom => 0,
        }
    }

    pub const fn cargo_feature(self) -> &'static str {
        match self {
            Self::NoSoftDevice => "board-promicro-nosd",
            Self::SoftDeviceS140V6 => "board-nicenano-s140",
            Self::Custom => "custom-board",
        }
    }

    pub const fn code(self) -> u32 {
        match self {
            Self::NoSoftDevice => 1,
            Self::SoftDeviceS140V6 => 2,
            Self::Custom => 255,
        }
    }
}

pub struct HostContract;

impl HostContract {
    pub const fn maintenance_cdc_mi() -> &'static str {
        MAINTENANCE_CDC_MI
    }

    pub const fn user_cdc_mi() -> &'static str {
        USER_CDC_MI
    }

    pub const fn upload_touch_baud() -> u32 {
        UPLOAD_TOUCH_BAUD
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportStatus {
    Missing,
    InProgress,
    Pass,
    Fail(u32),
    Corrupt,
}

impl ReportStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::InProgress => "in_progress",
            Self::Pass => "pass",
            Self::Fail(_) => "fail",
            Self::Corrupt => "corrupt",
        }
    }

    pub const fn code(self) -> u32 {
        match self {
            Self::Pass => 0,
            Self::Missing => 1,
            Self::InProgress => 2,
            Self::Corrupt => 3,
            Self::Fail(code) => 0x8000_0000 | code,
        }
    }

    pub const fn class_code(self) -> u32 {
        match self {
            Self::Pass => 0,
            Self::Missing => 1,
            Self::InProgress => 2,
            Self::Corrupt => 3,
            Self::Fail(_) => 4,
        }
    }

    pub const fn is_pass(self) -> bool {
        matches!(self, Self::Pass)
    }

    pub const fn error_code(self) -> Option<u32> {
        match self {
            Self::Fail(code) => Some(code),
            Self::Missing | Self::InProgress | Self::Pass | Self::Corrupt => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootStage {
    BoardProfile,
    BoardPackage,
    Manifest,
    AdapterCompatibility,
    Admission,
    Runtime,
}

impl BootStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::BoardProfile => "board_profile",
            Self::BoardPackage => "board_package",
            Self::Manifest => "manifest",
            Self::AdapterCompatibility => "adapter_compatibility",
            Self::Admission => "admission",
            Self::Runtime => "runtime",
        }
    }

    pub const fn code(self) -> u32 {
        match self {
            Self::BoardProfile => 1,
            Self::BoardPackage => 2,
            Self::Manifest => 3,
            Self::AdapterCompatibility => 4,
            Self::Admission => 5,
            Self::Runtime => 6,
        }
    }

    pub const fn symbol(self) -> &'static str {
        match self {
            Self::BoardProfile => BOARD_PROFILE_REPORT_SYMBOL,
            Self::BoardPackage => BOARD_PACKAGE_REPORT_SYMBOL,
            Self::Manifest => MANIFEST_REPORT_SYMBOL,
            Self::AdapterCompatibility => ADAPTER_COMPAT_REPORT_SYMBOL,
            Self::Admission => ADMISSION_REPORT_SYMBOL,
            Self::Runtime => RUNTIME_REPORT_SYMBOL,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootDiagnostic {
    pub stage: BootStage,
    pub status: ReportStatus,
}

impl BootDiagnostic {
    pub const fn is_passing(self) -> bool {
        self.status.is_pass()
    }

    pub const fn stage_label(self) -> &'static str {
        self.stage.label()
    }

    pub const fn status_label(self) -> &'static str {
        self.status.label()
    }

    pub const fn stage_symbol(self) -> &'static str {
        self.stage.symbol()
    }

    pub const fn error_code(self) -> Option<u32> {
        self.status.error_code()
    }

    pub const fn error_label(self) -> Option<&'static str> {
        let Some(code) = self.error_code() else {
            return None;
        };
        match self.stage {
            BootStage::BoardProfile | BootStage::Runtime => None,
            BootStage::BoardPackage => board_package_error_label(code),
            BootStage::Manifest => manifest_error_label(code),
            BootStage::AdapterCompatibility => adapter_compat_error_label(code),
            BootStage::Admission => admission_error_label(code),
        }
    }

    pub const fn code(self) -> u32 {
        let error = match self.status.error_code() {
            Some(code) => code & 0xFFFF,
            None => 0,
        };
        (self.stage.code() << 24) | (self.status.class_code() << 16) | error
    }
}

pub const fn board_package_error_label(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("empty_platform_id"),
        2 => Some("empty_board_id"),
        3 => Some("unaligned_flash_origin"),
        4 => Some("empty_flash_region"),
        5 => Some("empty_ram_region"),
        6 => Some("empty_capacity"),
        7 => Some("duplicate_critical_pin"),
        _ => None,
    }
}

pub const fn manifest_error_label(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("full"),
        2 => Some("duplicate_module"),
        3 => Some("capability_ownership_conflict"),
        4 => Some("missing_owned_capability"),
        5 => Some("missing_deadline"),
        6 => Some("invalid_deadline"),
        7 => Some("invalid_fault_threshold"),
        8 => Some("empty_memory_budget"),
        9 => Some("module_limit_exceeded"),
        10 => Some("budget_exceeded"),
        11 => Some("user_owns_kernel_capability"),
        12 => Some("empty_manifest"),
        13 => Some("missing_kernel"),
        14 => Some("invalid_kernel_contract"),
        15 => Some("overutilized"),
        _ => None,
    }
}

pub const fn adapter_compat_error_label(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("full"),
        2 => Some("duplicate_module"),
        3 => Some("capability_ownership_conflict"),
        4 => Some("module_limit_exceeded"),
        5 => Some("budget_exceeded"),
        _ => None,
    }
}

pub const fn admission_error_label(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("manifest"),
        2 => Some("startup"),
        3 => Some("quota"),
        4 => Some("capability"),
        5 => Some("missing_startup_node"),
        6 => Some("unknown_startup_node"),
        _ => None,
    }
}

pub const fn module_tag_label(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("kernel"),
        2 => Some("hal"),
        3 => Some("bus"),
        4 => Some("radio"),
        5 => Some("sensor"),
        6 => Some("actuator"),
        7 => Some("stream"),
        8 => Some("crypto"),
        9 => Some("ai"),
        0x100..=0x1FF => Some("app"),
        _ => None,
    }
}

pub const fn app_module_tag_index(code: u32) -> Option<u8> {
    match code {
        0x100..=0x1FF => Some((code - 0x100) as u8),
        _ => None,
    }
}

pub const fn capability_bit_label(bit: u32) -> Option<&'static str> {
    match bit {
        0 => Some("timebase"),
        1 => Some("deadline_timer"),
        2 => Some("event_capture"),
        3 => Some("bus0"),
        4 => Some("bus1"),
        5 => Some("radio"),
        6 => Some("servo_pwm"),
        7 => Some("stream"),
        8 => Some("crypto"),
        9 => Some("sample_pool"),
        10 => Some("host_report"),
        11 => Some("ai_inference"),
        12 => Some("ai_endpoint"),
        13 => Some("mailbox"),
        14 => Some("alarm"),
        15 => Some("kv_store"),
        _ => None,
    }
}

pub const fn capability_mask_label(mask: u32) -> Option<&'static str> {
    match mask {
        0x0000_0001 => Some("timebase"),
        0x0000_0002 => Some("deadline_timer"),
        0x0000_0004 => Some("event_capture"),
        0x0000_0008 => Some("bus0"),
        0x0000_0010 => Some("bus1"),
        0x0000_0020 => Some("radio"),
        0x0000_0040 => Some("servo_pwm"),
        0x0000_0080 => Some("stream"),
        0x0000_0100 => Some("crypto"),
        0x0000_0200 => Some("sample_pool"),
        0x0000_0400 => Some("host_report"),
        0x0000_0800 => Some("ai_inference"),
        0x0000_1000 => Some("ai_endpoint"),
        0x0000_2000 => Some("mailbox"),
        0x0000_4000 => Some("alarm"),
        0x0000_8000 => Some("kv_store"),
        _ => None,
    }
}

pub const fn runtime_state_label(code: u32) -> Option<&'static str> {
    match code {
        0 => Some("cold_boot"),
        1 => Some("validate_manifest"),
        2 => Some("init_drivers"),
        3 => Some("running"),
        4 => Some("degraded"),
        5 => Some("recovering"),
        6 => Some("halted"),
        _ => None,
    }
}

pub const fn event_severity_label(code: u32) -> Option<&'static str> {
    match code {
        0 => Some("trace"),
        1 => Some("info"),
        2 => Some("warn"),
        3 => Some("error"),
        4 => Some("fatal"),
        _ => None,
    }
}

pub const fn event_kind_label(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("boot"),
        2 => Some("health"),
        3 => Some("recovery"),
        4 => Some("task_overrun"),
        5 => Some("lease"),
        6 => Some("sample_pool"),
        7 => Some("manifest"),
        8 => Some("host"),
        _ => None,
    }
}

pub const fn event_payload_kind_label(code: u32) -> Option<&'static str> {
    match code {
        0 => Some("none"),
        1 => Some("error"),
        2 => Some("action"),
        3 => Some("counter"),
        4 => Some("pair"),
        _ => None,
    }
}

pub const fn module_runtime_state_label(code: u32) -> Option<&'static str> {
    match code {
        1 => Some("registered"),
        2 => Some("active"),
        3 => Some("suspended"),
        4 => Some("faulted"),
        5 => Some("recovering"),
        6 => Some("disabled"),
        _ => None,
    }
}

pub const fn degrade_reason_label(code: u32) -> Option<&'static str> {
    match code {
        0 => Some("none"),
        1 => Some("flash_budget"),
        2 => Some("ram_budget"),
        3 => Some("pool_budget"),
        4 => Some("module_limit"),
        _ => None,
    }
}

pub trait HostReport {
    const SYMBOL: &'static str;
    const MAGIC: u32;
    const VERSION: u32;

    fn raw_magic(&self) -> u32;
    fn raw_version(&self) -> u32;
    fn completed(&self) -> u32;
    fn checksum(&self) -> u32;
    fn verify_checksum(&self) -> bool;
    fn status(&self) -> ReportStatus;

    fn is_missing(&self) -> bool {
        self.raw_magic() == 0 && self.raw_version() == 0 && self.checksum() == 0
    }

    fn has_expected_header(&self) -> bool {
        self.raw_magic() == Self::MAGIC && self.raw_version() == Self::VERSION
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportSlot {
    pub stage: BootStage,
    pub symbol: &'static str,
    pub status: ReportStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootSummary {
    pub diagnostic: BootDiagnostic,
    pub slots: [ReportSlot; BOOT_REPORT_STAGE_COUNT],
    pub pass_count: u8,
    pub missing_count: u8,
    pub in_progress_count: u8,
    pub fail_count: u8,
    pub corrupt_count: u8,
}

impl BootSummary {
    pub const fn is_passing(self) -> bool {
        self.diagnostic.is_passing()
    }

    pub const fn diagnostic_code(self) -> u32 {
        self.diagnostic.code()
    }

    pub const fn first_stage_label(self) -> &'static str {
        self.diagnostic.stage_label()
    }

    pub const fn first_status_label(self) -> &'static str {
        self.diagnostic.status_label()
    }

    pub const fn first_symbol(self) -> &'static str {
        self.diagnostic.stage_symbol()
    }

    pub const fn first_error_label(self) -> Option<&'static str> {
        self.diagnostic.error_label()
    }

    pub const fn observed_count(self) -> u8 {
        self.pass_count
            + self.missing_count
            + self.fail_count
            + self.corrupt_count
            + self.in_progress_count
    }
}

macro_rules! impl_host_report {
    ($report:ty, $symbol:expr, $magic:expr, $version:expr) => {
        impl HostReport for $report {
            const SYMBOL: &'static str = $symbol;
            const MAGIC: u32 = $magic;
            const VERSION: u32 = $version;

            fn raw_magic(&self) -> u32 {
                self.magic
            }

            fn raw_version(&self) -> u32 {
                self.version
            }

            fn completed(&self) -> u32 {
                self.completed
            }

            fn checksum(&self) -> u32 {
                self.checksum
            }

            fn verify_checksum(&self) -> bool {
                <$report>::verify_checksum(self)
            }

            fn status(&self) -> ReportStatus {
                <$report>::status(self)
            }
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootReports {
    pub board_profile: BoardProfileReport,
    pub board_package: BoardPackageReport,
    pub manifest: ManifestReport,
    pub adapter_compatibility: AdapterCompatibilityReport,
    pub admission: AdmissionReport,
    pub runtime: RuntimeReport,
}

impl BootReports {
    pub const fn new(
        board_profile: BoardProfileReport,
        board_package: BoardPackageReport,
        manifest: ManifestReport,
        adapter_compatibility: AdapterCompatibilityReport,
        admission: AdmissionReport,
        runtime: RuntimeReport,
    ) -> Self {
        Self {
            board_profile,
            board_package,
            manifest,
            adapter_compatibility,
            admission,
            runtime,
        }
    }

    pub fn diagnostic(&self) -> BootDiagnostic {
        let board = self.board_profile.status();
        if board != ReportStatus::Pass {
            return BootDiagnostic {
                stage: BootStage::BoardProfile,
                status: board,
            };
        }

        let package = self.board_package.status();
        if package != ReportStatus::Pass {
            return BootDiagnostic {
                stage: BootStage::BoardPackage,
                status: package,
            };
        }

        let manifest = self.manifest.status();
        if manifest != ReportStatus::Pass {
            return BootDiagnostic {
                stage: BootStage::Manifest,
                status: manifest,
            };
        }

        let adapter = self.adapter_compatibility.status();
        if adapter != ReportStatus::Pass {
            return BootDiagnostic {
                stage: BootStage::AdapterCompatibility,
                status: adapter,
            };
        }

        let admission = self.admission.status();
        if admission != ReportStatus::Pass {
            return BootDiagnostic {
                stage: BootStage::Admission,
                status: admission,
            };
        }

        BootDiagnostic {
            stage: BootStage::Runtime,
            status: self.runtime.status(),
        }
    }

    pub fn slots(&self) -> [ReportSlot; BOOT_REPORT_STAGE_COUNT] {
        [
            ReportSlot {
                stage: BootStage::BoardProfile,
                symbol: <BoardProfileReport as HostReport>::SYMBOL,
                status: self.board_profile.status(),
            },
            ReportSlot {
                stage: BootStage::BoardPackage,
                symbol: <BoardPackageReport as HostReport>::SYMBOL,
                status: self.board_package.status(),
            },
            ReportSlot {
                stage: BootStage::Manifest,
                symbol: <ManifestReport as HostReport>::SYMBOL,
                status: self.manifest.status(),
            },
            ReportSlot {
                stage: BootStage::AdapterCompatibility,
                symbol: <AdapterCompatibilityReport as HostReport>::SYMBOL,
                status: self.adapter_compatibility.status(),
            },
            ReportSlot {
                stage: BootStage::Admission,
                symbol: <AdmissionReport as HostReport>::SYMBOL,
                status: self.admission.status(),
            },
            ReportSlot {
                stage: BootStage::Runtime,
                symbol: <RuntimeReport as HostReport>::SYMBOL,
                status: self.runtime.status(),
            },
        ]
    }

    pub fn summary(&self) -> BootSummary {
        let slots = self.slots();
        let mut pass_count = 0;
        let mut missing_count = 0;
        let mut in_progress_count = 0;
        let mut fail_count = 0;
        let mut corrupt_count = 0;

        for slot in slots {
            match slot.status {
                ReportStatus::Pass => pass_count += 1,
                ReportStatus::Missing => missing_count += 1,
                ReportStatus::InProgress => in_progress_count += 1,
                ReportStatus::Fail(_) => fail_count += 1,
                ReportStatus::Corrupt => corrupt_count += 1,
            }
        }

        BootSummary {
            diagnostic: self.diagnostic(),
            slots,
            pass_count,
            missing_count,
            in_progress_count,
            fail_count,
            corrupt_count,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardProfileReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub platform_hash: u32,
    pub board_hash: u32,
    pub app_flash_start: u32,
    pub flash_budget_bytes: u32,
    pub ram_budget_bytes: u32,
    pub sample_pool_slots: u32,
    pub max_modules: u32,
    pub servo_pin: u32,
    pub servo_center_us: u32,
    pub led_pin: u32,
    pub mvk_trigger_pin: u32,
    pub checksum: u32,
}

impl BoardProfileReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            platform_hash: 0,
            board_hash: 0,
            app_flash_start: 0,
            flash_budget_bytes: 0,
            ram_budget_bytes: 0,
            sample_pool_slots: 0,
            max_modules: 0,
            servo_pin: 0,
            servo_center_us: 0,
            led_pin: 0,
            mvk_trigger_pin: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = BOARD_PROFILE_REPORT_MAGIC;
        self.version = BOARD_PROFILE_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == BOARD_PROFILE_REPORT_MAGIC
            && self.version == BOARD_PROFILE_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != BOARD_PROFILE_REPORT_MAGIC || self.version != BOARD_PROFILE_REPORT_VERSION
        {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if self.verify_checksum() {
            ReportStatus::Pass
        } else {
            ReportStatus::Corrupt
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.platform_hash
            ^ self.board_hash
            ^ self.app_flash_start
            ^ self.flash_budget_bytes
            ^ self.ram_budget_bytes
            ^ self.sample_pool_slots
            ^ self.max_modules
            ^ self.servo_pin
            ^ self.servo_center_us
            ^ self.led_pin
            ^ self.mvk_trigger_pin
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardPackageReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub valid: u32,
    pub platform_hash: u32,
    pub board_hash: u32,
    pub boot_layout: u32,
    pub app_flash_start: u32,
    pub app_flash_len_bytes: u32,
    pub ram_start: u32,
    pub ram_len_bytes: u32,
    pub flash_budget_bytes: u32,
    pub ram_budget_bytes: u32,
    pub sample_pool_slots: u32,
    pub max_modules: u32,
    pub led_pin: u32,
    pub servo_pin: u32,
    pub mvk_trigger_pin: u32,
    pub error_code: u32,
    pub checksum: u32,
}

impl BoardPackageReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            valid: 0,
            platform_hash: 0,
            board_hash: 0,
            boot_layout: 0,
            app_flash_start: 0,
            app_flash_len_bytes: 0,
            ram_start: 0,
            ram_len_bytes: 0,
            flash_budget_bytes: 0,
            ram_budget_bytes: 0,
            sample_pool_slots: 0,
            max_modules: 0,
            led_pin: 0,
            servo_pin: 0,
            mvk_trigger_pin: 0,
            error_code: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = BOARD_PACKAGE_REPORT_MAGIC;
        self.version = BOARD_PACKAGE_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == BOARD_PACKAGE_REPORT_MAGIC
            && self.version == BOARD_PACKAGE_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != BOARD_PACKAGE_REPORT_MAGIC || self.version != BOARD_PACKAGE_REPORT_VERSION
        {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if !self.verify_checksum() {
            return ReportStatus::Corrupt;
        }
        if self.valid != 0 {
            ReportStatus::Pass
        } else {
            ReportStatus::Fail(self.error_code)
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.valid
            ^ self.platform_hash
            ^ self.board_hash
            ^ self.boot_layout
            ^ self.app_flash_start
            ^ self.app_flash_len_bytes
            ^ self.ram_start
            ^ self.ram_len_bytes
            ^ self.flash_budget_bytes
            ^ self.ram_budget_bytes
            ^ self.sample_pool_slots
            ^ self.max_modules
            ^ self.led_pin
            ^ self.servo_pin
            ^ self.mvk_trigger_pin
            ^ self.error_code
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManifestReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub valid: u32,
    pub module_count: u32,
    pub fingerprint: u32,
    pub required_bits: u32,
    pub owned_bits: u32,
    pub flash_used_bytes: u32,
    pub ram_used_bytes: u32,
    pub pool_used_slots: u32,
    pub error_code: u32,
    pub error_module_tag: u32,
    pub error_capability_bits: u32,
    pub checksum: u32,
}

impl ManifestReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            valid: 0,
            module_count: 0,
            fingerprint: 0,
            required_bits: 0,
            owned_bits: 0,
            flash_used_bytes: 0,
            ram_used_bytes: 0,
            pool_used_slots: 0,
            error_code: 0,
            error_module_tag: 0,
            error_capability_bits: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = MANIFEST_REPORT_MAGIC;
        self.version = MANIFEST_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == MANIFEST_REPORT_MAGIC
            && self.version == MANIFEST_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != MANIFEST_REPORT_MAGIC || self.version != MANIFEST_REPORT_VERSION {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if !self.verify_checksum() {
            return ReportStatus::Corrupt;
        }
        if self.valid != 0 {
            ReportStatus::Pass
        } else {
            ReportStatus::Fail(self.error_code)
        }
    }

    pub const fn error_module_label(&self) -> Option<&'static str> {
        module_tag_label(self.error_module_tag)
    }

    pub const fn error_capability_label(&self) -> Option<&'static str> {
        capability_mask_label(self.error_capability_bits)
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.valid
            ^ self.module_count
            ^ self.fingerprint
            ^ self.required_bits
            ^ self.owned_bits
            ^ self.flash_used_bytes
            ^ self.ram_used_bytes
            ^ self.pool_used_slots
            ^ self.error_code
            ^ self.error_module_tag
            ^ self.error_capability_bits
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdapterCompatibilityReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub compatible: u32,
    pub adapter_count: u32,
    pub required_bits: u32,
    pub owned_bits: u32,
    pub flash_used_bytes: u32,
    pub ram_used_bytes: u32,
    pub pool_used_slots: u32,
    pub error_code: u32,
    pub error_module_tag: u32,
    pub error_capability_bits: u32,
    pub checksum: u32,
}

impl AdapterCompatibilityReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            compatible: 0,
            adapter_count: 0,
            required_bits: 0,
            owned_bits: 0,
            flash_used_bytes: 0,
            ram_used_bytes: 0,
            pool_used_slots: 0,
            error_code: 0,
            error_module_tag: 0,
            error_capability_bits: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = ADAPTER_COMPAT_REPORT_MAGIC;
        self.version = ADAPTER_COMPAT_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == ADAPTER_COMPAT_REPORT_MAGIC
            && self.version == ADAPTER_COMPAT_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != ADAPTER_COMPAT_REPORT_MAGIC
            || self.version != ADAPTER_COMPAT_REPORT_VERSION
        {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if !self.verify_checksum() {
            return ReportStatus::Corrupt;
        }
        if self.compatible != 0 {
            ReportStatus::Pass
        } else {
            ReportStatus::Fail(self.error_code)
        }
    }

    pub const fn error_module_label(&self) -> Option<&'static str> {
        module_tag_label(self.error_module_tag)
    }

    pub const fn error_capability_label(&self) -> Option<&'static str> {
        capability_mask_label(self.error_capability_bits)
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.compatible
            ^ self.adapter_count
            ^ self.required_bits
            ^ self.owned_bits
            ^ self.flash_used_bytes
            ^ self.ram_used_bytes
            ^ self.pool_used_slots
            ^ self.error_code
            ^ self.error_module_tag
            ^ self.error_capability_bits
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AiModelReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub backend: u32,
    pub model_id: u32,
    pub input_bytes_max: u32,
    pub output_bytes_max: u32,
    pub arena_bytes: u32,
    pub timeout_us: u32,
    pub route_preference: u32,
    pub stale_after_us: u32,
    pub endpoint_failure_limit: u32,
    pub checksum: u32,
}

impl AiModelReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            backend: 0,
            model_id: 0,
            input_bytes_max: 0,
            output_bytes_max: 0,
            arena_bytes: 0,
            timeout_us: 0,
            route_preference: 0,
            stale_after_us: 0,
            endpoint_failure_limit: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = AI_MODEL_REPORT_MAGIC;
        self.version = AI_MODEL_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == AI_MODEL_REPORT_MAGIC
            && self.version == AI_MODEL_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != AI_MODEL_REPORT_MAGIC || self.version != AI_MODEL_REPORT_VERSION {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if !self.verify_checksum() {
            return ReportStatus::Corrupt;
        }
        ReportStatus::Pass
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.backend
            ^ self.model_id
            ^ self.input_bytes_max
            ^ self.output_bytes_max
            ^ self.arena_bytes
            ^ self.timeout_us
            ^ self.route_preference
            ^ self.stale_after_us
            ^ self.endpoint_failure_limit
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RosBridgeReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub transport: u32,
    pub bridge_id_hash: u32,
    pub topic_count: u32,
    pub service_count: u32,
    pub action_count: u32,
    pub parameter_count: u32,
    pub total_buffer_bytes: u32,
    pub max_timeout_us: u32,
    pub checksum: u32,
}

impl RosBridgeReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            transport: 0,
            bridge_id_hash: 0,
            topic_count: 0,
            service_count: 0,
            action_count: 0,
            parameter_count: 0,
            total_buffer_bytes: 0,
            max_timeout_us: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = ROS_BRIDGE_REPORT_MAGIC;
        self.version = ROS_BRIDGE_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == ROS_BRIDGE_REPORT_MAGIC
            && self.version == ROS_BRIDGE_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != ROS_BRIDGE_REPORT_MAGIC || self.version != ROS_BRIDGE_REPORT_VERSION {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if !self.verify_checksum() {
            return ReportStatus::Corrupt;
        }
        ReportStatus::Pass
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.transport
            ^ self.bridge_id_hash
            ^ self.topic_count
            ^ self.service_count
            ^ self.action_count
            ^ self.parameter_count
            ^ self.total_buffer_bytes
            ^ self.max_timeout_us
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdmissionReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub admitted: u32,
    pub module_count: u32,
    pub startup_len: u32,
    pub flash_used_bytes: u32,
    pub flash_limit_bytes: u32,
    pub ram_used_bytes: u32,
    pub ram_limit_bytes: u32,
    pub pool_used_slots: u32,
    pub pool_limit_slots: u32,
    pub error_code: u32,
    pub checksum: u32,
}

impl AdmissionReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            admitted: 0,
            module_count: 0,
            startup_len: 0,
            flash_used_bytes: 0,
            flash_limit_bytes: 0,
            ram_used_bytes: 0,
            ram_limit_bytes: 0,
            pool_used_slots: 0,
            pool_limit_slots: 0,
            error_code: 0,
            checksum: 0,
        }
    }

    pub fn seal(&mut self) {
        self.magic = ADMISSION_REPORT_MAGIC;
        self.version = ADMISSION_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == ADMISSION_REPORT_MAGIC
            && self.version == ADMISSION_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != ADMISSION_REPORT_MAGIC || self.version != ADMISSION_REPORT_VERSION {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if !self.verify_checksum() {
            return ReportStatus::Corrupt;
        }
        if self.admitted != 0 {
            ReportStatus::Pass
        } else {
            ReportStatus::Fail(self.error_code)
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.admitted
            ^ self.module_count
            ^ self.startup_len
            ^ self.flash_used_bytes
            ^ self.flash_limit_bytes
            ^ self.ram_used_bytes
            ^ self.ram_limit_bytes
            ^ self.pool_used_slots
            ^ self.pool_limit_slots
            ^ self.error_code
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub state: u32,
    pub module_count: u32,
    pub mailbox_len: u32,
    pub mailbox_dropped: u32,
    pub alarm_len: u32,
    pub next_alarm_due_us_lo: u32,
    pub next_alarm_due_us_hi: u32,
    pub kv_len: u32,
    pub kv_writes: u32,
    pub kv_deletes: u32,
    pub quota_flash_used_bytes: u32,
    pub quota_ram_used_bytes: u32,
    pub quota_pool_used_slots: u32,
    pub event_count: u32,
    pub dropped_events: u32,
    pub checksum: u32,
}

impl RuntimeReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            state: 0,
            module_count: 0,
            mailbox_len: 0,
            mailbox_dropped: 0,
            alarm_len: 0,
            next_alarm_due_us_lo: 0,
            next_alarm_due_us_hi: 0,
            kv_len: 0,
            kv_writes: 0,
            kv_deletes: 0,
            quota_flash_used_bytes: 0,
            quota_ram_used_bytes: 0,
            quota_pool_used_slots: 0,
            event_count: 0,
            dropped_events: 0,
            checksum: 0,
        }
    }

    pub fn set_next_alarm_due_us(&mut self, due_us: u64) {
        self.next_alarm_due_us_lo = due_us as u32;
        self.next_alarm_due_us_hi = (due_us >> 32) as u32;
    }

    pub fn next_alarm_due_us(&self) -> u64 {
        (u64::from(self.next_alarm_due_us_hi) << 32) | u64::from(self.next_alarm_due_us_lo)
    }

    pub const fn state_label(&self) -> Option<&'static str> {
        runtime_state_label(self.state)
    }

    pub fn seal(&mut self) {
        self.magic = RUNTIME_REPORT_MAGIC;
        self.version = RUNTIME_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == RUNTIME_REPORT_MAGIC
            && self.version == RUNTIME_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != RUNTIME_REPORT_MAGIC || self.version != RUNTIME_REPORT_VERSION {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if self.verify_checksum() {
            ReportStatus::Pass
        } else {
            ReportStatus::Corrupt
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.state
            ^ self.module_count
            ^ self.mailbox_len
            ^ self.mailbox_dropped
            ^ self.alarm_len
            ^ self.next_alarm_due_us_lo
            ^ self.next_alarm_due_us_hi
            ^ self.kv_len
            ^ self.kv_writes
            ^ self.kv_deletes
            ^ self.quota_flash_used_bytes
            ^ self.quota_ram_used_bytes
            ^ self.quota_pool_used_slots
            ^ self.event_count
            ^ self.dropped_events
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventLogReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub event_count: u32,
    pub capacity: u32,
    pub dropped_events: u32,
    pub latest_seq: u32,
    pub latest_at_us_lo: u32,
    pub latest_at_us_hi: u32,
    pub latest_module_tag: u32,
    pub latest_severity: u32,
    pub latest_kind: u32,
    pub latest_payload_kind: u32,
    pub latest_payload0: u32,
    pub latest_payload1: u32,
    pub checksum: u32,
}

impl EventLogReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            event_count: 0,
            capacity: 0,
            dropped_events: 0,
            latest_seq: 0,
            latest_at_us_lo: 0,
            latest_at_us_hi: 0,
            latest_module_tag: 0,
            latest_severity: 0,
            latest_kind: 0,
            latest_payload_kind: 0,
            latest_payload0: 0,
            latest_payload1: 0,
            checksum: 0,
        }
    }

    pub fn latest_at_us(&self) -> u64 {
        (u64::from(self.latest_at_us_hi) << 32) | u64::from(self.latest_at_us_lo)
    }

    pub const fn latest_severity_label(&self) -> Option<&'static str> {
        event_severity_label(self.latest_severity)
    }

    pub const fn latest_kind_label(&self) -> Option<&'static str> {
        event_kind_label(self.latest_kind)
    }

    pub const fn latest_payload_kind_label(&self) -> Option<&'static str> {
        event_payload_kind_label(self.latest_payload_kind)
    }

    pub const fn latest_module_label(&self) -> Option<&'static str> {
        module_tag_label(self.latest_module_tag)
    }

    pub fn seal(&mut self) {
        self.magic = EVENT_LOG_REPORT_MAGIC;
        self.version = EVENT_LOG_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == EVENT_LOG_REPORT_MAGIC
            && self.version == EVENT_LOG_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != EVENT_LOG_REPORT_MAGIC || self.version != EVENT_LOG_REPORT_VERSION {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if self.verify_checksum() {
            ReportStatus::Pass
        } else {
            ReportStatus::Corrupt
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.event_count
            ^ self.capacity
            ^ self.dropped_events
            ^ self.latest_seq
            ^ self.latest_at_us_lo
            ^ self.latest_at_us_hi
            ^ self.latest_module_tag
            ^ self.latest_severity
            ^ self.latest_kind
            ^ self.latest_payload_kind
            ^ self.latest_payload0
            ^ self.latest_payload1
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModuleRuntimeReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub module_count: u32,
    pub capacity: u32,
    pub active_count: u32,
    pub suspended_count: u32,
    pub faulted_count: u32,
    pub recovering_count: u32,
    pub disabled_count: u32,
    pub latest_module_tag: u32,
    pub latest_state: u32,
    pub latest_fault_count: u32,
    pub latest_recovery_count: u32,
    pub latest_change_us_lo: u32,
    pub latest_change_us_hi: u32,
    pub checksum: u32,
}

impl ModuleRuntimeReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            module_count: 0,
            capacity: 0,
            active_count: 0,
            suspended_count: 0,
            faulted_count: 0,
            recovering_count: 0,
            disabled_count: 0,
            latest_module_tag: 0,
            latest_state: 0,
            latest_fault_count: 0,
            latest_recovery_count: 0,
            latest_change_us_lo: 0,
            latest_change_us_hi: 0,
            checksum: 0,
        }
    }

    pub fn latest_change_us(&self) -> u64 {
        (u64::from(self.latest_change_us_hi) << 32) | u64::from(self.latest_change_us_lo)
    }

    pub const fn latest_state_label(&self) -> Option<&'static str> {
        module_runtime_state_label(self.latest_state)
    }

    pub const fn latest_module_label(&self) -> Option<&'static str> {
        module_tag_label(self.latest_module_tag)
    }

    pub fn seal(&mut self) {
        self.magic = MODULE_RUNTIME_REPORT_MAGIC;
        self.version = MODULE_RUNTIME_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == MODULE_RUNTIME_REPORT_MAGIC
            && self.version == MODULE_RUNTIME_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != MODULE_RUNTIME_REPORT_MAGIC
            || self.version != MODULE_RUNTIME_REPORT_VERSION
        {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if self.verify_checksum() {
            ReportStatus::Pass
        } else {
            ReportStatus::Corrupt
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.module_count
            ^ self.capacity
            ^ self.active_count
            ^ self.suspended_count
            ^ self.faulted_count
            ^ self.recovering_count
            ^ self.disabled_count
            ^ self.latest_module_tag
            ^ self.latest_state
            ^ self.latest_fault_count
            ^ self.latest_recovery_count
            ^ self.latest_change_us_lo
            ^ self.latest_change_us_hi
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DegradeApplicationReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub requested_count: u32,
    pub disabled_count: u32,
    pub already_disabled_count: u32,
    pub reason: u32,
    pub applied_at_us_lo: u32,
    pub applied_at_us_hi: u32,
    pub checksum: u32,
}

impl DegradeApplicationReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            requested_count: 0,
            disabled_count: 0,
            already_disabled_count: 0,
            reason: 0,
            applied_at_us_lo: 0,
            applied_at_us_hi: 0,
            checksum: 0,
        }
    }

    pub fn set_applied_at_us(&mut self, applied_at_us: u64) {
        self.applied_at_us_lo = applied_at_us as u32;
        self.applied_at_us_hi = (applied_at_us >> 32) as u32;
    }

    pub fn applied_at_us(&self) -> u64 {
        (u64::from(self.applied_at_us_hi) << 32) | u64::from(self.applied_at_us_lo)
    }

    pub const fn reason_label(&self) -> Option<&'static str> {
        degrade_reason_label(self.reason)
    }

    pub fn seal(&mut self) {
        self.magic = DEGRADE_APPLICATION_REPORT_MAGIC;
        self.version = DEGRADE_APPLICATION_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == DEGRADE_APPLICATION_REPORT_MAGIC
            && self.version == DEGRADE_APPLICATION_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != DEGRADE_APPLICATION_REPORT_MAGIC
            || self.version != DEGRADE_APPLICATION_REPORT_VERSION
        {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if self.verify_checksum() {
            ReportStatus::Pass
        } else {
            ReportStatus::Corrupt
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.requested_count
            ^ self.disabled_count
            ^ self.already_disabled_count
            ^ self.reason
            ^ self.applied_at_us_lo
            ^ self.applied_at_us_hi
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HealthReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub module_tag: u32,
    pub total_errors: u32,
    pub consecutive_errors: u32,
    pub last_error: u32,
    pub last_action: u32,
    pub event_count: u32,
    pub dropped_events: u32,
    pub error_events: u32,
    pub fatal_events: u32,
    pub last_seen_us_lo: u32,
    pub last_seen_us_hi: u32,
    pub checksum: u32,
}

impl HealthReport {
    pub const VERSION: u32 = 1;

    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            module_tag: 0,
            total_errors: 0,
            consecutive_errors: 0,
            last_error: 0,
            last_action: 0,
            event_count: 0,
            dropped_events: 0,
            error_events: 0,
            fatal_events: 0,
            last_seen_us_lo: 0,
            last_seen_us_hi: 0,
            checksum: 0,
        }
    }

    pub fn set_last_seen_us(&mut self, last_seen_us: u64) {
        self.last_seen_us_lo = last_seen_us as u32;
        self.last_seen_us_hi = (last_seen_us >> 32) as u32;
    }

    pub fn last_seen_us(&self) -> u64 {
        (u64::from(self.last_seen_us_hi) << 32) | u64::from(self.last_seen_us_lo)
    }

    pub const fn module_label(&self) -> Option<&'static str> {
        module_tag_label(self.module_tag)
    }

    pub fn seal(&mut self) {
        self.magic = HEALTH_REPORT_MAGIC;
        self.version = Self::VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == HEALTH_REPORT_MAGIC
            && self.version == Self::VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn status(&self) -> ReportStatus {
        if self.magic == 0 && self.version == 0 && self.checksum == 0 {
            return ReportStatus::Missing;
        }
        if self.magic != HEALTH_REPORT_MAGIC || self.version != Self::VERSION {
            return ReportStatus::Corrupt;
        }
        if self.completed == 0 {
            return ReportStatus::InProgress;
        }
        if self.verify_checksum() {
            ReportStatus::Pass
        } else {
            ReportStatus::Corrupt
        }
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.module_tag
            ^ self.total_errors
            ^ self.consecutive_errors
            ^ self.last_error
            ^ self.last_action
            ^ self.event_count
            ^ self.dropped_events
            ^ self.error_events
            ^ self.fatal_events
            ^ self.last_seen_us_lo
            ^ self.last_seen_us_hi
    }
}

impl_host_report!(
    BoardProfileReport,
    BOARD_PROFILE_REPORT_SYMBOL,
    BOARD_PROFILE_REPORT_MAGIC,
    BOARD_PROFILE_REPORT_VERSION
);
impl_host_report!(
    BoardPackageReport,
    BOARD_PACKAGE_REPORT_SYMBOL,
    BOARD_PACKAGE_REPORT_MAGIC,
    BOARD_PACKAGE_REPORT_VERSION
);
impl_host_report!(
    ManifestReport,
    MANIFEST_REPORT_SYMBOL,
    MANIFEST_REPORT_MAGIC,
    MANIFEST_REPORT_VERSION
);
impl_host_report!(
    AdapterCompatibilityReport,
    ADAPTER_COMPAT_REPORT_SYMBOL,
    ADAPTER_COMPAT_REPORT_MAGIC,
    ADAPTER_COMPAT_REPORT_VERSION
);
impl_host_report!(
    AiModelReport,
    AI_MODEL_REPORT_SYMBOL,
    AI_MODEL_REPORT_MAGIC,
    AI_MODEL_REPORT_VERSION
);
impl_host_report!(
    RosBridgeReport,
    ROS_BRIDGE_REPORT_SYMBOL,
    ROS_BRIDGE_REPORT_MAGIC,
    ROS_BRIDGE_REPORT_VERSION
);
impl_host_report!(
    AdmissionReport,
    ADMISSION_REPORT_SYMBOL,
    ADMISSION_REPORT_MAGIC,
    ADMISSION_REPORT_VERSION
);
impl_host_report!(
    RuntimeReport,
    RUNTIME_REPORT_SYMBOL,
    RUNTIME_REPORT_MAGIC,
    RUNTIME_REPORT_VERSION
);
impl_host_report!(
    EventLogReport,
    EVENT_LOG_REPORT_SYMBOL,
    EVENT_LOG_REPORT_MAGIC,
    EVENT_LOG_REPORT_VERSION
);
impl_host_report!(
    ModuleRuntimeReport,
    MODULE_RUNTIME_REPORT_SYMBOL,
    MODULE_RUNTIME_REPORT_MAGIC,
    MODULE_RUNTIME_REPORT_VERSION
);
impl_host_report!(
    DegradeApplicationReport,
    DEGRADE_APPLICATION_REPORT_SYMBOL,
    DEGRADE_APPLICATION_REPORT_MAGIC,
    DEGRADE_APPLICATION_REPORT_VERSION
);
impl_host_report!(
    HealthReport,
    HEALTH_REPORT_SYMBOL,
    HEALTH_REPORT_MAGIC,
    HealthReport::VERSION
);

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_CONTRACT_JSON: &str = include_str!("../../../../host/nobro-host-contract.json");

    #[test]
    fn boot_layouts_match_arduinonrf_policy() {
        assert_eq!(BootLayout::NoSoftDevice.app_start(), 0x1000);
        assert_eq!(BootLayout::SoftDeviceS140V6.app_start(), 0x26000);
        assert_eq!(BootLayout::Custom.app_start(), 0);
        assert_eq!(
            BootLayout::NoSoftDevice.cargo_feature(),
            "board-promicro-nosd"
        );
        assert_eq!(
            BootLayout::SoftDeviceS140V6.cargo_feature(),
            "board-nicenano-s140"
        );
        assert_eq!(BootLayout::Custom.cargo_feature(), "custom-board");
        assert_eq!(BootLayout::NoSoftDevice.code(), 1);
        assert_eq!(BootLayout::SoftDeviceS140V6.code(), 2);
        assert_eq!(BootLayout::Custom.code(), 255);
    }

    #[test]
    fn eval_contracts_are_stable() {
        assert_eq!(PHASE1_EVAL_SYMBOL, "NOBRO_EVAL_REPORT");
        assert_eq!(PHASE1_EVAL_MAGIC, 0x4E42_4E31);
        assert_eq!(PHASE2_EVAL_SYMBOL, "NOBRO_SAL_EVAL_REPORT");
        assert_eq!(PHASE2_EVAL_MAGIC, 0x4E42_4E32);
        assert_eq!(HEALTH_REPORT_SYMBOL, "NOBRO_HEALTH_REPORT");
        assert_eq!(HEALTH_REPORT_MAGIC, 0x4E42_484C);
        assert_eq!(EVENT_LOG_REPORT_SYMBOL, "NOBRO_EVENT_LOG_REPORT");
        assert_eq!(EVENT_LOG_REPORT_MAGIC, 0x4E42_454C);
        assert_eq!(EVENT_LOG_REPORT_VERSION, 1);
        assert_eq!(MODULE_RUNTIME_REPORT_SYMBOL, "NOBRO_MODULE_RUNTIME_REPORT");
        assert_eq!(MODULE_RUNTIME_REPORT_MAGIC, 0x4E42_4D52);
        assert_eq!(MODULE_RUNTIME_REPORT_VERSION, 1);
        assert_eq!(RUNTIME_REPORT_SYMBOL, "NOBRO_RUNTIME_REPORT");
        assert_eq!(RUNTIME_REPORT_MAGIC, 0x4E42_5254);
        assert_eq!(RUNTIME_REPORT_VERSION, 1);
        assert_eq!(BOARD_PROFILE_REPORT_SYMBOL, "NOBRO_BOARD_PROFILE_REPORT");
        assert_eq!(BOARD_PROFILE_REPORT_MAGIC, 0x4E42_4250);
        assert_eq!(BOARD_PROFILE_REPORT_VERSION, 1);
        assert_eq!(BOARD_PACKAGE_REPORT_SYMBOL, "NOBRO_BOARD_PACKAGE_REPORT");
        assert_eq!(BOARD_PACKAGE_REPORT_MAGIC, 0x4E42_424B);
        assert_eq!(BOARD_PACKAGE_REPORT_VERSION, 1);
        assert_eq!(MANIFEST_REPORT_SYMBOL, "NOBRO_MANIFEST_REPORT");
        assert_eq!(MANIFEST_REPORT_MAGIC, 0x4E42_4D46);
        assert_eq!(MANIFEST_REPORT_VERSION, 1);
        assert_eq!(ADAPTER_COMPAT_REPORT_SYMBOL, "NOBRO_ADAPTER_COMPAT_REPORT");
        assert_eq!(ADAPTER_COMPAT_REPORT_MAGIC, 0x4E42_4143);
        assert_eq!(ADAPTER_COMPAT_REPORT_VERSION, 1);
        assert_eq!(AI_MODEL_REPORT_SYMBOL, "NOBRO_AI_MODEL_REPORT");
        assert_eq!(AI_MODEL_REPORT_MAGIC, 0x4E42_4149);
        assert_eq!(AI_MODEL_REPORT_VERSION, 1);
        assert_eq!(ROS_BRIDGE_REPORT_SYMBOL, "NOBRO_ROS_BRIDGE_REPORT");
        assert_eq!(ROS_BRIDGE_REPORT_MAGIC, 0x4E42_5253);
        assert_eq!(ROS_BRIDGE_REPORT_VERSION, 1);
        assert_eq!(ADMISSION_REPORT_SYMBOL, "NOBRO_ADMISSION_REPORT");
        assert_eq!(ADMISSION_REPORT_MAGIC, 0x4E42_4144);
        assert_eq!(ADMISSION_REPORT_VERSION, 1);
    }

    #[test]
    fn json_contract_mentions_host_report_symbols() {
        assert!(HOST_CONTRACT_JSON.contains(PHASE1_EVAL_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(PHASE2_EVAL_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(HEALTH_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(EVENT_LOG_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(MODULE_RUNTIME_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(DEGRADE_APPLICATION_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(RUNTIME_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(BOARD_PROFILE_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(BOARD_PACKAGE_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(MANIFEST_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(ADAPTER_COMPAT_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(AI_MODEL_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(ROS_BRIDGE_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(ADMISSION_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains("0x4E424250"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E42424B"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E42454C"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E424D52"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E424447"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E424D46"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E424143"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E424149"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E425253"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E424144"));
        assert!(HOST_CONTRACT_JSON.contains("0x4E425254"));
        assert!(HOST_CONTRACT_JSON.contains("\"boot_diagnostics\""));
        assert!(HOST_CONTRACT_JSON.contains("\"board_profile\""));
        assert!(HOST_CONTRACT_JSON.contains("\"board_package\""));
        assert!(HOST_CONTRACT_JSON.contains("BOARD_PROFILE_FIXTURES"));
        assert!(HOST_CONTRACT_JSON.contains("BOARD_PACKAGE_FIXTURES"));
        assert!(HOST_CONTRACT_JSON.contains("\"adapter_compatibility\""));
        assert!(HOST_CONTRACT_JSON.contains("\"diagnostic_code\""));
        assert!(HOST_CONTRACT_JSON.contains("\"first_non_pass\""));
        assert!(HOST_CONTRACT_JSON.contains("\"summary_fields\""));
        assert!(HOST_CONTRACT_JSON.contains("\"module_tags\""));
        assert!(HOST_CONTRACT_JSON.contains("\"capability_bits\""));
        assert!(HOST_CONTRACT_JSON.contains("\"missing_owned_capability\""));
        assert!(HOST_CONTRACT_JSON.contains("\"capability_ownership_conflict\""));
        assert!(HOST_CONTRACT_JSON.contains("\"budget_exceeded\""));
        assert!(HOST_CONTRACT_JSON.contains("\"unknown_startup_node\""));
        assert!(HOST_CONTRACT_JSON.contains("\"capability\""));
    }

    #[test]
    fn user_and_maintenance_ports_are_separate() {
        assert_ne!(
            HostContract::maintenance_cdc_mi(),
            HostContract::user_cdc_mi()
        );
        assert_eq!(HostContract::upload_touch_baud(), 1200);
    }

    #[test]
    fn diagnostic_labels_and_symbols_are_stable() {
        assert_eq!(ReportStatus::Missing.label(), "missing");
        assert_eq!(ReportStatus::InProgress.label(), "in_progress");
        assert_eq!(ReportStatus::Pass.label(), "pass");
        assert_eq!(ReportStatus::Fail(9).label(), "fail");
        assert_eq!(ReportStatus::Corrupt.label(), "corrupt");
        assert_eq!(ReportStatus::Pass.code(), 0);
        assert_eq!(ReportStatus::Missing.code(), 1);
        assert_eq!(ReportStatus::InProgress.code(), 2);
        assert_eq!(ReportStatus::Corrupt.code(), 3);
        assert_eq!(ReportStatus::Fail(9).code(), 0x8000_0009);
        assert_eq!(ReportStatus::Pass.class_code(), 0);
        assert_eq!(ReportStatus::Missing.class_code(), 1);
        assert_eq!(ReportStatus::InProgress.class_code(), 2);
        assert_eq!(ReportStatus::Corrupt.class_code(), 3);
        assert_eq!(ReportStatus::Fail(9).class_code(), 4);
        assert_eq!(ReportStatus::Fail(9).error_code(), Some(9));
        assert_eq!(ReportStatus::Pass.error_code(), None);

        assert_eq!(BootStage::BoardProfile.label(), "board_profile");
        assert_eq!(BootStage::BoardPackage.label(), "board_package");
        assert_eq!(BootStage::Manifest.label(), "manifest");
        assert_eq!(
            BootStage::AdapterCompatibility.label(),
            "adapter_compatibility"
        );
        assert_eq!(BootStage::Admission.label(), "admission");
        assert_eq!(BootStage::Runtime.label(), "runtime");
        assert_eq!(BootStage::BoardProfile.code(), 1);
        assert_eq!(BootStage::BoardPackage.code(), 2);
        assert_eq!(BootStage::Manifest.code(), 3);
        assert_eq!(BootStage::AdapterCompatibility.code(), 4);
        assert_eq!(BootStage::Admission.code(), 5);
        assert_eq!(BootStage::Runtime.code(), 6);
        assert_eq!(BOOT_REPORT_STAGE_COUNT, 6);
        assert_eq!(
            BootStage::BoardPackage.symbol(),
            BOARD_PACKAGE_REPORT_SYMBOL
        );
        assert_eq!(
            BootStage::AdapterCompatibility.symbol(),
            ADAPTER_COMPAT_REPORT_SYMBOL
        );

        let diagnostic = BootDiagnostic {
            stage: BootStage::Manifest,
            status: ReportStatus::Fail(4),
        };
        assert!(!diagnostic.is_passing());
        assert_eq!(diagnostic.stage_label(), "manifest");
        assert_eq!(diagnostic.status_label(), "fail");
        assert_eq!(diagnostic.stage_symbol(), MANIFEST_REPORT_SYMBOL);
        assert_eq!(diagnostic.error_code(), Some(4));
        assert_eq!(diagnostic.error_label(), Some("missing_owned_capability"));
        assert_eq!(diagnostic.code(), 0x0304_0004);
        assert_eq!(
            BootDiagnostic {
                stage: BootStage::BoardPackage,
                status: ReportStatus::Fail(7),
            }
            .error_label(),
            Some("duplicate_critical_pin")
        );
        assert_eq!(manifest_error_label(10), Some("budget_exceeded"));
        assert_eq!(manifest_error_label(99), None);
        assert_eq!(board_package_error_label(3), Some("unaligned_flash_origin"));
        assert_eq!(board_package_error_label(99), None);
        assert_eq!(
            adapter_compat_error_label(3),
            Some("capability_ownership_conflict")
        );
        assert_eq!(adapter_compat_error_label(99), None);
        assert_eq!(admission_error_label(6), Some("unknown_startup_node"));
        assert_eq!(admission_error_label(99), None);
        assert_eq!(module_tag_label(1), Some("kernel"));
        assert_eq!(module_tag_label(5), Some("sensor"));
        assert_eq!(module_tag_label(9), Some("ai"));
        assert_eq!(module_tag_label(0x107), Some("app"));
        assert_eq!(module_tag_label(99), None);
        assert_eq!(app_module_tag_index(0x107), Some(7));
        assert_eq!(app_module_tag_index(5), None);
        assert_eq!(capability_bit_label(0), Some("timebase"));
        assert_eq!(capability_bit_label(10), Some("host_report"));
        assert_eq!(capability_bit_label(11), Some("ai_inference"));
        assert_eq!(capability_bit_label(12), Some("ai_endpoint"));
        assert_eq!(capability_bit_label(99), None);
        assert_eq!(capability_mask_label(0x0000_0008), Some("bus0"));
        assert_eq!(capability_mask_label(0x0000_0400), Some("host_report"));
        assert_eq!(capability_mask_label(0x0000_0800), Some("ai_inference"));
        assert_eq!(capability_mask_label(0x0000_1000), Some("ai_endpoint"));
        assert_eq!(capability_mask_label(0x0000_0408), None);
        assert_eq!(runtime_state_label(3), Some("running"));
        assert_eq!(runtime_state_label(6), Some("halted"));
        assert_eq!(runtime_state_label(99), None);
        assert_eq!(event_severity_label(4), Some("fatal"));
        assert_eq!(event_severity_label(99), None);
        assert_eq!(event_kind_label(8), Some("host"));
        assert_eq!(event_kind_label(99), None);
        assert_eq!(event_payload_kind_label(4), Some("pair"));
        assert_eq!(event_payload_kind_label(99), None);
        assert_eq!(module_runtime_state_label(1), Some("registered"));
        assert_eq!(module_runtime_state_label(2), Some("active"));
        assert_eq!(module_runtime_state_label(3), Some("suspended"));
        assert_eq!(module_runtime_state_label(4), Some("faulted"));
        assert_eq!(module_runtime_state_label(5), Some("recovering"));
        assert_eq!(module_runtime_state_label(6), Some("disabled"));
        assert_eq!(module_runtime_state_label(99), None);
        assert_eq!(degrade_reason_label(0), Some("none"));
        assert_eq!(degrade_reason_label(1), Some("flash_budget"));
        assert_eq!(degrade_reason_label(2), Some("ram_budget"));
        assert_eq!(degrade_reason_label(3), Some("pool_budget"));
        assert_eq!(degrade_reason_label(4), Some("module_limit"));
        assert_eq!(degrade_reason_label(99), None);

        let adapter = BootDiagnostic {
            stage: BootStage::AdapterCompatibility,
            status: ReportStatus::Fail(5),
        };
        assert_eq!(adapter.error_label(), Some("budget_exceeded"));

        let runtime = BootDiagnostic {
            stage: BootStage::Runtime,
            status: ReportStatus::Corrupt,
        };
        assert_eq!(runtime.error_label(), None);
    }

    #[test]
    fn health_report_seals_and_verifies() {
        let mut report = HealthReport {
            module_tag: 4,
            total_errors: 7,
            consecutive_errors: 2,
            last_error: 3,
            last_action: 2,
            event_count: 12,
            dropped_events: 1,
            error_events: 2,
            fatal_events: 0,
            ..HealthReport::zeroed()
        };
        report.set_last_seen_us(0x1234_5678_9ABC_DEF0);
        report.seal();

        assert!(report.verify_checksum());
        assert_eq!(report.last_seen_us(), 0x1234_5678_9ABC_DEF0);
        assert_eq!(report.module_label(), Some("radio"));

        report.total_errors += 1;
        assert!(!report.verify_checksum());
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn admission_report_status_decodes_success_and_failure() {
        let mut pass = AdmissionReport {
            admitted: 1,
            module_count: 3,
            startup_len: 3,
            flash_used_bytes: 32 * 1024,
            flash_limit_bytes: 80 * 1024,
            ram_used_bytes: 9 * 1024,
            ram_limit_bytes: 32 * 1024,
            pool_used_slots: 5,
            pool_limit_slots: 8,
            ..AdmissionReport::zeroed()
        };
        pass.seal();

        assert!(pass.verify_checksum());
        assert_eq!(pass.status(), ReportStatus::Pass);

        let mut fail = AdmissionReport {
            admitted: 0,
            error_code: 2,
            ..AdmissionReport::zeroed()
        };
        fail.seal();

        assert_eq!(fail.status(), ReportStatus::Fail(2));
    }

    #[test]
    fn board_profile_report_status_decodes_success_and_corruption() {
        let mut report = BoardProfileReport {
            platform_hash: 0x1111_2222,
            board_hash: 0x3333_4444,
            app_flash_start: 0x1000,
            flash_budget_bytes: 80 * 1024,
            ram_budget_bytes: 32 * 1024,
            sample_pool_slots: 8,
            max_modules: 16,
            servo_pin: 24,
            servo_center_us: 1500,
            led_pin: 15,
            mvk_trigger_pin: 17,
            ..BoardProfileReport::zeroed()
        };
        report.seal();

        assert!(report.verify_checksum());
        assert_eq!(report.status(), ReportStatus::Pass);

        report.servo_pin += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn board_package_report_status_decodes_success_and_failure() {
        let mut pass = BoardPackageReport {
            valid: 1,
            platform_hash: 0x1111_2222,
            board_hash: 0x3333_4444,
            boot_layout: BootLayout::NoSoftDevice.code(),
            app_flash_start: 0x1000,
            app_flash_len_bytes: 1020 * 1024,
            ram_start: 0x2000_0000,
            ram_len_bytes: 256 * 1024,
            flash_budget_bytes: 80 * 1024,
            ram_budget_bytes: 32 * 1024,
            sample_pool_slots: 8,
            max_modules: 16,
            led_pin: 15,
            servo_pin: 24,
            mvk_trigger_pin: 17,
            ..BoardPackageReport::zeroed()
        };
        pass.seal();

        assert!(pass.verify_checksum());
        assert_eq!(pass.status(), ReportStatus::Pass);

        let mut fail = BoardPackageReport {
            valid: 0,
            error_code: 7,
            ..pass
        };
        fail.seal();

        assert_eq!(fail.status(), ReportStatus::Fail(7));
        assert_eq!(board_package_error_label(7), Some("duplicate_critical_pin"));
        fail.ram_len_bytes += 1;
        assert_eq!(fail.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn manifest_report_status_decodes_success_and_failure() {
        let mut pass = ManifestReport {
            valid: 1,
            module_count: 3,
            fingerprint: 0xABCD_1234,
            required_bits: 0x10,
            owned_bits: 0x20,
            flash_used_bytes: 36 * 1024,
            ram_used_bytes: 10 * 1024,
            pool_used_slots: 6,
            ..ManifestReport::zeroed()
        };
        pass.seal();

        assert!(pass.verify_checksum());
        assert_eq!(pass.status(), ReportStatus::Pass);

        let mut fail = ManifestReport {
            valid: 0,
            module_count: 3,
            error_code: 4,
            error_module_tag: 5,
            error_capability_bits: 0x02,
            ..ManifestReport::zeroed()
        };
        fail.seal();

        assert_eq!(fail.status(), ReportStatus::Fail(4));
        assert_eq!(fail.error_module_label(), Some("sensor"));
        assert_eq!(fail.error_capability_label(), Some("deadline_timer"));
        fail.error_code = 5;
        assert_eq!(fail.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn host_report_trait_exposes_common_header_checks() {
        let mut report = ManifestReport {
            valid: 1,
            module_count: 2,
            fingerprint: 0xCAFE_BABE,
            ..ManifestReport::zeroed()
        };

        assert!(HostReport::is_missing(&report));
        report.seal();

        assert_eq!(
            <ManifestReport as HostReport>::SYMBOL,
            MANIFEST_REPORT_SYMBOL
        );
        assert_eq!(HostReport::raw_magic(&report), MANIFEST_REPORT_MAGIC);
        assert_eq!(HostReport::raw_version(&report), MANIFEST_REPORT_VERSION);
        assert_eq!(HostReport::completed(&report), 1);
        assert!(HostReport::has_expected_header(&report));
        assert!(HostReport::verify_checksum(&report));
        assert_eq!(HostReport::status(&report), ReportStatus::Pass);
    }

    #[test]
    fn adapter_compatibility_report_status_decodes_success_and_failure() {
        let mut pass = AdapterCompatibilityReport {
            compatible: 1,
            adapter_count: 2,
            required_bits: 0x03,
            owned_bits: 0x0C,
            flash_used_bytes: 8192,
            ram_used_bytes: 2048,
            pool_used_slots: 3,
            ..AdapterCompatibilityReport::zeroed()
        };
        pass.seal();

        assert!(pass.verify_checksum());
        assert_eq!(pass.status(), ReportStatus::Pass);

        let mut fail = AdapterCompatibilityReport {
            compatible: 0,
            adapter_count: 2,
            error_code: 3,
            error_module_tag: 3,
            error_capability_bits: 0x02,
            ..AdapterCompatibilityReport::zeroed()
        };
        fail.seal();

        assert_eq!(fail.status(), ReportStatus::Fail(3));
        assert_eq!(fail.error_module_label(), Some("bus"));
        assert_eq!(fail.error_capability_label(), Some("deadline_timer"));
        fail.error_capability_bits = 0x04;
        assert_eq!(fail.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn ai_model_report_status_tracks_contract_corruption() {
        let mut report = AiModelReport {
            backend: 4,
            model_id: 7,
            input_bytes_max: 16,
            output_bytes_max: 24,
            arena_bytes: 4096,
            timeout_us: 5_000,
            route_preference: 4,
            stale_after_us: 30_000,
            endpoint_failure_limit: 2,
            ..AiModelReport::zeroed()
        };
        report.seal();

        assert_eq!(report.status(), ReportStatus::Pass);
        assert_eq!(HostReport::raw_magic(&report), AI_MODEL_REPORT_MAGIC);
        assert_eq!(HostReport::status(&report), ReportStatus::Pass);

        report.timeout_us += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn ros_bridge_report_status_tracks_contract_corruption() {
        let mut report = RosBridgeReport {
            transport: 1,
            bridge_id_hash: 0xA11CE,
            topic_count: 1,
            service_count: 1,
            action_count: 1,
            parameter_count: 1,
            total_buffer_bytes: 340,
            max_timeout_us: 100_000,
            ..RosBridgeReport::zeroed()
        };
        report.seal();

        assert_eq!(report.status(), ReportStatus::Pass);
        assert_eq!(HostReport::raw_magic(&report), ROS_BRIDGE_REPORT_MAGIC);
        assert_eq!(HostReport::status(&report), ReportStatus::Pass);

        report.topic_count += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn boot_reports_return_first_non_passing_stage() {
        let mut board = BoardProfileReport {
            platform_hash: 1,
            board_hash: 2,
            app_flash_start: 0x1000,
            flash_budget_bytes: 80 * 1024,
            ram_budget_bytes: 32 * 1024,
            sample_pool_slots: 8,
            max_modules: 16,
            servo_pin: 24,
            servo_center_us: 1500,
            led_pin: 15,
            mvk_trigger_pin: 17,
            ..BoardProfileReport::zeroed()
        };
        board.seal();

        let mut package = BoardPackageReport {
            valid: 1,
            platform_hash: 1,
            board_hash: 2,
            boot_layout: BootLayout::NoSoftDevice.code(),
            app_flash_start: 0x1000,
            app_flash_len_bytes: 1020 * 1024,
            ram_start: 0x2000_0000,
            ram_len_bytes: 256 * 1024,
            flash_budget_bytes: 80 * 1024,
            ram_budget_bytes: 32 * 1024,
            sample_pool_slots: 8,
            max_modules: 16,
            led_pin: 15,
            servo_pin: 24,
            mvk_trigger_pin: 17,
            ..BoardPackageReport::zeroed()
        };
        package.seal();

        let mut adapter = AdapterCompatibilityReport {
            compatible: 1,
            adapter_count: 2,
            ..AdapterCompatibilityReport::zeroed()
        };
        adapter.seal();

        let mut manifest = ManifestReport {
            valid: 1,
            module_count: 3,
            fingerprint: 0x1234,
            ..ManifestReport::zeroed()
        };
        manifest.seal();

        let mut admission = AdmissionReport {
            admitted: 1,
            module_count: 3,
            startup_len: 3,
            ..AdmissionReport::zeroed()
        };
        admission.seal();

        let mut runtime = RuntimeReport {
            state: 3,
            module_count: 3,
            ..RuntimeReport::zeroed()
        };
        runtime.seal();

        let reports = BootReports::new(board, package, manifest, adapter, admission, runtime);
        let slots = reports.slots();
        assert_eq!(
            slots,
            [
                ReportSlot {
                    stage: BootStage::BoardProfile,
                    symbol: BOARD_PROFILE_REPORT_SYMBOL,
                    status: ReportStatus::Pass,
                },
                ReportSlot {
                    stage: BootStage::BoardPackage,
                    symbol: BOARD_PACKAGE_REPORT_SYMBOL,
                    status: ReportStatus::Pass,
                },
                ReportSlot {
                    stage: BootStage::Manifest,
                    symbol: MANIFEST_REPORT_SYMBOL,
                    status: ReportStatus::Pass,
                },
                ReportSlot {
                    stage: BootStage::AdapterCompatibility,
                    symbol: ADAPTER_COMPAT_REPORT_SYMBOL,
                    status: ReportStatus::Pass,
                },
                ReportSlot {
                    stage: BootStage::Admission,
                    symbol: ADMISSION_REPORT_SYMBOL,
                    status: ReportStatus::Pass,
                },
                ReportSlot {
                    stage: BootStage::Runtime,
                    symbol: RUNTIME_REPORT_SYMBOL,
                    status: ReportStatus::Pass,
                },
            ]
        );
        assert_eq!(
            reports.diagnostic(),
            BootDiagnostic {
                stage: BootStage::Runtime,
                status: ReportStatus::Pass,
            }
        );
        let summary = reports.summary();
        assert!(summary.is_passing());
        assert_eq!(summary.diagnostic_code(), 0x0600_0000);
        assert_eq!(summary.pass_count, BOOT_REPORT_STAGE_COUNT as u8);
        assert_eq!(summary.observed_count(), BOOT_REPORT_STAGE_COUNT as u8);
        assert_eq!(summary.missing_count, 0);
        assert_eq!(summary.first_stage_label(), "runtime");
        assert_eq!(summary.first_status_label(), "pass");
        assert_eq!(summary.first_symbol(), RUNTIME_REPORT_SYMBOL);
        assert_eq!(summary.first_error_label(), None);

        let mut failed_adapter = adapter;
        failed_adapter.compatible = 0;
        failed_adapter.error_code = 3;
        failed_adapter.seal();
        let reports =
            BootReports::new(board, package, manifest, failed_adapter, admission, runtime);
        assert_eq!(
            reports.diagnostic(),
            BootDiagnostic {
                stage: BootStage::AdapterCompatibility,
                status: ReportStatus::Fail(3),
            }
        );
        let summary = reports.summary();
        assert!(!summary.is_passing());
        assert_eq!(summary.pass_count, 5);
        assert_eq!(summary.fail_count, 1);
        assert_eq!(summary.missing_count, 0);
        assert_eq!(summary.first_stage_label(), "adapter_compatibility");
        assert_eq!(summary.first_status_label(), "fail");
        assert_eq!(
            summary.first_error_label(),
            Some("capability_ownership_conflict")
        );
        assert_eq!(summary.diagnostic_code(), 0x0404_0003);
        assert_eq!(summary.slots[3].symbol, ADAPTER_COMPAT_REPORT_SYMBOL);

        let reports = BootReports::new(
            BoardProfileReport::zeroed(),
            package,
            manifest,
            failed_adapter,
            admission,
            runtime,
        );
        assert_eq!(
            reports.diagnostic(),
            BootDiagnostic {
                stage: BootStage::BoardProfile,
                status: ReportStatus::Missing,
            }
        );
        let summary = reports.summary();
        assert_eq!(summary.missing_count, 1);
        assert_eq!(summary.observed_count(), BOOT_REPORT_STAGE_COUNT as u8);
        assert_eq!(summary.first_stage_label(), "board_profile");

        let reports = BootReports::new(
            board,
            BoardPackageReport::zeroed(),
            manifest,
            adapter,
            admission,
            runtime,
        );
        assert_eq!(
            reports.diagnostic(),
            BootDiagnostic {
                stage: BootStage::BoardPackage,
                status: ReportStatus::Missing,
            }
        );

        let reports = BootReports::new(
            board,
            package,
            ManifestReport::zeroed(),
            adapter,
            admission,
            runtime,
        );
        assert_eq!(
            reports.diagnostic(),
            BootDiagnostic {
                stage: BootStage::Manifest,
                status: ReportStatus::Missing,
            }
        );
    }

    #[test]
    fn runtime_report_seals_and_verifies() {
        let mut report = RuntimeReport {
            state: 3,
            module_count: 4,
            mailbox_len: 2,
            mailbox_dropped: 1,
            alarm_len: 1,
            kv_len: 3,
            kv_writes: 5,
            kv_deletes: 1,
            quota_flash_used_bytes: 4096,
            quota_ram_used_bytes: 1024,
            quota_pool_used_slots: 2,
            event_count: 7,
            dropped_events: 1,
            ..RuntimeReport::zeroed()
        };
        report.set_next_alarm_due_us(0x0123_4567_89AB_CDEF);
        report.seal();

        assert!(report.verify_checksum());
        assert_eq!(report.next_alarm_due_us(), 0x0123_4567_89AB_CDEF);
        assert_eq!(report.state_label(), Some("running"));
        assert_eq!(report.status(), ReportStatus::Pass);

        report.kv_writes += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn event_log_report_seals_and_verifies() {
        let mut report = EventLogReport {
            event_count: 6,
            capacity: 16,
            dropped_events: 1,
            latest_seq: 7,
            latest_module_tag: 5,
            latest_severity: 3,
            latest_kind: 2,
            latest_payload_kind: 1,
            latest_payload0: 4,
            latest_payload1: 0,
            ..EventLogReport::zeroed()
        };
        report.latest_at_us_lo = 0x89AB_CDEF;
        report.latest_at_us_hi = 0x0123_4567;
        report.seal();

        assert!(report.verify_checksum());
        assert_eq!(report.latest_at_us(), 0x0123_4567_89AB_CDEF);
        assert_eq!(report.latest_module_label(), Some("sensor"));
        assert_eq!(report.latest_severity_label(), Some("error"));
        assert_eq!(report.latest_kind_label(), Some("health"));
        assert_eq!(report.latest_payload_kind_label(), Some("error"));
        assert_eq!(report.status(), ReportStatus::Pass);
        assert_eq!(
            <EventLogReport as HostReport>::SYMBOL,
            EVENT_LOG_REPORT_SYMBOL
        );

        report.latest_seq += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn module_runtime_report_seals_and_verifies() {
        let mut report = ModuleRuntimeReport {
            module_count: 4,
            capacity: 8,
            active_count: 2,
            suspended_count: 1,
            faulted_count: 1,
            recovering_count: 0,
            disabled_count: 0,
            latest_module_tag: 5,
            latest_state: 4,
            latest_fault_count: 3,
            latest_recovery_count: 1,
            ..ModuleRuntimeReport::zeroed()
        };
        report.latest_change_us_lo = 0x89AB_CDEF;
        report.latest_change_us_hi = 0x0123_4567;
        report.seal();

        assert!(report.verify_checksum());
        assert_eq!(report.latest_change_us(), 0x0123_4567_89AB_CDEF);
        assert_eq!(report.latest_module_label(), Some("sensor"));
        assert_eq!(report.latest_state_label(), Some("faulted"));
        assert_eq!(report.status(), ReportStatus::Pass);
        assert_eq!(
            <ModuleRuntimeReport as HostReport>::SYMBOL,
            MODULE_RUNTIME_REPORT_SYMBOL
        );

        report.faulted_count += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn degrade_application_report_seals_and_verifies() {
        let mut report = DegradeApplicationReport {
            requested_count: 2,
            disabled_count: 1,
            already_disabled_count: 1,
            reason: 4,
            ..DegradeApplicationReport::zeroed()
        };
        report.set_applied_at_us(0x0123_4567_89AB_CDEF);
        report.seal();

        assert!(report.verify_checksum());
        assert_eq!(report.applied_at_us(), 0x0123_4567_89AB_CDEF);
        assert_eq!(report.reason_label(), Some("module_limit"));
        assert_eq!(report.status(), ReportStatus::Pass);
        assert_eq!(
            <DegradeApplicationReport as HostReport>::SYMBOL,
            DEGRADE_APPLICATION_REPORT_SYMBOL
        );

        report.disabled_count += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn report_status_detects_missing_and_corrupt_reports() {
        assert_eq!(AdmissionReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(BoardProfileReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(BoardPackageReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(ManifestReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(
            AdapterCompatibilityReport::zeroed().status(),
            ReportStatus::Missing
        );
        assert_eq!(AiModelReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(RosBridgeReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(HealthReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(EventLogReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(
            ModuleRuntimeReport::zeroed().status(),
            ReportStatus::Missing
        );
        assert_eq!(
            DegradeApplicationReport::zeroed().status(),
            ReportStatus::Missing
        );
        assert_eq!(RuntimeReport::zeroed().status(), ReportStatus::Missing);

        let mut report = AdmissionReport {
            admitted: 1,
            ..AdmissionReport::zeroed()
        };
        report.seal();
        report.flash_used_bytes += 1;

        assert_eq!(report.status(), ReportStatus::Corrupt);
    }
}
