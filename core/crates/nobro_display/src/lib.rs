//! Bounded display-provider contracts.
//!
//! Pixel generation, panels, buses, and render strategies stay behind the
//! provider. Applications submit bounded regions without owning a framebuffer
//! or a vendor display object.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Mono1,
    Gray4,
    Rgb565,
    Rgb666,
    Rgb888,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayState {
    Down,
    Ready,
    Busy,
    Suspended,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayError {
    InvalidConfig,
    InvalidRegion,
    InvalidPayload,
    NotReady,
    Backpressured,
    DeadlineMiss,
    Cancelled,
    Transport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayLimits {
    pub width: u16,
    pub height: u16,
    pub format: PixelFormat,
    pub max_transfer_bytes: u32,
    pub max_pending: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Region {
    pub const fn is_valid(self, limits: DisplayLimits) -> bool {
        self.width > 0
            && self.height > 0
            && self.x < limits.width
            && self.y < limits.height
            && (self.x as u32 + self.width as u32) <= limits.width as u32
            && (self.y as u32 + self.height as u32) <= limits.height as u32
    }
}

pub trait DisplayBackend {
    type Receipt: Copy;

    fn state(&self) -> DisplayState;
    fn limits(&self) -> DisplayLimits;
    fn pending(&self) -> u8;
    fn submit(
        &mut self,
        region: Region,
        pixels: &[u8],
        deadline_us: u64,
    ) -> Result<Self::Receipt, DisplayError>;
    fn cancel(&mut self, receipt: Self::Receipt) -> Result<(), DisplayError>;
    fn quiesce(&mut self) -> Result<(), DisplayError>;
    fn recover(&mut self) -> Result<(), DisplayError>;
    fn release(&mut self) -> Result<(), DisplayError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_bounds_do_not_wrap() {
        let limits = DisplayLimits {
            width: 240,
            height: 320,
            format: PixelFormat::Rgb565,
            max_transfer_bytes: 4_096,
            max_pending: 2,
        };
        assert!(Region {
            x: 200,
            y: 300,
            width: 40,
            height: 20,
        }
        .is_valid(limits));
        assert!(!Region {
            x: u16::MAX,
            y: 0,
            width: 2,
            height: 1,
        }
        .is_valid(limits));
    }
}
