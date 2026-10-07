//! Two-world noninterference check. Host only, software model only.
//!
//! The same honest workload runs in two worlds:
//!
//! * **World 0:** tenants A and B run a tiny i32 MLP on one shared software
//!   NPU and attend their own KV pages; tenant C is idle.
//! * **World 1:** identical, except that between every honest step tenant C
//!   issues a burst of seeded pseudo-random syscalls and accelerator ops
//!   (arena alloc, map, unmap, accelerator submit with random shapes /
//!   addresses / forged tenant tags, stream bind, SET_SID, arena handoff, cap
//!   derive / revoke, KV attend and KV pin).
//!
//! Everything A and B can observe is recorded per step: every job result
//! (including the completion sequence number), every KV attend result, the
//! full bytes of their arenas, their Soft-SMMU translations and their arena
//! metadata. The check passes only if the two worlds' observations are
//! **byte-identical** for every seed: nothing C does changes what A or B see.
//!
//! C's syscalls are modelled on the same `aether_core` functions the kernel
//! calls, with the kernel's own gates: `SYS_MAP` takes the pinned address
//! from the caller's arena capability via `sysnr::map_pin_addr`, `SYS_UNMAP`
//! is the capability-checked `IommuMap::unmap_for` (the fix proposed for
//! issue #161), and an arena handoff names the caller's own tile.
//!
//! Negative controls ([`Control`]) re-open known holes and must produce
//! divergences, so the check is shown to be able to fail.
//!
//! Not hardware isolation, no timing / cache / power side channels, no
//! performance claim.

use std::cell::Cell;

use aether_core::accel::{
    AccelError, AccelJobDesc, AccelOp, DType, DmaView, SliceMem, SoftNpu, TenantQueue,
};
use aether_core::arena::{Arena, ArenaAllocator, ArenaId, ArenaRequest};
use aether_core::caps::{CPtr, CapKind, CapRights, CapTable, Capability};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::kvfabric::{attend, pin_kv, AttendReq, KvKind, KvLedger, KvObject, KvWindow};
use aether_core::sysnr::map_pin_addr;
use aether_core::types::{BankId, ChipletId, PhysAddr, TenantId, TileId, PAGE_4K};

pub const RAM_BASE: u64 = 0x0100_0000;
pub const RAM_LEN: usize = 64 * 1024;

const X: u64 = 0;
const W1: u64 = 64;
const B1: u64 = 128;
const H: u64 = 192;
const R: u64 = 256;
const W2: u64 = 320;
const Y: u64 = 384;
const B2: u64 = 448;
const Z: u64 = 512;
const FLOOR: u64 = 576;
const OUT: u64 = 640;
/// KV page inside each tenant's arena.
const KV_OFF: u64 = 0x800;
const KV_LEN: u64 = 0x100;

const A: usize = 0;
const B: usize = 1;
const C: usize = 2;

/// Honest models (same shapes as the two-tenant demo).
const MODELS: [([i32; 8], [i32; 16], [i32; 4], [i32; 8], [i32; 2], i32); 2] = [
    (
        [1, 2, 3, 4, -1, 0, 2, 1],
        [1, 0, -1, 2, 0, 1, 1, -1, 2, -1, 0, 1, -1, 2, 1, 0],
        [1, -2, 0, 3],
        [1, -1, 2, 0, -1, 1, 0, 2],
        [-3, 1],
        -4,
    ),
    (
        [3, -2, 1, 0, 2, 2, -1, 1],
        [2, 1, 0, -1, -1, 3, 1, 0, 0, -2, 2, 1, 1, 0, -1, 2],
        [0, 1, -1, 2],
        [3, 0, -2, 1, 1, 1, 0, -1],
        [2, -5],
        -2,
    ),
];

