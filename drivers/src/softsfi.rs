//! Soft-CP host for the toy SoftSFI bytecode sandbox.
//!
//! Lives **beside** [`crate::fakecp`] so SoftGreenCtx / SoftCmdFirewall
//! can keep editing the CP without merging this ISA. GPU-AToLL-shaped
//! SFI: every load/store/dma/`atomic_add` proves `base+bound` in the
//! SID IOVA window. Not NVVM, not “safe multi-tenant kernels.”
//! Tensor / heap / unknown stay `Unmodeled` as **named refuses** (sell:
//! `[softsfi] tensor=refused` / `[softsfi] heap=refused` /
//! `[softsfi] unknown=refused`). Load/store with no proved base window is
//! `UnknownBase` (sell: `[softsfi] unknown-base=refused`) — separate from
//! Unmodeled. Heap is not a bump allocator. Unknown is bad opcode /
//! illegal width — not AddImm deepen. Not SoftSFI deepen past Unmodeled
//! for new ops. `atomic_add` is a sequential toy RMW, not a coherent
//! hardware atomic.

use aether_core::accel::DmaView;
use aether_core::iommu::{IommuMap, StreamId};
use aether_core::softsfi::{
    execute_unverified, run as sfi_run, verify as sfi_verify, Program, SfiError, SfiExec, SfiMem,
    SidSandbox,
};
use aether_core::types::PhysAddr;
use aether_hal::HalError;

use crate::fakecp::SoftCommandProcessor;

fn map_sfi_error(e: SfiError) -> HalError {
    match e {
        SfiError::BadInsn => HalError::BadArg,
        SfiError::Oob | SfiError::Unmodeled | SfiError::UnknownBase => HalError::Fault,
    }
}

/// SoftSFI backing: SID walk then [`DmaView`]. Not a CUDA device arena.
struct CpSfiMem<'a, M: DmaView> {
    iommu: &'a IommuMap,
    sid: StreamId,
    mem: &'a mut M,
}

impl<M: DmaView> SfiMem for CpSfiMem<'_, M> {
    fn load_u32(&self, addr: u64) -> Result<u32, SfiError> {
        let pa = self
            .iommu
            .resolve_stream(self.sid.raw(), PhysAddr(addr))
            .ok_or(SfiError::Oob)?;
        self.mem
            .load_i32(pa)
            .map(|v| v as u32)
            .map_err(|_| SfiError::Oob)
    }

    fn store_u32(&mut self, addr: u64, val: u32) -> Result<(), SfiError> {
        let pa = self
            .iommu
            .resolve_stream(self.sid.raw(), PhysAddr(addr))
            .ok_or(SfiError::Oob)?;
        self.mem
            .store_i32(pa, val as i32)
            .map_err(|_| SfiError::Oob)
    }
}

impl<M: DmaView> SoftCommandProcessor<M> {
    /// SID-allowed IOVA windows for SoftSFI. Program addresses are IOVAs.
    pub fn sfi_sandbox(&self, sid: StreamId) -> SidSandbox {
        SidSandbox::from_iommu(&self.iommu, sid)
    }

    /// Static SFI proof against this SID's pins. Does not execute.
    pub fn verify_sfi(&self, sid: StreamId, prog: &Program) -> Result<(), SfiError> {
        sfi_verify(prog, &self.sfi_sandbox(sid))
    }

    /// Verify then run toy bytecode. Memory ops resolve IOVA → PA on `sid`.
    pub fn submit_sfi(&mut self, sid: StreamId, prog: &Program) -> Result<SfiExec, HalError> {
        let sandbox = self.sfi_sandbox(sid);
        let mut view = CpSfiMem {
            iommu: &self.iommu,
            sid,
            mem: &mut self.mem,
        };
        sfi_run(prog, &sandbox, &mut view).map_err(map_sfi_error)
    }

