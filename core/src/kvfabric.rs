//! Sequence-scoped KV handoff for disaggregated prefill / decode.
//!
//! The noun that moves is not a virtual address. It is a capability to
//! one sequence's token window. Weights are a different object and are
//! never placed in the grant. Decode receives READ|MAP and cannot write,
//! re-grant, or outlive `revoke`. A second tenant on the same HBM bank
//! cannot mint the page, and a stolen object id still fails the object
//! tenant check.
//!
//! Toy byte counts. The ratio is the claim: the fabric carries a 32-byte
//! grant record; the KV page and the weights stay resident. Soft SMMU is
//! software. Not NVLink, not a CUDA graph, not MIG, not measured TTFT.

use crate::arena::{ArenaAllocator, ArenaRequest};
use crate::caps::{CapError, CapKind, CapRights, CapTable, Capability, CPtr};
use crate::iommu::{IommuMap, MapError, MapRequest, StreamId};
use crate::space::MemorySpace;
use crate::types::{BankId, ChipletId, PhysAddr, TenantId, TileId};

/// On-fabric authority record. Not the tensor.
pub const GRANT_RECORD_BYTES: u64 = 32;

/// Toy geometry so the host test is exact. Not a model shape.
pub const DEMO_LAYERS: u16 = 4;
pub const DEMO_TOKENS: u32 = 128;
pub const DEMO_BYTES_PER_TOKEN: u64 = 256;
pub const DEMO_KV_BYTES: u64 =
    (DEMO_LAYERS as u64) * (DEMO_TOKENS as u64) * DEMO_BYTES_PER_TOKEN;
pub const DEMO_WEIGHT_BYTES: u64 = 8 * 1024 * 1024;

/// Prefill GPU, decode GPU, neighbor tenant. Distinct STE keys.
pub const SID_PREFILL: u32 = StreamId::accel(ChipletId(0), TileId(2), 0).0;
pub const SID_DECODE: u32 = StreamId::accel(ChipletId(1), TileId(3), 0).0;
pub const SID_NEIGHBOR: u32 = StreamId::accel(ChipletId(0), TileId(4), 0).0;

