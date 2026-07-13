//! Pure provider-report policy shared by the target and host tests.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderEvidence {
    pub system: bool,
    pub timebase: bool,
    pub deadline: bool,
    pub usb_configured: bool,
}

impl ProviderEvidence {
    pub const fn new(system: bool, timebase: bool, deadline: bool, usb_configured: bool) -> Self {
        Self {
            system,
            timebase,
            deadline,
            usb_configured,
        }
    }

    pub const fn core_passes(self) -> bool {
        self.system && self.timebase && self.deadline
    }

    pub const fn all_passes(self) -> bool {
        self.core_passes() && self.usb_configured
    }

    pub const fn with_usb(self, configured: bool) -> Self {
        Self {
            usb_configured: configured,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderEvidence;

    #[test]
    fn usb_success_cannot_be_reported_before_current_configuration() {
        let core = ProviderEvidence::new(true, true, true, false);
        assert!(core.core_passes());
        assert!(!core.all_passes());
        assert!(core.with_usb(true).all_passes());
    }

    #[test]
    fn any_clock_or_deadline_failure_forces_every_aggregate_false() {
        for evidence in [
            ProviderEvidence::new(false, true, true, true),
            ProviderEvidence::new(true, false, true, true),
            ProviderEvidence::new(true, true, false, true),
        ] {
            assert!(!evidence.core_passes());
            assert!(!evidence.all_passes());
        }
    }
}
