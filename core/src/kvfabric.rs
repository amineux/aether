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

/// Host red-team report for KV grants missing a needed right.
///
/// Sell line `[redteam] attack=kv-insufficient-rights` — existing [`attend`]
/// / [`pin_kv`] only. A same-tenant KV cap derived **without READ** cannot
/// attend (read) a token, and a cap **without MAP** (or a non-Memory cap
/// naming the KV object) cannot pin the page for DMA: both are
/// [`KvError::InsufficientRights`], and the refused pin installs no Soft-SMMU
/// translation for its SID. Rights come from the cap, never the caller.
/// **Not** kv `write` (`WouldWrite`) / `regrant` / `weights` / `oob` /
/// `forge` (`CrossTenant`) / `wrong-sid`; Soft SMMU is software; no new
/// opcodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvInsufficientRightsReport {
    /// Control: a READ|MAP grant attends and pins (one translation).
    pub grant_ok: bool,
    /// WRITE-only cap (no READ) read-attend → `InsufficientRights`.
    pub no_read_refused: bool,
    /// READ-only cap (no MAP) `pin_kv` → `InsufficientRights`; no translation.
    pub no_map_refused: bool,
    /// Non-Memory cap naming the KV object `pin_kv` → `InsufficientRights`.
    pub wrong_kind_refused: bool,
    /// After the refusals the Soft-SMMU still holds only the control pin.
    pub no_stray_mapping: bool,
}

impl KvInsufficientRightsReport {
    pub fn all_ok(&self) -> bool {
        self.grant_ok
            && self.no_read_refused
            && self.no_map_refused
            && self.wrong_kind_refused
            && self.no_stray_mapping
    }
}

/// KV grant missing READ / MAP (or wrong kind) → [`KvError::InsufficientRights`].
pub fn run_kv_insufficient_rights_demo() -> KvInsufficientRightsReport {
    let fail = KvInsufficientRightsReport {
        grant_ok: false,
        no_read_refused: false,
        no_map_refused: false,
        wrong_kind_refused: false,
        no_stray_mapping: false,
    };
    let tenant = TenantId(1);
    let seq = 1u32;
    let Ok(mut arenas) =
        ArenaAllocator::new(&[(BankId(0), PhysAddr(0x8000_0000), 32 * 1024 * 1024)])
    else {
        return fail;
    };
    let Ok(kv) = arenas.alloc(
        ArenaRequest::tensor(DEMO_KV_BYTES, Some(BankId(0)))
            .in_space(MemorySpace::DeviceHbm)
            .for_tenant(tenant),
    ) else {
        return fail;
    };
    let mut ledger = KvLedger::new();
    if ledger
        .insert(KvObject {
            id: kv.id.0,
            seq,
            base: kv.base,
            bytes: kv.size,
            kind: KvKind::Kv,
            window: KvWindow::demo(),
            tenant,
        })
        .is_err()
    {
        return fail;
    }

    let mut prefill = CapTable::new(tenant);
    let Ok(root) = prefill.mint(Capability::new(
        CapKind::Memory,
        CapRights::MEM_FULL,
        kv.id.0,
        tenant,
    )) else {
        return fail;
    };
    let (Ok(rm), Ok(wo), Ok(ro)) = (
        prefill.derive(root, CapRights(CapRights::READ | CapRights::MAP)),
        prefill.derive(root, CapRights(CapRights::WRITE)),
        prefill.derive(root, CapRights(CapRights::READ)),
    ) else {
        return fail;
    };
    let read = AttendReq {
        seq,
        layer: 1,
        token: 16,
        write: false,
    };
    let sid_ok = StreamId::from_raw(SID_DECODE);
    let sid_ro = StreamId::from_raw(SID_PREFILL);
    let sid_kind = StreamId::from_raw(SID_NEIGHBOR);

    let mut iommu = IommuMap::new();
    let grant_ok = attend(&prefill, rm, &ledger, read).is_ok()
        && prefill
            .lookup(rm)
            .map(|cap| pin_kv(&mut iommu, cap, &ledger, sid_ok).is_ok())
            == Ok(true)
        && iommu.len() == 1;

    let no_read_refused = attend(&prefill, wo, &ledger, read) == Err(KvError::InsufficientRights);

    let no_map_refused = attend(&prefill, ro, &ledger, read).is_ok()
        && prefill
            .lookup(ro)
            .map(|cap| pin_kv(&mut iommu, cap, &ledger, sid_ro).err())
            == Ok(Some(KvError::InsufficientRights))
        && !iommu.covers_stream(sid_ro.raw(), kv.base, kv.size);

    let wrong_kind = Capability::new(CapKind::OperatorKernel, CapRights::MEM_FULL, kv.id.0, tenant);
    let wrong_kind_refused = pin_kv(&mut iommu, &wrong_kind, &ledger, sid_kind).err()
        == Some(KvError::InsufficientRights)
        && !iommu.covers_stream(sid_kind.raw(), kv.base, kv.size);

    let no_stray_mapping = iommu.len() == 1 && iommu.covers_stream(sid_ok.raw(), kv.base, kv.size);

    KvInsufficientRightsReport {
        grant_ok,
        no_read_refused,
        no_map_refused,
        wrong_kind_refused,
        no_stray_mapping,
    }
}

