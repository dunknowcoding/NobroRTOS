/// A disjoint deployment-RAM accounting dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RamRegionKind {
    StaticSections = 0,
    TaskStacks = 1,
    ProviderStacks = 2,
    ArenasAndPools = 3,
    RetainedHeap = 4,
    VendorReserved = 5,
}

impl RamRegionKind {
    const COUNT: usize = Self::VendorReserved as usize + 1;

    const fn mask(self) -> u8 {
        1_u8 << self as u8
    }
}

/// How one RAM charge is proven not to overlap another charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RamPlacement {
    /// A half-open range `[start, start + bytes)` in a named address space.
    Addressed { address_space: u8, start: u32 },
    /// A measured or vendor-priced reservation without an application address.
    Priced { reservation_id: u32 },
    /// An explicit zero for a dimension that this deployment does not use.
    DeclaredZero,
}

/// One independently placed or priced RAM charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RamRegion {
    pub kind: RamRegionKind,
    pub placement: RamPlacement,
    pub bytes: u32,
}

impl RamRegion {
    pub const fn addressed(kind: RamRegionKind, address_space: u8, start: u32, bytes: u32) -> Self {
        Self {
            kind,
            placement: RamPlacement::Addressed {
                address_space,
                start,
            },
            bytes,
        }
    }

    pub const fn priced(kind: RamRegionKind, reservation_id: u32, bytes: u32) -> Self {
        Self {
            kind,
            placement: RamPlacement::Priced { reservation_id },
            bytes,
        }
    }

