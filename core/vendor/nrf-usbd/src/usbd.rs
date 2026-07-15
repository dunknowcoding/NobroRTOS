//! A `usb-device` implementation using the USBD peripheral.
//!
//! Difficulties:
//! * Control EP 0 is special:
//!   * Setup stage is put in registers, not RAM.
//!   * Different events are used to initiate transfers.
//!   * No notification when the status stage is ACK'd.

use core::cell::{Cell, UnsafeCell};
use core::mem::{offset_of, size_of};
use core::sync::atomic::{compiler_fence, AtomicBool, Ordering};
use critical_section::{CriticalSection, Mutex};
use usb_device::{
    bus::{PollResult, UsbBus},
    endpoint::{EndpointAddress, EndpointType},
    UsbDirection, UsbError,
};

use crate::pac::usbd::RegisterBlock;
use crate::{errata, UsbPeripheral, UsbdFault};

const DMA_BUFFER_SIZE: usize = 64;
// If the peripheral supplies a monotonic microsecond clock, these are elapsed-time
// limits. Otherwise the corresponding POLL_BUDGET is a count of failed calls to the
// driver's non-blocking lifecycle poller, not a wall-clock claim.
const ENABLE_READY_TIMEOUT_US: u32 = 100_000;
const ENABLE_READY_POLL_BUDGET: usize = 6_000_000;
const HFCLK_READY_TIMEOUT_US: u32 = 100_000;
const HFCLK_READY_POLL_BUDGET: usize = 6_000_000;
const POWER_READY_TIMEOUT_US: u32 = 100_000;
const POWER_READY_POLL_BUDGET: usize = 6_000_000;
const WAKE_READY_TIMEOUT_US: u32 = 100_000;
const WAKE_READY_POLL_BUDGET: usize = 6_000_000;
// USB 2.0 section 7.1.7.3 requires a downstream port to observe continuous SE0
// for more than 2.5 us before it recognizes disconnect. Keep the bootloader-owned
// D+ pull-up off for a deliberately conservative interval after ENABLE has read
// back Disabled; an immediate disable/re-enable has no lower bound and can be
// invisible to the host.
const INITIAL_DETACH_TIMEOUT_US: u32 = 20_000;
const INITIAL_DETACH_POLL_BUDGET: usize = 6_000_000;
const DETACH_TIMEOUT_US: u32 = 1_000;
const DETACH_POLL_BUDGET: usize = 6_000_000;
const PARITY_DMA_TIMEOUT_US: u32 = 100_000;
const PARITY_DMA_POLL_BUDGET: usize = 6_000_000;
const HANDOFF_DISABLE_POLL_BUDGET: usize = 100_000;
const DMA_COMPLETE_POLL_BUDGET: usize = 2_048;
const ISO_SPLIT_HALF_IN: u32 = 0x80;

const REGISTER_WORD_BYTES: usize = size_of::<u32>();
const ENDEPIN_EVENT_BASE: usize = offset_of!(RegisterBlock, events_endepin);
const ENDEPOUT_EVENT_BASE: usize = offset_of!(RegisterBlock, events_endepout);

// Every latched event in the nRF52 USBD register block. The offsets are computed from
// the vendored PAC layout so handoff cleanup cannot silently drift to a neighbouring
// register when the raw register map is maintained.
const HANDOFF_EVENT_CLEAR_OFFSETS: [usize; 25] = [
    offset_of!(RegisterBlock, events_usbreset),
    offset_of!(RegisterBlock, events_started),
    ENDEPIN_EVENT_BASE,
    ENDEPIN_EVENT_BASE + REGISTER_WORD_BYTES,
    ENDEPIN_EVENT_BASE + 2 * REGISTER_WORD_BYTES,
    ENDEPIN_EVENT_BASE + 3 * REGISTER_WORD_BYTES,
    ENDEPIN_EVENT_BASE + 4 * REGISTER_WORD_BYTES,
    ENDEPIN_EVENT_BASE + 5 * REGISTER_WORD_BYTES,
    ENDEPIN_EVENT_BASE + 6 * REGISTER_WORD_BYTES,
    ENDEPIN_EVENT_BASE + 7 * REGISTER_WORD_BYTES,
    offset_of!(RegisterBlock, events_ep0datadone),
    offset_of!(RegisterBlock, events_endisoin),
    ENDEPOUT_EVENT_BASE,
    ENDEPOUT_EVENT_BASE + REGISTER_WORD_BYTES,
    ENDEPOUT_EVENT_BASE + 2 * REGISTER_WORD_BYTES,
    ENDEPOUT_EVENT_BASE + 3 * REGISTER_WORD_BYTES,
    ENDEPOUT_EVENT_BASE + 4 * REGISTER_WORD_BYTES,
    ENDEPOUT_EVENT_BASE + 5 * REGISTER_WORD_BYTES,
    ENDEPOUT_EVENT_BASE + 6 * REGISTER_WORD_BYTES,
    ENDEPOUT_EVENT_BASE + 7 * REGISTER_WORD_BYTES,
    offset_of!(RegisterBlock, events_endisoout),
    offset_of!(RegisterBlock, events_sof),
    offset_of!(RegisterBlock, events_usbevent),
    offset_of!(RegisterBlock, events_ep0setup),
    offset_of!(RegisterBlock, events_epdata),
];

// Write-one-to-clear status registers that can otherwise replay a bootloader transfer.
const HANDOFF_W1C_CLEAR_OFFSETS: [usize; 3] = [
    offset_of!(RegisterBlock, eventcause),
    offset_of!(RegisterBlock, epstatus),
    offset_of!(RegisterBlock, epdatastatus),
];

// Persistent routing/configuration owned by the previous USB session. Command-style
// DTOGGLE/EPSTALL registers are intentionally excluded; the authoritative cleanup runs
// after ENABLE/READY, and forced DP/DM drive is released explicitly just before it.
const HANDOFF_CONFIG_ZERO_OFFSETS: [usize; 4] = [
    offset_of!(RegisterBlock, shorts),
    offset_of!(RegisterBlock, epinen),
    offset_of!(RegisterBlock, epouten),
    offset_of!(RegisterBlock, isoinconfig),
];

#[cfg(test)]
const SESSION_SANITIZATION_WRITE_COUNT: usize = HANDOFF_CONFIG_ZERO_OFFSETS.len()
    + HANDOFF_EVENT_CLEAR_OFFSETS.len()
    + HANDOFF_W1C_CLEAR_OFFSETS.len();

fn apply_session_register_sanitization(mut write: impl FnMut(usize, u32)) {
    for offset in HANDOFF_CONFIG_ZERO_OFFSETS {
        write(offset, 0);
    }
    for offset in HANDOFF_EVENT_CLEAR_OFFSETS {
        write(offset, 0);
    }
    for offset in HANDOFF_W1C_CLEAR_OFFSETS {
        write(offset, u32::MAX);
    }
}

fn finish_enable_session(
    mut configure_enabled_errata: impl FnMut(),
    mut write: impl FnMut(usize, u32),
) {
    // A bootloader can leave TASKS_DPDMDRIVE ownership behind. The handoff pass
    // also triggers NODRIVE, but that task is not guaranteed to be accepted while
    // USBD is disabled. Repeat it only after ENABLE/READY and any inherited
    // LOWPOWER exit have both completed.
    write(offset_of!(RegisterBlock, tasks_dpdmnodrive), 1);
    configure_enabled_errata();
    apply_session_register_sanitization(&mut write);
    // Match Nordic's enabled-session baseline even though this fork currently
    // rejects ISO endpoint allocation. Erratum 166 and HalfIN make future ISO IN/OUT
    // sharing correct; neither setting changes the regular EP0/CDC attach path.
    // Write HalfIN after generic cleanup so handoff sanitation cannot erase it.
    write(offset_of!(RegisterBlock, isosplit), ISO_SPLIT_HALF_IN);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnableSessionCompletion {
    Finished,
    WakePending,
}

fn complete_enable_session(
    mut keep_pullup_disabled: impl FnMut(),
    mut clear_ready: impl FnMut(),
    mut complete_errata: impl FnMut(),
    mut inherited_lowpower: impl FnMut() -> bool,
    mut clear_wake_allowed: impl FnMut(),
    mut begin_wake: impl FnMut(),
    mut force_normal: impl FnMut(),
    configure_enabled_errata: impl FnMut(),
    write: impl FnMut(usize, u32),
) -> EnableSessionCompletion {
    // These callbacks deliberately encode the hardware ordering. The session
    // registers are documented as accessible only after ENABLE/READY, while D+
    // must remain detached until both this cleanup and regulator readiness finish.
    keep_pullup_disabled();
    clear_ready();
    complete_errata();
    if inherited_lowpower() {
        // LOWPOWER -> ForceNormal is an acknowledged Erratum 171 transaction.
        // Keep D+ detached and do not release inherited DP/DM drive until the MAC
        // reports USBWUALLOWED and the wake bracket has been closed.
        clear_wake_allowed();
        begin_wake();
        force_normal();
        EnableSessionCompletion::WakePending
    } else {
        finish_enable_session(configure_enabled_errata, write);
        EnableSessionCompletion::Finished
    }
}

fn reset_ep0_transaction_registers(
    mut clear_status_shortcut: impl FnMut(),
    mut clear_data_done_event: impl FnMut(),
) {
    // USBRESET is an abort boundary for the old transfer, so its status shortcut and
    // data-stage completion must not survive. Deliberately preserve EP0SETUP: a host
    // may issue its first post-reset SETUP before usb-device calls `UsbBus::reset`, and
    // discarding that already-latched request would stall enumeration until retry.
    clear_status_shortcut();
    clear_data_done_event();
}

const EVENTCAUSE_SUSPEND_MASK: u32 = 1 << 8;
const EVENTCAUSE_RESUME_MASK: u32 = 1 << 9;

fn acknowledge_usbevent(
    mut clear_event: impl FnMut(),
    mut read_causes: impl FnMut() -> u32,
    mut clear_causes: impl FnMut(u32),
) -> u32 {
    // Re-arm the aggregate event before snapshotting and W1C-clearing its causes.
    // A cause arriving after this write can therefore raise a fresh USBEVENT.
    clear_event();
    let causes = read_causes();
    clear_causes(causes);
    causes
}

fn suspend_resume_poll_result(causes: u32) -> Option<PollResult> {
    // If both causes accumulated before polling, RESUME describes the final link
    // state and must win because the snapshot is cleared as one transaction.
    if causes & EVENTCAUSE_RESUME_MASK != 0 {
        Some(PollResult::Resume)
    } else if causes & EVENTCAUSE_SUSPEND_MASK != 0 {
        Some(PollResult::Suspend)
    } else {
        None
    }
}

/// Result of disconnecting a bootloader-owned USBD session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffSanitization {
    /// Factory identity is not an nRF52840 maintained by this fork. No USBD register
    /// was written, so an unsupported part cannot be damaged by guessed cleanup.
    UnsupportedSilicon,
    /// `ENABLE` read back disabled and the post-disable best-effort cleanup writes
    /// were issued. The newly enabled session repeats those writes after `READY`,
    /// when the controller guarantees that its session registers are accessible.
    /// This result does not claim readback verification of the cleanup writes.
    Complete,
    /// `ENABLE` did not read back disabled within the fallback observation budget.
    /// No endpoint, event, or configuration registers were cleared in this state.
    DisableTimeout,
}

fn sanitize_if_supported(
    applicable: Option<errata::Applicability>,
    sanitize: impl FnOnce(errata::Applicability) -> HandoffSanitization,
) -> HandoffSanitization {
    match applicable {
        Some(applicable) => sanitize(applicable),
        None => HandoffSanitization::UnsupportedSilicon,
    }
}

fn apply_handoff_sanitization(
    mut write: impl FnMut(usize, u32),
    mut enable_is_disabled: impl FnMut() -> bool,
    mut after_disable: impl FnMut(),
) -> HandoffSanitization {
    // Disconnect first, then prevent a stale peripheral or NVIC source from observing
    // partially sanitized state. The board layer separately masks/unpends the NVIC line.
    write(offset_of!(RegisterBlock, usbpullup), 0);
    write(offset_of!(RegisterBlock, intenclr), u32::MAX);
    write(offset_of!(RegisterBlock, tasks_dpdmnodrive), 1);
    write(offset_of!(RegisterBlock, enable), 0);

    if !poll_with_budget(HANDOFF_DISABLE_POLL_BUDGET, &mut enable_is_disabled) {
        return HandoffSanitization::DisableTimeout;
    }

    // The shared Erratum 187/211 flag remains owned until hardware confirms
    // disable. Only then is it safe to close that active-lifetime workaround.
    after_disable();

    // Nordic does not guarantee access to these registers while USBD is disabled,
    // but retain this pass as a best-effort cleanup for implementations that accept
    // it. `complete_enable_session` repeats the exact pass after ENABLE/READY and
    // before pull-up, which is the authoritative session boundary.
    apply_session_register_sanitization(write);
    HandoffSanitization::Complete
}

/// Disconnects and disables a bootloader-owned nRF52 USBD session, then issues a
/// best-effort post-disable cleanup pass.
///
/// The driver repeats the same session-register cleanup after the next successful
/// `ENABLE`/`READY` transition and retriggers `TASKS_DPDMNODRIVE` before connecting D+,
/// because those registers and tasks are guaranteed to be accessible only while USBD
/// is enabled.
///
/// # Safety
///
/// `T::REGISTERS` must identify the exclusive live nRF52 USBD register block, and the
/// caller must prevent concurrent access while this handoff transaction runs. The
/// factory-identity gate runs before this pointer is dereferenced.
#[doc(hidden)]
pub unsafe fn sanitize_handoff<T: UsbPeripheral>() -> HandoffSanitization {
    let base = T::REGISTERS.cast::<u8>();
    sanitize_if_supported(errata::detect(), |applicable| {
        apply_handoff_sanitization(
            |offset, value| unsafe {
                base.add(offset)
                    .cast_mut()
                    .cast::<u32>()
                    .write_volatile(value);
            },
            || unsafe {
                base.add(offset_of!(RegisterBlock, enable))
                    .cast::<u32>()
                    .read_volatile()
                    == 0
            },
            || errata::end_disabled(applicable),
        )
    })
}

