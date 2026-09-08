//! Soft-CP host for OperatorInject (resident worker + versioned ops).
//!
//! Lives **beside** [`crate::fakecp`] so XQueue / SoftGreenCtx / SoftSFI
//! can keep editing the CP without merging this IR. GPUOS / Mirage MPK
//! inspiration: one resident loop, host-published slots, hot-add without
//! relaunch. Own bytecode only — not NVRTC, not CUDA, not a full LLM
//! compiler. SID-at-submit + SoftCmdFirewall copy-then-validate still
//! gate every call.

use aether_core::accel::{AccelOp, DType, DmaView};
use aether_core::iommu::StreamId;
use aether_core::opinject::{InjectError, OpCall, OpMem};
use aether_core::phase::Phase;
use aether_core::softsfi::SidSandbox;
use aether_core::space::MemorySpace;
use aether_core::types::PhysAddr;
use aether_hal::HalError;

use crate::fakecp::{CpCmd, SoftCommandProcessor, CP_FLAG_SET_SID, CP_PKT_MAGIC, CP_SSID};

fn map_inject_error(e: InjectError) -> HalError {
    match e {
        InjectError::NotRunning | InjectError::Oob | InjectError::Busy => HalError::Fault,
        InjectError::UnknownSlot | InjectError::StaleVersion | InjectError::BadArg => {
            HalError::BadArg
        }
    }
}

/// SID walk then [`DmaView`]. Same shape as SoftSFI's `CpSfiMem`.
struct CpOpMem<'a, M: DmaView> {
    iommu: &'a aether_core::iommu::IommuMap,
    sid: StreamId,
    mem: &'a mut M,
}

impl<M: DmaView> OpMem for CpOpMem<'_, M> {
    fn load_i32(&self, addr: u64) -> Result<i32, InjectError> {
        let pa = self
            .iommu
            .resolve_stream(self.sid.raw(), PhysAddr(addr))
            .ok_or(InjectError::Oob)?;
        self.mem.load_i32(pa).map_err(|_| InjectError::Oob)
    }

    fn store_i32(&mut self, addr: u64, val: i32) -> Result<(), InjectError> {
        let pa = self
            .iommu
            .resolve_stream(self.sid.raw(), PhysAddr(addr))
            .ok_or(InjectError::Oob)?;
        self.mem.store_i32(pa, val).map_err(|_| InjectError::Oob)
    }
}

fn pack_call_cmd(call: &OpCall, sid: StreamId) -> Result<CpCmd, HalError> {
    if call.n == 0 || call.n > u16::MAX as u32 {
        return Err(HalError::BadArg);
    }
    // MatMul-shaped reloc: A covers src (n words), C covers dst (n words),
    // B is a 4-byte cap at dst so firewall's k×n walk is in-window.
    Ok(CpCmd {
        magic: CP_PKT_MAGIC,
        opcode: AccelOp::MatMul as u32 as u8,
        dtype: DType::I32 as u8,
        space: MemorySpace::Host as u8,
        phase: Phase::Compute as u8,
        m: call.n as u16,
        n: 1,
        k: 1,
        flags: CP_FLAG_SET_SID,
        stream_id: sid.raw(),
        chiplet: sid.chiplet().0 as u16,
        tile: sid.tile().0,
        iova_a: call.src,
        iova_b: call.dst,
        iova_c: call.dst,
        iova_bias: 0,
        fence_id: 0,
    })
}

impl<M: DmaView> SoftCommandProcessor<M> {
    pub fn opinject_running(&self) -> bool {
        self.opinject.running()
    }

    pub fn opinject_epoch(&self) -> u32 {
        self.opinject.epoch()
    }

    pub fn opinject_launches(&self) -> u32 {
        self.opinject.launches()
    }

    /// Hot-add `scale` on the live worker. Epoch / launches stay put.
    pub fn hot_add_scale(&mut self) -> Result<u32, HalError> {
        self.opinject.hot_add_scale().map_err(map_inject_error)
    }

    pub fn slot_version(&self, slot: u8) -> Option<u32> {
        self.opinject.slot_version(slot)
    }

