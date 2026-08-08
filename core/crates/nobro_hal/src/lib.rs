//! NobroRTOS hardware abstraction with portable traits and platform backends.
//!
//! Application code should prefer:
//! - `traits::*` and `platform::ActivePlatform` for portable logic
//! - Legacy module paths (`timer`, `pwm`, etc.) remain for the nRF52840 port

#![no_std]

pub mod board_catalog;
pub mod board_desc;
pub mod completion;
pub mod dma_lease;
pub mod esp;
pub mod isolation;
pub mod lease;
pub mod mpu;
pub mod platform;
pub mod rp2;
pub mod snapshots;
pub mod traits;

#[cfg(all(feature = "board-promicro-nosd", feature = "board-promicro-s140"))]
compile_error!("nobro-hal: enable exactly one board-* feature");

#[cfg(all(feature = "pmsa-v7", feature = "pmsa-v8"))]
compile_error!("nobro-hal: select exactly one PMSA architecture");

#[cfg(all(
    feature = "platform-nrf52840",
    not(any(feature = "board-promicro-nosd", feature = "board-promicro-s140"))
))]
compile_error!("nobro-hal: enable one board-* feature");

#[cfg(feature = "platform-nrf52840")]
pub mod board;
#[cfg(feature = "platform-nrf52840")]
pub mod bus;
// `board-promicro-s140` selects the resident S140 flash/IRQ layout. ArduinoNRF
// leaves that SoftDevice dormant unless an application explicitly enables it,
// so direct PendSV/NVIC ownership is valid for the default composition. An
// active SoftDevice needs a separately admitted integration and is not claimed.
#[cfg(all(feature = "cortex-m-slice", target_has_atomic = "32"))]
pub mod context_switch;
#[cfg(all(feature = "cortex-m0-slice", not(target_has_atomic = "32")))]
pub mod context_switch_m0;
#[cfg(feature = "platform-nrf52840")]
pub mod deadline_timer;
#[cfg(feature = "platform-nrf52840")]
pub mod nrf_peripherals;
#[cfg(feature = "platform-nrf52840-rt")]
pub mod power_nrf;
#[cfg(feature = "platform-nrf52840")]
pub mod ppi;
#[cfg(feature = "platform-nrf52840")]
pub mod priority_ceiling;
#[cfg(feature = "platform-nrf52840")]
pub mod pwm;
mod quiesce;
#[cfg(feature = "platform-nrf52840")]
pub mod radio_hw;
#[cfg(feature = "platform-nrf52840")]
pub mod radio_sim;
#[cfg(feature = "platform-nrf52840")]
pub mod spim_hw;
#[cfg(feature = "platform-nrf52840")]
pub mod timer;
#[cfg(feature = "platform-nrf52840")]
pub mod twim_hw;

pub use board_catalog::{
    exact_profile_for_feature, package_for_feature, profile_for_feature, BoardPackageDefinition,
    BoardProfileDefinition, ExactBoardProfileDefinition, BOARD_PACKAGES, BOARD_PROFILES,
    EXACT_BOARD_PROFILES, NRF52840_BOARD_CAPACITY, NRF52840_BOARD_PINS, NRF52840_SERVO_CENTER_US,
    PROMICRO_NRF52840_NOSD_PACKAGE, PROMICRO_NRF52840_S140_PACKAGE,
};
pub use board_desc::{
    BoardCapacity, BoardDesc, BoardPackage, BoardPackageError, BoardPins, BootLayout, BootProfile,
    BusLayout, ServoProfile,
};
pub use completion::{CompletionCell, CompletionError, StagedTransferError, StagedTransferPlan};
pub use dma_lease::{
    DmaBufferDescriptor, DmaCoherency, DmaCompletionReceipt, DmaDirection, DmaLease,
    DmaLeaseBackend, DmaLeaseError, DmaLeaseRegistry, DmaLeaseRequest, DmaOwnerId,
    DmaRecoveryReason, DmaRecoveryReceipt,
};
pub use esp::{
    EspAdc, EspAdcBackend, EspAlarm, EspAlarmBackend, EspByteIo, EspByteIoBackend,
    EspCacheContract, EspContractError, EspDmaBackend, EspDmaCompletion, EspDmaDomain, EspDmaKind,
    EspDmaPlan, EspEventBackend, EspEventCapture, EspGpio, EspGpioBackend, EspI2c, EspIoController,
    EspIoRoute, EspIrq, EspIrqBackend, EspLeaseGuard, EspLeases, EspMulticoreContract,
    EspP4CsiPlan, EspP4CsiSession, EspP4MediaContract, EspPower, EspPowerBackend, EspProviderError,
    EspPulse, EspPulseBackend, EspPwm, EspPwmBackend, EspReset, EspResetBackend,
    EspRuntimeContract, EspSilicon, EspSpi, ESP32C3_RUNTIME, ESP32P4_PICO_IO_ROUTES,
    ESP32P4_PICO_MEDIA, ESP32P4_RUNTIME, ESP32S3_RUNTIME, ESP32_BOARD_IO_ROUTES, ESP32_RUNTIME,
};
pub use isolation::{
    IsolationAccess, IsolationArchitecture, IsolationCapabilities, IsolationEpoch, IsolationError,
    IsolationPlan, IsolationReceipt, IsolationRegion, IsolationRegionRole, IsolationState,
    MAX_ISOLATION_REGIONS,
};
pub use lease::{LeaseError, LeaseGuard, LeaseRecoveryReceipt, Resource, ResourceLease};
pub use mpu::hardware_isolation_capabilities;
#[cfg(feature = "platform-nrf52840")]
pub use platform::nrf52840::NrfSchedulingSession;
#[cfg(feature = "platform-nrf52840")]
pub use platform::ActivePlatform;
pub use rp2::{
    Rp2Adc, Rp2AdcBackend, Rp2Alarm, Rp2AlarmBackend, Rp2ByteIo, Rp2ByteIoBackend, Rp2Cache,
    Rp2CacheBackend, Rp2ContractError, Rp2Cyw43Backend, Rp2Cyw43Contract, Rp2DmaPlan, Rp2Flash,
    Rp2FlashBackend, Rp2I2c, Rp2LeaseGuard, Rp2Leases, Rp2MulticoreContract, Rp2PioPlan, Rp2Power,
    Rp2PowerBackend, Rp2Pulse, Rp2PulseBackend, Rp2Pwm, Rp2PwmBackend, Rp2Reset, Rp2ResetBackend,
    Rp2Resource, Rp2Rtc, Rp2RtcBackend, Rp2RuntimeContract, Rp2Silicon, Rp2Spi, Rp2Watchdog,
    Rp2WatchdogBackend, RP2040_RUNTIME, RP2350_RUNTIME,
};
pub use snapshots::{
    BoardPackageReport, BoardParity, BoardProfileReport, EventCaptureSnapshot, PwmSnapshot,
    BOARD_PACKAGE_REPORT_MAGIC, BOARD_PACKAGE_REPORT_VERSION, BOARD_PROFILE_REPORT_MAGIC,
    BOARD_PROFILE_REPORT_VERSION, OPTIONAL_PIN_ABSENT,
};
pub use traits::{
    CapabilityDeclarationState, CapabilityProfileKind, HalAdcChannel, HalAlarm, HalBus, HalByteIo,
    HalClock, HalCompatibility, HalDeadline, HalEventCapture, HalI2c, HalLease, HalPower,
    HalPwmChannel, HalReset, HalSchedulingProvider, HalServoPwm, HalSpi, HalTimebaseProvider,
    HardwareCapability, HardwareCapabilityDeclaration, HardwareCapabilitySet,
    HardwareCapabilityWitness, IdleMode, LeaseClass, LeaseId, PlatformHal, TransferMode,
    HARDWARE_CAPABILITY_CONTRACT_VERSION, HARDWARE_CAPABILITY_COUNT,
};

