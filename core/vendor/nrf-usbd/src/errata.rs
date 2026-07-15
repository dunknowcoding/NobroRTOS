/// Writes `val` to `addr`. Used to apply errata workarounds.
unsafe fn poke(addr: u32, val: u32) {
    (addr as *mut u32).write_volatile(val);
}

/// Reads 32 bits from `addr`.
unsafe fn peek(addr: u32) -> u32 {
    (addr as *const u32).read_volatile()
}

const FICR_PART: u32 = 0x1000_0100;
const LEGACY_FAMILY: u32 = 0x1000_0130;
const LEGACY_REVISION: u32 = 0x1000_0134;
const NRF52840_PART: u32 = 0x0005_2840;
const NRF52840_FAMILY: u32 = 0x0000_0008;
const NRF52833_PART: u32 = 0x0005_2833;
const NRF52833_FAMILY: u32 = 0x0000_000D;
const NRF52820_PART: u32 = 0x0005_2820;
const NRF52820_FAMILY: u32 = 0x0000_0010;

const TRIM: u32 = 0x4006_EC00;
const TRIM_MAGIC: u32 = 0x0000_9375;
const ERRATA_171_FLAG: u32 = 0x4006_EC14;
const ERRATA_187_211_FLAG: u32 = 0x4006_ED14;
const ERRATA_199_DMA_PENDING: u32 = 0x4002_7C1C;
const ERRATA_166_SELECT: u32 = 0x4002_7800;
const ERRATA_166_VALUE: u32 = 0x4002_7804;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Applicability {
    erratum_166: bool,
    erratum_171: bool,
    erratum_187: bool,
    erratum_199: bool,
    erratum_211: bool,
}

impl Applicability {
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        erratum_166: false,
        erratum_171: false,
        erratum_187: false,
        erratum_199: false,
        erratum_211: false,
    };
}

/// Computes USBD workaround applicability from immutable factory IDs.
///
/// `0x1000_0130/134` are the legacy Nordic family/revision words used by the
/// production MDK predicates. Checking `FICR.INFO.PART` as well prevents an
/// unrelated nRF52 family from being treated as a supported part solely because
/// a legacy word happens to have the same value.
pub(crate) fn applicability_for_ids(
    part: u32,
    family: u32,
    revision: u32,
) -> Option<Applicability> {
    match (part, family) {
        (NRF52840_PART, NRF52840_FAMILY) => Some(Applicability {
            // Current Nordic MDK predicates mark 166 and 171 for every known
            // nRF52840 revision. They exclude revision 0 only for 187 and 211.
            erratum_166: true,
            erratum_171: true,
            erratum_187: revision != 0,
            erratum_199: true,
            erratum_211: revision != 0,
        }),
        // nRF52820/nRF52833 require Erratum 223's first-enable double cycle.
        // This fork has no PAC, board integration, or HIL coverage for either part,
        // so accepting their otherwise similar USBD register blocks would be unsafe.
        // Add them only together with the asynchronous verified-disable phase and
        // silicon/bootloader HIL that Erratum 223 requires.
        (NRF52833_PART, NRF52833_FAMILY) | (NRF52820_PART, NRF52820_FAMILY) => None,
        _ => None,
    }
}

pub(crate) fn detect() -> Option<Applicability> {
    unsafe { applicability_for_ids(peek(FICR_PART), peek(LEGACY_FAMILY), peek(LEGACY_REVISION)) }
}

/// Replays the transaction used by current Nordic nrfx USBD production code.
///
/// Keep this body literal. When TRIM is zero nrfx emits `magic, flag, magic`;
/// after a bootloader has initialized TRIM it writes only the flag. The
/// standalone published errata snippets have used a different unconditional
/// final-magic form, so they must not be substituted for this handoff-safe nrfx
/// sequence without revalidating bootloader-to-application operation.
fn apply_nrfx_transaction(
    trim: u32,
    flag_addr: u32,
    flag_value: u32,
    mut write: impl FnMut(u32, u32),
) {
    if trim == 0 {
        write(TRIM, TRIM_MAGIC);
        write(flag_addr, flag_value);
        write(TRIM, TRIM_MAGIC);
    } else {
        write(flag_addr, flag_value);
    }
}

fn apply_live(flag_addr: u32, flag_value: u32) {
    unsafe {
        let trim = peek(TRIM);
        apply_nrfx_transaction(trim, flag_addr, flag_value, |addr, value| poke(addr, value));
    }
}

fn apply_erratum_166_configuration(applicable: Applicability, mut write: impl FnMut(u32, u32)) {
    if applicable.erratum_166 {
        // Keep this pair literal and ordered like current Nordic nrfx_usbd_enable().
        // The first word selects the hidden ISO configuration register; the second
        // commits its required value for this enabled peripheral session.
        write(ERRATA_166_SELECT, 0x0000_07E3);
        write(ERRATA_166_VALUE, 0x0000_0040);
    }
}

