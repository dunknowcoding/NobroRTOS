//! ESP32-C3/S3 fixed-function USB-Serial-JTAG backend.
//!
//! The peripheral owns its descriptors and enumeration state machine. NobroRTOS owns
//! only the bounded EP1 byte path and link observation. C3 and S3 deliberately use
//! separate Cargo features because their register blocks have different base addresses.
//! A fresh SOF proves a live USB bus; a zero-length IN probe followed by a later EP1 IN
//! token or OUT packet proves that the fixed-function data endpoint is usable. The
//! reset-high `SERIAL_IN_EMPTY` status is never configuration evidence. Neither valid
//! observation is latched forever: bus reset and an eight-millisecond SOF watchdog fail
//! closed.

use crate::{backend_id, CdcState, UsbConfig, UsbStack};

#[cfg(feature = "backend-usb-serial-jtag-esp32c3")]
const BASE: usize = 0x6004_3000;
#[cfg(feature = "backend-usb-serial-jtag-esp32s3")]
const BASE: usize = 0x6003_8000;

const EP1: *mut u32 = BASE as *mut u32;
const EP1_CONF: *mut u32 = (BASE + 0x04) as *mut u32;
const INT_RAW: *mut u32 = (BASE + 0x08) as *mut u32;
const INT_CLR: *mut u32 = (BASE + 0x14) as *mut u32;

const WR_DONE: u32 = 1 << 0;
const IN_EP_DATA_FREE: u32 = 1 << 1;
const OUT_EP_DATA_AVAIL: u32 = 1 << 2;
const SOF_INT: u32 = 1 << 1;
const OUT_RECV_PKT_INT: u32 = 1 << 2;
const SERIAL_IN_EMPTY_INT: u32 = 1 << 3;
const TOKEN_REC_IN_EP1_INT: u32 = 1 << 8;
const BUS_RESET_INT: u32 = 1 << 9;
const CLEAR_ON_POLL_INTS: u32 = SERIAL_IN_EMPTY_INT | TOKEN_REC_IN_EP1_INT;
const CONFIGURATION_EVIDENCE_INTS: u32 = TOKEN_REC_IN_EP1_INT | OUT_RECV_PKT_INT;
const OUT_FIFO_DISCARD_LIMIT: usize = 64;

// ESP32-C3/S3 SYSTIMER unit 0 is a 16 MHz monotonic counter after ROM/platform
// initialization. Both chips place this register block at the same address.
const SYSTIMER_BASE: usize = 0x6002_3000;
const SYSTIMER_UNIT0_OP: *mut u32 = (SYSTIMER_BASE + 0x04) as *mut u32;
const SYSTIMER_UNIT0_VALUE_LO: *const u32 = (SYSTIMER_BASE + 0x44) as *const u32;
const SYSTIMER_VALUE_VALID: u32 = 1 << 29;
const SYSTIMER_UPDATE: u32 = 1 << 30;
const SOF_TIMEOUT_TICKS: u32 = 8 * 16_000;