    /// Skip the static verifier (fault injection). Runtime SID trap remains;
    /// a foreign IOVA does not fill dest and does not read the other tenant.
    pub fn inject_sfi_skip_verify(
        &mut self,
        sid: StreamId,
        prog: &Program,
    ) -> Result<SfiExec, HalError> {
        let sandbox = self.sfi_sandbox(sid);
        let mut view = CpSfiMem {
            iommu: &self.iommu,
            sid,
            mem: &mut self.mem,
        };
        execute_unverified(prog, &sandbox, &mut view).map_err(map_sfi_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakecp::CP_SSID;
    use aether_core::accel::SliceMem;
    use aether_core::caps::{CapKind, CapRights, Capability};
    use aether_core::iommu::MapRequest;
    use aether_core::softsfi::{
        heap_alloc_prog, illegal_width_prog, in_bounds_atomic_prog, in_bounds_prog,
        oob_atomic_prog, oob_load_prog, tensor_prog, unknown_base_load_prog,
        unknown_base_store_prog, unknown_opcode_prog, Insn, Program, SFI_SECRET_B, SFI_BASE_A,
    };
    use aether_core::types::{ChipletId, TenantId, TileId};

    fn two_tenant_cp(
        backing: &mut [u8],
    ) -> (
        SoftCommandProcessor<SliceMem<'_>>,
        StreamId,
        StreamId,
        PhysAddr,
        PhysAddr,
    ) {
        let sid_a = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
        let sid_b = StreamId::accel(ChipletId(1), TileId(3), CP_SSID);
        let cap_a = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 21, TenantId(1))
            .with_generation(1);
        let cap_b = Capability::new(CapKind::Memory, CapRights::MEM_FULL, 22, TenantId(2))
            .with_generation(1);
        let mem = SliceMem {
            base: PhysAddr(0),
            bytes: backing,
        };
        let mut d = SoftCommandProcessor::new(mem);
        let iova_a = d
            .map_with_cap(&cap_a, MapRequest::pin_accel(PhysAddr(0), 256, sid_a))
            .unwrap();
        let iova_b = d
            .map_with_cap(&cap_b, MapRequest::pin_accel(PhysAddr(256), 256, sid_b))
            .unwrap();
        (d, sid_a, sid_b, iova_a, iova_b)
    }

    #[test]
    fn softsfi_accepts_in_bounds_rejects_oob_on_soft_cp() {
        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, iova_a, _iova_b) = two_tenant_cp(&mut backing);

        let ok = in_bounds_prog(iova_a.0);
        assert!(d.verify_sfi(sid_a, &ok).is_ok());
        let exec = d.submit_sfi(sid_a, &ok).unwrap();
        assert_eq!(exec.regs[2], 4);
        drop(d);
        let stored = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(stored, 4);

        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, _iova_a, iova_b) = two_tenant_cp(&mut backing);
        let oob = oob_load_prog(iova_b.0);
        assert_eq!(d.verify_sfi(sid_a, &oob), Err(SfiError::Oob));
        assert_eq!(d.submit_sfi(sid_a, &oob).unwrap_err(), HalError::Fault);