/// Host red-team report for a KV grant used against another sequence.
///
/// Sell line `[redteam] attack=kv-seq-mismatch` — existing [`attend`] only.
/// A decode cap granted for one sequence's KV page cannot attend a token of
/// a different sequence (same tenant, same bank): [`KvError::SeqMismatch`].
/// The sequence gate runs before the rights checks, so a wrong-sequence
/// write probe is also `SeqMismatch` (it learns nothing about WRITE). After
/// `revoke`, the same cap is [`KvError::Revoked`] for every sequence, and
/// the other sequence's own grant keeps working. **Not** kv
/// `insufficient-rights` / `write` (`WouldWrite`) / `oob` (`WindowOob`) /
/// `forge` (`CrossTenant`) / CapTable; Soft SMMU is software; no new opcodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvSeqMismatchReport {
    /// Control: seq-1 grant attends a seq-1 token.
    pub own_seq_ok: bool,
    /// Seq-1 grant naming seq 2 (read, write, `u32::MAX`) → `SeqMismatch`.
    pub other_seq_refused: bool,
    /// Seq-2 grant still attends seq 2; decode never holds the seq-2 page.
    pub other_seq_untouched: bool,
    /// After revoke: seq-1 cap → `Revoked` for seq 1 and seq 2 alike.
    pub revoked_refused: bool,
    /// Revoking the seq-1 grant leaves the seq-2 grant working.
    pub revoke_scoped: bool,
}

impl KvSeqMismatchReport {
    pub fn all_ok(&self) -> bool {
        self.own_seq_ok
            && self.other_seq_refused
            && self.other_seq_untouched
            && self.revoked_refused
            && self.revoke_scoped
    }
}

/// KV grant for sequence 1 attending sequence 2 → [`KvError::SeqMismatch`];
/// after revoke → [`KvError::Revoked`].
pub fn run_kv_seq_mismatch_demo() -> KvSeqMismatchReport {
    let fail = KvSeqMismatchReport {
        own_seq_ok: false,
        other_seq_refused: false,
        other_seq_untouched: false,
        revoked_refused: false,
        revoke_scoped: false,
    };
    let tenant = TenantId(1);
    let (seq_1, seq_2) = (1u32, 2u32);
    let Ok(mut arenas) =
        ArenaAllocator::new(&[(BankId(0), PhysAddr(0x8000_0000), 32 * 1024 * 1024)])
    else {
        return fail;
    };
    let req = ArenaRequest::tensor(DEMO_KV_BYTES, Some(BankId(0)))
        .in_space(MemorySpace::DeviceHbm)
        .for_tenant(tenant);
    let (Ok(kv_1), Ok(kv_2)) = (arenas.alloc(req), arenas.alloc(req)) else {
        return fail;
    };
    let mut ledger = KvLedger::new();
    for (a, seq) in [(&kv_1, seq_1), (&kv_2, seq_2)] {
        if ledger
            .insert(KvObject {
                id: a.id.0,
                seq,
                base: a.base,
                bytes: a.size,
                kind: KvKind::Kv,
                window: KvWindow::demo(),
                tenant,
            })
            .is_err()
        {
            return fail;
        }
    }

    let mut prefill = CapTable::new(tenant);
    let mut decode = CapTable::new(tenant);
    let mut decode_2 = CapTable::new(tenant);
    let (Ok(root_1), Ok(root_2)) = (
        prefill.mint(Capability::new(CapKind::Memory, CapRights::MEM_FULL, kv_1.id.0, tenant)),
        prefill.mint(Capability::new(CapKind::Memory, CapRights::MEM_FULL, kv_2.id.0, tenant)),
    ) else {
        return fail;
    };
    let rights = CapRights(CapRights::READ | CapRights::MAP);
    let (Ok(dec), Ok(dec_2)) = (
        prefill.transfer(root_1, &mut decode, rights, false),
        prefill.transfer(root_2, &mut decode_2, rights, false),
    ) else {
        return fail;
    };
    let at = |seq: u32, write: bool| AttendReq {
        seq,
        layer: 1,
        token: 16,
        write,
    };

    let own_seq_ok = attend(&decode, dec, &ledger, at(seq_1, false)).is_ok();
    let other_seq_refused = attend(&decode, dec, &ledger, at(seq_2, false))
        == Err(KvError::SeqMismatch)
        && attend(&decode, dec, &ledger, at(seq_2, true)) == Err(KvError::SeqMismatch)
        && attend(&decode, dec, &ledger, at(u32::MAX, false)) == Err(KvError::SeqMismatch);
    let other_seq_untouched = attend(&decode_2, dec_2, &ledger, at(seq_2, false)).is_ok()
        && !decode.holds(CapKind::Memory, kv_2.id.0);

    if prefill.revoke_in(root_1, &mut [&mut decode]).is_err() {
        return fail;
    }
    let revoked_refused = attend(&decode, dec, &ledger, at(seq_1, false)) == Err(KvError::Revoked)
        && attend(&decode, dec, &ledger, at(seq_2, false)) == Err(KvError::Revoked)
        && !decode.holds(CapKind::Memory, kv_1.id.0);
    let revoke_scoped = attend(&decode_2, dec_2, &ledger, at(seq_2, false)).is_ok()
        && decode_2.holds(CapKind::Memory, kv_2.id.0);

    KvSeqMismatchReport {
        own_seq_ok,
        other_seq_refused,
        other_seq_untouched,
        revoked_refused,
        revoke_scoped,
    }
}