#[repr(align(4))]
struct StaticDmaBuffer(UnsafeCell<[u8; DMA_BUFFER_SIZE]>);

// Safety: all accesses occur inside the global critical section. USBD is a singleton
// peripheral, and a fatal timeout permanently prevents this driver instance from issuing
// another transfer. Static storage also remains valid if timed-out EasyDMA finishes late.
unsafe impl Sync for StaticDmaBuffer {}

static DMA_IN_BUFFER: StaticDmaBuffer = StaticDmaBuffer(UnsafeCell::new([0; DMA_BUFFER_SIZE]));
static DMA_OUT_BUFFER: StaticDmaBuffer = StaticDmaBuffer(UnsafeCell::new([0; DMA_BUFFER_SIZE]));

struct PermanentClaim(AtomicBool);

impl PermanentClaim {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn try_claim(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

// Permanent by design: after a timeout hardware may finish against the static buffers
// later, so no subsequent bus instance may ever reuse them in the same process.
static DMA_STORAGE_CLAIM: PermanentClaim = PermanentClaim::new();
static BOOTLOADER_HANDOFF_REQUEST: AtomicBool = AtomicBool::new(false);

/// Marks the next `UsbBus::force_reset()` call as a one-way bootloader handoff.
///
/// Unlike ordinary re-enumeration, that call remains `WouldBlock` until EasyDMA
/// parity repair, controller disable readback, and errata release have completed.
pub fn request_bootloader_handoff() {
    BOOTLOADER_HANDOFF_REQUEST.store(true, Ordering::Release);
}

fn initial_fault_for_silicon(
    applicability: Option<errata::Applicability>,
    try_claim_storage: impl FnOnce() -> bool,
) -> Option<UsbdFault> {
    if applicability.is_none() {
        return Some(UsbdFault::UnsupportedSilicon);
    }
    if try_claim_storage() {
        None
    } else {
        Some(UsbdFault::DmaStorageAlreadyClaimed)
    }
}

#[derive(Clone, Copy)]
struct TerminalFault(Option<UsbdFault>);

impl TerminalFault {
    const fn new(fault: Option<UsbdFault>) -> Self {
        Self(fault)
    }

    fn permits_operation(self) -> bool {
        self.0.is_none()
    }

    fn fault(self) -> Option<UsbdFault> {
        self.0
    }

    fn latch(self, fault: UsbdFault) -> (Self, bool) {
        if self.permits_operation() {
            (Self(Some(fault)), true)
        } else {
            (self, false)
        }
    }
}

fn dma_start(applicable: errata::Applicability) {
    errata::begin_dma(applicable);
    compiler_fence(Ordering::Release);
}

fn dma_end(applicable: errata::Applicability) {
    compiler_fence(Ordering::Acquire);
    errata::complete_dma(applicable);
}

fn accumulated_dma_odd(currently_odd: bool, amount: u8) -> bool {
    currently_odd ^ (amount & 1 != 0)
}

fn poll_with_budget(limit: usize, mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..limit {
        if ready() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AsyncWait {
    started_us: Option<u32>,
    last_us: Option<u32>,
    timeout_us: u32,
    polls_remaining: usize,
    poll_budget: usize,
}

impl AsyncWait {
    fn new(now_us: Option<u32>, timeout_us: u32, poll_budget: usize) -> Self {
        Self {
            started_us: now_us,
            last_us: now_us,
            timeout_us,
            polls_remaining: poll_budget,
            poll_budget,
        }
    }

    /// Records one failed readiness observation and reports expiration.
    ///
    /// A progressing clock is authoritative and the fallback budget is refreshed;
    /// this prevents a tight poll loop from expiring before the promised wall time.
    /// A missing or frozen clock still consumes the conservative poll fallback.
    /// With no clock at construction, a later clock is intentionally ignored because
    /// there is no elapsed origin.
    fn failed_observation(&mut self, now_us: Option<u32>) -> bool {
        let elapsed_expired = matches!(
            (self.started_us, now_us),
            (Some(started), Some(now)) if now.wrapping_sub(started) >= self.timeout_us
        );
        if elapsed_expired {
            return true;
        }

        // A progressing clock is authoritative: do not let a fast caller consume
        // the fallback count before the promised wall time has elapsed. Reset the
        // fallback only on observed progress. A missing or frozen clock still expires
        // after a deliberately conservative number of failed observations.
        if self.started_us.is_some() && now_us.is_some() && now_us != self.last_us {
            self.last_us = now_us;
            self.polls_remaining = self.poll_budget;
            return false;
        }
        self.polls_remaining = self.polls_remaining.saturating_sub(1);
        self.polls_remaining == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitialDetachPreparation {
    AwaitVbus,
    Disabling(AsyncWait),
    Dwelling(AsyncWait),
}

fn prepare_initial_detach(
    mut keep_pullup_disabled: impl FnMut(),
    mut enable_is_disabled: impl FnMut() -> bool,
    mut request_disable: impl FnMut(),
    vbus_present: bool,
    mut now_us: impl FnMut() -> Option<u32>,
) -> InitialDetachPreparation {
    // Disconnect is the first externally observable action. ENABLE readback is
    // then the ownership boundary: the dwell must never start while the old
    // controller session can still be driving D+/D-.
    keep_pullup_disabled();
    if !enable_is_disabled() {
        request_disable();
        InitialDetachPreparation::Disabling(AsyncWait::new(
            now_us(),
            ENABLE_READY_TIMEOUT_US,
            ENABLE_READY_POLL_BUDGET,
        ))
    } else if vbus_present {
        InitialDetachPreparation::Dwelling(AsyncWait::new(
            now_us(),
            INITIAL_DETACH_TIMEOUT_US,
            INITIAL_DETACH_POLL_BUDGET,
        ))
    } else {
        // Physical VBUS absence already supplies an unbounded disconnect. A new
        // VBUS edge can therefore proceed through the ordinary enable path.
        InitialDetachPreparation::AwaitVbus
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitialDetachObservation {
    Waiting,
    StartEnable,
    VbusLost,
}

fn observe_initial_detach(
    wait: &mut AsyncWait,
    vbus_present: bool,
    now_us: Option<u32>,
) -> InitialDetachObservation {
    // VBUS loss wins even on the nominal last dwell observation: a physical
    // disconnect must not be followed by an ENABLE request until VBUS returns.
    if !vbus_present {
        InitialDetachObservation::VbusLost
    } else if wait.failed_observation(now_us) {
        InitialDetachObservation::StartEnable
    } else {
        InitialDetachObservation::Waiting
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrataOwnership {
    Enabling(errata::Applicability),
    Active(errata::Applicability),
    Waking(errata::Applicability),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisablePreparation {
    StartDisable,
    WaitForPower,
    AwaitWake,
    BeginWake,
    StartParityDma,
    UnsafeLifecycle,
}

fn disable_preparation(
    dma_odd: bool,
    ownership: Option<ErrataOwnership>,
    vbus_present: bool,
    power_ready: bool,
    lowpower: bool,
) -> DisablePreparation {
    if !dma_odd {
        return DisablePreparation::StartDisable;
    }
    if !vbus_present || !power_ready {
        return DisablePreparation::WaitForPower;
    }
    match ownership {
        Some(ErrataOwnership::Waking(_)) => DisablePreparation::AwaitWake,
        Some(ErrataOwnership::Active(_)) if lowpower => DisablePreparation::BeginWake,
        Some(ErrataOwnership::Active(_)) => DisablePreparation::StartParityDma,
        // An odd cumulative DMA count can only be produced by the Active state.
        // Never guess that an unacknowledged enable phase is safe for EasyDMA.
        Some(ErrataOwnership::Enabling(_)) | None => DisablePreparation::UnsafeLifecycle,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WakeTarget {
    Active,
    FinishEnable,
}

fn complete_wake_session(
    target: WakeTarget,
    mut clear_wake_allowed: impl FnMut(),
    mut complete_wake: impl FnMut(),
    configure_enabled_errata: impl FnMut(),
    write: impl FnMut(usize, u32),
) -> WakeTarget {
    clear_wake_allowed();
    complete_wake();
    if matches!(target, WakeTarget::FinishEnable) {
        finish_enable_session(configure_enabled_errata, write);
    }
    target
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    AwaitVbus,
    InitialDisabling {
        wait: AsyncWait,
        errata: errata::Applicability,
    },
    InitialDetach {
        wait: AsyncWait,
    },
    PreparingDisable {
        errata: Option<ErrataOwnership>,
        pending_fault: Option<UsbdFault>,
    },
    DisableWaking {
        wait: AsyncWait,
        errata: errata::Applicability,
        pending_fault: Option<UsbdFault>,
    },
    ParityFixing {
        wait: AsyncWait,
        errata: errata::Applicability,
        pending_fault: Option<UsbdFault>,
    },
    Disabling {
        wait: AsyncWait,
        errata: Option<ErrataOwnership>,
        pending_fault: Option<UsbdFault>,
    },
    HfclkStarting {
        wait: AsyncWait,
        errata: errata::Applicability,
    },
    Enabling {
        wait: AsyncWait,
        errata: errata::Applicability,
    },
    PowerReady {
        wait: AsyncWait,
        errata: errata::Applicability,
    },
    Active {
        errata: errata::Applicability,
    },
    Suspended {
        errata: errata::Applicability,
    },
    Waking {
        wait: AsyncWait,
        errata: errata::Applicability,
        target: WakeTarget,
    },
    Detaching {
        wait: AsyncWait,
        errata: errata::Applicability,
    },
    HandoffComplete,
}

#[cfg(test)]
impl Lifecycle {
    fn accepts_full_usb_events(self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

fn release_ep0_after_status_fallback(busy_in_endpoints: u16) -> u16 {
    busy_in_endpoints & !1
}

fn validate_endpoint_request(
    ep_type: EndpointType,
    max_packet_size: u16,
) -> usb_device::Result<()> {
    if matches!(ep_type, EndpointType::Isochronous { .. }) {
        // This driver only implements the regular EP0..EP7 register path. Advertising
        // ISO endpoint 8 would make read/write index the shorter regular-endpoint arrays.
        return Err(UsbError::Unsupported);
    }
    if usize::from(max_packet_size) > DMA_BUFFER_SIZE {
        return Err(UsbError::EndpointMemoryOverflow);
    }
    Ok(())
}

fn endpoint_is_owned(used_in: u8, used_out: u8, ep: EndpointAddress) -> bool {
    let index = ep.index();
    if index >= 8 {
        return false;
    }
    let used = if ep.is_in() { used_in } else { used_out };
    used & (1 << index) != 0
}

fn link_restore_allowed(faulted: bool, vbus_present: bool) -> bool {
    !faulted && vbus_present
}

fn publish_out_transfer(
    completed_dma_buf: Option<&[u8]>,
    caller_buf: &mut [u8],
    count: usize,
) -> usb_device::Result<usize> {
    let dma_buf = completed_dma_buf.ok_or(UsbError::InvalidState)?;
    caller_buf[..count].copy_from_slice(&dma_buf[..count]);
    Ok(count)
}

struct Buffers {
    // Buffers can be up to 64 Bytes since this is a Full-Speed implementation.
    in_lens: [u8; 8],
    out_lens: [u8; 8],
}

impl Buffers {
    fn new() -> Self {
        Self {
            in_lens: [0; 8],
            out_lens: [0; 8],
        }
    }
}

#[derive(Copy, Clone)]
enum TransferState {
    NoTransfer,
    Started(u16),
}

#[derive(Copy, Clone)]
struct EP0State {
    direction: UsbDirection,
    remaining_size: u16,
    in_transfer_state: TransferState,
    is_set_address: bool,
}

fn abort_ep0_for_new_transaction(state: &mut EP0State, busy_in_endpoints: u16) -> u16 {
    // A bus reset or a newly ACKed SETUP packet is the hardware-defined abort
    // boundary for the previous control transfer. EP0DATADONE is not guaranteed
    // for that aborted transfer, so software must not retain its busy/fallback
    // state and wait for a completion that can never arrive.
    state.direction = UsbDirection::Out;
    state.remaining_size = 0;
    state.in_transfer_state = TransferState::NoTransfer;
    state.is_set_address = false;
    busy_in_endpoints & !1
}

/// USB device implementation.
///
/// This type implements the [`UsbBus`] trait and can be passed to a [`UsbBusAllocator`] to
/// configure and use the USB device.
///
/// [`UsbBusAllocator`]: usb_device::bus::UsbBusAllocator
pub struct Usbd<T: UsbPeripheral> {
    _periph: Mutex<T>,
    applicability: Option<errata::Applicability>,
    // argument passed to `UsbDeviceBuilder.max_packet_size_0`
    max_packet_size_0: u16,
    bufs: Buffers,
    used_in: u8,
    used_out: u8,
    ep0_state: Mutex<Cell<EP0State>>,
    busy_in_endpoints: Mutex<Cell<u16>>,
    dma_odd: Mutex<Cell<bool>>,
    fault: Mutex<Cell<TerminalFault>>,
    lifecycle: Mutex<Cell<Lifecycle>>,
    bootloader_handoff: Mutex<Cell<bool>>,
}

impl<T: UsbPeripheral> Usbd<T> {
    /// Creates a new USB device wrapper, taking ownership of the raw peripheral.
    ///
    /// # Parameters
    ///
    /// * `periph`: The raw USBD peripheral.
    ///
    /// Unsupported factory identity is faulted before controller access or shared-storage
    /// claim. On nRF52840, the first instance permanently claims the process-wide,
    /// EasyDMA-safe staging buffers. A later instance is constructed in a faulted state and reports
    /// [`UsbdFault::DmaStorageAlreadyClaimed`]; staging storage is never reused because a
    /// timed-out transfer has no documented cancellation task and may complete late.
    #[inline]
    pub fn new(periph: T) -> Self {
        // Factory identity is immutable. Gate it before claiming shared DMA storage or
        // allowing any lifecycle method to touch USBD.
        let applicability = errata::detect();
        T::on_operational_change(false);
        let initial_fault =
            initial_fault_for_silicon(applicability, || DMA_STORAGE_CLAIM.try_claim());
        if let Some(fault) = initial_fault {
            T::on_fault(fault);
        }
        Self {
            _periph: Mutex::new(periph),
            applicability,
            max_packet_size_0: 0,
            bufs: Buffers::new(),
            used_in: 0,
            used_out: 0,
            ep0_state: Mutex::new(Cell::new(EP0State {
                direction: UsbDirection::Out,
                remaining_size: 0,
                in_transfer_state: TransferState::NoTransfer,
                is_set_address: false,
            })),
            busy_in_endpoints: Mutex::new(Cell::new(0)),
            dma_odd: Mutex::new(Cell::new(false)),
            fault: Mutex::new(Cell::new(TerminalFault::new(initial_fault))),
            lifecycle: Mutex::new(Cell::new(Lifecycle::AwaitVbus)),
            bootloader_handoff: Mutex::new(Cell::new(false)),
        }
    }

    fn regs<'a>(&self, _cs: &'a CriticalSection) -> &'a RegisterBlock {
        unsafe { &*(T::REGISTERS as *const RegisterBlock) }
    }

    /// Fetches the address assigned to the device (only valid when device is configured).
    pub fn device_address(&self) -> u8 {
        unsafe { &*(T::REGISTERS as *const RegisterBlock) }
            .usbaddr
            .read()
            .addr()
            .bits()
    }

    fn is_used(&self, ep: EndpointAddress) -> bool {
        endpoint_is_owned(self.used_in, self.used_out, ep)
    }

    fn has_fault(&self, cs: CriticalSection<'_>) -> bool {
        !self.fault.borrow(cs).get().permits_operation()
    }

    fn operational_errata(
        &self,
        cs: CriticalSection<'_>,
    ) -> Result<errata::Applicability, UsbError> {
        if self.has_fault(cs) {
            return Err(UsbError::InvalidState);
        }
        match self.lifecycle.borrow(cs).get() {
            Lifecycle::Active { errata } => Ok(errata),
            _ => Err(UsbError::WouldBlock),
        }
    }

    fn latch_fault(&self, cs: CriticalSection<'_>, fault: UsbdFault) {
        let current = self.fault.borrow(cs);
        let (next, newly_latched) = current.get().latch(fault);
        if newly_latched {
            current.set(next);
            T::on_operational_change(false);
            T::on_fault(fault);
        }
    }

    fn async_wait(timeout_us: u32, poll_budget: usize) -> AsyncWait {
        AsyncWait::new(T::monotonic_us_32(), timeout_us, poll_budget)
    }

    fn set_lifecycle(&self, cs: CriticalSection<'_>, lifecycle: Lifecycle) {
        T::on_operational_change(matches!(lifecycle, Lifecycle::Active { .. }));
        self.lifecycle.borrow(cs).set(lifecycle);
    }

    /// Establishes the host-visible bootloader-to-application disconnect.
    ///
    /// The caller may invoke this with an inherited controller still active. In
    /// that case the lifecycle first observes ENABLE reach Disabled and only then
    /// starts the detach timer. No LOWPOWER, endpoint, event, or configuration
    /// register is accessed during this initial boundary.
    fn begin_initial_detach(&self, cs: CriticalSection<'_>, regs: &RegisterBlock) {
        let Some(applicable) = self.applicability else {
            return;
        };
        let preparation = prepare_initial_detach(
            || regs.usbpullup.write(|w| w.connect().disabled()),
            || regs.enable.read().enable().is_disabled(),
            || regs.enable.write(|w| w.enable().disabled()),
            T::vbus_present(),
            T::monotonic_us_32,
        );
        match preparation {
            InitialDetachPreparation::AwaitVbus => self.set_lifecycle(cs, Lifecycle::AwaitVbus),
            InitialDetachPreparation::Disabling(wait) => self.set_lifecycle(
                cs,
                Lifecycle::InitialDisabling {
                    wait,
                    errata: applicable,
                },
            ),
            InitialDetachPreparation::Dwelling(wait) => {
                self.set_lifecycle(cs, Lifecycle::InitialDetach { wait })
            }
        }
    }

    /// Starts exactly one asynchronous `ENABLE` attempt.
    ///
    /// A stale enabled handoff is first driven through a separately observed
    /// disable transition. The driver never issues a second ENABLE attempt from
    /// an uncertain state.
    fn start_enable(&self, cs: CriticalSection<'_>, regs: &RegisterBlock) {
        let Some(applicable) = self.applicability else {
            // Construction has already latched UnsupportedSilicon. Keep this guard at
            // the write boundary so even an accidental caller cannot touch USBD.
            return;
        };
        if !T::vbus_present() {
            self.set_lifecycle(cs, Lifecycle::AwaitVbus);
            return;
        }

        regs.usbpullup.write(|w| w.connect().disabled());
        if regs.enable.read().enable().is_enabled() {
            regs.enable.write(|w| w.enable().disabled());
            self.set_lifecycle(
                cs,
                Lifecycle::Disabling {
                    wait: Self::async_wait(ENABLE_READY_TIMEOUT_US, ENABLE_READY_POLL_BUDGET),
                    errata: None,
                    pending_fault: None,
                },
            );
            return;
        }

        self.start_hfclk_or_enable(cs, regs, applicable);
    }

    fn start_hfclk_or_enable(
        &self,
        cs: CriticalSection<'_>,
        regs: &RegisterBlock,
        applicable: errata::Applicability,
    ) {
        T::request_hfclk();
        if T::hfclk_running() {
            self.enable_controller(cs, regs, applicable);
        } else {
            self.set_lifecycle(
                cs,
                Lifecycle::HfclkStarting {
                    wait: Self::async_wait(HFCLK_READY_TIMEOUT_US, HFCLK_READY_POLL_BUDGET),
                    errata: applicable,
                },
            );
        }
    }

    fn enable_controller(
        &self,
        cs: CriticalSection<'_>,
        regs: &RegisterBlock,
        applicable: errata::Applicability,
    ) {
        // Nordic requires HFXO running before USBD is enabled. READY only
        // acknowledges the controller transition; it is not an oscillator request.
        // The board hook above owns clock arbitration. READY is W1C, so clear a stale
        // handoff event before creating the only 0->1 transition.
        regs.eventcause.write(|w| w.ready().set_bit());
        errata::begin_enable(applicable);
        regs.enable.write(|w| w.enable().enabled());
        self.set_lifecycle(
            cs,
            Lifecycle::Enabling {
                wait: Self::async_wait(ENABLE_READY_TIMEOUT_US, ENABLE_READY_POLL_BUDGET),
                errata: applicable,
            },
        );
    }

    fn begin_disabling(
        &self,
        cs: CriticalSection<'_>,
        regs: &RegisterBlock,
        errata: Option<ErrataOwnership>,
        pending_fault: Option<UsbdFault>,
    ) {
        regs.usbpullup.write(|w| w.connect().disabled());
        self.set_lifecycle(
            cs,
            Lifecycle::PreparingDisable {
                errata,
                pending_fault,
            },
        );
    }

    fn start_disable_readback(
        &self,
        cs: CriticalSection<'_>,
        regs: &RegisterBlock,
        errata: Option<ErrataOwnership>,
        pending_fault: Option<UsbdFault>,
    ) {
        regs.enable.write(|w| w.enable().disabled());
        self.set_lifecycle(
            cs,
            Lifecycle::Disabling {
                wait: Self::async_wait(ENABLE_READY_TIMEOUT_US, ENABLE_READY_POLL_BUDGET),
                errata,
                pending_fault,
            },
        );
    }

    fn start_parity_fix(
        &self,
        cs: CriticalSection<'_>,
        regs: &RegisterBlock,
        applicable: errata::Applicability,
        pending_fault: Option<UsbdFault>,
    ) {
        // Current Nordic nrfx performs this one-byte EPIN0 EasyDMA transaction before
        // disabling when the cumulative DMA byte count is odd. Without it the next
        // USBD enable can issue an invalid bus request. The pull-up is already off,
        // the controller is ForceNormal/OUTPUTRDY, and endpoint I/O is unpublished,
        // so this does not create a USB control transfer or mutate usb-device state.
        let ram_buf = unsafe { &mut *DMA_IN_BUFFER.0.get() };
        regs.events_endepin[0].reset();
        unsafe {
            regs.epin0.ptr.write(|w| w.bits(ram_buf.as_ptr() as u32));
            regs.epin0.maxcnt.write(|w| w.maxcnt().bits(1));
        }
        dma_start(applicable);
        regs.tasks_startepin[0].write(|w| w.tasks_startepin().set_bit());
        self.set_lifecycle(
            cs,
            Lifecycle::ParityFixing {
                wait: Self::async_wait(PARITY_DMA_TIMEOUT_US, PARITY_DMA_POLL_BUDGET),
                errata: applicable,
                pending_fault,
            },
        );
    }

    fn close_errata(ownership: Option<ErrataOwnership>) {
        match ownership {
            Some(ErrataOwnership::Enabling(applicable)) => errata::abort_enable(applicable),
            Some(ErrataOwnership::Active(applicable)) => errata::end_active(applicable),
            Some(ErrataOwnership::Waking(applicable)) => {
                // No wake acknowledgement is implied. The controller is already
                // confirmed disabled, so the open EC14 and active-lifetime ED14
                // flags can now be released.
                errata::complete_wake(applicable);
                errata::end_active(applicable);
            }
            None => {}
        }
    }

    /// Advances at most one hardware lifecycle observation.
    ///
    /// This runs inside a critical section, but performs no spin or delay: every
    /// invocation samples a readiness condition once and returns to the caller.
    fn advance_lifecycle(&self, cs: CriticalSection<'_>, regs: &RegisterBlock) {
        if self.has_fault(cs) {
            return;
        }

        let lifecycle = self.lifecycle.borrow(cs);
        match lifecycle.get() {
            Lifecycle::AwaitVbus => {
                if T::vbus_present() {
                    self.start_enable(cs, regs);
                }
            }
            Lifecycle::InitialDisabling {
                mut wait,
                errata: applicable,
            } => {
                if regs.enable.read().enable().is_disabled() {
                    // A bootloader can own any of the hidden enable/wake state.
                    // Close it only after ENABLE readback proves the old session
                    // cannot still be using those workarounds.
                    errata::end_disabled(applicable);
                    if T::vbus_present() {
                        self.set_lifecycle(
                            cs,
                            Lifecycle::InitialDetach {
                                wait: Self::async_wait(
                                    INITIAL_DETACH_TIMEOUT_US,
                                    INITIAL_DETACH_POLL_BUDGET,
                                ),
                            },
                        );
                    } else {
                        self.set_lifecycle(cs, Lifecycle::AwaitVbus);
                    }
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    // Retain errata ownership while ENABLE is uncertain. A reset
                    // is the only safe boundary after this terminal timeout.
                    self.set_lifecycle(
                        cs,
                        Lifecycle::InitialDisabling {
                            wait,
                            errata: applicable,
                        },
                    );
                    self.latch_fault(cs, UsbdFault::DisableTimeout);
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::InitialDisabling {
                            wait,
                            errata: applicable,
                        },
                    );
                }
            }
            Lifecycle::InitialDetach { mut wait } => {
                match observe_initial_detach(&mut wait, T::vbus_present(), T::monotonic_us_32()) {
                    InitialDetachObservation::Waiting => {
                        self.set_lifecycle(cs, Lifecycle::InitialDetach { wait });
                    }
                    InitialDetachObservation::StartEnable => self.start_enable(cs, regs),
                    InitialDetachObservation::VbusLost => {
                        self.set_lifecycle(cs, Lifecycle::AwaitVbus);
                    }
                }
            }
            Lifecycle::PreparingDisable {
                errata: ownership,
                pending_fault,
            } => {
                let vbus_present = T::vbus_present();
                let power_ready = vbus_present && T::power_ready();
                let lowpower = power_ready && regs.lowpower.read().lowpower().is_low_power();
                match disable_preparation(
                    self.dma_odd.borrow(cs).get(),
                    ownership,
                    vbus_present,
                    power_ready,
                    lowpower,
                ) {
                    DisablePreparation::StartDisable => {
                        self.start_disable_readback(cs, regs, ownership, pending_fault);
                    }
                    DisablePreparation::WaitForPower => self.set_lifecycle(
                        cs,
                        Lifecycle::PreparingDisable {
                            errata: ownership,
                            pending_fault,
                        },
                    ),
                    DisablePreparation::AwaitWake => {
                        let Some(ErrataOwnership::Waking(applicable)) = ownership else {
                            unreachable!("disable preparation classified wake ownership")
                        };
                        self.set_lifecycle(
                            cs,
                            Lifecycle::DisableWaking {
                                wait: Self::async_wait(
                                    WAKE_READY_TIMEOUT_US,
                                    WAKE_READY_POLL_BUDGET,
                                ),
                                errata: applicable,
                                pending_fault,
                            },
                        );
                    }
                    DisablePreparation::BeginWake => {
                        let Some(ErrataOwnership::Active(applicable)) = ownership else {
                            unreachable!("disable preparation classified active ownership")
                        };
                        regs.eventcause.write(|w| w.usbwuallowed().set_bit());
                        errata::begin_wake(applicable);
                        regs.lowpower.write(|w| w.lowpower().force_normal());
                        self.set_lifecycle(
                            cs,
                            Lifecycle::DisableWaking {
                                wait: Self::async_wait(
                                    WAKE_READY_TIMEOUT_US,
                                    WAKE_READY_POLL_BUDGET,
                                ),
                                errata: applicable,
                                pending_fault,
                            },
                        );
                    }
                    DisablePreparation::StartParityDma => {
                        let Some(ErrataOwnership::Active(applicable)) = ownership else {
                            unreachable!("disable preparation classified active ownership")
                        };
                        self.start_parity_fix(cs, regs, applicable, pending_fault);
                    }
                    DisablePreparation::UnsafeLifecycle => {
                        // Preserve ENABLE and all errata ownership. An odd count in an
                        // unacknowledged/non-active session violates the driver's own
                        // invariant, so neither a guessed DMA nor disable is safe.
                        self.set_lifecycle(
                            cs,
                            Lifecycle::PreparingDisable {
                                errata: ownership,
                                pending_fault,
                            },
                        );
                        self.latch_fault(cs, UsbdFault::ParityRepairUnavailable);
                    }
                }
            }
            Lifecycle::DisableWaking {
                mut wait,
                errata: applicable,
                pending_fault,
            } => {
                if !T::vbus_present() || !T::power_ready() {
                    // A disconnected regulator cannot acknowledge ForceNormal. Keep
                    // the pull-up off and the Erratum 171/211 ownership open, and give
                    // hardware a fresh bounded window after continuous power returns.
                    self.set_lifecycle(
                        cs,
                        Lifecycle::DisableWaking {
                            wait: Self::async_wait(WAKE_READY_TIMEOUT_US, WAKE_READY_POLL_BUDGET),
                            errata: applicable,
                            pending_fault,
                        },
                    );
                } else if regs.eventcause.read().usbwuallowed().is_allowed() {
                    regs.eventcause.write(|w| w.usbwuallowed().set_bit());
                    errata::complete_wake(applicable);
                    self.start_parity_fix(cs, regs, applicable, pending_fault);
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::DisableWaking {
                            wait,
                            errata: applicable,
                            pending_fault,
                        },
                    );
                    self.latch_fault(cs, UsbdFault::WakeTimeout);
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::DisableWaking {
                            wait,
                            errata: applicable,
                            pending_fault,
                        },
                    );
                }
            }
            Lifecycle::ParityFixing {
                mut wait,
                errata: applicable,
                pending_fault,
            } => {
                if regs.events_endepin[0].read().events_endepin().bit_is_set() {
                    regs.events_endepin[0].reset();
                    dma_end(applicable);
                    self.dma_odd.borrow(cs).set(false);
                    self.start_disable_readback(
                        cs,
                        regs,
                        Some(ErrataOwnership::Active(applicable)),
                        pending_fault,
                    );
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    // EasyDMA may still own the permanent buffer. Do not disable or
                    // close Erratum 199/211; only reset can recover this controller.
                    self.set_lifecycle(
                        cs,
                        Lifecycle::ParityFixing {
                            wait,
                            errata: applicable,
                            pending_fault,
                        },
                    );
                    self.latch_fault(cs, UsbdFault::InDmaTimeout { endpoint: 0 });
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::ParityFixing {
                            wait,
                            errata: applicable,
                            pending_fault,
                        },
                    );
                }
            }
            Lifecycle::Disabling {
                mut wait,
                errata: ownership,
                pending_fault,
            } => {
                if regs.enable.read().enable().is_disabled() {
                    Self::close_errata(ownership);
                    if self.bootloader_handoff.borrow(cs).get() {
                        self.set_lifecycle(cs, Lifecycle::HandoffComplete);
                    } else {
                        self.set_lifecycle(cs, Lifecycle::AwaitVbus);
                    }
                    if let Some(fault) = pending_fault {
                        self.latch_fault(cs, fault);
                    }
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    // Hardware may still be active, so retain the asserted errata
                    // workarounds and their ownership. Clearing ED14 here would
                    // violate Erratum 211's active-lifetime requirement precisely
                    // when the ENABLE state is uncertain. The terminal fault prevents
                    // this instance from touching the controller again; a subsequent
                    // system reset provides the only safe ownership boundary.
                    self.set_lifecycle(
                        cs,
                        Lifecycle::Disabling {
                            wait,
                            errata: ownership,
                            pending_fault,
                        },
                    );
                    self.latch_fault(cs, UsbdFault::DisableTimeout);
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::Disabling {
                            wait,
                            errata: ownership,
                            pending_fault,
                        },
                    );
                }
            }
            Lifecycle::HfclkStarting {
                mut wait,
                errata: applicable,
            } => {
                if !T::vbus_present() {
                    self.set_lifecycle(cs, Lifecycle::AwaitVbus);
                } else if T::hfclk_running() {
                    self.enable_controller(cs, regs, applicable);
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::HfclkStarting {
                            wait,
                            errata: applicable,
                        },
                    );
                    self.latch_fault(cs, UsbdFault::HfclkTimeout);
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::HfclkStarting {
                            wait,
                            errata: applicable,
                        },
                    );
                }
            }
            Lifecycle::Enabling {
                mut wait,
                errata: applicable,
            } => {
                if !T::vbus_present() {
                    self.begin_disabling(
                        cs,
                        regs,
                        Some(ErrataOwnership::Enabling(applicable)),
                        None,
                    );
                } else if regs.eventcause.read().ready().is_ready() {
                    let base = regs as *const RegisterBlock as *const u8;
                    let completion = complete_enable_session(
                        || regs.usbpullup.write(|w| w.connect().disabled()),
                        || regs.eventcause.write(|w| w.ready().set_bit()),
                        || errata::complete_enable(applicable),
                        || regs.lowpower.read().lowpower().is_low_power(),
                        || regs.eventcause.write(|w| w.usbwuallowed().set_bit()),
                        || errata::begin_wake(applicable),
                        || regs.lowpower.write(|w| w.lowpower().force_normal()),
                        || errata::configure_enabled_session(applicable),
                        |offset, value| unsafe {
                            base.add(offset)
                                .cast_mut()
                                .cast::<u32>()
                                .write_volatile(value);
                        },
                    );
                    match completion {
                        EnableSessionCompletion::Finished => self.set_lifecycle(
                            cs,
                            Lifecycle::PowerReady {
                                wait: Self::async_wait(
                                    POWER_READY_TIMEOUT_US,
                                    POWER_READY_POLL_BUDGET,
                                ),
                                errata: applicable,
                            },
                        ),
                        EnableSessionCompletion::WakePending => self.set_lifecycle(
                            cs,
                            Lifecycle::Waking {
                                wait: Self::async_wait(
                                    WAKE_READY_TIMEOUT_US,
                                    WAKE_READY_POLL_BUDGET,
                                ),
                                errata: applicable,
                                target: WakeTarget::FinishEnable,
                            },
                        ),
                    }
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    self.begin_disabling(
                        cs,
                        regs,
                        Some(ErrataOwnership::Enabling(applicable)),
                        Some(UsbdFault::EnableTimeout),
                    );
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::Enabling {
                            wait,
                            errata: applicable,
                        },
                    );
                }
            }
            Lifecycle::PowerReady {
                mut wait,
                errata: applicable,
            } => {
                if !T::vbus_present() {
                    self.begin_disabling(cs, regs, Some(ErrataOwnership::Active(applicable)), None);
                } else if T::power_ready() {
                    regs.usbpullup.write(|w| w.connect().enabled());
                    self.set_lifecycle(cs, Lifecycle::Active { errata: applicable });
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    self.begin_disabling(
                        cs,
                        regs,
                        Some(ErrataOwnership::Active(applicable)),
                        Some(UsbdFault::PowerReadyTimeout),
                    );
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::PowerReady {
                            wait,
                            errata: applicable,
                        },
                    );
                }
            }
            Lifecycle::Active { errata: applicable }
            | Lifecycle::Suspended { errata: applicable } => {
                if !T::vbus_present() {
                    self.begin_disabling(cs, regs, Some(ErrataOwnership::Active(applicable)), None);
                }
            }
            Lifecycle::Waking {
                mut wait,
                errata: applicable,
                target,
            } => {
                if !T::vbus_present() {
                    self.begin_disabling(cs, regs, Some(ErrataOwnership::Waking(applicable)), None);
                } else if regs.eventcause.read().usbwuallowed().is_allowed() {
                    let base = regs as *const RegisterBlock as *const u8;
                    let target = complete_wake_session(
                        target,
                        || regs.eventcause.write(|w| w.usbwuallowed().set_bit()),
                        || errata::complete_wake(applicable),
                        || errata::configure_enabled_session(applicable),
                        |offset, value| unsafe {
                            base.add(offset)
                                .cast_mut()
                                .cast::<u32>()
                                .write_volatile(value);
                        },
                    );
                    match target {
                        WakeTarget::Active => {
                            self.set_lifecycle(cs, Lifecycle::Active { errata: applicable });
                        }
                        WakeTarget::FinishEnable => {
                            self.set_lifecycle(
                                cs,
                                Lifecycle::PowerReady {
                                    wait: Self::async_wait(
                                        POWER_READY_TIMEOUT_US,
                                        POWER_READY_POLL_BUDGET,
                                    ),
                                    errata: applicable,
                                },
                            );
                        }
                    }
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    self.begin_disabling(
                        cs,
                        regs,
                        Some(ErrataOwnership::Waking(applicable)),
                        Some(UsbdFault::WakeTimeout),
                    );
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::Waking {
                            wait,
                            errata: applicable,
                            target,
                        },
                    );
                }
            }
            Lifecycle::Detaching {
                mut wait,
                errata: applicable,
            } => {
                if !T::vbus_present() {
                    self.begin_disabling(cs, regs, Some(ErrataOwnership::Active(applicable)), None);
                } else if wait.failed_observation(T::monotonic_us_32()) {
                    if T::power_ready() {
                        regs.usbpullup.write(|w| w.connect().enabled());
                        self.set_lifecycle(cs, Lifecycle::Active { errata: applicable });
                    } else {
                        self.set_lifecycle(
                            cs,
                            Lifecycle::PowerReady {
                                wait: Self::async_wait(
                                    POWER_READY_TIMEOUT_US,
                                    POWER_READY_POLL_BUDGET,
                                ),
                                errata: applicable,
                            },
                        );
                    }
                } else {
                    self.set_lifecycle(
                        cs,
                        Lifecycle::Detaching {
                            wait,
                            errata: applicable,
                        },
                    );
                }
            }
            Lifecycle::HandoffComplete => {}
        }
    }

    fn begin_wake(
        &self,
        cs: CriticalSection<'_>,
        regs: &RegisterBlock,
        applicable: errata::Applicability,
    ) {
        if regs.lowpower.read().lowpower().is_force_normal() {
            self.set_lifecycle(cs, Lifecycle::Active { errata: applicable });
            return;
        }

        if !T::vbus_present() {
            self.begin_disabling(cs, regs, Some(ErrataOwnership::Active(applicable)), None);
            return;
        }

        // USBWUALLOWED is W1C. Clear a stale acknowledgement before opening
        // Erratum 171 and issuing the one asynchronous wake request.
        regs.eventcause.write(|w| w.usbwuallowed().set_bit());
        errata::begin_wake(applicable);
        regs.lowpower.write(|w| w.lowpower().force_normal());
        self.set_lifecycle(
            cs,
            Lifecycle::Waking {
                wait: Self::async_wait(WAKE_READY_TIMEOUT_US, WAKE_READY_POLL_BUDGET),
                errata: applicable,
                target: WakeTarget::Active,
            },
        );
    }

    /// Returns the fatal fault latched by this bus instance, if any.
    ///
    /// A fault is permanent for the lifetime of the instance because the controller or
    /// EasyDMA state cannot be proven safe to reuse after a bounded operation times out.
    pub fn fault(&self) -> Option<UsbdFault> {
        critical_section::with(|cs| self.fault.borrow(cs).get().fault())
    }

    fn read_control_setup(
        &self,
        regs: &RegisterBlock,
        buf: &mut [u8],
        ep0_state: &mut EP0State,
    ) -> usb_device::Result<usize> {
        const SETUP_LEN: usize = 8;

        if buf.len() < SETUP_LEN {
            return Err(UsbError::BufferOverflow);
        }

        // This is unfortunate: Reassemble the nicely split-up setup packet back into bytes, only
        // for the usb-device code to copy the bytes back out into structured data.
        // The bytes are split across registers, leaving a 3-Byte gap between them, so we couldn't
        // even memcpy all of them at once. Weird peripheral.
        buf[0] = regs.bmrequesttype.read().bits() as u8;
        buf[1] = regs.brequest.read().brequest().bits();
        buf[2] = regs.wvaluel.read().wvaluel().bits();
        buf[3] = regs.wvalueh.read().wvalueh().bits();
        buf[4] = regs.windexl.read().windexl().bits();
        buf[5] = regs.windexh.read().windexh().bits();
        buf[6] = regs.wlengthl.read().wlengthl().bits();
        buf[7] = regs.wlengthh.read().wlengthh().bits();

        ep0_state.direction = match regs.bmrequesttype.read().direction().is_host_to_device() {
            false => UsbDirection::In,
            true => UsbDirection::Out,
        };
        ep0_state.remaining_size = (buf[6] as u16) | ((buf[7] as u16) << 8);
        ep0_state.is_set_address = (buf[0] == 0x00) && (buf[1] == 0x05);

        if ep0_state.direction == UsbDirection::Out {
            regs.tasks_ep0rcvout
                .write(|w| w.tasks_ep0rcvout().set_bit());
        }

        Ok(SETUP_LEN)
    }
}

impl<T: UsbPeripheral> UsbBus for Usbd<T> {
    fn alloc_ep(
        &mut self,
        ep_dir: UsbDirection,
        ep_addr: Option<EndpointAddress>,
        ep_type: EndpointType,
        max_packet_size: u16,
        _interval: u8,
    ) -> usb_device::Result<EndpointAddress> {
        if critical_section::with(|cs| self.has_fault(cs)) {
            return Err(UsbError::InvalidState);
        }
        validate_endpoint_request(ep_type, max_packet_size)?;
        // Endpoint addresses are fixed in hardware:
        // - 0x80 / 0x00 - Control        EP0
        // - 0x81 / 0x01 - Bulk/Interrupt EP1
        // - 0x82 / 0x02 - Bulk/Interrupt EP2
        // - 0x83 / 0x03 - Bulk/Interrupt EP3
        // - 0x84 / 0x04 - Bulk/Interrupt EP4
        // - 0x85 / 0x05 - Bulk/Interrupt EP5
        // - 0x86 / 0x06 - Bulk/Interrupt EP6
        // - 0x87 / 0x07 - Bulk/Interrupt EP7
        // Isochronous endpoint 8 is intentionally not advertised until this driver has
        // a dedicated 1024-byte ISO DMA path.

        // Endpoint directions are allocated individually.

        // store user-supplied value
        if ep_addr.map(|addr| addr.index()) == Some(0) {
            self.max_packet_size_0 = max_packet_size;
        }

        let (used, lens) = match ep_dir {
            UsbDirection::In => (&mut self.used_in, &mut self.bufs.in_lens),
            UsbDirection::Out => (&mut self.used_out, &mut self.bufs.out_lens),
        };

        let alloc_index = match ep_type {
            EndpointType::Isochronous { .. } => unreachable!("validated above"),
            EndpointType::Control => 0,
            EndpointType::Interrupt | EndpointType::Bulk => {
                let leading = used.leading_zeros();
                if leading == 0 {
                    return Err(UsbError::EndpointOverflow);
                }

                if leading == 8 {
                    // Even CONTROL is free, don't allocate that
                    1
                } else {
                    8 - leading
                }
            }
        };

        if *used & (1 << alloc_index) != 0 {
            return Err(UsbError::EndpointOverflow);
        }

        *used |= 1 << alloc_index;
        lens[alloc_index as usize] = max_packet_size as u8;

        let addr = EndpointAddress::from_parts(alloc_index as usize, ep_dir);
        Ok(addr)
    }

    #[inline]
    fn enable(&mut self) {
        critical_section::with(move |cs| {
            if self.has_fault(cs) {
                return;
            }
            let regs = self.regs(&cs);
            self.begin_initial_detach(cs, regs);
        });
    }

    #[inline]
    fn reset(&self) {
        critical_section::with(move |cs| {
            if self.has_fault(cs) {
                return;
            }
            let regs = self.regs(&cs);

            reset_ep0_transaction_registers(
                || {
                    regs.shorts
                        .modify(|_, w| w.ep0datadone_ep0status().clear_bit())
                },
                || regs.events_ep0datadone.reset(),
            );
            let ep0_state = self.ep0_state.borrow(cs);
            let mut state = ep0_state.get();
            let busy =
                abort_ep0_for_new_transaction(&mut state, self.busy_in_endpoints.borrow(cs).get());
            ep0_state.set(state);
            self.busy_in_endpoints.borrow(cs).set(busy);

            // TODO: Initialize ISO buffers

            // XXX this is not spec compliant; the endpoints should only be enabled after the device
            // has been put in the Configured state. However, usb-device provides no hook to do that
            // TODO: Merge `used_{in,out}` with `iso_{in,out}_used` so ISO is enabled here as well.
            // Make the enabled endpoints respond to traffic.
            unsafe {
                regs.epinen.write(|w| w.bits(self.used_in.into()));
                regs.epouten.write(|w| w.bits(self.used_out.into()));
            }

            for i in 1..8 {
                let out_enabled = self.used_out & (1 << i) != 0;

                // when first enabled, bulk/interrupt OUT endpoints will *not* receive data (the
                // peripheral will NAK all incoming packets) until we write a zero to the SIZE
                // register (see figure 203 of the 52840 manual). To avoid that we write a 0 to the
                // SIZE register
                if out_enabled {
                    regs.size.epout[i].reset();
                }
            }

            self.busy_in_endpoints.borrow(cs).set(0);
        });
    }

    #[inline]
    fn set_device_address(&self, _addr: u8) {
        // Nothing to do, the peripheral handles this.
    }

    // nRF USBD performs SET_ADDRESS in hardware as part of the status stage.
    // Publish the software Addressed state before accepting that status stage;
    // a later set_device_address() call is intentionally a no-op on this bus.
    const QUIRK_SET_ADDRESS_BEFORE_STATUS: bool = true;

    fn write(&self, ep_addr: EndpointAddress, buf: &[u8]) -> usb_device::Result<usize> {
        critical_section::with(|cs| self.operational_errata(cs))?;
        if !self.is_used(ep_addr) {
            return Err(UsbError::InvalidEndpoint);
        }

        if ep_addr.is_out() {
            return Err(UsbError::InvalidEndpoint);
        }

        // A 0-length write to Control EP 0 is a status stage acknowledging a control write xfer
        if ep_addr.index() == 0 && buf.is_empty() {
            let exit = critical_section::with(move |cs| {
                let regs = self.regs(&cs);

                let ep0_state = self.ep0_state.borrow(cs).get();

                if ep0_state.is_set_address {
                    // Inhibit
                    return true;
                }

                if ep0_state.direction == UsbDirection::Out {
                    regs.tasks_ep0status
                        .write(|w| w.tasks_ep0status().set_bit());
                    return true;
                }

                if ep0_state.direction == UsbDirection::In && ep0_state.remaining_size == 0 {
                    // Device sent all the requested data, no need to send ZLP.
                    // Host will issue an OUT transfer in this case, device should
                    // respond with a status stage.
                    regs.tasks_ep0status
                        .write(|w| w.tasks_ep0status().set_bit());
                    return true;
                }

                false
            });

            if exit {
                return Ok(0);
            }
        }

        let i = ep_addr.index();

        if usize::from(self.bufs.in_lens[i]) < buf.len() || buf.len() > DMA_BUFFER_SIZE {
            return Err(UsbError::BufferOverflow);
        }

        critical_section::with(move |cs| {
            let applicable = self.operational_errata(cs)?;
            let regs = self.regs(&cs);
            let busy_in_endpoints = self.busy_in_endpoints.borrow(cs);

            if busy_in_endpoints.get() & (1 << i) != 0 {
                // Maybe this endpoint is not busy?
                let epdatastatus = regs.epdatastatus.read().bits();
                if epdatastatus & (1 << i) != 0 {
                    // Clear the event flag
                    regs.epdatastatus.write(|w| unsafe { w.bits(1 << i) });

                    // Clear the busy status and continue
                    busy_in_endpoints.set(busy_in_endpoints.get() & !(1 << i));
                } else {
                    return Err(UsbError::WouldBlock);
                }
            }
            // EP0 availability is serialized by `busy_in_endpoints` and the bounded
            // ENDEPIN wait below. EPSTATUS is not an additional reliable busy gate for
            // the control endpoint, so apply that check only to regular data endpoints.
            if i != 0 && regs.epstatus.read().bits() & (1 << i) != 0 {
                return Err(UsbError::WouldBlock);
            }

            // EasyDMA may finish after a bounded timeout. A static staging buffer keeps
            // its pointer valid even after this call returns with `InvalidState`.
            let ram_buf = unsafe { &mut *DMA_IN_BUFFER.0.get() };
            ram_buf[..buf.len()].copy_from_slice(buf);

            let epin = [
                &regs.epin0,
                &regs.epin1,
                &regs.epin2,
                &regs.epin3,
                &regs.epin4,
                &regs.epin5,
                &regs.epin6,
                &regs.epin7,
            ];

            // Set the buffer length so the right number of bytes are transmitted.
            // Safety: `buf.len()` has been checked to be <= the max buffer length.
            unsafe {
                if buf.is_empty() {
                    epin[i].ptr.write(|w| w.bits(0));
                } else {
                    epin[i].ptr.write(|w| w.bits(ram_buf.as_ptr() as u32));
                }
                epin[i].maxcnt.write(|w| w.maxcnt().bits(buf.len() as u8));
            }

            if i == 0 {
                // A short EPIN0 packet ends the control data stage, so arm the hardware
                // transition to EP0STATUS only for that final packet. An intermediate
                // full-size packet must keep the data stage open for the next packet;
                // poll() retains its bounded software fallback for the final status stage.
                let is_short_packet = buf.len() < self.max_packet_size_0 as usize;
                regs.shorts.modify(|_, w| {
                    if is_short_packet {
                        w.ep0datadone_ep0status().set_bit()
                    } else {
                        w.ep0datadone_ep0status().clear_bit()
                    }
                });

                let mut ep0_state = self.ep0_state.borrow(cs).get();
                ep0_state.remaining_size =
                    ep0_state.remaining_size.saturating_sub(buf.len() as u16);
                self.ep0_state.borrow(cs).set(ep0_state);

                // Hack: trigger status stage if the IN transfer is not acknowledged after a few frames,
                // so record the current frame here; the actual test and status stage activation happens
                // in the poll method.
                let frame_counter = regs.framecntr.read().framecntr().bits();
                let ep0_state = self.ep0_state.borrow(cs);
                let mut state = ep0_state.get();
                state.in_transfer_state = TransferState::Started(frame_counter);
                ep0_state.set(state);
            }

            // Clear ENDEPIN[i] flag
            regs.events_endepin[i].reset();

            // Kick off device -> host transmission. This starts DMA, so a compiler fence is needed.
            dma_start(applicable);
            regs.tasks_startepin[i].write(|w| w.tasks_startepin().set_bit());
            if !poll_with_budget(DMA_COMPLETE_POLL_BUDGET, || {
                regs.events_endepin[i].read().events_endepin().bit_is_set()
            }) {
                // Nordic forbids disabling USBD while an EasyDMA transfer may still
                // be active. Detach D+ and fault the instance, but keep ENABLE and the
                // active-lifetime Erratum 211 flag untouched. The process-wide staging
                // buffer remains valid for a late completion; only a system reset can
                // safely reclaim this terminal controller state.
                regs.usbpullup.write(|w| w.connect().disabled());
                self.latch_fault(cs, UsbdFault::InDmaTimeout { endpoint: i as u8 });
                return Err(UsbError::InvalidState);
            }
            regs.events_endepin[i].reset();
            let amount = epin[i].amount.read().amount().bits();
            let dma_odd = self.dma_odd.borrow(cs);
            dma_odd.set(accumulated_dma_odd(dma_odd.get(), amount));
            dma_end(applicable);

            // Clear EPSTATUS.EPIN[i] flag
            regs.epstatus.write(|w| unsafe { w.bits(1 << i) });

            // Mark the endpoint as busy
            busy_in_endpoints.set(busy_in_endpoints.get() | (1 << i));

            Ok(buf.len())
        })
    }

    fn read(&self, ep_addr: EndpointAddress, buf: &mut [u8]) -> usb_device::Result<usize> {
        if !self.is_used(ep_addr) {
            return Err(UsbError::InvalidEndpoint);
        }

        if ep_addr.is_in() {
            return Err(UsbError::InvalidEndpoint);
        }

        let i = ep_addr.index();
        critical_section::with(move |cs| {
            let applicable = self.operational_errata(cs)?;
            let regs = self.regs(&cs);

            // Control EP 0 is special
            if i == 0 {
                // Control setup packet is special, since it is put in registers, not a buffer.
                if regs.events_ep0setup.read().events_ep0setup().bit_is_set() {
                    regs.events_ep0setup.reset();

                    let ep0_state = self.ep0_state.borrow(cs);
                    let mut state = ep0_state.get();
                    let n = self.read_control_setup(regs, buf, &mut state)?;
                    ep0_state.set(state);

                    return Ok(n);
                } else {
                    // Is the endpoint ready?
                    if regs
                        .events_ep0datadone
                        .read()
                        .events_ep0datadone()
                        .bit_is_clear()
                    {
                        // Not yet ready.
                        return Err(UsbError::WouldBlock);
                    }
                }
            } else {
                // Is the endpoint ready?
                let epdatastatus = regs.epdatastatus.read().bits();
                if epdatastatus & (1 << (i + 16)) == 0 {
                    // Not yet ready.
                    return Err(UsbError::WouldBlock);
                }
            }

            // Check that the packet fits into the buffer
            let size = regs.size.epout[i].read().bits();
            if size as usize > buf.len() || size as usize > DMA_BUFFER_SIZE {
                return Err(UsbError::BufferOverflow);
            }

            // Clear status
            if i == 0 {
                regs.events_ep0datadone.reset();
            } else {
                regs.epdatastatus
                    .write(|w| unsafe { w.bits(1 << (i + 16)) });
            }

            // We checked that the endpoint has data, time to read it

            let epout = [
                &regs.epout0,
                &regs.epout1,
                &regs.epout2,
                &regs.epout3,
                &regs.epout4,
                &regs.epout5,
                &regs.epout6,
                &regs.epout7,
            ];
            // Receive into static storage so a late EasyDMA completion cannot write
            // through the caller's expired mutable slice after a timeout return.
            let dma_buf = unsafe { &mut *DMA_OUT_BUFFER.0.get() };
            epout[i]
                .ptr
                .write(|w| unsafe { w.bits(dma_buf.as_mut_ptr() as u32) });
            // MAXCNT must match SIZE
            epout[i].maxcnt.write(|w| unsafe { w.bits(size) });

            regs.events_endepout[i].reset();
            dma_start(applicable);
            regs.tasks_startepout[i].write(|w| w.tasks_startepout().set_bit());
            let completed = poll_with_budget(DMA_COMPLETE_POLL_BUDGET, || {
                regs.events_endepout[i]
                    .read()
                    .events_endepout()
                    .bit_is_set()
            });
            if !completed {
                // As above, an absent ENDEPOUT acknowledgement means EasyDMA may
                // still own the permanent staging buffer. Disconnect the host, retain
                // ENABLE and the 211 workaround, and require a system reset.
                regs.usbpullup.write(|w| w.connect().disabled());
                self.latch_fault(cs, UsbdFault::OutDmaTimeout { endpoint: i as u8 });
                return publish_out_transfer(None, buf, size as usize);
            }
            regs.events_endepout[i].reset();
            let amount = epout[i].amount.read().amount().bits();
            let dma_odd = self.dma_odd.borrow(cs);
            dma_odd.set(accumulated_dma_odd(dma_odd.get(), amount));
            dma_end(applicable);
            let count = publish_out_transfer(Some(dma_buf), buf, size as usize)?;

            // TODO: ISO

            // Enable the endpoint
            regs.size.epout[i].reset();

            Ok(count)
        })
    }

    fn set_stalled(&self, ep_addr: EndpointAddress, stalled: bool) {
        // usb-device forwards endpoint recipients from host control requests. Reject
        // unadvertised directions and EP8..EP15 before masking or indexing the regular
        // EP0..EP7 hardware arrays.
        if !self.is_used(ep_addr) {
            return;
        }
        critical_section::with(move |cs| {
            if self.operational_errata(cs).is_err() {
                return;
            }
            let regs = self.regs(&cs);

            unsafe {
                if ep_addr.index() == 0 {
                    regs.tasks_ep0stall
                        .write(|w| w.tasks_ep0stall().bit(stalled));
                } else {
                    regs.epstall.write(|w| {
                        w.ep()
                            .bits(ep_addr.index() as u8 & 0b111)
                            .io()
                            .bit(ep_addr.is_in())
                            .stall()
                            .bit(stalled)
                    });
                }
            }

            if stalled {
                let busy_in_endpoints = self.busy_in_endpoints.borrow(cs);
                busy_in_endpoints.set(busy_in_endpoints.get() & !(1 << ep_addr.index()));
            }
        });
    }

    fn is_stalled(&self, ep_addr: EndpointAddress) -> bool {
        if !self.is_used(ep_addr) {
            return false;
        }
        critical_section::with(move |cs| {
            if self.operational_errata(cs).is_err() {
                return false;
            }
            let regs = self.regs(&cs);

            let i = ep_addr.index();
            match ep_addr.direction() {
                UsbDirection::Out => regs.halted.epout[i].read().getstatus().is_halted(),
                UsbDirection::In => regs.halted.epin[i].read().getstatus().is_halted(),
            }
        })
    }

    #[inline]
    fn suspend(&self) {
        critical_section::with(move |cs| {
            if self.has_fault(cs) || !T::vbus_present() {
                return;
            }
            let regs = self.regs(&cs);
            let lifecycle = self.lifecycle.borrow(cs);
            let Lifecycle::Active { errata: applicable } = lifecycle.get() else {
                return;
            };
            if regs.eventcause.read().resume().bit_is_set() {
                return;
            }
            regs.lowpower.write(|w| w.lowpower().low_power());
            self.set_lifecycle(cs, Lifecycle::Suspended { errata: applicable });

            // RESUME can race the LOWPOWER write. Wake immediately when that cause is
            // already pending, but leave the cause set so usb-device still observes
            // and publishes the Resume transition on its next poll.
            if regs.eventcause.read().resume().bit_is_set() {
                self.begin_wake(cs, regs, applicable);
            }
        });
    }

    #[inline]
    fn resume(&self) {
        critical_section::with(move |cs| {
            if self.has_fault(cs) {
                return;
            }
            let regs = self.regs(&cs);
            match self.lifecycle.borrow(cs).get() {
                Lifecycle::Suspended { errata: applicable }
                | Lifecycle::Active { errata: applicable } => self.begin_wake(cs, regs, applicable),
                Lifecycle::Waking { .. } => {}
                _ => {}
            }
        });
    }

    fn poll(&self) -> PollResult {
        critical_section::with(move |cs| {
            if self.has_fault(cs) {
                return PollResult::None;
            }
            let regs = self.regs(&cs);
            self.advance_lifecycle(cs, regs);
            if self.has_fault(cs) {
                return PollResult::None;
            }
            match self.lifecycle.borrow(cs).get() {
                Lifecycle::Active { .. } => {}
                Lifecycle::Suspended { .. } => {
                    // Nordic marks endpoint, SOF, and configuration registers
                    // unavailable in LOWPOWER. Only the wake-capable USBEVENT path
                    // is sampled until resume has been acknowledged.
                    if regs.events_usbevent.read().events_usbevent().bit_is_set() {
                        let causes = acknowledge_usbevent(
                            || regs.events_usbevent.reset(),
                            || regs.eventcause.read().bits(),
                            |causes| regs.eventcause.write(|w| unsafe { w.bits(causes) }),
                        );
                        if let Some(result) = suspend_resume_poll_result(causes) {
                            return result;
                        }
                    }
                    return PollResult::None;
                }
                _ => return PollResult::None,
            }
            let busy_in_endpoints = self.busy_in_endpoints.borrow(cs);

            if regs.events_usbreset.read().events_usbreset().bit_is_set() {
                regs.events_usbreset.reset();
                return PollResult::Reset;
            } else if regs.events_usbevent.read().events_usbevent().bit_is_set() {
                let causes = acknowledge_usbevent(
                    || regs.events_usbevent.reset(),
                    || regs.eventcause.read().bits(),
                    |causes| regs.eventcause.write(|w| unsafe { w.bits(causes) }),
                );
                if let Some(result) = suspend_resume_poll_result(causes) {
                    return result;
                }
            }

            if regs.events_sof.read().events_sof().bit_is_set() {
                regs.events_sof.reset();

                // Check if we have a timeout for EP0 IN transfer
                let ep0_state = self.ep0_state.borrow(cs);
                let mut state = ep0_state.get();
                if let TransferState::Started(counter) = state.in_transfer_state {
                    let frame_counter = regs.framecntr.read().framecntr().bits();
                    if frame_counter.wrapping_sub(counter) >= 5 {
                        // Send a status stage to ACK a pending OUT transfer
                        regs.tasks_ep0status
                            .write(|w| w.tasks_ep0status().set_bit());

                        // reset the state
                        state.in_transfer_state = TransferState::NoTransfer;
                        ep0_state.set(state);
                        // Windows immediately starts a second Get Device Descriptor
                        // without the intervening bus reset commonly issued by Linux.
                        // The status fallback cancels the first transfer, so EP0 must
                        // no longer remain busy or that second response WouldBlock.
                        busy_in_endpoints
                            .set(release_ep0_after_status_fallback(busy_in_endpoints.get()));
                    }
                }
            }

            // Check for any finished transmissions.
            let mut in_complete = 0;
            let mut out_complete = 0;
            if regs
                .events_ep0datadone
                .read()
                .events_ep0datadone()
                .bit_is_set()
            {
                let ep0_state = self.ep0_state.borrow(cs).get();
                if ep0_state.direction == UsbDirection::In {
                    // Clear event, since we must only report this once.
                    regs.events_ep0datadone.reset();

                    in_complete |= 1;

                    // Reset a timeout for the IN transfer
                    let ep0_state = self.ep0_state.borrow(cs);
                    let mut state = ep0_state.get();
                    state.in_transfer_state = TransferState::NoTransfer;
                    ep0_state.set(state);

                    // Mark the endpoint as not busy
                    busy_in_endpoints.set(busy_in_endpoints.get() & !1);
                } else {
                    // Do not clear OUT events, since we have to continue reporting them until the
                    // buffer is read.

                    out_complete |= 1;
                }
            }
            let epdatastatus = regs.epdatastatus.read().bits();
            for i in 1..=7 {
                if epdatastatus & (1 << i) != 0 {
                    // EPDATASTATUS.EPIN[i] is set

                    // Clear event, since we must only report this once.
                    regs.epdatastatus.write(|w| unsafe { w.bits(1 << i) });

                    in_complete |= 1 << i;

                    // Mark the endpoint as not busy
                    busy_in_endpoints.set(busy_in_endpoints.get() & !(1 << i));
                }
                if epdatastatus & (1 << (i + 16)) != 0 {
                    // EPDATASTATUS.EPOUT[i] is set
                    // This flag will be cleared in `read()`

                    out_complete |= 1 << i;
                }
            }

            // Setup packets are only relevant on the control EP 0.
            let mut ep_setup = 0;
            if regs.events_ep0setup.read().events_ep0setup().bit_is_set() {
                ep_setup = 1;

                // A fresh SETUP ACK aborts any older EP0 transfer even when the
                // old data stage never raised EP0DATADONE. Release EP0 before
                // usb-device handles this setup so its immediate response cannot
                // be stranded behind a stale WouldBlock.
                let ep0_state = self.ep0_state.borrow(cs);
                let mut state = ep0_state.get();
                let busy = abort_ep0_for_new_transaction(&mut state, busy_in_endpoints.get());
                ep0_state.set(state);
                busy_in_endpoints.set(busy);
                regs.events_ep0datadone.reset();

                // Reset the status-stage shortcut inherited from the aborted request.
                regs.shorts
                    .modify(|_, w| w.ep0datadone_ep0status().clear_bit());
            }

            // TODO: Check ISO EP

            if out_complete != 0 || in_complete != 0 || ep_setup != 0 {
                PollResult::Data {
                    ep_out: out_complete,
                    ep_in_complete: in_complete,
                    ep_setup,
                }
            } else {
                PollResult::None
            }
        })
    }

    fn force_reset(&self) -> usb_device::Result<()> {
        critical_section::with(move |cs| {
            if BOOTLOADER_HANDOFF_REQUEST.swap(false, Ordering::AcqRel) {
                self.bootloader_handoff.borrow(cs).set(true);
            }
            let regs = self.regs(&cs);
            self.advance_lifecycle(cs, regs);
            if self.has_fault(cs) {
                return Err(UsbError::InvalidState);
            }
            if self.bootloader_handoff.borrow(cs).get() {
                return match self.lifecycle.borrow(cs).get() {
                    Lifecycle::HandoffComplete => Ok(()),
                    Lifecycle::Active { errata: applicable }
                    | Lifecycle::Suspended { errata: applicable } => {
                        self.begin_disabling(
                            cs,
                            regs,
                            Some(ErrataOwnership::Active(applicable)),
                            None,
                        );
                        Err(UsbError::WouldBlock)
                    }
                    Lifecycle::Detaching {
                        errata: applicable, ..
                    } => {
                        self.begin_disabling(
                            cs,
                            regs,
                            Some(ErrataOwnership::Active(applicable)),
                            None,
                        );
                        Err(UsbError::WouldBlock)
                    }
                    Lifecycle::PreparingDisable { .. }
                    | Lifecycle::DisableWaking { .. }
                    | Lifecycle::ParityFixing { .. }
                    | Lifecycle::Disabling { .. } => Err(UsbError::WouldBlock),
                    _ => Err(UsbError::InvalidState),
                };
            }
            if !link_restore_allowed(false, T::vbus_present()) {
                return Err(UsbError::InvalidState);
            }
            match self.lifecycle.borrow(cs).get() {
                Lifecycle::Active { errata: applicable } => {
                    regs.usbpullup.write(|w| w.connect().disabled());
                    self.set_lifecycle(
                        cs,
                        Lifecycle::Detaching {
                            wait: Self::async_wait(DETACH_TIMEOUT_US, DETACH_POLL_BUDGET),
                            errata: applicable,
                        },
                    );
                    // Fork contract: Ok means the request has been accepted, not that
                    // reattachment has already completed. Subsequent bus polls keep D+
                    // detached until the elapsed-time or poll-count boundary, then
                    // reattach without blocking this call or masking interrupts.
                    Ok(())
                }
                Lifecycle::Detaching { .. } => Ok(()),
                Lifecycle::Suspended { errata: applicable } => {
                    self.begin_wake(cs, regs, applicable);
                    Err(UsbError::WouldBlock)
                }
                Lifecycle::Waking { .. }
                | Lifecycle::HfclkStarting { .. }
                | Lifecycle::Enabling { .. }
                | Lifecycle::PowerReady { .. }
                | Lifecycle::InitialDisabling { .. }
                | Lifecycle::InitialDetach { .. }
                | Lifecycle::PreparingDisable { .. }
                | Lifecycle::DisableWaking { .. }
                | Lifecycle::ParityFixing { .. }
                | Lifecycle::Disabling { .. }
                | Lifecycle::HandoffComplete
                | Lifecycle::AwaitVbus => Err(UsbError::WouldBlock),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use usb_device::{
        bus::PollResult,
        endpoint::{
            EndpointAddress, EndpointType, IsochronousSynchronizationType, IsochronousUsageType,
        },
        UsbDirection, UsbError,
    };

    use super::{
        abort_ep0_for_new_transaction, accumulated_dma_odd, acknowledge_usbevent,
        apply_handoff_sanitization, apply_session_register_sanitization, complete_enable_session,
        complete_wake_session, disable_preparation, endpoint_is_owned, initial_fault_for_silicon,
        link_restore_allowed, observe_initial_detach, poll_with_budget, prepare_initial_detach,
        publish_out_transfer, release_ep0_after_status_fallback, reset_ep0_transaction_registers,
        sanitize_if_supported, suspend_resume_poll_result, validate_endpoint_request, AsyncWait,
        DisablePreparation, EP0State, EnableSessionCompletion, ErrataOwnership,
        HandoffSanitization, InitialDetachObservation, InitialDetachPreparation, Lifecycle,
        PermanentClaim, RegisterBlock, StaticDmaBuffer, TerminalFault, TransferState, WakeTarget,
        DMA_BUFFER_SIZE, HANDOFF_CONFIG_ZERO_OFFSETS, HANDOFF_EVENT_CLEAR_OFFSETS,
        HANDOFF_W1C_CLEAR_OFFSETS, INITIAL_DETACH_TIMEOUT_US, ISO_SPLIT_HALF_IN,
        SESSION_SANITIZATION_WRITE_COUNT,
    };
    use crate::UsbdFault;

    #[test]
    fn staging_storage_is_easydma_aligned() {
        assert!(core::mem::align_of::<StaticDmaBuffer>() >= 4);
        assert!(core::mem::size_of::<StaticDmaBuffer>() >= DMA_BUFFER_SIZE);
    }

    #[test]
    fn cumulative_dma_parity_tracks_the_hardware_amount_not_the_request_size() {
        assert!(!accumulated_dma_odd(false, 0));
        assert!(accumulated_dma_odd(false, 1));
        assert!(!accumulated_dma_odd(false, 64));
        assert!(!accumulated_dma_odd(true, 1));
        assert!(accumulated_dma_odd(true, 2));
    }

    #[test]
    fn even_dma_count_can_disable_without_usb_power() {
        assert_eq!(
            disable_preparation(false, None, false, false, false),
            DisablePreparation::StartDisable
        );
    }

    #[test]
    fn odd_dma_count_waits_for_continuous_vbus_and_regulator_power() {
        let applicable = crate::errata::Applicability::NONE;
        let active = Some(ErrataOwnership::Active(applicable));
        assert_eq!(
            disable_preparation(true, active, false, false, false),
            DisablePreparation::WaitForPower
        );
        assert_eq!(
            disable_preparation(true, active, true, false, false),
            DisablePreparation::WaitForPower
        );
    }

    #[test]
    fn odd_dma_count_wakes_before_repair_and_repairs_before_disable() {
        let applicable = crate::errata::Applicability::NONE;
        assert_eq!(
            disable_preparation(
                true,
                Some(ErrataOwnership::Active(applicable)),
                true,
                true,
                true,
            ),
            DisablePreparation::BeginWake
        );
        assert_eq!(
            disable_preparation(
                true,
                Some(ErrataOwnership::Waking(applicable)),
                true,
                true,
                true,
            ),
            DisablePreparation::AwaitWake
        );
        assert_eq!(
            disable_preparation(
                true,
                Some(ErrataOwnership::Active(applicable)),
                true,
                true,
                false,
            ),
            DisablePreparation::StartParityDma
        );
    }

    #[test]
    fn odd_dma_count_never_runs_a_repair_in_an_unacknowledged_enable_phase() {
        let applicable = crate::errata::Applicability::NONE;
        assert_eq!(
            disable_preparation(
                true,
                Some(ErrataOwnership::Enabling(applicable)),
                true,
                true,
                false,
            ),
            DisablePreparation::UnsafeLifecycle
        );
        assert_eq!(
            disable_preparation(true, None, true, true, false),
            DisablePreparation::UnsafeLifecycle
        );
    }

    #[test]
    fn staging_storage_claim_is_permanent_and_refuses_a_second_owner() {
        let claim = PermanentClaim::new();
        {
            assert!(claim.try_claim());
        }
        // Leaving the owner's scope cannot release storage: a timed-out peripheral may
        // still complete EasyDMA against it later.
        assert!(!claim.try_claim());
    }

    #[test]
    fn unsupported_silicon_is_rejected_before_dma_claim_or_handoff_writes() {
        for (part, family, revision) in [
            (0x0005_2820, 0x0000_0010, 0),
            (0x0005_2833, 0x0000_000d, 0),
            (0, 0, 0),
        ] {
            let applicability = crate::errata::applicability_for_ids(part, family, revision);
            let claims = Cell::new(0usize);
            assert_eq!(
                initial_fault_for_silicon(applicability, || {
                    claims.set(claims.get() + 1);
                    true
                }),
                Some(UsbdFault::UnsupportedSilicon)
            );
            assert_eq!(claims.get(), 0);

            let lifecycle_writes = Cell::new(0usize);
            assert_eq!(
                sanitize_if_supported(applicability, |_| {
                    lifecycle_writes.set(lifecycle_writes.get() + 1);
                    HandoffSanitization::Complete
                }),
                HandoffSanitization::UnsupportedSilicon
            );
            assert_eq!(lifecycle_writes.get(), 0);
        }
    }

    #[test]
    fn nrf52840_identity_enters_the_claim_and_handoff_paths_once() {
        let applicability = crate::errata::applicability_for_ids(0x0005_2840, 8, 3);
        let claims = Cell::new(0usize);
        assert_eq!(
            initial_fault_for_silicon(applicability, || {
                claims.set(claims.get() + 1);
                true
            }),
            None
        );
        assert_eq!(claims.get(), 1);

        let handoffs = Cell::new(0usize);
        assert_eq!(
            sanitize_if_supported(applicability, |_| {
                handoffs.set(handoffs.get() + 1);
                HandoffSanitization::Complete
            }),
            HandoffSanitization::Complete
        );
        assert_eq!(handoffs.get(), 1);
    }

    #[test]
    fn zero_budget_never_polls_hardware() {
        let calls = Cell::new(0);
        assert!(!poll_with_budget(0, || {
            calls.set(calls.get() + 1);
            true
        }));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn readiness_on_the_last_permitted_poll_succeeds() {
        let calls = Cell::new(0);
        assert!(poll_with_budget(3, || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        }));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn readiness_after_the_budget_times_out_at_the_exact_limit() {
        let calls = Cell::new(0);
        assert!(!poll_with_budget(3, || {
            calls.set(calls.get() + 1);
            calls.get() == 4
        }));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn asynchronous_wait_without_a_clock_expires_on_exact_poll_budget() {
        let mut wait = AsyncWait::new(None, 100, 3);
        assert!(!wait.failed_observation(None));
        assert!(!wait.failed_observation(None));
        assert!(wait.failed_observation(None));
        assert!(wait.failed_observation(None));
    }

    #[test]
    fn asynchronous_wait_uses_wrapping_elapsed_microseconds_when_available() {
        let mut wait = AsyncWait::new(Some(u32::MAX - 4), 10, 10);
        assert!(!wait.failed_observation(Some(u32::MAX)));
        assert!(!wait.failed_observation(Some(4)));
        assert!(wait.failed_observation(Some(5)));
    }

    #[test]
    fn disappearing_clock_degrades_to_poll_count_without_time_claims() {
        let mut wait = AsyncWait::new(Some(10), 100, 2);
        assert!(!wait.failed_observation(None));
        assert!(wait.failed_observation(None));
    }

    #[test]
    fn frozen_clock_cannot_defeat_the_poll_count_fallback() {
        let mut wait = AsyncWait::new(Some(10), 100, 2);
        assert!(!wait.failed_observation(Some(10)));
        assert!(wait.failed_observation(Some(10)));
    }

    #[test]
    fn progressing_clock_cannot_expire_early_from_a_fast_poll_loop() {
        let mut wait = AsyncWait::new(Some(10), 100, 2);
        for now in 11..110 {
            assert!(!wait.failed_observation(Some(now)));
        }
        assert!(wait.failed_observation(Some(110)));
    }

    #[test]
    fn initial_detach_starts_only_after_pullup_off_and_confirmed_disable() {
        let phase = Cell::new(0usize);
        let preparation = prepare_initial_detach(
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
            },
            || {
                assert_eq!(phase.get(), 1);
                phase.set(2);
                true
            },
            || panic!("an already-disabled controller must not get another disable write"),
            true,
            || {
                assert_eq!(phase.get(), 2);
                phase.set(3);
                Some(47)
            },
        );

        let InitialDetachPreparation::Dwelling(wait) = preparation else {
            panic!("confirmed disable with VBUS must enter the dwell");
        };
        assert_eq!(phase.get(), 3);
        assert_eq!(wait.started_us, Some(47));
        assert_eq!(wait.timeout_us, INITIAL_DETACH_TIMEOUT_US);
    }

    #[test]
    fn inherited_enabled_session_is_disabled_before_any_detach_timestamp() {
        let phase = Cell::new(0usize);
        let preparation = prepare_initial_detach(
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
            },
            || {
                assert_eq!(phase.get(), 1);
                phase.set(2);
                false
            },
            || {
                assert_eq!(phase.get(), 2);
                phase.set(3);
            },
            true,
            || {
                assert_eq!(phase.get(), 3);
                phase.set(4);
                Some(81)
            },
        );

        let InitialDetachPreparation::Disabling(wait) = preparation else {
            panic!("ENABLE must read back zero before the detach dwell starts");
        };
        assert_eq!(phase.get(), 4);
        assert_eq!(wait.started_us, Some(81));
        assert_eq!(wait.timeout_us, super::ENABLE_READY_TIMEOUT_US);
    }

    #[test]
    fn initial_detach_poll_fallback_starts_enable_on_the_exact_last_observation() {
        let mut wait = AsyncWait::new(None, INITIAL_DETACH_TIMEOUT_US, 3);
        assert_eq!(
            observe_initial_detach(&mut wait, true, None),
            InitialDetachObservation::Waiting
        );
        assert_eq!(
            observe_initial_detach(&mut wait, true, None),
            InitialDetachObservation::Waiting
        );
        assert_eq!(
            observe_initial_detach(&mut wait, true, None),
            InitialDetachObservation::StartEnable
        );
    }

    #[test]
    fn initial_detach_clock_never_starts_enable_before_twenty_milliseconds() {
        let started = u32::MAX - 9_999;
        let mut wait = AsyncWait::new(Some(started), INITIAL_DETACH_TIMEOUT_US, 2);
        assert_eq!(
            observe_initial_detach(
                &mut wait,
                true,
                Some(started.wrapping_add(INITIAL_DETACH_TIMEOUT_US - 1)),
            ),
            InitialDetachObservation::Waiting
        );
        assert_eq!(
            observe_initial_detach(
                &mut wait,
                true,
                Some(started.wrapping_add(INITIAL_DETACH_TIMEOUT_US)),
            ),
            InitialDetachObservation::StartEnable
        );
    }

    #[test]
    fn vbus_loss_wins_over_an_expired_initial_detach() {
        let mut wait = AsyncWait::new(None, INITIAL_DETACH_TIMEOUT_US, 1);
        assert_eq!(
            observe_initial_detach(&mut wait, false, None),
            InitialDetachObservation::VbusLost
        );
        // The VBUS-loss observation does not consume the final fallback poll or
        // authorize ENABLE. If VBUS were still present, that same poll would be
        // the exact transition boundary.
        assert_eq!(
            observe_initial_detach(&mut wait, true, None),
            InitialDetachObservation::StartEnable
        );
    }

    #[test]
    fn absent_vbus_skips_the_synthetic_dwell_after_a_confirmed_disable() {
        let timestamps = Cell::new(0usize);
        let preparation = prepare_initial_detach(
            || {},
            || true,
            || panic!("confirmed disabled"),
            false,
            || {
                timestamps.set(timestamps.get() + 1);
                Some(0)
            },
        );
        assert_eq!(preparation, InitialDetachPreparation::AwaitVbus);
        assert_eq!(timestamps.get(), 0);
    }

    #[test]
    fn windows_status_fallback_releases_only_control_endpoint_zero() {
        assert_eq!(release_ep0_after_status_fallback(0), 0);
        assert_eq!(release_ep0_after_status_fallback(1), 0);
        assert_eq!(release_ep0_after_status_fallback(0b1011), 0b1010);
        assert_eq!(release_ep0_after_status_fallback(u16::MAX), u16::MAX - 1);
    }

    #[test]
    fn fresh_setup_aborts_stale_ep0_transfer_before_the_next_response() {
        let mut state = EP0State {
            direction: UsbDirection::In,
            remaining_size: 64,
            in_transfer_state: TransferState::Started(17),
            is_set_address: true,
        };
        let busy = abort_ep0_for_new_transaction(&mut state, 0b1011);

        assert_eq!(busy, 0b1010);
        assert_eq!(state.direction, UsbDirection::Out);
        assert_eq!(state.remaining_size, 0);
        assert!(matches!(state.in_transfer_state, TransferState::NoTransfer));
        assert!(!state.is_set_address);
    }

    #[test]
    fn bus_reset_clears_old_ep0_state_without_consuming_a_fresh_setup() {
        let step = Cell::new(0usize);
        reset_ep0_transaction_registers(
            || {
                assert_eq!(step.get(), 0);
                step.set(1);
            },
            || {
                assert_eq!(step.get(), 1);
                step.set(2);
            },
        );
        assert_eq!(step.get(), 2);
    }

    #[test]
    fn only_active_lifecycle_publishes_full_usb_events() {
        let applicable = crate::errata::Applicability::NONE;
        assert!(Lifecycle::Active { errata: applicable }.accepts_full_usb_events());
        assert!(!Lifecycle::Suspended { errata: applicable }.accepts_full_usb_events());
        assert!(!Lifecycle::AwaitVbus.accepts_full_usb_events());
        assert!(!Lifecycle::InitialDetach {
            wait: AsyncWait::new(None, 1, 1),
        }
        .accepts_full_usb_events());
        assert!(!Lifecycle::Enabling {
            wait: AsyncWait::new(None, 1, 1),
            errata: applicable,
        }
        .accepts_full_usb_events());
    }

    #[test]
    fn isochronous_allocation_is_rejected_before_regular_endpoint_indexing() {
        let endpoint_type = EndpointType::Isochronous {
            synchronization: IsochronousSynchronizationType::NoSynchronization,
            usage: IsochronousUsageType::Data,
        };
        assert_eq!(
            validate_endpoint_request(endpoint_type, DMA_BUFFER_SIZE as u16),
            Err(UsbError::Unsupported)
        );
    }

    #[test]
    fn regular_endpoint_packets_must_fit_the_staging_storage() {
        assert_eq!(
            validate_endpoint_request(EndpointType::Bulk, DMA_BUFFER_SIZE as u16),
            Ok(())
        );
        assert_eq!(
            validate_endpoint_request(EndpointType::Bulk, DMA_BUFFER_SIZE as u16 + 1),
            Err(UsbError::EndpointMemoryOverflow)
        );
    }

    #[test]
    fn handoff_offsets_and_actions_cover_the_pac_event_map() {
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_usbreset), 0x100);
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_started), 0x104);
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_endepin), 0x108);
        assert_eq!(
            core::mem::offset_of!(RegisterBlock, events_ep0datadone),
            0x128
        );
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_endisoin), 0x12c);
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_endepout), 0x130);
        assert_eq!(
            core::mem::offset_of!(RegisterBlock, events_endisoout),
            0x150
        );
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_sof), 0x154);
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_usbevent), 0x158);
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_ep0setup), 0x15c);
        assert_eq!(core::mem::offset_of!(RegisterBlock, events_epdata), 0x160);
        assert_eq!(core::mem::offset_of!(RegisterBlock, intenclr), 0x308);
        assert_eq!(core::mem::offset_of!(RegisterBlock, eventcause), 0x400);
        assert_eq!(core::mem::offset_of!(RegisterBlock, epstatus), 0x468);
        assert_eq!(core::mem::offset_of!(RegisterBlock, epdatastatus), 0x46c);
        assert_eq!(core::mem::offset_of!(RegisterBlock, enable), 0x500);
        assert_eq!(core::mem::offset_of!(RegisterBlock, usbpullup), 0x504);

        let mut actions = [(usize::MAX, 0u32); 4 + SESSION_SANITIZATION_WRITE_COUNT];
        let mut count = 0;
        let after_disable = Cell::new(false);
        let result = apply_handoff_sanitization(
            |offset, value| {
                actions[count] = (offset, value);
                count += 1;
            },
            || true,
            || after_disable.set(true),
        );
        assert_eq!(result, HandoffSanitization::Complete);
        assert!(after_disable.get());
        assert_eq!(count, actions.len());
        assert_eq!(actions[0], (0x504, 0));
        assert_eq!(actions[1], (0x308, u32::MAX));
        assert_eq!(actions[2], (0x05c, 1));
        assert_eq!(actions[3], (0x500, 0));

        let config_start = 4;
        for (index, offset) in HANDOFF_CONFIG_ZERO_OFFSETS.iter().enumerate() {
            assert_eq!(actions[config_start + index], (*offset, 0));
        }
        let event_start = config_start + HANDOFF_CONFIG_ZERO_OFFSETS.len();
        for (index, offset) in HANDOFF_EVENT_CLEAR_OFFSETS.iter().enumerate() {
            assert_eq!(actions[event_start + index], (*offset, 0));
        }
        let w1c_start = event_start + HANDOFF_EVENT_CLEAR_OFFSETS.len();
        for (index, offset) in HANDOFF_W1C_CLEAR_OFFSETS.iter().enumerate() {
            assert_eq!(actions[w1c_start + index], (*offset, u32::MAX));
        }
    }

    #[test]
    fn enabled_session_sanitizer_has_the_exact_register_write_order() {
        let mut actions = [(usize::MAX, 0u32); SESSION_SANITIZATION_WRITE_COUNT];
        let mut count = 0usize;
        apply_session_register_sanitization(|offset, value| {
            actions[count] = (offset, value);
            count += 1;
        });

        assert_eq!(count, SESSION_SANITIZATION_WRITE_COUNT);
        let mut cursor = 0usize;
        for offset in HANDOFF_CONFIG_ZERO_OFFSETS {
            assert_eq!(actions[cursor], (offset, 0));
            cursor += 1;
        }
        for offset in HANDOFF_EVENT_CLEAR_OFFSETS {
            assert_eq!(actions[cursor], (offset, 0));
            cursor += 1;
        }
        for offset in HANDOFF_W1C_CLEAR_OFFSETS {
            assert_eq!(actions[cursor], (offset, u32::MAX));
            cursor += 1;
        }
        assert_eq!(cursor, SESSION_SANITIZATION_WRITE_COUNT);
        assert!(
            !HANDOFF_CONFIG_ZERO_OFFSETS.contains(&core::mem::offset_of!(RegisterBlock, lowpower))
        );
        assert!(
            !HANDOFF_CONFIG_ZERO_OFFSETS.contains(&core::mem::offset_of!(RegisterBlock, isosplit))
        );
    }

    #[test]
    fn ready_completion_without_inherited_lowpower_finishes_before_releasing_dpdm() {
        let phase = Cell::new(0usize);
        let mut actions = [(usize::MAX, 0u32); 2 + SESSION_SANITIZATION_WRITE_COUNT];
        let mut count = 0usize;
        let result = complete_enable_session(
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
            },
            || {
                assert_eq!(phase.get(), 1);
                phase.set(2);
            },
            || {
                assert_eq!(phase.get(), 2);
                phase.set(3);
            },
            || {
                assert_eq!(phase.get(), 3);
                phase.set(4);
                false
            },
            || panic!("ForceNormal must not be requested when LOWPOWER is clear"),
            || panic!("Erratum 171 must not be reopened when LOWPOWER is clear"),
            || panic!("LOWPOWER must not be rewritten when it is already ForceNormal"),
            || {
                assert_eq!(phase.get(), 5);
                phase.set(6);
            },
            |offset, value| {
                if count == 0 {
                    assert_eq!(phase.get(), 4);
                    phase.set(5);
                } else {
                    assert_eq!(phase.get(), 6);
                }
                actions[count] = (offset, value);
                count += 1;
            },
        );

        assert_eq!(result, EnableSessionCompletion::Finished);
        assert_eq!(phase.get(), 6);
        assert_eq!(count, actions.len());
        assert_eq!(
            actions[0],
            (core::mem::offset_of!(RegisterBlock, tasks_dpdmnodrive), 1)
        );
        assert_eq!(actions[1], (HANDOFF_CONFIG_ZERO_OFFSETS[0], 0));
        assert_eq!(
            actions[actions.len() - 1],
            (
                core::mem::offset_of!(RegisterBlock, isosplit),
                ISO_SPLIT_HALF_IN,
            )
        );
    }

    #[test]
    fn inherited_lowpower_opens_an_acknowledged_wake_before_any_dpdm_release() {
        let phase = Cell::new(0usize);
        let writes = Cell::new(0usize);
        let result = complete_enable_session(
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
            },
            || {
                assert_eq!(phase.get(), 1);
                phase.set(2);
            },
            || {
                assert_eq!(phase.get(), 2);
                phase.set(3);
            },
            || {
                assert_eq!(phase.get(), 3);
                phase.set(4);
                true
            },
            || {
                assert_eq!(phase.get(), 4);
                phase.set(5);
            },
            || {
                assert_eq!(phase.get(), 5);
                phase.set(6);
            },
            || {
                assert_eq!(phase.get(), 6);
                phase.set(7);
            },
            || panic!("Erratum 166 must wait for USBWUALLOWED"),
            |_, _| writes.set(writes.get() + 1),
        );

        assert_eq!(result, EnableSessionCompletion::WakePending);
        assert_eq!(phase.get(), 7);
        assert_eq!(writes.get(), 0);
    }

    #[test]
    fn acknowledged_inherited_wake_closes_171_before_dpdm_release() {
        let phase = Cell::new(0usize);
        let mut actions = [(usize::MAX, 0u32); 2 + SESSION_SANITIZATION_WRITE_COUNT];
        let mut count = 0usize;
        let target = complete_wake_session(
            WakeTarget::FinishEnable,
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
            },
            || {
                assert_eq!(phase.get(), 1);
                phase.set(2);
            },
            || {
                assert_eq!(phase.get(), 3);
                phase.set(4);
            },
            |offset, value| {
                if count == 0 {
                    assert_eq!(phase.get(), 2);
                    phase.set(3);
                } else {
                    assert_eq!(phase.get(), 4);
                }
                actions[count] = (offset, value);
                count += 1;
            },
        );

        assert_eq!(target, WakeTarget::FinishEnable);
        assert_eq!(phase.get(), 4);
        assert_eq!(count, actions.len());
        assert_eq!(
            actions[0],
            (core::mem::offset_of!(RegisterBlock, tasks_dpdmnodrive), 1)
        );
        assert_eq!(
            actions[actions.len() - 1],
            (
                core::mem::offset_of!(RegisterBlock, isosplit),
                ISO_SPLIT_HALF_IN,
            )
        );
    }

    #[test]
    fn ordinary_resume_never_runs_enable_session_sanitization() {
        let phase = Cell::new(0usize);
        let writes = Cell::new(0usize);
        let target = complete_wake_session(
            WakeTarget::Active,
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
            },
            || {
                assert_eq!(phase.get(), 1);
                phase.set(2);
            },
            || panic!("ordinary resume must not reapply enabled-session Erratum 166"),
            |_, _| writes.set(writes.get() + 1),
        );

        assert_eq!(target, WakeTarget::Active);
        assert_eq!(phase.get(), 2);
        assert_eq!(writes.get(), 0);
    }

    #[test]
    fn usbevent_acknowledgement_rearms_before_snapshot_and_exact_w1c() {
        let phase = Cell::new(0usize);
        let observed = (1 << 8) | (1 << 9) | (1 << 10);
        let cleared = Cell::new(0u32);
        let result = acknowledge_usbevent(
            || {
                assert_eq!(phase.get(), 0);
                phase.set(1);
            },
            || {
                assert_eq!(phase.get(), 1);
                phase.set(2);
                observed
            },
            |causes| {
                assert_eq!(phase.get(), 2);
                phase.set(3);
                cleared.set(causes);
            },
        );

        assert_eq!(phase.get(), 3);
        assert_eq!(result, observed);
        assert_eq!(cleared.get(), observed);
    }

    #[test]
    fn resume_wins_when_suspend_and_resume_share_one_usbevent_snapshot() {
        assert!(matches!(
            suspend_resume_poll_result(1 << 8),
            Some(PollResult::Suspend)
        ));
        assert!(matches!(
            suspend_resume_poll_result(1 << 9),
            Some(PollResult::Resume)
        ));
        assert!(matches!(
            suspend_resume_poll_result((1 << 8) | (1 << 9)),
            Some(PollResult::Resume)
        ));
        assert!(suspend_resume_poll_result(1 << 10).is_none());
    }

    #[test]
    fn handoff_timeout_never_clears_registers_after_unconfirmed_disable() {
        let writes = Cell::new(0usize);
        let after_disable = Cell::new(false);
        let result = apply_handoff_sanitization(
            |_, _| writes.set(writes.get() + 1),
            || false,
            || after_disable.set(true),
        );
        assert_eq!(result, HandoffSanitization::DisableTimeout);
        assert_eq!(writes.get(), 4);
        assert!(!after_disable.get());
    }

    #[test]
    fn handoff_closes_errata_after_disable_readback_and_before_clearing() {
        let writes = Cell::new(0usize);
        let reads = Cell::new(0usize);
        let result = apply_handoff_sanitization(
            |_, _| writes.set(writes.get() + 1),
            || {
                reads.set(reads.get() + 1);
                reads.get() == 2
            },
            || assert_eq!(writes.get(), 4),
        );
        assert_eq!(result, HandoffSanitization::Complete);
        assert_eq!(reads.get(), 2);
        assert_eq!(writes.get(), 4 + SESSION_SANITIZATION_WRITE_COUNT);
    }

    #[test]
    fn endpoint_ownership_rejects_hostile_indices_and_wrong_directions() {
        let used_in = 0b0000_0011; // EP0 + EP1 IN
        let used_out = 0b0000_0101; // EP0 + EP2 OUT
        assert!(endpoint_is_owned(
            used_in,
            used_out,
            EndpointAddress::from_parts(1, UsbDirection::In)
        ));
        assert!(!endpoint_is_owned(
            used_in,
            used_out,
            EndpointAddress::from_parts(1, UsbDirection::Out)
        ));
        assert!(endpoint_is_owned(
            used_in,
            used_out,
            EndpointAddress::from_parts(2, UsbDirection::Out)
        ));
        assert!(!endpoint_is_owned(
            used_in,
            used_out,
            EndpointAddress::from_parts(2, UsbDirection::In)
        ));
        for index in 8..=15 {
            assert!(!endpoint_is_owned(
                used_in,
                used_out,
                EndpointAddress::from_parts(index, UsbDirection::In)
            ));
            assert!(!endpoint_is_owned(
                used_in,
                used_out,
                EndpointAddress::from_parts(index, UsbDirection::Out)
            ));
        }
    }

    #[test]
    fn pullup_restore_requires_both_a_live_link_and_a_healthy_bus() {
        assert!(link_restore_allowed(false, true));
        assert!(!link_restore_allowed(true, true));
        assert!(!link_restore_allowed(false, false));
        assert!(!link_restore_allowed(true, false));
    }

    #[test]
    fn timed_out_out_transfer_never_publishes_staging_bytes_to_the_caller() {
        let staging = [0x3c; 8];
        let mut caller = [0xa5; 8];
        let original = caller;

        assert_eq!(
            publish_out_transfer(None, &mut caller, staging.len()),
            Err(UsbError::InvalidState)
        );
        assert_eq!(caller, original);

        assert_eq!(
            publish_out_transfer(Some(&staging), &mut caller, staging.len()),
            Ok(staging.len())
        );
        assert_eq!(caller, staging);
    }

    #[test]
    fn first_fault_is_terminal_and_prevents_later_operation_starts() {
        let mut state = TerminalFault::new(None);
        let starts = Cell::new(0);
        if state.permits_operation() {
            starts.set(starts.get() + 1);
        }

        let first = UsbdFault::OutDmaTimeout { endpoint: 3 };
        let (latched, newly_latched) = state.latch(first);
        state = latched;
        assert!(newly_latched);
        assert_eq!(state.fault(), Some(first));

        if state.permits_operation() {
            starts.set(starts.get() + 1);
        }
        let (still_latched, newly_latched) = state.latch(UsbdFault::EnableTimeout);
        assert!(!newly_latched);
        assert_eq!(still_latched.fault(), Some(first));
        assert!(!still_latched.permits_operation());
        assert_eq!(starts.get(), 1);
    }
}