const LEDGER_CAP: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvKind {
    /// Activations for one sequence. This is what decode is allowed to read.
    Kv,
    /// Parameters. They stay on the producer. A KV grant does not name them.
    Weights,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvWindow {
    pub layer_lo: u16,
    pub layer_hi: u16,
    pub token_lo: u32,
    pub token_hi: u32,
}

impl KvWindow {
    pub const fn demo() -> Self {
        Self {
            layer_lo: 0,
            layer_hi: DEMO_LAYERS,
            token_lo: 0,
            token_hi: DEMO_TOKENS,
        }
    }

    pub const fn contains(self, layer: u16, token: u32) -> bool {
        layer >= self.layer_lo
            && layer < self.layer_hi
            && token >= self.token_lo
            && token < self.token_hi
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvObject {
    pub id: u32,
    pub seq: u32,
    pub base: PhysAddr,
    pub bytes: u64,
    pub kind: KvKind,
    pub window: KvWindow,
    pub tenant: TenantId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttendReq {
    pub seq: u32,
    pub layer: u16,
    pub token: u32,
    pub write: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvError {
    CrossTenant,
    InsufficientRights,
    WouldWrite,
    WouldRegrant,
    WeightsNotGranted,
    WindowOob,
    SeqMismatch,
    WrongStream,
    Revoked,
    DeadlineMiss,
    BadGrant,
}

#[derive(Clone, Debug)]
pub struct KvLedger {
    objs: [Option<KvObject>; LEDGER_CAP],
}

impl KvLedger {
    pub const fn new() -> Self {
        Self {
            objs: [None; LEDGER_CAP],
        }
    }

    pub fn insert(&mut self, obj: KvObject) -> Result<(), KvError> {
        let slot = self
            .objs
            .iter()
            .position(|s| s.is_none())
            .ok_or(KvError::BadGrant)?;
        self.objs[slot] = Some(obj);
        Ok(())
    }

    pub fn get(&self, id: u32) -> Option<&KvObject> {
        self.objs.iter().flatten().find(|o| o.id == id)
    }
}

/// Admit a decode wave against an inter-token budget.
///
/// A miss drops that wave. It does not widen the grant and it does not
/// stop a neighbor sequence. Ticks are a software counter, not a timer
/// and not a latency measurement.
pub fn admit_kv_wave(cost_ticks: u32, budget_ticks: u32) -> Result<(), KvError> {
    if cost_ticks > budget_ticks {
        Err(KvError::DeadlineMiss)
    } else {
        Ok(())
    }
}

/// Attend one token. The cap's object must be a KV page owned by the
/// same tenant. Naming a weight id, a stolen object id, or a token
/// outside the window fails closed.
pub fn attend(
    table: &CapTable,
    cptr: CPtr,
    ledger: &KvLedger,
    req: AttendReq,
) -> Result<(), KvError> {
    let cap = match table.lookup(cptr) {
        Ok(cap) => cap,
        Err(CapError::EmptySlot) | Err(CapError::InvalidCptr) => return Err(KvError::Revoked),
        Err(CapError::CrossTenant) => return Err(KvError::CrossTenant),
        Err(_) => return Err(KvError::BadGrant),
    };
    if cap.kind != CapKind::Memory {
        return Err(KvError::BadGrant);
    }
    if cap.tenant != table.owner() {
        return Err(KvError::CrossTenant);
    }
    let obj = ledger.get(cap.object).ok_or(KvError::BadGrant)?;
    if obj.tenant != cap.tenant {
        return Err(KvError::CrossTenant);
    }
    if obj.kind != KvKind::Kv {
        return Err(KvError::WeightsNotGranted);
    }
    if req.seq != obj.seq {
        return Err(KvError::SeqMismatch);
    }
    if req.write {
        if !cap.rights.contains(CapRights::WRITE) {
            return Err(KvError::WouldWrite);
        }
    } else if !cap.rights.contains(CapRights::READ) {
        return Err(KvError::InsufficientRights);
    }
    if !obj.window.contains(req.layer, req.token) {
        return Err(KvError::WindowOob);
    }
    Ok(())
}

/// Decode may not derive or move the grant. GRANT was stripped at handoff.
pub fn try_regrant(
    src_table: &mut CapTable,
    src: CPtr,
    dest: &mut CapTable,
) -> Result<CPtr, KvError> {
    let cap = src_table.lookup(src).map_err(|_| KvError::Revoked)?;
    if !cap.rights.contains(CapRights::GRANT) {
        return Err(KvError::WouldRegrant);
    }
    src_table
        .transfer(src, dest, cap.rights, false)
        .map_err(|_| KvError::WouldRegrant)
}

/// Pin KV for DMA. `writable` is taken from the cap, not from the caller.
/// A READ|MAP grant cannot install a writable translation.
pub fn pin_kv(
    iommu: &mut IommuMap,
    cap: &Capability,
    ledger: &KvLedger,
    sid: StreamId,
) -> Result<crate::iommu::MappedRegion, KvError> {
    if cap.kind != CapKind::Memory || !cap.rights.contains(CapRights::MAP) {
        return Err(KvError::InsufficientRights);
    }
    let obj = ledger.get(cap.object).ok_or(KvError::BadGrant)?;
    if obj.tenant != cap.tenant {
        return Err(KvError::CrossTenant);
    }
    if obj.kind != KvKind::Kv {
        return Err(KvError::WeightsNotGranted);
    }
    let writable = cap.rights.contains(CapRights::WRITE);
    let req = MapRequest {
        guest_pa: obj.base,
        len: obj.bytes,
        stream_id: sid.raw(),
        writable,
    };
    iommu.map(cap, req).map_err(map_err)
}

fn map_err(err: MapError) -> KvError {
    match err {
        MapError::CrossTenant => KvError::CrossTenant,
        MapError::WrongStream => KvError::WrongStream,
        MapError::StreamAbort => KvError::WrongStream,
        MapError::NoMemoryCap => KvError::InsufficientRights,
        _ => KvError::BadGrant,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvReport {
    pub handoff_ok: bool,
    pub weights_stay: bool,
    pub read_only: bool,
    pub regrant_refused: bool,
    pub forge_refused: bool,
    pub oob_refused: bool,
    pub wrong_sid: bool,
    pub revoke_ok: bool,
    pub neighbor_lives: bool,
    pub deadline_isolated: bool,
    pub decode_readonly_dma: bool,
    pub kv_bytes: u64,
    pub weight_bytes: u64,
    pub fabric_bytes: u64,
    pub copy_bytes: u64,
}

impl KvReport {
    pub fn all_ok(&self) -> bool {
        self.handoff_ok
            && self.weights_stay
            && self.read_only
            && self.regrant_refused
            && self.forge_refused
            && self.oob_refused
            && self.wrong_sid
            && self.revoke_ok
            && self.neighbor_lives
            && self.deadline_isolated
            && self.decode_readonly_dma
            && self.fabric_bytes == GRANT_RECORD_BYTES
            && self.copy_bytes == self.kv_bytes
            && self.fabric_bytes < self.kv_bytes
            && self.kv_bytes < self.weight_bytes
    }
}

/// Host-identical clip. Same path a kernel self-check would call.
pub fn run_kv_fabric_demo() -> KvReport {
    let tenant_a = TenantId(1);
    let tenant_b = TenantId(2);
    let seq_a = 1u32;
    let seq_b = 2u32;

    let mut arenas = ArenaAllocator::new(&[(
        BankId(0),
        PhysAddr(0x8000_0000),
        32 * 1024 * 1024,
    )])
    .unwrap();
    let kv_a = arenas
        .alloc(
            ArenaRequest::tensor(DEMO_KV_BYTES, Some(BankId(0)))
                .in_space(MemorySpace::DeviceHbm)
                .for_tenant(tenant_a),
        )
        .unwrap();
    let w_a = arenas
        .alloc(
            ArenaRequest::tensor(DEMO_WEIGHT_BYTES, Some(BankId(0)))
                .in_space(MemorySpace::DeviceHbm)
                .for_tenant(tenant_a),
        )
        .unwrap();
    let kv_b = arenas
        .alloc(
            ArenaRequest::tensor(DEMO_KV_BYTES, Some(BankId(0)))
                .in_space(MemorySpace::DeviceHbm)
                .for_tenant(tenant_b),
        )
        .unwrap();

    let mut ledger = KvLedger::new();
    ledger
        .insert(KvObject {
            id: kv_a.id.0,
            seq: seq_a,
            base: kv_a.base,
            bytes: kv_a.size,
            kind: KvKind::Kv,
            window: KvWindow::demo(),
            tenant: tenant_a,
        })
        .unwrap();
    ledger
        .insert(KvObject {
            id: w_a.id.0,
            seq: seq_a,
            base: w_a.base,
            bytes: w_a.size,
            kind: KvKind::Weights,
            window: KvWindow::demo(),
            tenant: tenant_a,
        })
        .unwrap();
    ledger
        .insert(KvObject {
            id: kv_b.id.0,
            seq: seq_b,
            base: kv_b.base,
            bytes: kv_b.size,
            kind: KvKind::Kv,
            window: KvWindow::demo(),
            tenant: tenant_b,
        })
        .unwrap();

    let mut prefill = CapTable::new(tenant_a);
    let mut decode = CapTable::new(tenant_a);
    let mut neighbor = CapTable::new(tenant_b);
    let mut sidecar = CapTable::new(TenantId(3));

    let root = prefill
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            kv_a.id.0,
            tenant_a,
        ))
        .unwrap();
    let weights = prefill
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            w_a.id.0,
            tenant_a,
        ))
        .unwrap();
    let neigh_kv = neighbor
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            kv_b.id.0,
            tenant_b,
        ))
        .unwrap();

    let decode_rights = CapRights(CapRights::READ | CapRights::MAP);
    let dec = prefill
        .transfer(root, &mut decode, decode_rights, false)
        .unwrap();

    let inside = AttendReq {
        seq: seq_a,
        layer: 1,
        token: 16,
        write: false,
    };
    let handoff_ok = attend(&decode, dec, &ledger, inside).is_ok()
        && decode
            .require(dec, CapKind::Memory, CapRights::READ)
            .is_ok()
        && !decode.lookup(dec).unwrap().rights.contains(CapRights::WRITE)
        && !decode.lookup(dec).unwrap().rights.contains(CapRights::GRANT)
        && prefill.holds(CapKind::Memory, kv_a.id.0);

    let weights_stay = prefill.holds(CapKind::Memory, w_a.id.0)
        && !decode.holds(CapKind::Memory, w_a.id.0)
        && ledger.get(w_a.id.0).unwrap().kind == KvKind::Weights
        && attend(
            &prefill,
            weights,
            &ledger,
            AttendReq {
                seq: seq_a,
                layer: 0,
                token: 0,
                write: false,
            },
        ) == Err(KvError::WeightsNotGranted);

    let read_only = attend(
        &decode,
        dec,
        &ledger,
        AttendReq {
            seq: seq_a,
            layer: 1,
            token: 16,
            write: true,
        },
    ) == Err(KvError::WouldWrite);

    let regrant_refused = try_regrant(&mut decode, dec, &mut sidecar) == Err(KvError::WouldRegrant)
        && !sidecar.holds(CapKind::Memory, kv_a.id.0);

    let mint_other = neighbor.mint(Capability::new(
        CapKind::Memory,
        CapRights::MEM_FULL,
        kv_a.id.0,
        tenant_a,
    ));
    let stolen = neighbor
        .mint(Capability::new(
            CapKind::Memory,
            CapRights::MEM_FULL,
            kv_a.id.0,
            tenant_b,
        ))
        .unwrap();
    let stolen_attend = attend(&neighbor, stolen, &ledger, inside);
    neighbor.revoke(stolen).unwrap();
    let forge_refused = mint_other == Err(CapError::CrossTenant)
        && stolen_attend == Err(KvError::CrossTenant)
        && !neighbor.holds(CapKind::Memory, kv_a.id.0);

    let oob_refused = attend(
        &decode,
        dec,
        &ledger,
        AttendReq {
            seq: seq_a,
            layer: 1,
            token: DEMO_TOKENS,
            write: false,
        },
    ) == Err(KvError::WindowOob);

    let mut iommu = IommuMap::new();
    let pin_prefill = pin_kv(
        &mut iommu,
        prefill.lookup(root).unwrap(),
        &ledger,
        StreamId::from_raw(SID_PREFILL),
    )
    .unwrap();
    let pin_decode = pin_kv(
        &mut iommu,
        decode.lookup(dec).unwrap(),
        &ledger,
        StreamId::from_raw(SID_DECODE),
    )
    .unwrap();
    let pin_neighbor = pin_kv(
        &mut iommu,
        neighbor.lookup(neigh_kv).unwrap(),
        &ledger,
        StreamId::from_raw(SID_NEIGHBOR),
    )
    .unwrap();
    let decode_walk = iommu
        .walk(StreamId::from_raw(SID_DECODE), pin_decode.iova)
        .map(|w| w.pa);
    let neighbor_walk = iommu.resolve_result(SID_NEIGHBOR, pin_decode.iova, None);
    let wrong_sid = decode_walk == Ok(kv_a.base)
        && neighbor_walk == Err(MapError::WrongStream)
        && pin_prefill.writable
        && !pin_decode.writable;
    let decode_readonly_dma = !pin_decode.writable
        && iommu.walk(StreamId::from_raw(SID_DECODE), pin_decode.iova).is_ok()
        && !iommu.covers_stream(SID_DECODE, w_a.base, w_a.size)
        && iommu
            .walk(StreamId::from_raw(SID_NEIGHBOR), pin_neighbor.iova)
            .map(|w| w.pa)
            == Ok(kv_b.base);

    let wave_a = admit_kv_wave(40, 30);
    let wave_b = admit_kv_wave(12, 30);
    let b_still = attend(
        &neighbor,
        neigh_kv,
        &ledger,
        AttendReq {
            seq: seq_b,
            layer: 0,
            token: 4,
            write: false,
        },
    );
    let deadline_isolated = wave_a == Err(KvError::DeadlineMiss) && wave_b.is_ok() && b_still.is_ok();

    prefill.revoke_in(root, &mut [&mut decode]).unwrap();
    let after = attend(&decode, dec, &ledger, inside);
    let b_after = attend(
        &neighbor,
        neigh_kv,
        &ledger,
        AttendReq {
            seq: seq_b,
            layer: 2,
            token: 8,
            write: false,
        },
    );
    let revoke_ok = after == Err(KvError::Revoked) && decode.lookup(dec).is_err();
    let neighbor_lives = b_after.is_ok() && neighbor.holds(CapKind::Memory, kv_b.id.0);

    KvReport {
        handoff_ok,
        weights_stay,
        read_only,
        regrant_refused,
        forge_refused,
        oob_refused,
        wrong_sid,
        revoke_ok,
        neighbor_lives,
        deadline_isolated,
        decode_readonly_dma,
        kv_bytes: DEMO_KV_BYTES,
        weight_bytes: DEMO_WEIGHT_BYTES,
        fabric_bytes: GRANT_RECORD_BYTES,
        copy_bytes: DEMO_KV_BYTES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_fabric_grant_is_the_only_thing_that_moves() {
        let r = run_kv_fabric_demo();
        assert!(r.handoff_ok, "handoff");
        assert!(r.weights_stay, "weights");
        assert!(r.read_only, "write");
        assert!(r.regrant_refused, "regrant");
        assert!(r.forge_refused, "forge");
        assert!(r.oob_refused, "oob");
        assert!(r.wrong_sid, "sid");
        assert!(r.revoke_ok, "revoke");
        assert!(r.neighbor_lives, "neighbor");
        assert!(r.deadline_isolated, "deadline");
        assert!(r.decode_readonly_dma, "dma");
        assert!(r.all_ok());
        assert_eq!(r.kv_bytes, 131_072);
        assert_eq!(r.fabric_bytes, 32);
        assert_eq!(r.weight_bytes, 8 * 1024 * 1024);
        assert_ne!(SID_PREFILL, SID_DECODE);
        assert_ne!(SID_DECODE, SID_NEIGHBOR);
    }
}