/// Host red-team report for a KV pin on an out-of-range Soft-SMMU substream.
///
/// Sell line `[redteam] attack=kv-wrong-stream`. It uses only the existing
/// [`pin_kv`] path. A valid READ|MAP decode grant pinned on a stream whose
/// SSID is past the Soft-SMMU context-descriptor range (`>= MAX_CDS`) is
/// refused. The Soft SMMU aborts the stream (`MapError::StreamAbort`), and
/// `pin_kv` reports that as [`KvError::WrongStream`]: the KV layer folds
/// both `WrongStream` and `StreamAbort` into one variant. The refused pin
/// creates no STE and installs no translation, and the grant's own pin on
/// SSID 0 keeps walking to the KV page. **Not** kv `insufficient-rights` /
/// `seq-mismatch` / `wrong-sid` (resolve on a neighbor SID) /
/// smmu-ssid-abort (raw `MapError`, no KV grant); Soft SMMU is software;
/// no new opcodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvWrongStreamReport {
    /// Control: the grant pins on SSID 0 (one translation, one STE).
    pub pin_ok: bool,
    /// Same grant on SSID `MAX_CDS` and `0xFF` → `WrongStream`.
    pub bad_stream_refused: bool,
    /// Refusals add no translation or STE; nothing covers the bad SIDs.
    pub no_stray_state: bool,
    /// The SSID-0 pin still walks to the KV page base.
    pub own_pin_intact: bool,
}

impl KvWrongStreamReport {
    pub fn all_ok(&self) -> bool {
        self.pin_ok && self.bad_stream_refused && self.no_stray_state && self.own_pin_intact
    }
}

/// KV pin on an out-of-range substream → [`KvError::WrongStream`].
pub fn run_kv_wrong_stream_demo() -> KvWrongStreamReport {
    let fail = KvWrongStreamReport {
        pin_ok: false,
        bad_stream_refused: false,
        no_stray_state: false,
        own_pin_intact: false,
    };
    let tenant = TenantId(1);
    let Some((ledger, kv)) = demo_kv_ledger(tenant, 1) else {
        return fail;
    };
    let mut prefill = CapTable::new(tenant);
    let Ok(root) = prefill.mint(Capability::new(
        CapKind::Memory,
        CapRights::MEM_FULL,
        kv.id.0,
        tenant,
    )) else {
        return fail;
    };
    let Ok(rm) = prefill.derive(root, CapRights(CapRights::READ | CapRights::MAP)) else {
        return fail;
    };
    let Ok(cap) = prefill.lookup(rm).copied() else {
        return fail;
    };
    let sid_ok = StreamId::from_raw(SID_DECODE);
    let bad = [
        sid_ok.with_ssid(crate::iommu::MAX_CDS as u8),
        sid_ok.with_ssid(0xFF),
    ];

    let mut iommu = IommuMap::new();
    let pin = pin_kv(&mut iommu, &cap, &ledger, sid_ok);
    let pin_ok = pin.is_ok() && iommu.len() == 1 && iommu.ste_count() == 1;

    let bad_stream_refused = bad
        .iter()
        .all(|&sid| pin_kv(&mut iommu, &cap, &ledger, sid).err() == Some(KvError::WrongStream));

    let no_stray_state = iommu.len() == 1
        && iommu.ste_count() == 1
        && bad.iter().all(|sid| !iommu.covers_stream(sid.raw(), kv.base, kv.size));

    let own_pin_intact = match pin {
        Ok(m) => iommu.walk(sid_ok, m.iova).map(|w| w.pa) == Ok(kv.base),
        Err(_) => false,
    };

    KvWrongStreamReport {
        pin_ok,
        bad_stream_refused,
        no_stray_state,
        own_pin_intact,
    }
}