#[cfg(feature = "platform-nrf52840")]
pub use board::{Board, ACTIVE_BOARD_PACKAGE, I2C_SCL_PIN, I2C_SDA_PIN};
#[cfg(feature = "platform-nrf52840")]
pub use bus::{BusError, TwimBus, TWIM0_BASE, TWIM1_BASE};
#[cfg(all(feature = "cortex-m-slice", target_has_atomic = "32"))]
pub use context_switch::{ContextRecord, ContextSwitchError, CortexMSliceSwitch};
#[cfg(all(feature = "cortex-m0-slice", not(target_has_atomic = "32")))]
pub use context_switch_m0::{CortexM0ContextRecord, CortexM0SliceSwitch, CortexM0SwitchError};
#[cfg(feature = "platform-nrf52840")]
pub use deadline_timer::DeadlineTimer;
#[cfg(feature = "platform-nrf52840")]
pub use nrf_peripherals::{
    absolute_pin as nrf_absolute_pin, NrfCpuPower, NrfGpioPort, NrfGpioteInput, NrfNvmc,
    NrfPeripheralError, NrfPulseCapture, NrfReset, NrfResetCause, NrfRtc2, NrfSaadc, NrfUarte0,
    NrfWatchdog, APP_STORAGE_END, APP_STORAGE_START, FLASH_PAGE_SIZE,
};
#[cfg(feature = "platform-nrf52840-rt")]
pub use power_nrf::NrfTimerPower;
#[cfg(feature = "platform-nrf52840")]
pub use ppi::{PpiWake, PpiWakeError, PpiWakeRoute};
#[cfg(feature = "platform-nrf52840")]
pub use priority_ceiling::{
    CompletionInterruptPriority, CompletionInterruptPriorityError, PriorityCeiling,
    PriorityCeilingError,
};
#[cfg(feature = "platform-nrf52840")]
pub use pwm::{PwmBank, PwmBankSession, PwmServo, PwmSession, SERVO_PIN};
#[cfg(feature = "platform-nrf52840")]
pub use radio_hw::{Radio, RadioError, RadioSession};
#[cfg(feature = "platform-nrf52840")]
pub use radio_sim::RadioRxSim;
#[cfg(feature = "platform-nrf52840")]
pub use spim_hw::Spim0;
#[cfg(feature = "platform-nrf52840")]
pub use timer::MicroTimer;
#[cfg(feature = "platform-nrf52840")]
pub use twim_hw::{Twim0, TwimFrequency};
#[cfg(feature = "nrf-twim-async")]
pub use twim_hw::{TwimTransfer, TWIM_XFER_MAX};

/// Event-capture snapshot produced by the nRF52840 PPI provider.
#[cfg(feature = "platform-nrf52840")]
pub type PpiRadioSnapshot = EventCaptureSnapshot;