/// Known holes the negative controls re-open. `None` is the real check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    None,
    /// `SYS_UNMAP` without the tenant check (issue #161 shape).
    UncheckedUnmap,
    /// `SYS_MAP` pins the caller-supplied address (pre-#203 shape).
    RawMapAddr,
    /// C holds a leaked copy of A's Memory+MAP capability.
    LeakedCap,
    /// Completions numbered from the device-wide counter (pre-#208 shape).
    GlobalSeq,
}

impl Control {
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::UncheckedUnmap => "unchecked-unmap",
            Self::RawMapAddr => "raw-map-addr",
            Self::LeakedCap => "leaked-cap",
            Self::GlobalSeq => "global-job-seq",
        }
    }
}

/// Deterministic splitmix64; the seed fully determines C's ops.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    pub const fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

#[derive(Clone, Copy, Debug)]
struct Tenant {
    id: TenantId,
    tile: u16,
    sid: StreamId,
    cap: Capability,
    arena: ArenaId,
    pa: u64,
    iova: u64,
}

struct TenantDma<'a, 'm> {
    iommu: &'a IommuMap,
    sid: StreamId,
    tenant: TenantId,
    mem: &'a mut SliceMem<'m>,
    fault: Cell<Option<MapError>>,
}

impl TenantDma<'_, '_> {
    fn pa(&self, iova: PhysAddr) -> Result<PhysAddr, AccelError> {
        self.iommu
            .resolve_submit(self.sid.raw(), iova, Some(self.tenant))
            .map_err(|e| {
                if self.fault.get().is_none() {
                    self.fault.set(Some(e));
                }
                AccelError::Overflow
            })
    }
}

impl DmaView for TenantDma<'_, '_> {
    fn load_i32(&self, addr: PhysAddr) -> Result<i32, AccelError> {
        let pa = self.pa(addr)?;
        self.mem.load_i32(pa)
    }
    fn store_i32(&mut self, addr: PhysAddr, val: i32) -> Result<(), AccelError> {
        let pa = self.pa(addr)?;
        self.mem.store_i32(pa, val)
    }
    fn load_u16(&self, addr: PhysAddr) -> Result<u16, AccelError> {
        let pa = self.pa(addr)?;
        self.mem.load_u16(pa)
    }
    fn store_u16(&mut self, addr: PhysAddr, val: u16) -> Result<(), AccelError> {
        let pa = self.pa(addr)?;
        self.mem.store_u16(pa, val)
    }
}

struct World {
    ram: Vec<u8>,
    arenas: ArenaAllocator,
    iommu: IommuMap,
    npu: SoftNpu,
    /// Each tenant's own submit queue on the shared NPU.
    queues: [TenantQueue; 3],
    ledger: KvLedger,
    t: [Tenant; 3],
    tables: [CapTable; 3],
    /// Arenas tenant C was handed by `SYS_ARENA_ALLOC` (the kernel's
    /// per-caller arena record that `SYS_MAP` looks a cap's object up in).
    c_arenas: Vec<Arena>,
    /// IOVAs C's own `SYS_MAP` calls returned (C aims DMA at them).
    c_iovas: Vec<u64>,
    /// Device-wide counter handed back instead of the tenant queue's
    /// (control only: the pre-fix completion numbering).
    global_seq: bool,
}

/// What A and B can observe after one honest step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub event: String,
    pub arena_bytes: [Vec<u8>; 2],
    pub smmu_view: [Vec<String>; 2],
    pub arena_meta: [String; 2],
}

/// C's own tally (not compared; shows C did real work).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CStats {
    pub ops: u64,
    pub accepted: u64,
    pub refused: u64,
    pub jobs_ok: u64,
}

