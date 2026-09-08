//! PASID / SVA on Soft SMMU — software bind of process VA ↔ SSID.
//!
//! Linux SVA (`iommu_sva_bind_device`) / PASID inspiration: each
//! [`IommuMap`] is one AccelDevice PASID space. [`IommuMap::bind_mm`]
//! records the process [`MmId`] on a context descriptor; DMA addresses
//! are that process VA; host unmap invalidates the SSID ATC (TLB).
//! Skipping invalidate is a stale translate — the negative test, not a
//! product unified VA.
//!
//! **Not claimed.** This is not ARM SVA, not PCIe PASID/PRI, not CUDA
//! UVA, not hardware SMMU / ATS, and **not** zero-copy SVA without the
//! unmap → SSID TLB invalidate path. SID-at-submit stays required.
//!
//! Serial: `[sva] …`.

use crate::caps::{CapKind, CapRights, Capability};
use crate::iommu::{InvCmd, IommuMap, MapError, MmId, StreamId};
use crate::types::{ChipletId, PhysAddr, TenantId, TileId};

/// Soft-CP-shaped SSID (ssid = 1). Same STE shape as SID-at-submit.
pub const SVA_SID: u32 = StreamId::accel(ChipletId(0), TileId(2), 1).0;
/// Process mm bound on [`SVA_SID`]. Not a Linux `mm_struct`.
pub const SVA_MM: MmId = MmId(0x0100);
/// Process-shaped VA used as the DMA address (below the 4 GiB IOVA window).
pub const SVA_VA: u64 = 0x0040_0000;
/// Guest PA backing that VA. Distinct so the walk is not identity.
pub const SVA_PA: u64 = 0x0100_0000;
pub const SVA_LEN: u64 = 0x1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SvaReport {
    pub bind_ok: bool,
    pub dma_va: bool,
    pub sid_submit: bool,
    pub unmap_inv: bool,
    pub stale_fault: bool,
}

impl SvaReport {
    pub fn all_ok(&self) -> bool {
        self.bind_ok && self.dma_va && self.sid_submit && self.unmap_inv && self.stale_fault
    }
}

fn mem_cap() -> Capability {
    Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1)).with_generation(1)
}