/// Applies enabled-session Erratum 166 configuration after READY (and after an
/// inherited LOWPOWER wake, when one is required).
///
/// Anomaly 166 concerns the ISO buffer path, so this is necessary for a complete
/// nRF52840 USBD initialization but is not, by itself, a fix for EP0/CDC attach.
pub(crate) fn configure_enabled_session(applicable: Applicability) {
    if applicable.erratum_166 {
        apply_erratum_166_configuration(applicable, |address, value| unsafe {
            poke(address, value)
        });
        // nrfx follows the hidden-register writes with instruction and data barriers.
        cortex_m::asm::isb();
        cortex_m::asm::dsb();
    }
}

/// Begins one USBD enable attempt.
pub(crate) fn begin_enable(applicable: Applicability) {
    // Erratum 187 brackets the ENABLE -> READY transition. Erratum 211 uses the
    // same hidden register, but owns it only after READY and for the active
    // peripheral lifetime. Do not merge those two phases: current nrfx closes
    // 187 and then opens 211 even when both apply to the same part.
    if applicable.erratum_187 {
        apply_live(ERRATA_187_211_FLAG, 0x0000_0003);
    }
    if applicable.erratum_171 {
        apply_live(ERRATA_171_FLAG, 0x0000_00C0);
    }
}

/// Completes a successful enable attempt.
///
/// Erratum 211 shares ED14 with 187 but requires the workaround to remain in
/// effect for the active USBD lifetime. Current nrfx still closes the 187 phase
/// at READY, then reopens the same flag as a distinct 211 lifetime.
pub(crate) fn complete_enable(applicable: Applicability) {
    if applicable.erratum_171 {
        apply_live(ERRATA_171_FLAG, 0x0000_0000);
    }
    if applicable.erratum_187 {
        apply_live(ERRATA_187_211_FLAG, 0x0000_0000);
    }
    if applicable.erratum_211 {
        apply_live(ERRATA_187_211_FLAG, 0x0000_0003);
    }
    clear_dma_pending(applicable);
}

/// Applies Erratum 199 immediately before starting an EasyDMA transaction.
pub(crate) fn begin_dma(applicable: Applicability) {
    if applicable.erratum_199 {
        unsafe { poke(ERRATA_199_DMA_PENDING, 0x0000_0082) };
    }
}

/// Closes Erratum 199 only after the matching ENDEPIN/ENDEPOUT event proves
/// that EasyDMA no longer owns the transfer buffer.
pub(crate) fn complete_dma(applicable: Applicability) {
    clear_dma_pending(applicable);
}

/// Clears inherited hidden EasyDMA-pending state while USBD is known disabled
/// or before the first transfer of a newly enabled session.
pub(crate) fn clear_dma_pending(applicable: Applicability) {
    if applicable.erratum_199 {
        unsafe { poke(ERRATA_199_DMA_PENDING, 0x0000_0000) };
    }
}

/// Aborts an enable attempt after a fault or before a confirmed active state.
pub(crate) fn abort_enable(applicable: Applicability) {
    if applicable.erratum_171 {
        apply_live(ERRATA_171_FLAG, 0x0000_0000);
    }
    if applicable.erratum_187 || applicable.erratum_211 {
        apply_live(ERRATA_187_211_FLAG, 0x0000_0000);
    }
}

/// Opens the Erratum 171 bracket immediately before a LOWPOWER -> ForceNormal write.
///
/// This is also required when a bootloader hands an enabled peripheral to the
/// application with LOWPOWER still asserted. Pair it with `complete_wake` only
/// after EVENTCAUSE.USBWUALLOWED, or retain ownership until confirmed disable.
/// The active-lifetime Erratum 211 flag is deliberately left untouched.
pub(crate) fn begin_wake(applicable: Applicability) {
    if applicable.erratum_171 {
        apply_live(ERRATA_171_FLAG, 0x0000_00C0);
    }
}

/// Closes the Erratum 171 wake bracket after USBWUALLOWED has been W1C-cleared.
/// This must not close the separate Erratum 211 active-session lifetime.
pub(crate) fn complete_wake(applicable: Applicability) {
    if applicable.erratum_171 {
        apply_live(ERRATA_171_FLAG, 0x0000_0000);
    }
}

/// Ends the active-lifetime portion of the workaround after confirmed disable.
pub(crate) fn end_active(applicable: Applicability) {
    if applicable.erratum_211 {
        apply_live(ERRATA_187_211_FLAG, 0x0000_0000);
    }
}

