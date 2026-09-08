//! Soft-CP host for SoftNoI-IS admit on a shared fake NoI.
//!
//! Lives **beside** [`crate::fakecp`] so SoftGreenCtx / SoftCCT can keep
//! editing the CP. PARL / NoI inspiration: Interference Score
//! `IS = max T_solo / T_con`. SoftChipletSync advertises the per-tenant
//! estimate; XQueue submit refuses when projected IS > budget.
//! **Admit control, not topology synthesis, not UniCNet.**

use aether_core::accel::{AccelJobDesc, AccelOp, DmaView};
use aether_core::noi::{IsEstimate, SoftNoI};
use aether_core::types::{ChipletId, PhysAddr, TenantId};
use aether_hal::HalError;

use crate::fakecp::{map_noi_error, SoftCommandProcessor};

impl<M: DmaView> SoftCommandProcessor<M> {
    /// SoftChipletSync's fake NoI (IS advertisement + admit).
    pub fn noi(&self) -> &SoftNoI {
        self.chipsync.noi()
    }

    pub fn enable_noi(&mut self, on: bool) {
        self.chipsync.enable_noi(on);
    }

    /// Per-tenant IS milli advertised by SoftChipletSync. `None` if empty.
    pub fn advertised_is_milli(&self, tenant: TenantId) -> Option<u32> {
        self.chipsync.advertised_is_milli(tenant)
    }

    /// Admit `tenant` onto the fake NoI. Does not enqueue an XQueue slot.
    pub fn admit_noi(&mut self, tenant: TenantId, demand: u32) -> Result<IsEstimate, HalError> {
        self.chipsync
            .admit_noi(tenant, demand)
            .map_err(map_noi_error)
    }

    pub fn release_noi(&mut self, tenant: TenantId) -> Result<(), HalError> {
        self.chipsync.release_noi(tenant).map_err(map_noi_error)
    }

    /// XQueue submit gated by SoftNoI-IS. Refuses when projected IS > budget.
    ///
    /// [`SoftCommandProcessor::submit_xqueue`] stays ungated (SID / XQueue
    /// path unchanged when NoI is off).
    pub fn submit_xqueue_noi(
        &mut self,
        queue: u16,
        job: &AccelJobDesc,
        demand: u32,
    ) -> Result<u32, HalError> {
        let tenant = TenantId(job.tenant);
        self.chipsync
            .admit_noi(tenant, demand)
            .map_err(map_noi_error)?;
        match self.submit_xqueue(queue, job) {
            Ok(seq) => Ok(seq),
            Err(e) => {
                let _ = self.chipsync.release_noi(tenant);
                Err(e)
            }
        }
    }
}

/// Host-tested two-tenant clip on Soft-CP (Nop jobs; no DMA).
pub fn two_tenant_nop(tenant: u32, chiplet: u8, tile: u16) -> AccelJobDesc {
    let mut job = AccelJobDesc::matmul_i32(1, 1, 1, PhysAddr(0), PhysAddr(0), PhysAddr(0), tenant);
    job.op = AccelOp::Nop;
    job.place.chiplet = ChipletId(chiplet);
    job.place = job.place.with_tile(tile);
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakecp::{SoftCommandProcessor, CP_SSID};
    use aether_core::accel::SliceMem;
    use aether_core::iommu::StreamId;
    use aether_core::noi::{
        run_softnoi_demo, DEMO_HEAVY_DEMAND, DEMO_LIGHT_DEMAND, IS_BUDGET_MILLI, IS_SOLO_MILLI,
    };
    use aether_core::types::{ChipletId, TileId};
    use aether_hal::AccelDevice;

    fn two_queues(d: &mut SoftCommandProcessor<SliceMem<'_>>) {
        let sid_a = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        let sid_b = StreamId::accel(ChipletId(1), TileId(3), CP_SSID);
        d.create_xqueue(0, sid_a, 0).unwrap();
        d.create_xqueue(1, sid_b, 0).unwrap();
    }

    #[test]
    fn softnoi_solo_vs_concurrent_then_xqueue_refuse() {
        let r = run_softnoi_demo();
        assert!(r.all_ok());
        assert_eq!(r.light_is_milli, IS_SOLO_MILLI);
        assert!(r.heavy_is_milli > IS_BUDGET_MILLI);

        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        two_queues(&mut d);
        d.enable_noi(true);
        assert!(d.noi().enabled());

        let job_a = two_tenant_nop(1, 0, 2);
        let job_b = two_tenant_nop(2, 1, 3);
        d.submit_xqueue_noi(0, &job_a, DEMO_LIGHT_DEMAND).unwrap();
        d.submit_xqueue_noi(1, &job_b, DEMO_LIGHT_DEMAND).unwrap();
        assert_eq!(d.advertised_is_milli(TenantId(1)), Some(IS_SOLO_MILLI));
        assert_eq!(d.advertised_is_milli(TenantId(2)), Some(IS_SOLO_MILLI));
        assert_eq!(AccelDevice::noi_is_milli(&d, 1), Some(IS_SOLO_MILLI));
        assert_eq!(d.service().unwrap().status, 0);
        assert_eq!(d.service().unwrap().status, 0);

        d.release_noi(TenantId(1)).unwrap();
        d.release_noi(TenantId(2)).unwrap();
        assert_eq!(d.noi().occupancy(), 0);

        d.submit_xqueue_noi(0, &job_a, DEMO_HEAVY_DEMAND).unwrap();
        assert_eq!(
            d.submit_xqueue_noi(1, &job_b, DEMO_HEAVY_DEMAND)
                .unwrap_err(),
            HalError::Busy
        );
        assert_eq!(d.noi().occupancy(), 1);
        assert_eq!(d.noi().refused(), 1);
        assert_eq!(d.advertised_is_milli(TenantId(1)), Some(IS_SOLO_MILLI));
        assert_eq!(d.advertised_is_milli(TenantId(2)), None);
        // Ungated XQueue path still enqueues when NoI refuse already fired.
        d.submit_xqueue(1, &job_b).unwrap();
        assert!(!d.xqueue(1).unwrap().is_empty());

        let (worst, tput) = d
            .noi()
            .measure(&[DEMO_HEAVY_DEMAND, DEMO_HEAVY_DEMAND])
            .unwrap();
        assert_eq!(worst, 1600);
        assert_eq!(tput.solo, DEMO_HEAVY_DEMAND);
        assert_eq!(tput.concurrent, 500);
        assert_eq!(tput.demand, DEMO_HEAVY_DEMAND);
    }

    #[test]
    fn softnoi_hal_admit_refuse_over_budget() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        d.enable_noi(true);
        assert_eq!(
            AccelDevice::admit_noi(&mut d, 1, DEMO_HEAVY_DEMAND).unwrap(),
            IS_SOLO_MILLI
        );
        assert_eq!(
            AccelDevice::admit_noi(&mut d, 2, DEMO_HEAVY_DEMAND).unwrap_err(),
            HalError::Busy
        );
    }

    #[test]
    fn default_submit_xqueue_ignores_noi() {
        let mut backing = [0u8; 16];
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        two_queues(&mut d);
        // NoI off: two heavy submits are ordinary XQueue work.
        d.submit_xqueue(0, &two_tenant_nop(1, 0, 2)).unwrap();
        d.submit_xqueue(1, &two_tenant_nop(2, 1, 3)).unwrap();
        assert_eq!(d.noi().occupancy(), 0);
        assert!(!d.noi().enabled());
    }
}
