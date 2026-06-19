//! Host-side contract constants shared by scripts, tools, and documentation.

#![no_std]

pub const MAINTENANCE_CDC_MI: &str = "MI_00";
pub const USER_CDC_MI: &str = "MI_02";
pub const UPLOAD_TOUCH_BAUD: u32 = 1200;

pub const APP_START_NO_SOFTDEVICE: u32 = 0x1000;
pub const APP_START_S140_V6: u32 = 0x26000;

pub const PHASE1_EVAL_SYMBOL: &str = "AIRON_EVAL_REPORT";
pub const PHASE1_EVAL_MAGIC: u32 = 0x4152_4E31;
pub const PHASE2_EVAL_SYMBOL: &str = "AIRON_SAL_EVAL_REPORT";
pub const PHASE2_EVAL_MAGIC: u32 = 0x4152_4E32;
pub const HEALTH_REPORT_SYMBOL: &str = "AIRON_HEALTH_REPORT";
pub const HEALTH_REPORT_MAGIC: u32 = 0x4152_484C;
pub const RUNTIME_REPORT_SYMBOL: &str = "AIRON_RUNTIME_REPORT";
pub const RUNTIME_REPORT_MAGIC: u32 = 0x4152_5254;
pub const RUNTIME_REPORT_VERSION: u32 = 1;
pub const BOARD_PROFILE_REPORT_SYMBOL: &str = "AIRON_BOARD_PROFILE_REPORT";
pub const BOARD_PROFILE_REPORT_MAGIC: u32 = 0x4152_4250;
pub const BOARD_PROFILE_REPORT_VERSION: u32 = 1;
pub const MANIFEST_REPORT_SYMBOL: &str = "AIRON_MANIFEST_REPORT";
pub const MANIFEST_REPORT_MAGIC: u32 = 0x4152_4D46;
pub const MANIFEST_REPORT_VERSION: u32 = 1;
pub const ADAPTER_COMPAT_REPORT_SYMBOL: &str = "AIRON_ADAPTER_COMPAT_REPORT";
pub const ADAPTER_COMPAT_REPORT_MAGIC: u32 = 0x4152_4143;
pub const ADAPTER_COMPAT_REPORT_VERSION: u32 = 1;
pub const ADMISSION_REPORT_SYMBOL: &str = "AIRON_ADMISSION_REPORT";
pub const ADMISSION_REPORT_MAGIC: u32 = 0x4152_4144;
pub const ADMISSION_REPORT_VERSION: u32 = 1;

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
}

impl BootLayout {
    pub const fn app_start(self) -> u32 {
        match self {
            Self::NoSoftDevice => APP_START_NO_SOFTDEVICE,
            Self::SoftDeviceS140V6 => APP_START_S140_V6,
        }
    }

    pub const fn cargo_feature(self) -> &'static str {
        match self {
            Self::NoSoftDevice => "board-promicro-nosd",
            Self::SoftDeviceS140V6 => "board-nicenano-s140",
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
    Manifest,
    AdapterCompatibility,
    Admission,
    Runtime,
}

impl BootStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::BoardProfile => "board_profile",
            Self::Manifest => "manifest",
            Self::AdapterCompatibility => "adapter_compatibility",
            Self::Admission => "admission",
            Self::Runtime => "runtime",
        }
    }

    pub const fn symbol(self) -> &'static str {
        match self {
            Self::BoardProfile => BOARD_PROFILE_REPORT_SYMBOL,
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
            BootStage::Manifest => manifest_error_label(code),
            BootStage::AdapterCompatibility => adapter_compat_error_label(code),
            BootStage::Admission => admission_error_label(code),
        }
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
    pub manifest: ManifestReport,
    pub adapter_compatibility: AdapterCompatibilityReport,
    pub admission: AdmissionReport,
    pub runtime: RuntimeReport,
}