impl World {
    fn new() -> Self {
        let mut arenas = ArenaAllocator::new(&[(BankId(0), PhysAddr(RAM_BASE), RAM_LEN as u64)])
            .expect("arena bank");
        let mut iommu = IommuMap::new();
        let mut ledger = KvLedger::new();
        let mut tables = [
            CapTable::new(TenantId(1)),
            CapTable::new(TenantId(2)),
            CapTable::new(TenantId(3)),
        ];
        let mut mk = |idx: usize, id: u32, tile: u16| {
            let tenant = TenantId(id);
            let arena = arenas
                .alloc(ArenaRequest::tensor(PAGE_4K, Some(BankId(0))).for_tenant(tenant))
                .expect("arena alloc");
            arenas
                .transfer_owner(arena.id, None, tile, id)
                .expect("assign owner tile");
            let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, arena.id.0, tenant)
                .with_generation(1);
            let sid = StreamId::accel(ChipletId(0), TileId(tile), 1);
            iommu.bind_stream(&cap, sid).expect("bind own SID");
            let region = iommu
                .map(&cap, MapRequest::pin_accel(arena.base, arena.size, sid))
                .expect("map own arena");
            tables[idx].mint(cap).expect("root memory cap");
            let kv_id = 100 + id;
            ledger
                .insert(KvObject {
                    id: kv_id,
                    seq: 7,
                    base: PhysAddr(arena.base.0 + KV_OFF),
                    bytes: KV_LEN,
                    kind: KvKind::Kv,
                    window: KvWindow { layer_lo: 0, layer_hi: 4, token_lo: 0, token_hi: 16 },
                    tenant,
                })
                .expect("kv object");
            tables[idx]
                .mint(Capability::new(
                    CapKind::Memory,
                    CapRights(CapRights::READ | CapRights::WRITE | CapRights::MAP),
                    kv_id,
                    tenant,
                ))
                .expect("kv grant");
            Tenant {
                id: tenant,
                tile,
                sid,
                cap,
                arena: arena.id,
                pa: arena.base.0,
                iova: region.iova.0,
            }
        };
        let a = mk(A, 1, 1);
        let b = mk(B, 2, 2);
        let c = mk(C, 3, 3);
        let c_arena = *arenas.get(c.arena).expect("c arena");
        Self {
            ram: vec![0u8; RAM_LEN],
            arenas,
            iommu,
            npu: SoftNpu::new(),
            queues: [TenantQueue::new(); 3],
            ledger,
            t: [a, b, c],
            tables,
            c_arenas: vec![c_arena],
            c_iovas: vec![c.iova],
            global_seq: false,
        }
    }

    fn load_models(&mut self) {
        for t in [A, B] {
            let (x, w1, b1, w2, b2, floor) = MODELS[t];
            let base = self.t[t].pa;
            let mut put = |at: u64, vals: &[i32]| {
                for (i, v) in vals.iter().enumerate() {
                    let o = (base + at - RAM_BASE) as usize + i * 4;
                    self.ram[o..o + 4].copy_from_slice(&v.to_le_bytes());
                }
            };
            put(X, &x);
            put(W1, &w1);
            put(B1, &b1);
            put(W2, &w2);
            put(B2, &[b2[0], b2[1], b2[0], b2[1]]);
            put(FLOOR, &[floor; 4]);
        }
    }

    fn layer_job(&self, t: usize, n: u32) -> AccelJobDesc {
        let ten = &self.t[t];
        let io = |o: u64| PhysAddr(ten.iova + o);
        match n {
            1 => {
                let mut j = AccelJobDesc::matmul_i32(2, 4, 4, io(X), io(W1), io(H), ten.id.0);
                j.op = AccelOp::Wave;
                j.bias = io(B1);
                j
            }
            2 => AccelJobDesc::relu_i32(2, 4, io(H), io(R), ten.id.0),
            3 => AccelJobDesc::matmul_i32(2, 2, 4, io(R), io(W2), io(Y), ten.id.0),
            4 => AccelJobDesc::add_i32(2, 2, io(Y), io(B2), io(Z), ten.id.0),
            _ => AccelJobDesc::max_i32(2, 2, io(Z), io(FLOOR), io(OUT), ten.id.0),
        }
    }

    /// SET_SID with `cap`, run `job` with DMA resolved for `tenant` on `sid`,
    /// drop the latch. Same path for honest tenants and for C.
    fn submit(
        &mut self,
        q: usize,
        tenant: TenantId,
        cap: &Capability,
        sid: StreamId,
        job: &AccelJobDesc,
    ) -> Result<u32, String> {
        self.iommu.set_sid(cap, sid).map_err(|e| format!("MapError::{e:?}"))?;
        let mut mem = SliceMem {
            base: PhysAddr(RAM_BASE),
            bytes: &mut self.ram,
        };
        let mut dma = TenantDma {
            iommu: &self.iommu,
            sid,
            tenant,
            mem: &mut mem,
            fault: Cell::new(None),
        };
        let r = if self.global_seq {
            self.npu.execute(job, &mut dma)
        } else {
            self.npu.execute_queued(&mut self.queues[q], job, &mut dma)
        };
        let fault = dma.fault.get();
        self.iommu.clear_submit_sid();
        match (r, fault) {
            (Ok(c), _) => Ok(c.job_seq),
            (Err(_), Some(f)) => Err(format!("MapError::{f:?}")),
            (Err(e), None) => Err(format!("AccelError::{e:?}")),
        }
    }

    fn observe(&self, event: String) -> Observation {
        let bytes = |t: usize| {
            let o = (self.t[t].pa - RAM_BASE) as usize;
            self.ram[o..o + PAGE_4K as usize].to_vec()
        };
        let view = |t: usize| {
            let ten = &self.t[t];
            (0..PAGE_4K / 0x100)
                .map(|k| {
                    let off = k * 0x100;
                    format!(
                        "{:?}|{:?}",
                        self.iommu.resolve_result(ten.sid.raw(), PhysAddr(ten.iova + off), Some(ten.id)),
                        self.iommu.translate_result(ten.sid.raw(), PhysAddr(ten.pa + off), Some(ten.id)),
                    )
                })
                .collect::<Vec<_>>()
        };
        let meta = |t: usize| format!("{:?}", self.arenas.get(self.t[t].arena));
        Observation {
            event,
            arena_bytes: [bytes(A), bytes(B)],
            smmu_view: [view(A), view(B)],
            arena_meta: [meta(A), meta(B)],
        }
    }

    /// An address C might name: its own, A's or B's (IOVA or PA), or junk.
    fn pick_addr(&self, rng: &mut Rng) -> u64 {
        let t = &self.t[rng.below(3) as usize];
        let off = if rng.coin() { rng.below(PAGE_4K) & !3 } else { rng.below(2 * PAGE_4K) };
        match rng.below(8) {
            0 | 1 => t.iova.wrapping_add(off),
            2 => t.pa.wrapping_add(off),
            3 | 4 => {
                let i = rng.below(self.c_iovas.len() as u64) as usize;
                self.c_iovas[i].wrapping_add(off)
            }
            5 => RAM_BASE + rng.below(RAM_LEN as u64),
            6 => 0,
            _ => rng.next(),
        }
    }

    fn pick_sid(&self, rng: &mut Rng) -> StreamId {
        match rng.below(5) {
            0..=2 => self.t[rng.below(3) as usize].sid,
            3 => StreamId::accel(ChipletId(0), TileId(rng.below(8) as u16), rng.below(4) as u8),
            _ => StreamId(rng.next() as u32),
        }
    }

    fn pick_cptr(&self, rng: &mut Rng) -> CPtr {
        if rng.below(4) == 0 {
            CPtr(rng.next() as u16)
        } else {
            CPtr(rng.below(8) as u16)
        }
    }

    /// C's capability for an op: from C's own table, or (control only) A's.
    fn c_cap(&self, rng: &mut Rng, ctl: Control) -> Option<Capability> {
        if ctl == Control::LeakedCap && rng.below(3) == 0 {
            return Some(self.t[A].cap);
        }
        self.tables[C].lookup(self.pick_cptr(rng)).ok().copied()
    }

    fn random_job(&self, rng: &mut Rng) -> AccelJobDesc {
        let dim = |rng: &mut Rng| match rng.below(10) {
            0 => 0,
            1 => 65,
            _ => 1 + rng.below(4) as u32,
        };
        let tenant = [1u32, 2, 3, rng.next() as u32][rng.below(4) as usize];
        let mut j = AccelJobDesc::matmul_i32(
            dim(rng),
            dim(rng),
            dim(rng),
            PhysAddr(self.pick_addr(rng)),
            PhysAddr(self.pick_addr(rng)),
            PhysAddr(self.pick_addr(rng)),
            tenant,
        );
        j.op = [
            AccelOp::Nop,
            AccelOp::MatMul,
            AccelOp::Wave,
            AccelOp::Add,
            AccelOp::Relu,
            AccelOp::Mul,
            AccelOp::Max,
        ][rng.below(7) as usize];
        j.dtype = [DType::I32, DType::F16, DType::F32][rng.below(3) as usize];
        if rng.coin() {
            j.bias = PhysAddr(self.pick_addr(rng));
        }
        j
    }

    /// A job that reads C's own arena and writes into an IOVA one of C's
    /// own `SYS_MAP` calls returned: the obvious way to use a mapping.
    fn targeted_job(&self, rng: &mut Rng) -> AccelJobDesc {
        let c = &self.t[C];
        let i = rng.below(self.c_iovas.len() as u64) as usize;
        let dst = self.c_iovas[i].wrapping_add(rng.below(PAGE_4K / 4) * 4);
        let m = 1 + rng.below(2) as u32;
        let n = 1 + rng.below(4) as u32;
        let src = PhysAddr(c.iova + X);
        match rng.below(3) {
            0 => AccelJobDesc::relu_i32(m, n, src, PhysAddr(dst), c.id.0),
            1 => AccelJobDesc::add_i32(m, n, src, src, PhysAddr(dst), c.id.0),
            _ => AccelJobDesc::max_i32(m, n, src, src, PhysAddr(dst), c.id.0),
        }
    }

    /// One pseudo-random operation by tenant C. Returns whether it was accepted.
    fn c_op(&mut self, rng: &mut Rng, ctl: Control, stats: &mut CStats) {
        let c = self.t[C];
        stats.ops += 1;
        let ok: bool = match rng.below(10) {
            // SYS_ARENA_ALLOC
            0 => {
                let size = [0, 1, PAGE_4K, 2 * PAGE_4K, 70_000, rng.below(20_000)][rng.below(6) as usize];
                match self
                    .arenas
                    .alloc(ArenaRequest::tensor(size, Some(BankId(0))).for_tenant(c.id))
                {
                    Ok(a) => {
                        self.c_arenas.push(a);
                        let cap = Capability::new(CapKind::Memory, CapRights::MEM_FULL, a.id.0, c.id);
                        self.tables[C].mint(cap).is_ok()
                    }
                    Err(_) => false,
                }
            }
            // SYS_MAP
            1 => {
                let vaddr = match rng.below(4) {
                    0 => 0,
                    // A page-aligned physical guess: some tenant's arena base.
                    1 | 2 => self.t[rng.below(3) as usize].pa,
                    _ => self.pick_addr(rng),
                };
                let sid = self.pick_sid(rng);
                let Some(cap) = self.c_cap(rng, ctl) else {
                    stats.refused += 1;
                    return;
                };
                if !(cap.kind == CapKind::Memory && cap.rights.contains(CapRights::MAP)) {
                    false
                } else {
                    let arena = self
                        .c_arenas
                        .iter()
                        .find(|a| a.id.0 == cap.object)
                        .copied()
                        .or_else(|| {
                            (ctl == Control::LeakedCap && cap.tenant == self.t[A].id)
                                .then(|| *self.arenas.get(self.t[A].arena).unwrap())
                        });
                    match arena {
                        None => false,
                        Some(arena) => {
                            let pa = if ctl == Control::RawMapAddr && vaddr != 0 {
                                Some(vaddr)
                            } else {
                                map_pin_addr(arena.base.0, vaddr)
                            };
                            match pa {
                                None => false,
                                Some(pa) => match self
                                    .iommu
                                    .map(&cap, MapRequest::pin_accel(PhysAddr(pa), arena.size, sid))
                                {
                                    Ok(region) => {
                                        self.c_iovas.push(region.iova.0);
                                        true
                                    }
                                    Err(_) => false,
                                },
                            }
                        }
                    }
                }
            }
            // SYS_UNMAP
            2 => {
                let iova = PhysAddr(self.pick_addr(rng));
                if ctl == Control::UncheckedUnmap {
                    self.iommu.unmap(iova).is_ok()
                } else {
                    match self.c_cap(rng, ctl) {
                        Some(cap) => self.iommu.unmap_for(&cap, iova).is_ok(),
                        None => false,
                    }
                }
            }
            // SYS_ACCEL_SUBMIT (random shape / addresses / forged tenant tag)
            3 | 4 => {
                let job = if rng.coin() {
                    self.random_job(rng)
                } else {
                    self.targeted_job(rng)
                };
                let sid = if rng.below(3) == 0 { self.pick_sid(rng) } else { c.sid };
                match self.c_cap(rng, ctl) {
                    Some(cap) => {
                        let r = self.submit(C, c.id, &cap, sid, &job).is_ok();
                        if r {
                            stats.jobs_ok += 1;
                        }
                        r
                    }
                    None => false,
                }
            }
            // stream bind
            5 => {
                let sid = self.pick_sid(rng);
                match self.c_cap(rng, ctl) {
                    Some(cap) => self.iommu.bind_stream(&cap, sid).is_ok(),
                    None => false,
                }
            }
            // SET_SID then drop the latch
            6 => {
                let sid = self.pick_sid(rng);
                let r = match self.c_cap(rng, ctl) {
                    Some(cap) => self.iommu.set_sid(&cap, sid).is_ok(),
                    None => false,
                };
                self.iommu.clear_submit_sid();
                r
            }
            // arena handoff, claimed by C's own tile
            7 => {
                let id = ArenaId(rng.below(12) as u32);
                let to = rng.below(8) as u16;
                let tenant = [1u32, 2, 3][rng.below(3) as usize];
                let from = if rng.below(4) == 0 { None } else { Some(c.tile) };
                let r = self.arenas.transfer_owner(id, from, to, tenant).is_ok();
                if r {
                    // A handed-off arena is no longer C's to map.
                    self.c_arenas.retain(|a| a.id != id);
                }
                r
            }
            // cap derive / revoke in C's own table
            8 => {
                let cptr = self.pick_cptr(rng);
                if rng.coin() {
                    self.tables[C].derive(cptr, CapRights(rng.next() as u16)).is_ok()
                } else {
                    self.tables[C].revoke(cptr).is_ok()
                }
            }
            // KV attend / KV pin
            _ => {
                let cptr = self.pick_cptr(rng);
                if rng.coin() {
                    let req = AttendReq {
                        seq: [7, rng.next() as u32][rng.below(2) as usize],
                        layer: rng.below(6) as u16,
                        token: rng.below(20) as u32,
                        write: rng.coin(),
                    };
                    attend(&self.tables[C], cptr, &self.ledger, req).is_ok()
                } else {
                    let sid = self.pick_sid(rng);
                    match self.c_cap(rng, ctl) {
                        Some(cap) => pin_kv(&mut self.iommu, &cap, &self.ledger, sid).is_ok(),
                        None => false,
                    }
                }
            }
        };
        if ok {
            stats.accepted += 1;
        } else {
            stats.refused += 1;
        }
    }
}

