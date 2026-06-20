//! Board-level constants portable across MCUs.

/// Static board description selected at compile time via Cargo features.
pub trait BoardDesc {
    /// SoC / HAL backend id, e.g. `"nrf52840"`, `"rp2040"`, `"esp32s3"`.
    const PLATFORM_ID: &'static str;
    /// Board package id, e.g. `"promicro_nrf52840_nosd"`.
    const BOARD_ID: &'static str;
    /// Application image flash origin (after bootloader / partition table).
    const APP_FLASH_START: u32;
    /// Default NobroRTOS software budget for this board family.
    const CAPACITY: BoardCapacity;
    const LED_PIN: u8;
    const SERVO_PWM_PIN: u8;
    const SERVO_CENTER_US: u32;
    const MVK_TRIGGER_PIN: u8;
}

/// Board-class budget used before firmware touches hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardCapacity {
    pub flash_budget_bytes: u32,
    pub ram_budget_bytes: u32,
    pub sample_pool_slots: u16,
    pub max_modules: usize,
}

impl BoardCapacity {
    pub const fn new(
        flash_budget_bytes: u32,
        ram_budget_bytes: u32,
        sample_pool_slots: u16,
        max_modules: usize,
    ) -> Self {
        Self {
            flash_budget_bytes,
            ram_budget_bytes,
            sample_pool_slots,
            max_modules,
        }
    }

    pub const fn is_usable(self) -> bool {
        self.flash_budget_bytes != 0
            && self.ram_budget_bytes != 0
            && self.sample_pool_slots != 0
            && self.max_modules != 0
    }
}

/// Bootloader and memory layout selected by the active board package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootLayout {
    NoSoftDevice,
    SoftDeviceS140V6,
    Custom,
}

/// Static memory regions that an app image may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootProfile {
    pub layout: BootLayout,
    pub app_flash_start: u32,
    pub app_flash_len_bytes: u32,
    pub ram_start: u32,
    pub ram_len_bytes: u32,
}

impl BootProfile {
    pub const fn new(
        layout: BootLayout,
        app_flash_start: u32,
        app_flash_len_bytes: u32,
        ram_start: u32,
        ram_len_bytes: u32,
    ) -> Self {
        Self {
            layout,
            app_flash_start,
            app_flash_len_bytes,
            ram_start,
            ram_len_bytes,
        }
    }
}

impl BootLayout {
    pub const fn code(self) -> u32 {
        match self {
            Self::NoSoftDevice => 1,
            Self::SoftDeviceS140V6 => 2,
            Self::Custom => 255,
        }
    }
}

/// Board pins that are critical to bring-up and diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardPins {
    pub led_pin: u8,
    pub servo_pwm_pin: u8,
    pub mvk_trigger_pin: u8,
}

impl BoardPins {
    pub const fn new(led_pin: u8, servo_pwm_pin: u8, mvk_trigger_pin: u8) -> Self {
        Self {
            led_pin,
            servo_pwm_pin,
            mvk_trigger_pin,
        }
    }
}

/// Complete board-package contract used for host review and port bring-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardPackage {
    pub platform_id: &'static str,
    pub board_id: &'static str,
    pub boot: BootProfile,
    pub capacity: BoardCapacity,
    pub pins: BoardPins,
}

impl BoardPackage {
    pub const fn new(
        platform_id: &'static str,
        board_id: &'static str,
        boot: BootProfile,
        capacity: BoardCapacity,
        pins: BoardPins,
    ) -> Self {
        Self {
            platform_id,
            board_id,
            boot,
            capacity,
            pins,
        }
    }

    pub fn from_board<B: BoardDesc>(
        layout: BootLayout,
        app_flash_len_bytes: u32,
        ram_start: u32,
        ram_len_bytes: u32,
    ) -> Self {
        Self::new(
            B::PLATFORM_ID,
            B::BOARD_ID,
            BootProfile::new(
                layout,
                B::APP_FLASH_START,
                app_flash_len_bytes,
                ram_start,
                ram_len_bytes,
            ),
            B::CAPACITY,
            BoardPins::new(B::LED_PIN, B::SERVO_PWM_PIN, B::MVK_TRIGGER_PIN),
        )
    }