    pub const fn declared_zero(kind: RamRegionKind) -> Self {
        Self {
            kind,
            placement: RamPlacement::DeclaredZero,
            bytes: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RamReceiptError {
    EmptyNonzeroCharge,
    NonzeroDeclaredZero,
    AddressOverflow,
    AddressOverlap { first: usize, second: usize },
    DuplicateReservation { first: usize, second: usize },
    ContradictoryDeclaredZero { kind: RamRegionKind },
    MissingDimension { kind: RamRegionKind },
    TotalOverflow,
    BudgetExceeded { total_bytes: u32, budget_bytes: u32 },
}

/// Validated, disjoint total-RAM receipt for one exact deployment.
///
/// Statically allocated stacks or arenas already present in `.data`/`.bss`
/// must not be charged again. Their stack/arena dimensions use
/// [`RamRegion::declared_zero`]; separately allocated ranges use `addressed`,
/// and heap/vendor reservations use stable, unique `priced` identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeploymentRamReceipt {
    by_kind: [u32; RamRegionKind::COUNT],
    total_bytes: u32,
}

impl DeploymentRamReceipt {
    pub fn validate(regions: &[RamRegion], budget_bytes: u32) -> Result<Self, RamReceiptError> {
        let mut by_kind = [0_u32; RamRegionKind::COUNT];
        let mut seen = 0_u8;
        let mut zero = 0_u8;

        for (index, region) in regions.iter().enumerate() {
            match region.placement {
                RamPlacement::DeclaredZero => {
                    if region.bytes != 0 {
                        return Err(RamReceiptError::NonzeroDeclaredZero);
                    }
                    zero |= region.kind.mask();
                }
                RamPlacement::Addressed {
                    address_space,
                    start,
                } => {
                    if region.bytes == 0 {
                        return Err(RamReceiptError::EmptyNonzeroCharge);
                    }
                    let end = start
                        .checked_add(region.bytes)
                        .ok_or(RamReceiptError::AddressOverflow)?;
                    for (other_index, other) in regions[..index].iter().enumerate() {
                        let RamPlacement::Addressed {
                            address_space: other_space,
                            start: other_start,
                        } = other.placement
                        else {
                            continue;
                        };
                        if address_space != other_space {
                            continue;
                        }
                        let other_end = other_start
                            .checked_add(other.bytes)
                            .ok_or(RamReceiptError::AddressOverflow)?;
                        if start < other_end && other_start < end {
                            return Err(RamReceiptError::AddressOverlap {
                                first: other_index,
                                second: index,
                            });
                        }
                    }
                }
                RamPlacement::Priced { reservation_id } => {
                    if region.bytes == 0 {
                        return Err(RamReceiptError::EmptyNonzeroCharge);
                    }
                    for (other_index, other) in regions[..index].iter().enumerate() {
                        if matches!(
                            other.placement,
                            RamPlacement::Priced {
                                reservation_id: other_id
                            } if other_id == reservation_id
                        ) {
                            return Err(RamReceiptError::DuplicateReservation {
                                first: other_index,
                                second: index,
                            });
                        }
                    }
                }
            }
            seen |= region.kind.mask();
            by_kind[region.kind as usize] = by_kind[region.kind as usize]
                .checked_add(region.bytes)
                .ok_or(RamReceiptError::TotalOverflow)?;
        }

        for kind in [
            RamRegionKind::StaticSections,
            RamRegionKind::TaskStacks,
            RamRegionKind::ProviderStacks,
            RamRegionKind::ArenasAndPools,
            RamRegionKind::RetainedHeap,
            RamRegionKind::VendorReserved,
        ] {
            if seen & kind.mask() == 0 {
                return Err(RamReceiptError::MissingDimension { kind });
            }
            if zero & kind.mask() != 0 && by_kind[kind as usize] != 0 {
                return Err(RamReceiptError::ContradictoryDeclaredZero { kind });
            }
        }

        let total_bytes = by_kind.iter().try_fold(0_u32, |total, bytes| {
            total
                .checked_add(*bytes)
                .ok_or(RamReceiptError::TotalOverflow)
        })?;
        if total_bytes > budget_bytes {
            return Err(RamReceiptError::BudgetExceeded {
                total_bytes,
                budget_bytes,
            });
        }
        Ok(Self {
            by_kind,
            total_bytes,
        })
    }

    pub const fn bytes(self, kind: RamRegionKind) -> u32 {
        self.by_kind[kind as usize]
    }

    pub const fn total_bytes(self) -> u32 {
        self.total_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_regions() -> [RamRegion; 6] {
        [
            RamRegion::addressed(RamRegionKind::StaticSections, 0, 0x2000_0000, 1024),
            RamRegion::addressed(RamRegionKind::TaskStacks, 0, 0x2000_0800, 512),
            RamRegion::declared_zero(RamRegionKind::ProviderStacks),
            RamRegion::addressed(RamRegionKind::ArenasAndPools, 1, 0, 256),
            RamRegion::priced(RamRegionKind::RetainedHeap, 1, 128),
            RamRegion::priced(RamRegionKind::VendorReserved, 2, 2048),
        ]
    }

    #[test]
    fn complete_disjoint_receipt_sums_every_dimension_once() {
        let receipt = DeploymentRamReceipt::validate(&complete_regions(), 4096).unwrap();
        assert_eq!(receipt.total_bytes(), 3968);
        assert_eq!(receipt.bytes(RamRegionKind::TaskStacks), 512);
        assert_eq!(receipt.bytes(RamRegionKind::ProviderStacks), 0);
    }

    #[test]
    fn overlap_duplicate_price_and_missing_dimension_fail_closed() {
        let mut overlap = complete_regions();
        overlap[1] = RamRegion::addressed(RamRegionKind::TaskStacks, 0, 0x2000_0200, 512);
        assert_eq!(
            DeploymentRamReceipt::validate(&overlap, u32::MAX),
            Err(RamReceiptError::AddressOverlap {
                first: 0,
                second: 1
            })
        );

        let mut duplicate = complete_regions();
        duplicate[5] = RamRegion::priced(RamRegionKind::VendorReserved, 1, 2048);
        assert_eq!(
            DeploymentRamReceipt::validate(&duplicate, u32::MAX),
            Err(RamReceiptError::DuplicateReservation {
                first: 4,
                second: 5
            })
        );

        assert_eq!(
            DeploymentRamReceipt::validate(&complete_regions()[..5], u32::MAX),
            Err(RamReceiptError::MissingDimension {
                kind: RamRegionKind::VendorReserved
            })
        );
    }

    #[test]
    fn contradictory_zero_overflow_and_budget_fail_closed() {
        let mut regions = complete_regions().to_vec();
        regions.push(RamRegion::priced(RamRegionKind::ProviderStacks, 3, 64));
        assert_eq!(
            DeploymentRamReceipt::validate(&regions, u32::MAX),
            Err(RamReceiptError::ContradictoryDeclaredZero {
                kind: RamRegionKind::ProviderStacks
            })
        );

        let mut overflow = complete_regions();
        overflow[0] = RamRegion::addressed(RamRegionKind::StaticSections, 0, 0, u32::MAX);
        overflow[1] = RamRegion::addressed(RamRegionKind::TaskStacks, 2, 0, 512);
        assert_eq!(
            DeploymentRamReceipt::validate(&overflow, u32::MAX),
            Err(RamReceiptError::TotalOverflow)
        );

        assert_eq!(
            DeploymentRamReceipt::validate(&complete_regions(), 3967),
            Err(RamReceiptError::BudgetExceeded {
                total_bytes: 3968,
                budget_bytes: 3967
            })
        );
    }
}