fn now_ticks() -> Option<u32> {
    unsafe {
        SYSTIMER_UNIT0_OP.write_volatile(SYSTIMER_UPDATE);
        for _ in 0..64 {
            if SYSTIMER_UNIT0_OP.read_volatile() & SYSTIMER_VALUE_VALID != 0 {
                return Some(SYSTIMER_UNIT0_VALUE_LO.read_volatile());
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinkObservation {
    state: CdcState,
    clear_mask: u32,
    arm_probe: bool,
    discard_out: bool,
    accept_out: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinkTracker {
    last_bus_tick: Option<u32>,
    saw_sof: bool,
    probe_armed: bool,
    data_ready: bool,
    state: CdcState,
}

impl LinkTracker {
    const fn new() -> Self {
        Self {
            last_bus_tick: None,
            saw_sof: false,
            probe_armed: false,
            data_ready: false,
            state: CdcState::Disconnected,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn observe(
        &mut self,
        raw: u32,
        now: u32,
        in_fifo_free: bool,
        out_data_available: bool,
    ) -> LinkObservation {
        let reset = raw & BUS_RESET_INT != 0;
        let sof = raw & SOF_INT != 0;
        let stale_out = raw & OUT_RECV_PKT_INT != 0 || out_data_available;
        // SERIAL_IN_EMPTY is a reset-high status on these chips. Clear every sampled
        // instance, but never feed it to the configured-state transition.
        let mut clear_mask = raw & (BUS_RESET_INT | SOF_INT | CLEAR_ON_POLL_INTS);

        if reset {
            self.reset();
            // A reset is proof of an attached host, but not of a configured EP1.
            self.last_bus_tick = Some(now);
            self.state = CdcState::Default;
            // Discard endpoint flags that can predate the new enumeration session.
            clear_mask |= OUT_RECV_PKT_INT;
        }
        if sof {
            self.last_bus_tick = Some(now);
            self.saw_sof = true;
        }

        let fresh = self
            .last_bus_tick
            .is_some_and(|last| now.wrapping_sub(last) <= SOF_TIMEOUT_TICKS);
        if !fresh {
            self.reset();
            if stale_out {
                clear_mask |= raw & OUT_RECV_PKT_INT;
            }
            return LinkObservation {
                state: CdcState::Disconnected,
                clear_mask,
                arm_probe: false,
                discard_out: stale_out,
                accept_out: false,
            };
        }

        // Even when reset and SOF are sampled together, enumeration evidence must come
        // from a later poll. This also prevents reset-coincident endpoint flags from
        // being reinterpreted as a completed probe.
        if reset {
            return LinkObservation {
                state: self.state,
                clear_mask,
                arm_probe: false,
                discard_out: stale_out,
                accept_out: false,
            };
        }

        // Ignore, clear, and physically drain OUT data present before the probe. Merely
        // clearing its interrupt can leave EP1 full forever, so the backend consumes a
        // bounded stale packet before it is allowed to arm.
        if !self.probe_armed && stale_out {
            clear_mask |= raw & OUT_RECV_PKT_INT;
            self.state = CdcState::Default;
            return LinkObservation {
                state: self.state,
                clear_mask,
                arm_probe: false,
                discard_out: true,
                accept_out: false,
            };
        }

        // Ignore and clear every other possible endpoint-evidence bit present before
        // the probe. In particular, a latched TOKEN from ROM/boot code must not become
        // a post-probe completion on the next poll.
        if self.saw_sof && !self.probe_armed {
            if !in_fifo_free {
                self.state = CdcState::Default;
                return LinkObservation {
                    state: self.state,
                    clear_mask,
                    arm_probe: false,
                    discard_out: false,
                    accept_out: false,
                };
            }
            self.probe_armed = true;
            self.state = CdcState::Default;
            return LinkObservation {
                state: self.state,
                clear_mask,
                arm_probe: true,
                discard_out: false,
                accept_out: false,
            };
        }

        let accept_out = self.probe_armed && (raw & OUT_RECV_PKT_INT != 0 || out_data_available);
        if self.probe_armed && (raw & CONFIGURATION_EVIDENCE_INTS != 0 || out_data_available) {
            self.data_ready = true;
        }
        if accept_out {
            clear_mask |= raw & OUT_RECV_PKT_INT;
        }
        self.state = if self.data_ready {
            CdcState::Configured
        } else {
            CdcState::Default
        };
        LinkObservation {
            state: self.state,
            clear_mask,
            arm_probe: false,
            discard_out: false,
            accept_out,
        }
    }

    fn note_out_packet(&mut self) {
        if self.probe_armed {
            self.data_ready = true;
            self.state = CdcState::Configured;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RxPacketGate {
    open: bool,
}

impl RxPacketGate {
    const fn new() -> Self {
        Self { open: false }
    }

    fn begin(&mut self, packet_interrupt: bool) -> bool {
        self.open |= packet_interrupt;
        self.open
    }

    fn finish(&mut self, bytes_remain: bool) {
        self.open = bytes_remain;
    }

    fn reset(&mut self) {
        self.open = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TxTracker {
    pending: bool,
}

impl TxTracker {
    const fn new() -> Self {
        Self { pending: false }
    }

    fn mark_write_after_empty_clear(&mut self) {
        self.pending = true;
    }

    fn observe(&mut self, state: CdcState, serial_empty: bool) {
        if state != CdcState::Configured || (self.pending && serial_empty) {
            self.pending = false;
        }
    }

    fn idle(&self) -> bool {
        !self.pending
    }
}

fn discard_stale_out_fifo() {
    // EP1 is one full-speed 64-byte packet FIFO. If silicon still reports data after
    // this bound, the next pre-probe poll requests another bounded discard instead of
    // entering an unbounded hardware loop or interpreting it as configured evidence.
    for _ in 0..OUT_FIFO_DISCARD_LIMIT {
        if unsafe { EP1_CONF.read_volatile() } & OUT_EP_DATA_AVAIL == 0 {
            break;
        }
        let _ = unsafe { EP1.read_volatile() };
    }
}

pub(crate) struct UsbSerialJtagCdc {
    link: LinkTracker,
    rx_packet: RxPacketGate,
    tx: TxTracker,
}

impl UsbSerialJtagCdc {
    /// Silicon owns the descriptors, so the common request is accepted but ignored.
    pub(crate) fn mount(_cfg: &UsbConfig) -> Self {
        Self {
            link: LinkTracker::new(),
            rx_packet: RxPacketGate::new(),
            tx: TxTracker::new(),
        }
    }
}

impl UsbStack for UsbSerialJtagCdc {
    fn poll(&mut self) -> CdcState {
        let Some(now) = now_ticks() else {
            self.link.reset();
            self.rx_packet.reset();
            self.tx.observe(CdcState::Disconnected, false);
            return CdcState::Disconnected;
        };
        let raw = unsafe { INT_RAW.read_volatile() };
        let ep1_conf = unsafe { EP1_CONF.read_volatile() };
        let in_fifo_free = ep1_conf & IN_EP_DATA_FREE != 0;
        let out_data_available = ep1_conf & OUT_EP_DATA_AVAIL != 0;
        let observation = self
            .link
            .observe(raw, now, in_fifo_free, out_data_available);
        if raw & BUS_RESET_INT != 0 {
            self.rx_packet.reset();
        }
        if observation.clear_mask != 0 {
            unsafe { INT_CLR.write_volatile(observation.clear_mask) };
        }
        if observation.discard_out {
            self.rx_packet.reset();
            discard_stale_out_fifo();
        }
        if observation.accept_out {
            let _ = self.rx_packet.begin(true);
        }
        self.tx
            .observe(observation.state, raw & SERIAL_IN_EMPTY_INT != 0);
        if observation.arm_probe {
            // `observe` arms only while the IN FIFO is free. A ZLP is harmless to a CDC
            // reader and creates a later EP1 token event only after the host has
            // configured the fixed-function data endpoint.
            unsafe { EP1_CONF.write_volatile(WR_DONE) };
        }
        observation.state
    }

    fn write(&mut self, data: &[u8]) -> usize {
        if self.link.state != CdcState::Configured || !self.tx.idle() {
            return 0;
        }
        let mut n = 0;
        for &b in data {
            if unsafe { EP1_CONF.read_volatile() } & IN_EP_DATA_FREE == 0 {
                break;
            }
            unsafe { EP1.write_volatile(u32::from(b)) };
            n += 1;
        }
        if n > 0 {
            unsafe {
                // IN_EP_DATA_FREE means "not full", not "empty". Clear the reset-high
                // completion bit before publishing this batch, then wait for a later
                // SERIAL_IN_EMPTY event before another write or successful flush.
                INT_CLR.write_volatile(SERIAL_IN_EMPTY_INT);
                self.tx.mark_write_after_empty_clear();
                EP1_CONF.write_volatile(WR_DONE);
            }
        }
        n
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        let raw = unsafe { INT_RAW.read_volatile() };
        let packet_interrupt = raw & OUT_RECV_PKT_INT != 0;
        let out_data_available = unsafe { EP1_CONF.read_volatile() } & OUT_EP_DATA_AVAIL != 0;
        let packet_present = packet_interrupt || out_data_available;
        if packet_present && !self.link.probe_armed {
            if packet_interrupt {
                unsafe { INT_CLR.write_volatile(OUT_RECV_PKT_INT) };
            }
            self.rx_packet.reset();
            discard_stale_out_fifo();
            return 0;
        }
        if packet_present {
            if packet_interrupt {
                unsafe { INT_CLR.write_volatile(OUT_RECV_PKT_INT) };
            }
            self.link.note_out_packet();
        }
        if !self.rx_packet.begin(packet_present) {
            return 0;
        }

        let mut n = 0;
        while n < buf.len() && unsafe { EP1_CONF.read_volatile() } & OUT_EP_DATA_AVAIL != 0 {
            buf[n] = unsafe { EP1.read_volatile() } as u8;
            n += 1;
        }
        let bytes_remain = unsafe { EP1_CONF.read_volatile() } & OUT_EP_DATA_AVAIL != 0;
        self.rx_packet.finish(bytes_remain);
        n
    }

    fn flush(&mut self) -> bool {
        self.link.state == CdcState::Configured && self.tx.idle()
    }

    fn configured(&self) -> bool {
        self.link.state == CdcState::Configured
    }

    fn backend_id(&self) -> u32 {
        backend_id::USB_SERIAL_JTAG
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CdcState, LinkTracker, RxPacketGate, TxTracker, BUS_RESET_INT, OUT_RECV_PKT_INT,
        SERIAL_IN_EMPTY_INT, SOF_INT, SOF_TIMEOUT_TICKS, TOKEN_REC_IN_EP1_INT,
    };

    #[test]
    fn reset_and_probe_reject_all_stale_endpoint_flags() {
        let mut link = LinkTracker::new();
        let all_endpoint_flags = SERIAL_IN_EMPTY_INT | TOKEN_REC_IN_EP1_INT | OUT_RECV_PKT_INT;
        let reset = link.observe(BUS_RESET_INT | all_endpoint_flags, 10, true, true);
        assert_eq!(reset.state, CdcState::Default);
        assert!(!reset.arm_probe);
        assert!(reset.discard_out);
        assert_eq!(reset.clear_mask & all_endpoint_flags, all_endpoint_flags);

        let reset_and_sof =
            link.observe(BUS_RESET_INT | SOF_INT | all_endpoint_flags, 11, true, true);
        assert_eq!(reset_and_sof.state, CdcState::Default);
        assert!(!reset_and_sof.arm_probe);
        assert!(reset_and_sof.discard_out);

        let first_sof = link.observe(SOF_INT | all_endpoint_flags, 20, true, true);
        assert_eq!(first_sof.state, CdcState::Default);
        assert!(!first_sof.arm_probe);
        assert!(first_sof.discard_out);
        assert_eq!(
            first_sof.clear_mask & all_endpoint_flags,
            all_endpoint_flags
        );

        let arm = link.observe(0, 21, true, false);
        assert!(arm.arm_probe);
        assert!(!arm.discard_out);

        let empty_only = link.observe(SERIAL_IN_EMPTY_INT, 22, true, false);
        assert_eq!(empty_only.state, CdcState::Default);
        assert!(!empty_only.arm_probe);

        let completion = link.observe(TOKEN_REC_IN_EP1_INT, 23, true, false);
        assert_eq!(completion.state, CdcState::Configured);
    }

    #[test]
    fn probe_waits_for_free_in_fifo() {
        let mut link = LinkTracker::new();
        let busy = link.observe(
            SOF_INT | SERIAL_IN_EMPTY_INT | TOKEN_REC_IN_EP1_INT,
            100,
            false,
            false,
        );
        assert_eq!(busy.state, CdcState::Default);
        assert!(!busy.arm_probe);

        let free = link.observe(0, 101, true, false);
        assert_eq!(free.state, CdcState::Default);
        assert!(free.arm_probe);
        assert_eq!(
            link.observe(SERIAL_IN_EMPTY_INT, 102, true, false).state,
            CdcState::Default
        );
        assert_eq!(
            link.observe(TOKEN_REC_IN_EP1_INT, 103, true, false).state,
            CdcState::Configured
        );
    }

    #[test]
    fn only_post_probe_token_or_out_packet_configures() {
        let mut token_link = LinkTracker::new();
        assert!(token_link.observe(SOF_INT, 100, true, false).arm_probe);
        assert_eq!(
            token_link
                .observe(SERIAL_IN_EMPTY_INT, 101, true, false)
                .state,
            CdcState::Default
        );
        assert_eq!(
            token_link
                .observe(TOKEN_REC_IN_EP1_INT, 102, true, false)
                .state,
            CdcState::Configured
        );

        let mut out_link = LinkTracker::new();
        assert!(out_link.observe(SOF_INT, 200, true, false).arm_probe);
        assert_eq!(
            out_link.observe(OUT_RECV_PKT_INT, 201, true, true).state,
            CdcState::Configured
        );
    }

    #[test]
    fn stale_out_discard_is_requested_only_before_probe_or_on_reset() {
        let mut link = LinkTracker::new();
        let detached_stale = link.observe(OUT_RECV_PKT_INT, 10, true, true);
        assert_eq!(detached_stale.state, CdcState::Disconnected);
        assert!(detached_stale.discard_out);
        assert!(!detached_stale.arm_probe);

        assert!(link.observe(SOF_INT, 20, true, false).arm_probe);
        let post_probe = link.observe(OUT_RECV_PKT_INT, 21, true, true);
        assert_eq!(post_probe.state, CdcState::Configured);
        assert!(!post_probe.discard_out);
        assert!(post_probe.accept_out);

        let reset = link.observe(BUS_RESET_INT | OUT_RECV_PKT_INT, 22, true, true);
        assert_eq!(reset.state, CdcState::Default);
        assert!(reset.discard_out);
        assert!(!reset.accept_out);
    }

    #[test]
    fn tx_completion_requires_empty_event_after_write_is_marked() {
        let mut tx = TxTracker::new();

        // A reset-high/stale empty event before a write cannot complete future data.
        tx.observe(CdcState::Configured, true);
        assert!(tx.idle());
        tx.mark_write_after_empty_clear();
        assert!(!tx.idle());

        tx.observe(CdcState::Configured, false);
        assert!(!tx.idle());
        tx.observe(CdcState::Configured, true);
        assert!(tx.idle());

        tx.mark_write_after_empty_clear();
        tx.observe(CdcState::Disconnected, true);
        assert!(tx.idle());
    }

    #[test]
    fn serial_empty_never_configures_even_with_continuous_sof() {
        let mut link = LinkTracker::new();
        assert!(
            link.observe(SOF_INT | SERIAL_IN_EMPTY_INT, 1_000, true, false)
                .arm_probe
        );
        for tick in 1_001..1_017 {
            let observed = link.observe(SOF_INT | SERIAL_IN_EMPTY_INT, tick, true, false);
            assert_eq!(observed.state, CdcState::Default);
            assert!(!observed.arm_probe);
            assert_ne!(observed.clear_mask & SERIAL_IN_EMPTY_INT, 0);
        }
    }

    #[test]
    fn configured_state_expires_and_reconnect_reprobes() {
        let mut link = LinkTracker::new();
        assert!(link.observe(SOF_INT, 100, true, false).arm_probe);
        assert_eq!(
            link.observe(TOKEN_REC_IN_EP1_INT, 101, true, false).state,
            CdcState::Configured
        );
        assert_eq!(
            link.observe(0, 100 + SOF_TIMEOUT_TICKS, true, false).state,
            CdcState::Configured
        );
        assert_eq!(
            link.observe(0, 101 + SOF_TIMEOUT_TICKS, true, false).state,
            CdcState::Disconnected
        );
        let reconnect = link.observe(SOF_INT, 200 + SOF_TIMEOUT_TICKS, true, false);
        assert_eq!(reconnect.state, CdcState::Default);
        assert!(reconnect.arm_probe);
    }

    #[test]
    fn partial_read_keeps_packet_open_without_a_second_interrupt() {
        let mut gate = RxPacketGate::new();
        assert!(gate.begin(true));
        gate.finish(true); // first 32 bytes of a 64-byte packet were consumed
        assert!(gate.begin(false)); // remaining 32 bytes need no second packet IRQ
        gate.finish(false);
        assert!(!gate.begin(false));
    }

    #[test]
    fn chip_feature_selects_the_expected_register_map() {
        #[cfg(feature = "backend-usb-serial-jtag-esp32c3")]
        assert_eq!(super::BASE, 0x6004_3000);
        #[cfg(feature = "backend-usb-serial-jtag-esp32s3")]
        assert_eq!(super::BASE, 0x6003_8000);
    }
}