/// Host-identical clip. Kernel prints `[sva] …`.
pub fn run_sva_demo() -> SvaReport {
    let mut iommu = IommuMap::new();
    let cap = mem_cap();
    let sid = StreamId::from_raw(SVA_SID);

    let pasid = iommu.bind_mm(&cap, sid, SVA_MM).unwrap();
    let bind_ok = pasid == sid.ssid()
        && iommu.mm_of(sid) == Some(SVA_MM)
        && iommu.sid_for_mm(SVA_MM) == Some(sid);

    let pin = iommu
        .map_va(&cap, sid, PhysAddr(SVA_VA), PhysAddr(SVA_PA), SVA_LEN)
        .unwrap();
    let walked = iommu.walk(sid, pin.iova);
    let dma_va =
        pin.iova.0 == SVA_VA && pin.guest_pa.0 == SVA_PA && walked.map(|w| w.pa.0) == Ok(SVA_PA);

    let sid_submit = iommu.resolve_submit(sid.raw(), pin.iova, None) == Err(MapError::SubmitSid);
    iommu.set_sid(&cap, sid).unwrap();
    let dma_ok = iommu.resolve_submit(sid.raw(), pin.iova, None) == Ok(PhysAddr(SVA_PA)) && dma_va;

    // Stale ATC: fill TLB, drop S1 without invalidate, hit leftover PA.
    let _ = iommu.resolve_ats(sid.raw(), pin.iova).unwrap();
    let _ = iommu.unmap_va_keep_atc(sid, pin.iova).unwrap();
    let stale_hit = iommu.resolve_ats(sid.raw(), PhysAddr(SVA_VA)) == Ok(PhysAddr(SVA_PA));
    let _ = iommu.invalidate(InvCmd::CfgCd { sid }).unwrap();
    let after_inv = iommu.walk(sid, PhysAddr(SVA_VA)) == Err(MapError::NotMapped)
        && iommu.resolve_ats(sid.raw(), PhysAddr(SVA_VA)) == Err(MapError::NotMapped);
    let stale_fault = stale_hit && after_inv;

    // Honest unmap: remap, fill ATC, unmap_va drops SSID TLB, translate faults.
    let pin = iommu
        .map_va(&cap, sid, PhysAddr(SVA_VA), PhysAddr(SVA_PA), SVA_LEN)
        .unwrap();
    iommu.set_sid(&cap, sid).unwrap();
    let _ = iommu.resolve_ats(sid.raw(), pin.iova).unwrap();
    let _ = iommu.unmap_va(sid, pin.iova).unwrap();
    let unmap_inv = iommu.walk(sid, PhysAddr(SVA_VA)) == Err(MapError::NotMapped)
        && iommu.atc_len() == 0
        && iommu.resolve_ats(sid.raw(), PhysAddr(SVA_VA)) == Err(MapError::NotMapped);

    SvaReport {
        bind_ok,
        dma_va: dma_ok,
        sid_submit,
        unmap_inv,
        stale_fault,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iommu::StreamState;

    #[test]
    fn sva_demo_bind_dma_unmap_stale() {
        let r = run_sva_demo();
        assert!(r.bind_ok, "mm↔ssid bind");
        assert!(r.dma_va, "DMA uses process VA");
        assert!(r.sid_submit, "SET_SID still required");
        assert!(r.unmap_inv, "unmap invalidates SSID TLB");
        assert!(r.stale_fault, "stale ATC hits until invalidate");
        assert!(r.all_ok());
    }

    #[test]
    fn map_va_without_bind_mm_aborts() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap();
        let sid = StreamId::from_raw(SVA_SID);
        iommu.bind_stream(&cap, sid).unwrap();
        assert_eq!(iommu.mm_of(sid), None);
        assert_eq!(
            iommu.map_va(&cap, sid, PhysAddr(SVA_VA), PhysAddr(SVA_PA), SVA_LEN),
            Err(MapError::StreamAbort)
        );
        assert_eq!(iommu.bind_mm(&cap, sid, MmId(0)), Err(MapError::BadRange));
    }

    #[test]
    fn per_device_pasid_space_is_the_iommu_table() {
        let cap = mem_cap();
        let mm = SVA_MM;
        let sid_a = StreamId::accel(ChipletId(0), TileId(2), 1);
        let sid_b = StreamId::accel(ChipletId(1), TileId(3), 1);
        let mut dev_a = IommuMap::new();
        let mut dev_b = IommuMap::new();
        assert_eq!(dev_a.bind_mm(&cap, sid_a, mm).unwrap(), 1);
        assert_eq!(dev_b.bind_mm(&cap, sid_b, mm).unwrap(), 1);
        assert_eq!(dev_a.sid_for_mm(mm), Some(sid_a));
        assert_eq!(dev_b.sid_for_mm(mm), Some(sid_b));
        assert_ne!(sid_a, sid_b);
        assert_eq!(
            dev_a.bind_mm(&cap, sid_a.with_ssid(2), mm),
            Err(MapError::Overlap)
        );
    }

    #[test]
    fn bind_mm_idempotent_and_stream_stays_bound() {
        let mut iommu = IommuMap::new();
        let cap = mem_cap();
        let sid = StreamId::from_raw(SVA_SID);
        assert_eq!(iommu.bind_mm(&cap, sid, SVA_MM).unwrap(), 1);
        assert_eq!(iommu.bind_mm(&cap, sid, SVA_MM).unwrap(), 1);
        assert_eq!(iommu.stream_state(sid), StreamState::Bound);
        assert_eq!(
            iommu.bind_mm(&cap, sid, MmId(0x0200)),
            Err(MapError::Overlap)
        );
    }
}