/// One world's run. `attacker = None` is world 0 (C idle).
pub fn run_world(attacker: Option<(u64, u32, Control)>) -> (Vec<Observation>, CStats) {
    run_world_with(attacker, attacker.map(|(_, _, c)| c) == Some(Control::GlobalSeq))
}

fn run_world_with(attacker: Option<(u64, u32, Control)>, global_seq: bool) -> (Vec<Observation>, CStats) {
    let mut w = World::new();
    w.global_seq = global_seq;
    w.load_models();
    let mut obs = vec![w.observe("setup".into())];
    let mut stats = CStats::default();
    let mut rng = attacker.map(|(seed, _, _)| Rng::new(seed));
    let mut gap = |w: &mut World, stats: &mut CStats| {
        if let (Some((_, per_gap, ctl)), Some(rng)) = (attacker, rng.as_mut()) {
            for _ in 0..per_gap {
                w.c_op(rng, ctl, stats);
            }
        }
    };
    gap(&mut w, &mut stats);
    for layer in 1..=5u32 {
        for t in [A, B] {
            let ten = w.t[t];
            let job = w.layer_job(t, layer);
            let r = w.submit(t, ten.id, &ten.cap, ten.sid, &job);
            let o = w.observe(format!("tenant={t} layer={layer} op={:?} result={r:?}", job.op));
            obs.push(o);
            gap(&mut w, &mut stats);

            // Slot 1 of each table holds the tenant's own KV grant.
            let kv = CPtr(1);
            let req = AttendReq {
                seq: 7,
                layer: (layer % 4) as u16,
                token: layer,
                write: layer % 2 == 0,
            };
            let r = attend(&w.tables[t], kv, &w.ledger, req);
            let o = w.observe(format!("tenant={t} layer={layer} attend result={r:?}"));
            obs.push(o);
            gap(&mut w, &mut stats);
        }
    }
    let o = w.observe("final".into());
    obs.push(o);
    (obs, stats)
}