impl BootReports {
    pub const fn new(
        board_profile: BoardProfileReport,
        manifest: ManifestReport,
        adapter_compatibility: AdapterCompatibilityReport,
        admission: AdmissionReport,
        runtime: RuntimeReport,
    ) -> Self {
        Self {
            board_profile,
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

    pub fn slots(&self) -> [ReportSlot; 5] {
        [
            ReportSlot {
                stage: BootStage::BoardProfile,
                symbol: <BoardProfileReport as HostReport>::SYMBOL,
                status: self.board_profile.status(),
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

    const HOST_CONTRACT_JSON: &str = include_str!("../../../../host/airon-host-contract.json");

    #[test]
    fn boot_layouts_match_arduinonrf_policy() {
        assert_eq!(BootLayout::NoSoftDevice.app_start(), 0x1000);
        assert_eq!(BootLayout::SoftDeviceS140V6.app_start(), 0x26000);
        assert_eq!(
            BootLayout::NoSoftDevice.cargo_feature(),
            "board-promicro-nosd"
        );
        assert_eq!(
            BootLayout::SoftDeviceS140V6.cargo_feature(),
            "board-nicenano-s140"
        );
    }

    #[test]
    fn eval_contracts_are_stable() {
        assert_eq!(PHASE1_EVAL_SYMBOL, "AIRON_EVAL_REPORT");
        assert_eq!(PHASE1_EVAL_MAGIC, 0x4152_4E31);
        assert_eq!(PHASE2_EVAL_SYMBOL, "AIRON_SAL_EVAL_REPORT");
        assert_eq!(PHASE2_EVAL_MAGIC, 0x4152_4E32);
        assert_eq!(HEALTH_REPORT_SYMBOL, "AIRON_HEALTH_REPORT");
        assert_eq!(HEALTH_REPORT_MAGIC, 0x4152_484C);
        assert_eq!(RUNTIME_REPORT_SYMBOL, "AIRON_RUNTIME_REPORT");
        assert_eq!(RUNTIME_REPORT_MAGIC, 0x4152_5254);
        assert_eq!(RUNTIME_REPORT_VERSION, 1);
        assert_eq!(BOARD_PROFILE_REPORT_SYMBOL, "AIRON_BOARD_PROFILE_REPORT");
        assert_eq!(BOARD_PROFILE_REPORT_MAGIC, 0x4152_4250);
        assert_eq!(BOARD_PROFILE_REPORT_VERSION, 1);
        assert_eq!(MANIFEST_REPORT_SYMBOL, "AIRON_MANIFEST_REPORT");
        assert_eq!(MANIFEST_REPORT_MAGIC, 0x4152_4D46);
        assert_eq!(MANIFEST_REPORT_VERSION, 1);
        assert_eq!(ADAPTER_COMPAT_REPORT_SYMBOL, "AIRON_ADAPTER_COMPAT_REPORT");
        assert_eq!(ADAPTER_COMPAT_REPORT_MAGIC, 0x4152_4143);
        assert_eq!(ADAPTER_COMPAT_REPORT_VERSION, 1);
        assert_eq!(ADMISSION_REPORT_SYMBOL, "AIRON_ADMISSION_REPORT");
        assert_eq!(ADMISSION_REPORT_MAGIC, 0x4152_4144);
        assert_eq!(ADMISSION_REPORT_VERSION, 1);
    }

    #[test]
    fn json_contract_mentions_host_report_symbols() {
        assert!(HOST_CONTRACT_JSON.contains(PHASE1_EVAL_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(PHASE2_EVAL_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(HEALTH_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(RUNTIME_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(BOARD_PROFILE_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(MANIFEST_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(ADAPTER_COMPAT_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains(ADMISSION_REPORT_SYMBOL));
        assert!(HOST_CONTRACT_JSON.contains("0x41524250"));
        assert!(HOST_CONTRACT_JSON.contains("0x41524D46"));
        assert!(HOST_CONTRACT_JSON.contains("0x41524143"));
        assert!(HOST_CONTRACT_JSON.contains("0x41524144"));
        assert!(HOST_CONTRACT_JSON.contains("0x41525254"));
        assert!(HOST_CONTRACT_JSON.contains("\"boot_diagnostics\""));
        assert!(HOST_CONTRACT_JSON.contains("\"board_profile\""));
        assert!(HOST_CONTRACT_JSON.contains("\"adapter_compatibility\""));
        assert!(HOST_CONTRACT_JSON.contains("\"first_non_pass\""));
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
        assert_eq!(ReportStatus::Fail(9).error_code(), Some(9));
        assert_eq!(ReportStatus::Pass.error_code(), None);

        assert_eq!(BootStage::BoardProfile.label(), "board_profile");
        assert_eq!(BootStage::Manifest.label(), "manifest");
        assert_eq!(
            BootStage::AdapterCompatibility.label(),
            "adapter_compatibility"
        );
        assert_eq!(BootStage::Admission.label(), "admission");
        assert_eq!(BootStage::Runtime.label(), "runtime");
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
        assert_eq!(manifest_error_label(10), Some("budget_exceeded"));
        assert_eq!(manifest_error_label(99), None);
        assert_eq!(
            adapter_compat_error_label(3),
            Some("capability_ownership_conflict")
        );
        assert_eq!(adapter_compat_error_label(99), None);
        assert_eq!(admission_error_label(6), Some("unknown_startup_node"));
        assert_eq!(admission_error_label(99), None);

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
        fail.error_capability_bits = 0x04;
        assert_eq!(fail.status(), ReportStatus::Corrupt);
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

        let reports = BootReports::new(board, manifest, adapter, admission, runtime);
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

        let mut failed_adapter = adapter;
        failed_adapter.compatible = 0;
        failed_adapter.error_code = 3;
        failed_adapter.seal();
        let reports = BootReports::new(board, manifest, failed_adapter, admission, runtime);
        assert_eq!(
            reports.diagnostic(),
            BootDiagnostic {
                stage: BootStage::AdapterCompatibility,
                status: ReportStatus::Fail(3),
            }
        );

        let reports = BootReports::new(
            BoardProfileReport::zeroed(),
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

        let reports =
            BootReports::new(board, ManifestReport::zeroed(), adapter, admission, runtime);
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
        assert_eq!(report.status(), ReportStatus::Pass);

        report.kv_writes += 1;
        assert_eq!(report.status(), ReportStatus::Corrupt);
    }

    #[test]
    fn report_status_detects_missing_and_corrupt_reports() {
        assert_eq!(AdmissionReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(BoardProfileReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(ManifestReport::zeroed().status(), ReportStatus::Missing);
        assert_eq!(
            AdapterCompatibilityReport::zeroed().status(),
            ReportStatus::Missing
        );
        assert_eq!(HealthReport::zeroed().status(), ReportStatus::Missing);
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
