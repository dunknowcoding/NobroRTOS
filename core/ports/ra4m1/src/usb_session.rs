//! Pure native-USB session state shared by the RA4M1 target and host tests.
//!
//! Neither a line parser nor a partially transmitted report may survive a detach. The
//! target keeps the actual controller behind `Ra4m1Usb`; this module owns only bounded,
//! allocation-free protocol state so reconnect behavior can be tested on the host.

use nobro_usb::{UsbIoError, CDC_PACKET_SIZE};

/// Resume one bounded report across endpoint backpressure and short writes.
pub struct UsbReportCursor {
    offset: usize,
    active: bool,
}

impl UsbReportCursor {
    pub const fn new() -> Self {
        Self {
            offset: 0,
            active: false,
        }
    }

    pub fn pending(&self) -> bool {
        self.active
    }

    pub fn reset(&mut self) {
        self.offset = 0;
        self.active = false;
    }

    /// Attempt at most one CDC packet.
    ///
    /// `configured = false` is a session boundary and discards every old cursor byte.
    /// On reconnect, the next call starts at the report prefix rather than leaking an
    /// orphaned suffix from the prior host session.
    pub fn service(
        &mut self,
        configured: bool,
        report: &[u8],
        write: impl FnOnce(&[u8]) -> Result<(), UsbIoError>,
    ) {
        if !configured || report.is_empty() {
            self.reset();
            return;
        }
        if self.offset >= report.len() {
            self.reset();
        }
        self.active = true;

        let end = self
            .offset
            .saturating_add(CDC_PACKET_SIZE)
            .min(report.len());
        let packet = &report[self.offset..end];
        match write(packet) {
            Ok(()) => self.offset = end,
            Err(UsbIoError::ShortWrite { accepted, .. }) if accepted < packet.len() => {
                self.offset = self.offset.saturating_add(accepted);
            }
            Err(UsbIoError::Backpressure) => return,
            // NotConfigured is a detach race. Oversize/invalid counts and backend
            // failures violate or invalidate this report transaction; all restart from
            // the prefix rather than claiming a suffix is pending.
            Err(
                UsbIoError::NotConfigured
                | UsbIoError::Oversize { .. }
                | UsbIoError::ShortWrite { .. }
                | UsbIoError::InvalidWriteCount { .. }
                | UsbIoError::Backend(_),
            ) => {
                self.reset();
                return;
            }
            // `UsbIoError` is intentionally non-exhaustive. A future failure kind must
            // fail closed instead of preserving a suffix whose delivery is uncertain.
            Err(_) => {
                self.reset();
                return;
            }
        }
        if self.offset == report.len() {
            self.reset();
        }
    }
}

impl Default for UsbReportCursor {
    fn default() -> Self {
        Self::new()
    }
}

/// Parser for the one native-USB command accepted by the RA port.
pub struct HostCommand {
    matched: usize,
    connected: bool,
}

impl HostCommand {
    const ENTER_BOOTLOADER: &'static [u8] = b"NOBRO_BOOT";

    pub const fn new() -> Self {
        Self {
            matched: 0,
            connected: false,
        }
    }

    /// Record the current CDC session. A detach always destroys a partial command.
    pub fn observe_link(&mut self, configured: bool) {
        if !configured {
            self.matched = 0;
        }
        self.connected = configured;
    }

    pub fn push(&mut self, byte: u8) -> bool {
        if !self.connected {
            return false;
        }
        if byte == Self::ENTER_BOOTLOADER[self.matched] {
            self.matched += 1;
            if self.matched == Self::ENTER_BOOTLOADER.len() {
                self.matched = 0;
                return true;
            }
        } else if byte == b'\r' || byte == b'\n' {
            self.matched = 0;
        } else {
            self.matched = usize::from(byte == Self::ENTER_BOOTLOADER[0]);
        }
        false
    }
}

impl Default for HostCommand {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{HostCommand, UsbReportCursor};
    use nobro_usb::{UsbBackendError, UsbIoError};

    #[test]
    fn report_cursor_resumes_partial_write_and_preserves_backpressure() {
        let mut report = [0u8; 100];
        for (index, byte) in report.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let mut cursor = UsbReportCursor::new();

        cursor.service(true, &report, |packet| {
            assert_eq!(packet, &report[..64]);
            Err(UsbIoError::ShortWrite {
                requested: 64,
                accepted: 17,
            })
        });
        assert!(cursor.pending());

        cursor.service(true, &report, |packet| {
            assert_eq!(packet, &report[17..81]);
            Err(UsbIoError::Backpressure)
        });
        assert!(cursor.pending());

        cursor.service(true, &report, |packet| {
            assert_eq!(packet, &report[17..81]);
            Ok(())
        });
        cursor.service(true, &report, |packet| {
            assert_eq!(packet, &report[81..]);
            Ok(())
        });
        assert!(!cursor.pending());
    }

    #[test]
    fn detach_discards_report_suffix_and_reconnect_restarts_at_prefix() {
        let report = [0x5a; 80];
        let mut cursor = UsbReportCursor::new();
        cursor.service(true, &report, |_| Ok(()));
        assert!(cursor.pending());

        cursor.service(false, &report, |_| panic!("detached session wrote data"));
        assert!(!cursor.pending());
        cursor.service(true, &report, |packet| {
            assert_eq!(packet, &report[..64]);
            Err(UsbIoError::Backpressure)
        });
        assert!(cursor.pending());
    }

    #[test]
    fn backend_fault_discards_transaction_instead_of_leaking_a_suffix() {
        let report = [0x33; 80];
        let mut cursor = UsbReportCursor::new();
        cursor.service(true, &report, |_| Ok(()));
        assert!(cursor.pending());
        cursor.service(true, &report, |_| {
            Err(UsbIoError::Backend(UsbBackendError::Unavailable))
        });
        assert!(!cursor.pending());
        cursor.service(true, &report, |packet| {
            assert_eq!(packet, &report[..64]);
            Err(UsbIoError::Backpressure)
        });
    }

    #[test]
    fn boot_command_cannot_span_two_usb_sessions() {
        let mut parser = HostCommand::new();
        parser.observe_link(true);
        for &byte in b"NOBRO_" {
            assert!(!parser.push(byte));
        }
        parser.observe_link(false);
        parser.observe_link(true);
        for &byte in b"BOOT" {
            assert!(!parser.push(byte));
        }
        for (index, &byte) in b"NOBRO_BOOT".iter().enumerate() {
            assert_eq!(parser.push(byte), index + 1 == b"NOBRO_BOOT".len());
        }
    }
}