/// Index of the first differing observation, if any.
pub fn first_divergence(a: &[Observation], b: &[Observation]) -> Option<usize> {
    if a.len() != b.len() {
        return Some(a.len().min(b.len()));
    }
    a.iter().zip(b).position(|(x, y)| x != y)
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub seeds: u64,
    pub ops: u64,
    pub divergences: u64,
    pub c: CStats,
    /// First divergence for the report: (seed, step, world-0 event, world-1 event).
    pub first: Option<(u64, usize, String, String)>,
}

impl Summary {
    pub fn line(&self) -> String {
        format!(
            "[noninterference] worlds=2 seeds={} ops={} divergences={}",
            self.seeds, self.ops, self.divergences
        )
    }
}

/// Run `seeds` seeds of world 1 against the single world-0 baseline.
pub fn check(seeds: u64, per_gap: u32, ctl: Control) -> Summary {
    let (base, _) = run_world_with(None, ctl == Control::GlobalSeq);
    let mut s = Summary::default();
    for seed in 0..seeds {
        let (obs, st) = run_world(Some((seed, per_gap, ctl)));
        s.seeds += 1;
        s.ops += st.ops;
        s.c.ops += st.ops;
        s.c.accepted += st.accepted;
        s.c.refused += st.refused;
        s.c.jobs_ok += st.jobs_ok;
        if let Some(i) = first_divergence(&base, &obs) {
            s.divergences += 1;
            if s.first.is_none() {
                let ev = |v: &[Observation]| v.get(i).map(|o| o.event.clone()).unwrap_or_default();
                s.first = Some((seed, i, ev(&base), ev(&obs)));
            }
        }
    }
    s
}

pub const SCOPE_LINE: &str = "[noninterference] scope: host software model (core caps + arenas + Soft SMMU + SoftNpu + KV grants, syscall gates as the kernel applies them); not hardware, no timing/cache/power side channels";
