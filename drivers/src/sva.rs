//! Soft-CP host for PASID / SVA on Soft SMMU.
//!
//! Lives **beside** [`crate::fakecp`] so SID-at-submit / XQueue stay
//! intact. Bind process mm ↔ this AccelDevice's SSID; DMA uses the
//! process VA; host unmap invalidates the SSID ATC. Skipping
//! invalidate is a stale translate.
//!
//! Linux SVA / PASID inspiration. **Not** ARM SVA, **not** PCIe
//! PASID/PRI, **not** CUDA UVA, **not** hardware SMMU, **not**
//! zero-copy SVA without invalidate. No new syscall.

use aether_core::accel::DmaView;
use aether_core::iommu::{MmId, StreamId};
use aether_core::types::PhysAddr;
use aether_hal::HalError;

use crate::fakecp::SoftCommandProcessor;

impl<M: DmaView> SoftCommandProcessor<M> {
    /// Bind mm on `sid` then pin `va` → `guest_pa`. Returns the VA.
    pub fn bind_and_map_va(
        &mut self,
        cap: &aether_core::caps::Capability,
        sid: StreamId,
        mm: MmId,
        va: PhysAddr,
        guest_pa: PhysAddr,
        len: u64,
    ) -> Result<PhysAddr, HalError> {
        let _ = self.bind_sva_with_cap(cap, sid, mm)?;
        self.map_va_with_cap(cap, sid, va, guest_pa, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakecp::{stream_for_job, SoftCommandProcessor, CP_SSID};
    use aether_core::accel::{AccelJobDesc, SliceMem};
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::iommu::SOFT_SMMU_IOVA_BASE;
    use aether_core::sva::{SVA_MM, SVA_VA};
    use aether_core::types::{ChipletId, TenantId, TileId};
    use aether_hal::AccelDevice;

    fn mem_cap() -> Capability {
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 3, TenantId(1)).with_generation(1)
    }

    fn matmul_backing() -> ([u8; 256], AccelJobDesc) {
        let mut backing = [0u8; 256];
        for (i, v) in [1i32, 2, 3, 4].iter().enumerate() {
            backing[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [5i32, 6, 7, 8].iter().enumerate() {
            backing[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let job = AccelJobDesc::matmul_i32(2, 2, 2, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        (backing, job)
    }

    #[test]
    fn bind_sva_without_cap_is_nomemorycap() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        assert_eq!(d.bind_sva(SVA_MM.0, 0).unwrap_err(), HalError::NoMemoryCap);
        assert_eq!(
            d.map_va(0, PhysAddr(SVA_VA), PhysAddr(0), 0x10)
                .unwrap_err(),
            HalError::NoMemoryCap
        );
    }

    #[test]
    fn soft_cp_dma_uses_process_va() {
        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        job.fence_id = 9;
        let sid = stream_for_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let va = d
            .bind_and_map_va(&mem_cap(), sid, SVA_MM, PhysAddr(SVA_VA), PhysAddr(0), 256)
            .unwrap();
        assert_eq!(va.0, SVA_VA);
        assert!(va.0 < SOFT_SMMU_IOVA_BASE, "SVA DMA address is process VA");
        assert_eq!(StreamId::from_raw(sid.raw()).ssid(), CP_SSID);

        d.submit(&job).unwrap();
        let cmd = d.last_cmd().unwrap();
        assert_eq!(cmd.stream_id, sid.raw());
        assert_eq!(cmd.iova_a, SVA_VA, "packed DMA address is the process VA");
        assert_eq!(cmd.iova_b, SVA_VA + 16);
        assert_eq!(cmd.iova_c, SVA_VA + 32);
        assert_eq!(d.xqueue(0).unwrap().sid, Some(sid), "XQueue SID sticks");

        let cpl = d.service().unwrap();
        assert_eq!(cpl.status, 0);
        let got = i32::from_le_bytes(backing[32..36].try_into().unwrap());
        assert_eq!(got, 19, "1*5+2*7");
    }

    #[test]
    fn host_unmap_invalidates_ssid_tlb_then_stale_service_faults() {
        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        let sid = stream_for_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let va = d
            .bind_and_map_va(&mem_cap(), sid, SVA_MM, PhysAddr(SVA_VA), PhysAddr(0), 256)
            .unwrap();
        d.submit(&job).unwrap();
        d.unmap_va(sid, va).unwrap();
        assert_eq!(d.iommu.atc_len(), 0);
        let cpl = d.service().unwrap();
        assert_eq!(
            cpl.status, -2,
            "queued DMA faults after SSID TLB invalidate"
        );
        assert_eq!(
            d.submit(&job).unwrap_err(),
            HalError::Fault,
            "new submit cannot pack an unmapped VA"
        );
    }

    #[test]
    fn skip_invalidate_queued_cmd_stale_translate() {
        let (mut backing, mut job) = matmul_backing();
        job.place = job.place.with_tile(2);
        let sid = stream_for_job(&job);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let va = d
            .bind_and_map_va(&mem_cap(), sid, SVA_MM, PhysAddr(SVA_VA), PhysAddr(0), 256)
            .unwrap();
        d.submit(&job).unwrap();
        // Fill ATC as service would, then drop S1 without TLB invalidate.
        d.iommu.set_sid_bound(sid).unwrap();
        let _ = d.iommu.resolve_ats(sid.raw(), va).unwrap();
        d.unmap_va_keep_atc(sid, va).unwrap();
        assert!(d.iommu.atc_len() > 0, "stale SSID TLB remains");
        let cpl = d.service().unwrap();
        assert_eq!(
            cpl.status, 0,
            "stale ATC hit is the hazard without invalidate"
        );
    }

    #[test]
    fn two_acceldevices_have_independent_pasid_spaces() {
        let (mut backing_a, mut job_a) = matmul_backing();
        let (mut backing_b, mut job_b) = matmul_backing();
        job_a.place = job_a.place.with_tile(2);
        job_b.place =
            aether_core::space::Place::new(ChipletId(1), aether_core::space::MemorySpace::Host)
                .with_tile(3);
        let sid_a = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        let sid_b = StreamId::accel(ChipletId(1), TileId(3), CP_SSID);
        let mem_a = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing_a,
        };
        let mem_b = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing_b,
        };
        let mut a = SoftCommandProcessor::new(mem_a);
        let mut b = SoftCommandProcessor::new(mem_b);
        let va_a = a
            .bind_and_map_va(
                &mem_cap(),
                sid_a,
                SVA_MM,
                PhysAddr(SVA_VA),
                PhysAddr(0),
                256,
            )
            .unwrap();
        let va_b = b
            .bind_and_map_va(
                &mem_cap(),
                sid_b,
                SVA_MM,
                PhysAddr(SVA_VA),
                PhysAddr(0),
                256,
            )
            .unwrap();
        assert_eq!(va_a, va_b, "same process VA on two devices");
        assert_eq!(a.iommu.sid_for_mm(SVA_MM), Some(sid_a));
        assert_eq!(b.iommu.sid_for_mm(SVA_MM), Some(sid_b));
        a.submit(&job_a).unwrap();
        b.submit(&job_b).unwrap();
        assert_eq!(a.service().unwrap().status, 0);
        assert_eq!(b.service().unwrap().status, 0);
    }
}