/// Host red-team report for malformed / unregistered KV grants.
///
/// Sell line `[redteam] attack=kv-bad-grant`. It uses only the existing
/// [`attend`] / [`pin_kv`] / [`KvLedger::insert`] gates. The cases:
/// - a Memory cap naming an arena that is not a registered KV object
///   (same tenant, real allocation) is [`KvError::BadGrant`] on attend
///   and on pin, with no translation installed;
/// - a non-Memory cap (an operator-kernel cap naming the KV object id) is
///   `BadGrant` on attend;
/// - registering a KV object in a full ledger is `BadGrant`, and the
///   objects already in it stay readable.
///
/// The real grant keeps attending throughout. **Not** kv
/// `insufficient-rights` (non-Memory `pin_kv` is `InsufficientRights`,
/// named there) / `forge` (`CrossTenant`) / `seq-mismatch`; Soft SMMU is
/// software; no new opcodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvBadGrantReport {
    /// Control: the registered READ|MAP grant attends.
    pub grant_ok: bool,
    /// Cap on an unregistered arena → `BadGrant` (attend and pin), no pin.
    pub unregistered_refused: bool,
    /// Non-Memory cap naming the KV id → `BadGrant` on attend.
    pub wrong_kind_refused: bool,
    /// Insert into a full ledger → `BadGrant`; existing objects intact.
    pub ledger_full_refused: bool,
    /// The real grant still attends after every refusal.
    pub grant_still_ok: bool,
}

impl KvBadGrantReport {
    pub fn all_ok(&self) -> bool {
        self.grant_ok
            && self.unregistered_refused
            && self.wrong_kind_refused
            && self.ledger_full_refused
            && self.grant_still_ok
    }
}

/// Unregistered object / non-Memory cap / full ledger → [`KvError::BadGrant`].
pub fn run_kv_bad_grant_demo() -> KvBadGrantReport {
    let fail = KvBadGrantReport {
        grant_ok: false,
        unregistered_refused: false,
        wrong_kind_refused: false,
        ledger_full_refused: false,
        grant_still_ok: false,
    };
    let tenant = TenantId(1);
    let seq = 1u32;
    let Ok(mut arenas) =
        ArenaAllocator::new(&[(BankId(0), PhysAddr(0x8000_0000), 32 * 1024 * 1024)])
    else {
        return fail;
    };
    let req = ArenaRequest::tensor(DEMO_KV_BYTES, Some(BankId(0)))
        .in_space(MemorySpace::DeviceHbm)
        .for_tenant(tenant);
    let (Ok(kv), Ok(stray)) = (arenas.alloc(req), arenas.alloc(req)) else {
        return fail;
    };
    let obj = |id: u32, base: PhysAddr, bytes: u64| KvObject {
        id,
        seq,
        base,
        bytes,
        kind: KvKind::Kv,
        window: KvWindow::demo(),
        tenant,
    };
    let mut ledger = KvLedger::new();
    if ledger.insert(obj(kv.id.0, kv.base, kv.size)).is_err() {
        return fail;
    }

    let mut table = CapTable::new(tenant);
    let (Ok(good), Ok(unreg), Ok(opk)) = (
        table.mint(Capability::new(
            CapKind::Memory,
            CapRights(CapRights::READ | CapRights::MAP),
            kv.id.0,
            tenant,
        )),
        table.mint(Capability::new(
            CapKind::Memory,
            CapRights(CapRights::READ | CapRights::MAP),
            stray.id.0,
            tenant,
        )),
        table.mint(Capability::new(
            CapKind::OperatorKernel,
            CapRights::MEM_FULL,
            kv.id.0,
            tenant,
        )),
    ) else {
        return fail;
    };
    let read = AttendReq {
        seq,
        layer: 1,
        token: 16,
        write: false,
    };

    let grant_ok = attend(&table, good, &ledger, read).is_ok();

    let mut iommu = IommuMap::new();
    let unregistered_refused = attend(&table, unreg, &ledger, read) == Err(KvError::BadGrant)
        && table
            .lookup(unreg)
            .map(|cap| pin_kv(&mut iommu, cap, &ledger, StreamId::from_raw(SID_DECODE)).err())
            == Ok(Some(KvError::BadGrant))
        && iommu.is_empty()
        && !iommu.covers_stream(SID_DECODE, stray.base, stray.size);

    let wrong_kind_refused = attend(&table, opk, &ledger, read) == Err(KvError::BadGrant);

    let mut full = ledger.clone();
    let mut filled = true;
    for i in 1..LEDGER_CAP as u32 {
        filled &= full.insert(obj(0x1000 + i, kv.base, kv.size)).is_ok();
    }
    let ledger_full_refused = filled
        && full.insert(obj(0x2000, stray.base, stray.size)) == Err(KvError::BadGrant)
        && full.get(0x2000).is_none()
        && full.get(kv.id.0).is_some()
        && attend(&table, good, &full, read).is_ok();

    let grant_still_ok = attend(&table, good, &ledger, read).is_ok();

    KvBadGrantReport {
        grant_ok,
        unregistered_refused,
        wrong_kind_refused,
        ledger_full_refused,
        grant_still_ok,
    }
}

