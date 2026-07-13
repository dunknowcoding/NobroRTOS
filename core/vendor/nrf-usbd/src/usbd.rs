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
// These are deliberately iteration budgets rather than timing claims. On a normal
// nRF52840 both events arrive orders of magnitude sooner; the limits only stop a broken
// controller/handoff from trapping the whole application forever. These finite polls
// still execute inside a critical section: the constants bound iteration count, while
// the actual interrupt blackout remains dependent on target clock and silicon timing.
const ENABLE_READY_POLL_BUDGET: usize = 100_000;
const DMA_COMPLETE_POLL_BUDGET: usize = 2_048;

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
// DTOGGLE/EPSTALL registers are intentionally excluded; disabling the peripheral and
// clearing endpoint enables is the defined session boundary.
const HANDOFF_CONFIG_ZERO_OFFSETS: [usize; 6] = [
    offset_of!(RegisterBlock, shorts),
    offset_of!(RegisterBlock, epinen),
    offset_of!(RegisterBlock, epouten),
    offset_of!(RegisterBlock, isosplit),
    offset_of!(RegisterBlock, lowpower),
    offset_of!(RegisterBlock, isoinconfig),
];

fn apply_handoff_sanitization(mut write: impl FnMut(usize, u32)) {
    // Disconnect first, then prevent a stale peripheral or NVIC source from observing
    // partially sanitized state. The board layer separately masks/unpends the NVIC line.
    write(offset_of!(RegisterBlock, usbpullup), 0);
    write(offset_of!(RegisterBlock, intenclr), u32::MAX);
    write(offset_of!(RegisterBlock, tasks_dpdmnodrive), 1);
    write(offset_of!(RegisterBlock, enable), 0);

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

/// Disconnects, disables, and clears a bootloader-owned nRF52 USBD session.
///
/// # Safety
///
/// `T::REGISTERS` must identify the exclusive live nRF52 USBD register block, and the
/// caller must prevent concurrent access while this handoff transaction runs.
#[doc(hidden)]
pub unsafe fn sanitize_handoff<T: UsbPeripheral>() {
    let base = T::REGISTERS.cast::<u8>();
    apply_handoff_sanitization(|offset, value| unsafe {
        base.add(offset)
            .cast_mut()
            .cast::<u32>()
            .write_volatile(value);
    });
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

fn dma_start() {
    compiler_fence(Ordering::Release);
}

fn dma_end() {
    compiler_fence(Ordering::Acquire);
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

/// USB device implementation.
///
/// This type implements the [`UsbBus`] trait and can be passed to a [`UsbBusAllocator`] to
/// configure and use the USB device.
///
/// [`UsbBusAllocator`]: usb_device::bus::UsbBusAllocator
pub struct Usbd<T: UsbPeripheral> {
    _periph: Mutex<T>,
    // argument passed to `UsbDeviceBuilder.max_packet_size_0`
    max_packet_size_0: u16,
    bufs: Buffers,
    used_in: u8,
    used_out: u8,
    ep0_state: Mutex<Cell<EP0State>>,
    busy_in_endpoints: Mutex<Cell<u16>>,
    fault: Mutex<Cell<TerminalFault>>,
}

impl<T: UsbPeripheral> Usbd<T> {
    /// Creates a new USB device wrapper, taking ownership of the raw peripheral.
    ///
    /// # Parameters
    ///
    /// * `periph`: The raw USBD peripheral.
    ///
    /// The first instance permanently claims the process-wide, EasyDMA-safe staging
    /// buffers. A later instance is constructed in a faulted state and reports
    /// [`UsbdFault::DmaStorageAlreadyClaimed`]; staging storage is never reused because a
    /// timed-out transfer has no documented cancellation task and may complete late.
    #[inline]
    pub fn new(periph: T) -> Self {
        let initial_fault = if DMA_STORAGE_CLAIM.try_claim() {
            None
        } else {
            let fault = UsbdFault::DmaStorageAlreadyClaimed;
            T::on_fault(fault);
            Some(fault)
        };
        Self {
            _periph: Mutex::new(periph),
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
            fault: Mutex::new(Cell::new(TerminalFault::new(initial_fault))),
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

    fn latch_fault(&self, cs: CriticalSection<'_>, fault: UsbdFault) {
        let current = self.fault.borrow(cs);
        let (next, newly_latched) = current.get().latch(fault);
        if newly_latched {
            current.set(next);
            T::on_fault(fault);
        }
    }

    /// Returns the fatal fault latched by this bus instance, if any.
    ///
    /// A fault is permanent for the lifetime of the instance because an EasyDMA timeout
    /// cannot be cancelled safely by the nRF USBD register interface.
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

            errata::pre_enable();

            regs.enable.write(|w| w.enable().enabled());

            // Wait until the peripheral is ready.
            if !poll_with_budget(ENABLE_READY_POLL_BUDGET, || {
                regs.eventcause.read().ready().is_ready()
            }) {
                regs.usbpullup.write(|w| w.connect().disabled());
                regs.enable.write(|w| w.enable().disabled());
                errata::post_enable();
                self.latch_fault(cs, UsbdFault::EnableTimeout);
                return;
            }
            regs.eventcause.write(|w| w.ready().set_bit()); // Write 1 to clear.

            errata::post_enable();

            // Enable the USB pullup, allowing enumeration.
            regs.usbpullup.write(|w| w.connect().enabled());
        });
    }

    #[inline]
    fn reset(&self) {
        critical_section::with(move |cs| {
            if self.has_fault(cs) {
                return;
            }
            let regs = self.regs(&cs);

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

    fn write(&self, ep_addr: EndpointAddress, buf: &[u8]) -> usb_device::Result<usize> {
        if critical_section::with(|cs| self.has_fault(cs)) {
            return Err(UsbError::InvalidState);
        }
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
            if self.has_fault(cs) {
                return Err(UsbError::InvalidState);
            }
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
            // Clone-safe: skip the EPSTATUS busy check for EP0. On cloned nRF52840 USBD
            // silicon EPSTATUS reads a constant 0x00010001 (EPIN0/EPOUT0 bits stuck set),
            // so this always returned WouldBlock for EP0 and the device descriptor was
            // never sent (enumeration stuck at the very first GET_DESCRIPTOR). EP0 is
            // already serialised by busy_in_endpoints and the inline ENDEPIN wait below.
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
                // EPIN0: a short packet (len < max_packet_size0) ends the data stage; the
                // host then sends an OUT token we must ACK (the status stage). On genuine
                // silicon the EP0DATADONE->EP0STATUS hardware shortcut does this, but a
                // cloned USBD mishandles it and truncates multi-packet control IN. So we
                // never arm the shortcut and instead flag the final packet; poll() drives
                // TASKS_EP0STATUS in software once the host has read it.
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
            dma_start();
            regs.tasks_startepin[i].write(|w| w.tasks_startepin().set_bit());
            if !poll_with_budget(DMA_COMPLETE_POLL_BUDGET, || {
                regs.events_endepin[i].read().events_endepin().bit_is_set()
            }) {
                regs.usbpullup.write(|w| w.connect().disabled());
                self.latch_fault(cs, UsbdFault::InDmaTimeout { endpoint: i as u8 });
                return Err(UsbError::InvalidState);
            }
            regs.events_endepin[i].reset();
            dma_end();

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
            if self.has_fault(cs) {
                return Err(UsbError::InvalidState);
            }
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

            dma_start();
            regs.events_endepout[i].reset();
            regs.tasks_startepout[i].write(|w| w.tasks_startepout().set_bit());
            let completed = poll_with_budget(DMA_COMPLETE_POLL_BUDGET, || {
                regs.events_endepout[i]
                    .read()
                    .events_endepout()
                    .bit_is_set()
            });
            if !completed {
                regs.usbpullup.write(|w| w.connect().disabled());
                self.latch_fault(cs, UsbdFault::OutDmaTimeout { endpoint: i as u8 });
                return publish_out_transfer(None, buf, size as usize);
            }
            regs.events_endepout[i].reset();
            dma_end();
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
            if self.has_fault(cs) {
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
            if self.has_fault(cs) {
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
            if self.has_fault(cs) {
                return;
            }
            let regs = self.regs(&cs);
            regs.lowpower.write(|w| w.lowpower().low_power());
        });
    }

    #[inline]
    fn resume(&self) {
        critical_section::with(move |cs| {
            if self.has_fault(cs) {
                return;
            }
            let regs = self.regs(&cs);

            errata::pre_wakeup();

            regs.lowpower.write(|w| w.lowpower().force_normal());
        });
    }

    fn poll(&self) -> PollResult {
        critical_section::with(move |cs| {
            if self.has_fault(cs) {
                return PollResult::None;
            }
            let regs = self.regs(&cs);
            let busy_in_endpoints = self.busy_in_endpoints.borrow(cs);

            if regs.events_usbreset.read().events_usbreset().bit_is_set() {
                regs.events_usbreset.reset();
                return PollResult::Reset;
            } else if regs.events_usbevent.read().events_usbevent().bit_is_set() {
                // "Write 1 to clear"
                if regs.eventcause.read().suspend().bit() {
                    regs.eventcause.write(|w| w.suspend().bit(true));
                    return PollResult::Suspend;
                } else if regs.eventcause.read().resume().bit() {
                    regs.eventcause.write(|w| w.resume().bit(true));
                    return PollResult::Resume;
                } else {
                    regs.events_usbevent.reset();
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

                // Reset shorts
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
            let regs = self.regs(&cs);
            regs.usbpullup.write(|w| w.connect().disabled());
            if !link_restore_allowed(self.has_fault(cs), T::vbus_present()) {
                return Err(UsbError::InvalidState);
            }
            Ok(())
        })?;

        // Delay for 1ms, to give the host a chance to detect this.
        // We run at 64 MHz, so 64k cycles are 1ms.
        cortex_m::asm::delay(64_000);

        critical_section::with(move |cs| {
            let regs = self.regs(&cs);
            if !link_restore_allowed(self.has_fault(cs), T::vbus_present()) {
                regs.usbpullup.write(|w| w.connect().disabled());
                return Err(UsbError::InvalidState);
            }
            regs.usbpullup.write(|w| w.connect().enabled());
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use usb_device::{
        endpoint::{
            EndpointAddress, EndpointType, IsochronousSynchronizationType, IsochronousUsageType,
        },
        UsbDirection, UsbError,
    };

    use super::{
        apply_handoff_sanitization, endpoint_is_owned, link_restore_allowed, poll_with_budget,
        publish_out_transfer, validate_endpoint_request, PermanentClaim, RegisterBlock,
        StaticDmaBuffer, TerminalFault, DMA_BUFFER_SIZE, HANDOFF_CONFIG_ZERO_OFFSETS,
        HANDOFF_EVENT_CLEAR_OFFSETS, HANDOFF_W1C_CLEAR_OFFSETS,
    };
    use crate::UsbdFault;

    #[test]
    fn staging_storage_is_easydma_aligned() {
        assert!(core::mem::align_of::<StaticDmaBuffer>() >= 4);
        assert!(core::mem::size_of::<StaticDmaBuffer>() >= DMA_BUFFER_SIZE);
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

        let mut actions = [(usize::MAX, 0u32); 38];
        let mut count = 0;
        apply_handoff_sanitization(|offset, value| {
            actions[count] = (offset, value);
            count += 1;
        });
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