    /// SET_SID-at-submit + SoftCmdFirewall copy-then-validate, then dispatch
    /// through the resident worker. `call.src` / `call.dst` are IOVAs.
    pub fn submit_injected(&mut self, sid: StreamId, call: &OpCall) -> Result<u32, HalError> {
        if sid.ssid() != CP_SSID {
            return Err(HalError::Fault);
        }
        if let Err(_e) = self.iommu.set_sid_bound(sid) {
            self.iommu.clear_submit_sid();
            return Err(HalError::Fault);
        }
        let cmd = match pack_call_cmd(call, sid) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.iommu.clear_submit_sid();
                return Err(e);
            }
        };
        let cmd = match self.firewall.admit_packed(cmd, &self.iommu, Some(sid)) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.iommu.clear_submit_sid();
                return Err(e);
            }
        };
        let sandbox = SidSandbox::from_iommu(&self.iommu, sid);
        let submitted = {
            let mut view = CpOpMem {
                iommu: &self.iommu,
                sid,
                mem: &mut self.mem,
            };
            self.opinject.submit(call, &sandbox, &mut view)
        };
        self.iommu.clear_submit_sid();
        match submitted {
            Ok(()) => {
                self.last_cmd = Some(cmd);
                self.submit_seq = self.submit_seq.wrapping_add(1);
                Ok(self.submit_seq)
            }
            Err(e) => Err(map_inject_error(e)),
        }
    }

    pub fn last_inject_firewall(&self) -> bool {
        self.firewall.last_sim.noted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakecp::CP_SSID;
    use aether_core::accel::SliceMem;
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::iommu::MapRequest;
    use aether_core::opinject::{
        run_opinject_demo, DEMO_WORDS, SLOT_MEMCPY, SLOT_SAXPY, SLOT_SCALE,
    };
    use aether_core::types::{ChipletId, TenantId, TileId};

    fn pinned_cp(
        backing: &mut [u8],
    ) -> (
        SoftCommandProcessor<SliceMem<'_>>,
        StreamId,
        PhysAddr,
        PhysAddr,
    ) {
        let sid = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 31, TenantId(1))
            .with_generation(1);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let src = d
            .map_with_cap(&cap, MapRequest::pin_accel(PhysAddr(0), 16, sid))
            .unwrap();
        let dst = d
            .map_with_cap(&cap, MapRequest::pin_accel(PhysAddr(16), 16, sid))
            .unwrap();
        (d, sid, src, dst)
    }

    #[test]
    fn resident_worker_is_up_on_soft_cp() {
        let mut backing = [0u8; 32];
        let (d, _sid, _src, _dst) = pinned_cp(&mut backing);
        assert!(d.opinject_running());
        assert_eq!(d.opinject_launches(), 1);
        assert!(d.slot_version(SLOT_MEMCPY).is_some());
        assert!(d.slot_version(SLOT_SAXPY).is_some());
        assert!(d.slot_version(SLOT_SCALE).is_none());
    }

    #[test]
    fn memcpy_saxpy_then_hot_add_scale_without_relaunch() {
        let mut backing = [0u8; 32];
        for i in 0..DEMO_WORDS {
            let off = (i as usize) * 4;
            backing[off..off + 4].copy_from_slice(&(i as i32 + 1).to_le_bytes());
        }
        let (mut d, sid, src, dst) = pinned_cp(&mut backing);
        let epoch0 = d.opinject_epoch();
        let launches0 = d.opinject_launches();

        let mv = d.slot_version(SLOT_MEMCPY).unwrap();
        d.submit_injected(sid, &OpCall::memcpy(mv, DEMO_WORDS, src.0, dst.0))
            .unwrap();
        assert!(d.last_inject_firewall());
        drop(d);
        assert_eq!(i32::from_le_bytes(backing[16..20].try_into().unwrap()), 1);
        assert_eq!(i32::from_le_bytes(backing[28..32].try_into().unwrap()), 4);

        let mut backing = [0u8; 32];
        for i in 0..DEMO_WORDS {
            let off = (i as usize) * 4;
            backing[off..off + 4].copy_from_slice(&(i as i32 + 1).to_le_bytes());
        }
        backing[16..32].copy_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0]);
        let (mut d, sid, src, dst) = pinned_cp(&mut backing);
        let sv = d.slot_version(SLOT_SAXPY).unwrap();
        d.submit_injected(sid, &OpCall::saxpy(sv, DEMO_WORDS, 2, src.0, dst.0))
            .unwrap();
        drop(d);
        // dst started as copy of src; saxpy α=2 → 3×src
        assert_eq!(i32::from_le_bytes(backing[16..20].try_into().unwrap()), 3);

        let mut backing = [0u8; 32];
        for i in 0..DEMO_WORDS {
            let off = (i as usize) * 4;
            backing[off..off + 4].copy_from_slice(&(i as i32 + 1).to_le_bytes());
        }
        let (mut d, sid, src, dst) = pinned_cp(&mut backing);
        assert_eq!(
            d.submit_injected(sid, &OpCall::scale(1, DEMO_WORDS, 10, src.0, dst.0)),
            Err(HalError::BadArg)
        );
        d.hot_add_scale().unwrap();
        assert_eq!(d.opinject_epoch(), epoch0);
        assert_eq!(d.opinject_launches(), launches0);
        let scv = d.slot_version(SLOT_SCALE).unwrap();
        d.submit_injected(sid, &OpCall::scale(scv, DEMO_WORDS, 10, src.0, dst.0))
            .unwrap();
        assert!(d.last_inject_firewall());
        drop(d);
        assert_eq!(i32::from_le_bytes(backing[16..20].try_into().unwrap()), 10);
    }

    #[test]
    fn sid_at_submit_refuses_unbound_and_foreign_iova() {
        let mut backing = [0u8; 512];
        for i in 0..DEMO_WORDS {
            let off = (i as usize) * 4;
            backing[off..off + 4].copy_from_slice(&(1i32).to_le_bytes());
        }
        backing[256..260].copy_from_slice(&0x1111_2222u32.to_le_bytes());
        let sid_a = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        let sid_b = StreamId::accel(ChipletId(1), TileId(3), CP_SSID);
        let cap_a = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 41, TenantId(1))
            .with_generation(1);
        let cap_b = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 42, TenantId(2))
            .with_generation(1);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: &mut backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let iova_a = d
            .map_with_cap(&cap_a, MapRequest::pin_accel(PhysAddr(0), 16, sid_a))
            .unwrap();
        let iova_b = d
            .map_with_cap(&cap_b, MapRequest::pin_accel(PhysAddr(256), 16, sid_b))
            .unwrap();
        let mv = d.slot_version(SLOT_MEMCPY).unwrap();

        // Unbound-looking submit: SID C never pinned.
        let sid_c = StreamId::accel(ChipletId(2), TileId(4), CP_SSID);
        assert_eq!(
            d.submit_injected(sid_c, &OpCall::memcpy(mv, DEMO_WORDS, iova_a.0, iova_a.0)),
            Err(HalError::Fault)
        );

        // Tenant A cannot DMA tenant B's IOVA (SID window).
        assert_eq!(
            d.submit_injected(sid_a, &OpCall::memcpy(mv, DEMO_WORDS, iova_b.0, iova_a.0)),
            Err(HalError::Fault)
        );
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, 0x1111_2222);
    }

    #[test]
    fn firewall_copy_then_validate_on_inject() {
        let mut backing = [0u8; 32];
        backing[0..4].copy_from_slice(&7i32.to_le_bytes());
        let (mut d, sid, src, dst) = pinned_cp(&mut backing);
        let mv = d.slot_version(SLOT_MEMCPY).unwrap();
        d.submit_injected(sid, &OpCall::memcpy(mv, 1, src.0, dst.0))
            .unwrap();
        assert!(d.last_inject_firewall());
        assert!(d.last_cmd().is_some());
        assert_eq!(
            d.last_cmd().unwrap().flags & CP_FLAG_SET_SID,
            CP_FLAG_SET_SID
        );
        assert_eq!(d.last_cmd().unwrap().stream_id, sid.raw());
    }

    #[test]
    fn core_demo_still_all_ok() {
        assert!(run_opinject_demo().all_ok());
    }
}