    pub fn validate(&self) -> Result<(), BoardPackageError> {
        if self.platform_id.is_empty() {
            return Err(BoardPackageError::EmptyPlatformId);
        }
        if self.board_id.is_empty() {
            return Err(BoardPackageError::EmptyBoardId);
        }
        if self.boot.app_flash_start & 0xFFF != 0 {
            return Err(BoardPackageError::UnalignedFlashOrigin);
        }
        if self.boot.app_flash_len_bytes == 0 {
            return Err(BoardPackageError::EmptyFlashRegion);
        }
        if self.boot.ram_len_bytes == 0 {
            return Err(BoardPackageError::EmptyRamRegion);
        }
        if !self.capacity.is_usable() {
            return Err(BoardPackageError::EmptyCapacity);
        }
        if self.pins.led_pin == self.pins.servo_pwm_pin
            || self.pins.led_pin == self.pins.mvk_trigger_pin
            || self.pins.servo_pwm_pin == self.pins.mvk_trigger_pin
        {
            return Err(BoardPackageError::DuplicateCriticalPin);
        }
        Ok(())
    }

    pub const fn app_flash_end(&self) -> u32 {
        self.boot.app_flash_start + self.boot.app_flash_len_bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardPackageError {
    EmptyPlatformId,
    EmptyBoardId,
    UnalignedFlashOrigin,
    EmptyFlashRegion,
    EmptyRamRegion,
    EmptyCapacity,
    DuplicateCriticalPin,
}

impl BoardPackageError {
    pub const fn code(self) -> u32 {
        match self {
            Self::EmptyPlatformId => 1,
            Self::EmptyBoardId => 2,
            Self::UnalignedFlashOrigin => 3,
            Self::EmptyFlashRegion => 4,
            Self::EmptyRamRegion => 5,
            Self::EmptyCapacity => 6,
            Self::DuplicateCriticalPin => 7,
        }
    }
}

/// Expected servo PWM profile for self-test / Arduino parity (scene D).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServoProfile {
    pub frequency_hz: u32,
    pub center_pulse_us: u32,
    pub pin: u8,
}

impl ServoProfile {
    pub const fn new(frequency_hz: u32, center_pulse_us: u32, pin: u8) -> Self {
        Self {
            frequency_hz,
            center_pulse_us,
            pin,
        }
    }
}

/// Bus peripheral base addresses exposed for parity checks (platform-specific layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusLayout {
    pub twim0_base: u32,
    pub twim1_base: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestBoard;

    impl BoardDesc for TestBoard {
        const PLATFORM_ID: &'static str = "test-platform";
        const BOARD_ID: &'static str = "test-board";
        const APP_FLASH_START: u32 = 0x1000;
        const CAPACITY: BoardCapacity = BoardCapacity::new(64 * 1024, 16 * 1024, 4, 8);
        const LED_PIN: u8 = 1;
        const SERVO_PWM_PIN: u8 = 2;
        const SERVO_CENTER_US: u32 = 1500;
        const MVK_TRIGGER_PIN: u8 = 3;
    }

    #[test]
    fn board_package_from_board_validates_static_contract() {
        let package = BoardPackage::from_board::<TestBoard>(
            BootLayout::NoSoftDevice,
            60 * 1024,
            0x2000_0000,
            16 * 1024,
        );

        assert_eq!(package.platform_id, "test-platform");
        assert_eq!(package.board_id, "test-board");
        assert_eq!(package.boot.layout, BootLayout::NoSoftDevice);
        assert_eq!(package.app_flash_end(), 0x1000 + 60 * 1024);
        assert_eq!(package.pins, BoardPins::new(1, 2, 3));
        assert_eq!(package.validate(), Ok(()));
    }

    #[test]
    fn board_package_rejects_invalid_contracts() {
        let mut package = BoardPackage::from_board::<TestBoard>(
            BootLayout::NoSoftDevice,
            60 * 1024,
            0x2000_0000,
            16 * 1024,
        );

        package.boot.app_flash_start = 0x1800;
        assert_eq!(
            package.validate(),
            Err(BoardPackageError::UnalignedFlashOrigin)
        );

        package.boot.app_flash_start = TestBoard::APP_FLASH_START;
        package.capacity = BoardCapacity::new(0, 16 * 1024, 4, 8);
        assert_eq!(package.validate(), Err(BoardPackageError::EmptyCapacity));

        package.capacity = TestBoard::CAPACITY;
        package.pins.servo_pwm_pin = package.pins.led_pin;
        assert_eq!(
            package.validate(),
            Err(BoardPackageError::DuplicateCriticalPin)
        );
    }
}