/// Closes every possibly inherited enable/wake transaction after hardware has
/// confirmed USBD disabled. This is intentionally stronger than `end_active`:
/// a bootloader may have been interrupted with either EC14 or ED14 still open.
pub(crate) fn end_disabled(applicable: Applicability) {
    if applicable.erratum_171 {
        apply_live(ERRATA_171_FLAG, 0x0000_0000);
    }
    if applicable.erratum_187 || applicable.erratum_211 {
        apply_live(ERRATA_187_211_FLAG, 0x0000_0000);
    }
    clear_dma_pending(applicable);
}

#[cfg(test)]
mod tests {
    use super::{
        applicability_for_ids, apply_erratum_166_configuration, apply_nrfx_transaction,
        Applicability, ERRATA_166_SELECT, ERRATA_166_VALUE, ERRATA_171_FLAG, ERRATA_187_211_FLAG,
        NRF52820_FAMILY, NRF52820_PART, NRF52833_FAMILY, NRF52833_PART, NRF52840_FAMILY,
        NRF52840_PART, TRIM, TRIM_MAGIC,
    };

    #[test]
    fn live_aad0_factory_ids_enable_all_five_workarounds() {
        let result = applicability_for_ids(NRF52840_PART, NRF52840_FAMILY, 3).unwrap();
        assert!(result.erratum_166);
        assert!(result.erratum_171);
        assert!(result.erratum_187);
        assert!(result.erratum_199);
        assert!(result.erratum_211);
    }

    #[test]
    fn smaller_usbd_parts_are_rejected_until_erratum_223_has_pac_and_hil_coverage() {
        for (part, family) in [
            (NRF52833_PART, NRF52833_FAMILY),
            (NRF52820_PART, NRF52820_FAMILY),
        ] {
            assert_eq!(applicability_for_ids(part, family, 0), None);
        }
    }

    #[test]
    fn revision_zero_keeps_171_but_excludes_187_and_211() {
        let result = applicability_for_ids(NRF52840_PART, NRF52840_FAMILY, 0).unwrap();
        assert!(result.erratum_166);
        assert!(result.erratum_171);
        assert!(!result.erratum_187);
        assert!(result.erratum_199);
        assert!(!result.erratum_211);
    }

    #[test]
    fn part_and_legacy_family_must_both_match() {
        assert_eq!(applicability_for_ids(0x52833, NRF52840_FAMILY, 3), None);
        assert_eq!(applicability_for_ids(NRF52840_PART, 0x0d, 3), None);
    }

    #[test]
    fn unknown_silicon_is_rejected_instead_of_running_without_workarounds() {
        assert_eq!(applicability_for_ids(0, 0, 0), None);
        assert_eq!(applicability_for_ids(0xffff_ffff, 8, 3), None);
    }

    #[test]
    fn zero_trim_gets_the_full_nrfx_transaction() {
        let mut writes = [(0, 0); 3];
        let mut count = 0;
        apply_nrfx_transaction(0, ERRATA_187_211_FLAG, 3, |address, value| {
            writes[count] = (address, value);
            count += 1;
        });
        assert_eq!(count, 3);
        assert_eq!(
            writes,
            [
                (TRIM, TRIM_MAGIC),
                (ERRATA_187_211_FLAG, 3),
                (TRIM, TRIM_MAGIC),
            ]
        );
    }

    #[test]
    fn initialized_trim_updates_only_the_requested_flag() {
        let mut writes = [(0, 0); 1];
        let mut count = 0;
        apply_nrfx_transaction(0x82, ERRATA_171_FLAG, 0xc0, |address, value| {
            writes[count] = (address, value);
            count += 1;
        });
        assert_eq!(count, 1);
        assert_eq!(writes, [(ERRATA_171_FLAG, 0xc0)]);
    }

    #[test]
    fn erratum_166_uses_the_exact_nrfx_hidden_register_pair() {
        let applicable = applicability_for_ids(NRF52840_PART, NRF52840_FAMILY, 3).unwrap();
        let mut writes = [(0, 0); 2];
        let mut count = 0usize;
        apply_erratum_166_configuration(applicable, |address, value| {
            writes[count] = (address, value);
            count += 1;
        });

        assert_eq!(count, 2);
        assert_eq!(
            writes,
            [
                (ERRATA_166_SELECT, 0x0000_07e3),
                (ERRATA_166_VALUE, 0x0000_0040),
            ]
        );
    }

    #[test]
    fn erratum_166_configuration_is_silent_when_not_applicable() {
        let writes = core::cell::Cell::new(0usize);
        apply_erratum_166_configuration(Applicability::NONE, |_, _| writes.set(writes.get() + 1));
        assert_eq!(writes.get(), 0);
    }
}