/// One registered KV page for `tenant` / `seq` (demo helper).
fn demo_kv_ledger(tenant: TenantId, seq: u32) -> Option<(KvLedger, crate::arena::Arena)> {
    let mut arenas =
        ArenaAllocator::new(&[(BankId(0), PhysAddr(0x8000_0000), 32 * 1024 * 1024)]).ok()?;
    let kv = arenas
        .alloc(
            ArenaRequest::tensor(DEMO_KV_BYTES, Some(BankId(0)))
                .in_space(MemorySpace::DeviceHbm)
                .for_tenant(tenant),
        )
        .ok()?;
    let mut ledger = KvLedger::new();
    ledger
        .insert(KvObject {
            id: kv.id.0,
            seq,
            base: kv.base,
            bytes: kv.size,
            kind: KvKind::Kv,
            window: KvWindow::demo(),
            tenant,
        })
        .ok()?;
    Some((ledger, kv))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_wrong_stream_demo_all_ok() {
        let r = run_kv_wrong_stream_demo();
        assert!(r.pin_ok, "grant pins on SSID 0: {r:?}");
        assert!(r.bad_stream_refused, "SSID >= MAX_CDS → WrongStream");
        assert!(r.no_stray_state, "refused pin adds no STE / translation");
        assert!(r.own_pin_intact, "SSID-0 pin still walks to the KV page");
        assert!(r.all_ok());
    }

    #[test]
    fn kv_bad_grant_demo_all_ok() {
        let r = run_kv_bad_grant_demo();
        assert!(r.grant_ok, "registered grant attends: {r:?}");
        assert!(r.unregistered_refused, "cap on unregistered arena → BadGrant, no pin");
        assert!(r.wrong_kind_refused, "non-Memory cap → BadGrant on attend");
        assert!(r.ledger_full_refused, "full ledger insert → BadGrant, existing intact");
        assert!(r.grant_still_ok, "real grant still attends");
        assert!(r.all_ok());
    }

    #[test]
    fn kv_seq_mismatch_demo_all_ok() {
        let r = run_kv_seq_mismatch_demo();
        assert!(r.own_seq_ok, "seq-1 grant attends seq 1: {r:?}");
        assert!(r.other_seq_refused, "seq-1 grant naming seq 2 → SeqMismatch");
        assert!(r.other_seq_untouched, "seq-2 grant still attends; decode lacks seq-2 page");
        assert!(r.revoked_refused, "revoked grant → Revoked for every seq");
        assert!(r.revoke_scoped, "revoking seq 1 leaves seq 2 working");
        assert!(r.all_ok());
    }

    #[test]
    fn kv_insufficient_rights_demo_all_ok() {
        let r = run_kv_insufficient_rights_demo();
        assert!(r.grant_ok, "READ|MAP grant attends + pins: {r:?}");
        assert!(r.no_read_refused, "WRITE-only cap read-attend → InsufficientRights");
        assert!(r.no_map_refused, "READ-only cap pin_kv → InsufficientRights, no translation");
        assert!(r.wrong_kind_refused, "non-Memory cap pin_kv → InsufficientRights");
        assert!(r.no_stray_mapping, "only the control pin is mapped");
        assert!(r.all_ok());
    }

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