        let tens = tensor_prog(1, 0, 16);
        assert_eq!(d.verify_sfi(sid_a, &tens), Err(SfiError::Unmodeled));
        assert_eq!(d.submit_sfi(sid_a, &tens).unwrap_err(), HalError::Fault);
        let heap = heap_alloc_prog(16);
        assert_eq!(d.verify_sfi(sid_a, &heap), Err(SfiError::Unmodeled));
        assert_eq!(d.submit_sfi(sid_a, &heap).unwrap_err(), HalError::Fault);
        let _ = iova_b;
    }

    #[test]
    fn softsfi_tensor_named_unmodeled_refuse() {
        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, _iova_a, _iova_b) = two_tenant_cp(&mut backing);
        let tens = tensor_prog(1, 0, 32);
        assert_eq!(d.verify_sfi(sid_a, &tens), Err(SfiError::Unmodeled));
        assert_eq!(d.submit_sfi(sid_a, &tens).unwrap_err(), HalError::Fault);
        drop(d);
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, SFI_SECRET_B);
        let a_word = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(a_word, 3);
    }

    #[test]
    fn softsfi_heap_alloc_named_unmodeled_refuse() {
        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, _iova_a, _iova_b) = two_tenant_cp(&mut backing);
        let heap = heap_alloc_prog(32);
        let mut alloc = Program::new();
        let _ = alloc.push(Insn::alloc(1, 0, 64));
        assert_eq!(d.verify_sfi(sid_a, &heap), Err(SfiError::Unmodeled));
        assert_eq!(d.verify_sfi(sid_a, &alloc), Err(SfiError::Unmodeled));
        assert_eq!(d.submit_sfi(sid_a, &heap).unwrap_err(), HalError::Fault);
        drop(d);
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, SFI_SECRET_B);
        let a_word = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(a_word, 3);
    }

    #[test]
    fn softsfi_unknown_opcode_illegal_width_unmodeled_refuse() {
        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, _iova_a, _iova_b) = two_tenant_cp(&mut backing);
        let unk = unknown_opcode_prog(0xFF);
        assert_eq!(d.verify_sfi(sid_a, &unk), Err(SfiError::Unmodeled));
        assert_eq!(d.submit_sfi(sid_a, &unk).unwrap_err(), HalError::Fault);
        let bad_w = illegal_width_prog(SFI_BASE_A);
        assert_eq!(d.verify_sfi(sid_a, &bad_w), Err(SfiError::Unmodeled));
        assert_eq!(d.submit_sfi(sid_a, &bad_w).unwrap_err(), HalError::Fault);
        drop(d);
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, SFI_SECRET_B);
        let a_word = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(a_word, 3);
    }

    #[test]
    fn softsfi_unknown_base_load_store_refused() {
        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, _iova_a, _iova_b) = two_tenant_cp(&mut backing);
        let load = unknown_base_load_prog();
        assert_eq!(d.verify_sfi(sid_a, &load), Err(SfiError::UnknownBase));
        assert_eq!(d.submit_sfi(sid_a, &load).unwrap_err(), HalError::Fault);
        let store = unknown_base_store_prog();
        assert_eq!(d.verify_sfi(sid_a, &store), Err(SfiError::UnknownBase));
        assert_eq!(d.submit_sfi(sid_a, &store).unwrap_err(), HalError::Fault);
        drop(d);
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, SFI_SECRET_B);
        let a_word = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(a_word, 3);
    }



    #[test]
    fn softsfi_atomic_in_range_accept_cross_tenant_reject() {
        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, iova_a, _iova_b) = two_tenant_cp(&mut backing);

        let ok = in_bounds_atomic_prog(iova_a.0, 5);
        assert!(d.verify_sfi(sid_a, &ok).is_ok());
        let exec = d.submit_sfi(sid_a, &ok).unwrap();
        assert_eq!(exec.regs[2], 3);
        drop(d);
        let stored = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(stored, 8);
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, SFI_SECRET_B);

        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&3u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, _iova_a, iova_b) = two_tenant_cp(&mut backing);
        let steal = oob_atomic_prog(iova_b.0);
        assert_eq!(d.verify_sfi(sid_a, &steal), Err(SfiError::Oob));
        assert_eq!(d.submit_sfi(sid_a, &steal).unwrap_err(), HalError::Fault);
        drop(d);
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, SFI_SECRET_B);
        let a_word = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(a_word, 3);
    }

    #[test]
    fn softsfi_two_tenants_fault_inject_no_cross_read() {
        let mut backing = [0u8; 512];
        backing[0..4].copy_from_slice(&1u32.to_le_bytes());
        backing[256..260].copy_from_slice(&SFI_SECRET_B.to_le_bytes());
        let (mut d, sid_a, _sid_b, _iova_a, iova_b) = two_tenant_cp(&mut backing);

        let steal = oob_load_prog(iova_b.0);
        assert_eq!(
            d.inject_sfi_skip_verify(sid_a, &steal).unwrap_err(),
            HalError::Fault
        );
        drop(d);
        let secret = u32::from_le_bytes(backing[256..260].try_into().unwrap());
        assert_eq!(secret, SFI_SECRET_B);
        let a_word = u32::from_le_bytes(backing[0..4].try_into().unwrap());
        assert_eq!(a_word, 1);
    }
}
