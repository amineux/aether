//! Hostile-tenant bounded seeded fuzz (host-only, Cargo-only).
//!
//! Tenant B (tenant 2) drives a fixed-seed xorshift stream of hostile ops
//! against **existing public** Aether APIs (Soft SMMU, SoftGreenPool,
//! SoftNoI, Timeline, `admit_wave`, OperatorInject) while tenant A
//! (tenant 1) holds a canary window and StreamID. After **every** op the
//! harness checks, with real comparisons (not a hardcoded zero):
//!
//! 1. B's outcome is `Ok` inside B's own SID / slice, or an error variant
//!    from the per-API expected set (anything else counts as `unnamed`;
//!    a panic is caught and counts as `unnamed`);
//! 2. A's canary translate / resolve, A's SID count, A's NoI occupant,
//!    A's green-context binding, and A's opinject canary bytes are
//!    unchanged;
//! 3. B never obtains A's StreamId (STE key) or A's PA / IOVA window.
//!
//! Scope, stated plainly: bounded, seeded, deterministic (no wall clock).
//! Evidence, **not** a proof, **not** a hardware claim.
//!
//! **Unmap paths (issue #161, fixed):** `IommuMap::unmap_stream(&cap, sid,
//! iova)`, `IommuMap::unmap(&cap, iova)` and `unmap_for(&cap, iova)` are all
//! tenant-checked. B drives all three, including `unmap_stream` with A's raw
//! StreamId + A's IOVA (the op-101 repro of seed `0x5AE7`); each must return
//! `MapError::CrossTenant` for B against A's pin and leave it mapped (ops 7
//! and 8 below). The unchecked helpers are no longer public.
//!
//! B only calls tenant-passing entry points (`resolve_result(.., Some(tenant))`,
//! `unmap_for` / `unmap` / `unmap_stream` (all cap-checked), `translate_result(.., Some(tenant))`, `set_sid(cap, ..)`);
//! untenanted `walk` is only called on B's own SIDs.
//! SoftGreenPool has no tenant identity, so B is confined to its own
//! queue index. Soft SMMU `map` authorizes by Memory+MAP cap and does not
//! compare `cap.object` with the PA range, so B's PA requests are drawn
//! from B's own slice (`B_PA_LO..`).

use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::color::{admit_wave, BankColor, ColorError};
use aether_core::fence::{FenceId, Timeline};
use aether_core::greenctx::{GreenCtxError, GreenCtxId, SmWqBudget, SoftGreenPool};
use aether_core::hodge::FlowClass;
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::noi::{NoiError, NoiOccupant, SoftNoI};
use aether_core::opinject::{
    FlatOpMem, InjectError, InjectKind, OpCall, OperatorInject, MAX_OP_SLOTS,
};
use aether_core::partition::{
    BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
};
use aether_core::phase::Phase;
use aether_core::softsfi::{SidRange, SidSandbox};
use aether_core::types::{BankId, ChipletId, PhysAddr, TenantId, TileId};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Fixed seed printed on the needle line.
pub const FUZZ_SEED: u64 = 0x5AE7;
/// Hostile ops on the needle line.
pub const FUZZ_OPS: u32 = 4096;

const TENANT_A: TenantId = TenantId(1);
const TENANT_B: TenantId = TenantId(2);
const A_PA: u64 = 0x0200_0000;
const A_LEN: u64 = 0x1000;
/// Start of B's own PA slice. A's canary PA is below this.
const B_PA_LO: u64 = 0x0300_0000;
const HOME_BANK: BankId = BankId(0);
const OTHER_BANK: BankId = BankId(1);
const OP_BASE: u64 = 0x00;
const OP_B_SPAN: u64 = 0x20;
const OP_MEM_LEN: usize = 0x60;
const OP_CANARY_LO: usize = 0x40;
const OP_CANARY: u8 = 0xA5;

/// Deterministic integer PRNG (xorshift64). No wall clock.
struct Xs(u64);

impl Xs {
    fn new(seed: u64) -> Self {
        let mut x = Self(seed | 1);
        for _ in 0..16 {
            x.next();
        }
        x
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u64) as usize]
    }
}

/// Result of one bounded fuzz run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TenantFuzzReport {
    pub seed: u64,
    pub ops: u32,
    /// Invariant violations (A canary moved, B got A's SID/PA, B Ok outside its slice).
    pub escapes: u32,
    /// B outcomes outside the per-API expected error set, plus caught panics.
    pub unnamed: u32,
    /// Distinct `<Enum>::<Variant>` errors hit across all APIs.
    pub variants: u32,
    /// Ok outcomes (sanity: B is not only refused).
    pub b_ok: u32,
}

impl TenantFuzzReport {
    /// Non-vacuous: zero escapes, zero unnamed, and a floor of distinct variants.
    pub fn all_ok(&self) -> bool {
        self.escapes == 0 && self.unnamed == 0 && self.variants >= 8 && self.b_ok > 0
    }
}

struct World {
    io: IommuMap,
    cap_b: Capability,
    cap_b_ro: Capability,
    cap_b_ep: Capability,
    sid_a: StreamId,
    iova_a: u64,
    pool: SoftGreenPool,
    ctx_a: GreenCtxId,
    ctx_a_budget: SmWqBudget,
    b_ctx: [Option<GreenCtxId>; 4],
    noi: SoftNoI,
    ta: Timeline,
    tb: Timeline,
    pa: PartitionProfile,
    pb: PartitionProfile,
    inj: OperatorInject,
    op_mem: [u8; OP_MEM_LEN],
    sandbox: SidSandbox,
    seen: Vec<&'static str>,
    escapes: u32,
    unnamed: u32,
    b_ok: u32,
}

fn mem_cap(obj: u32, tenant: TenantId, rights: CapRights, kind: CapKind) -> Capability {
    Capability::new(kind, rights, obj, tenant).with_generation(1)
}

fn profile(id: u32, chiplet: u8) -> PartitionProfile {
    PartitionProfile::new(
        PartitionId(id),
        SpatialSlice::single_chiplet(ChipletId(chiplet), 0b1, 0b1),
        QosBudget {
            bw_mbps: 1000,
            credits: 4,
        },
        BlastRadius {
            max_nodes: 4,
            max_hops: 1,
        },
    )
}

fn map_name(e: MapError) -> &'static str {
    match e {
        MapError::NoMemoryCap => "MapError::NoMemoryCap",
        MapError::BadRange => "MapError::BadRange",
        MapError::Overlap => "MapError::Overlap",
        MapError::TableFull => "MapError::TableFull",
        MapError::NotMapped => "MapError::NotMapped",
        MapError::CrossTenant => "MapError::CrossTenant",
        MapError::WrongStream => "MapError::WrongStream",
        MapError::StreamAbort => "MapError::StreamAbort",
        MapError::Stage2Fault => "MapError::Stage2Fault",
        MapError::SubmitSid => "MapError::SubmitSid",
        MapError::SidBudget => "MapError::SidBudget",
    }
}
fn green_name(e: GreenCtxError) -> &'static str {
    match e {
        GreenCtxError::Exhausted => "GreenCtxError::Exhausted",
        GreenCtxError::Overcommit => "GreenCtxError::Overcommit",
        GreenCtxError::Unbound => "GreenCtxError::Unbound",
        GreenCtxError::Busy => "GreenCtxError::Busy",
        GreenCtxError::BadArg => "GreenCtxError::BadArg",
    }
}
fn noi_name(e: NoiError) -> &'static str {
    match e {
        NoiError::BadArg => "NoiError::BadArg",
        NoiError::OverBudget => "NoiError::OverBudget",
        NoiError::RingExhausted => "NoiError::RingExhausted",
        NoiError::Exhausted => "NoiError::Exhausted",
        NoiError::Unbound => "NoiError::Unbound",
    }
}
fn part_name(e: PartitionError) -> &'static str {
    match e {
        PartitionError::OutsideSlice => "PartitionError::OutsideSlice",
        PartitionError::QosExceeded => "PartitionError::QosExceeded",
        PartitionError::BlastRadius => "PartitionError::BlastRadius",
        PartitionError::Unbound => "PartitionError::Unbound",
        PartitionError::CreditExhausted => "PartitionError::CreditExhausted",
        PartitionError::FenceNotReady => "PartitionError::FenceNotReady",
    }
}
fn color_name(e: ColorError) -> &'static str {
    match e {
        ColorError::ForeignBank => "ColorError::ForeignBank",
        ColorError::ForeignTenant => "ColorError::ForeignTenant",
        ColorError::Uncolored => "ColorError::Uncolored",
    }
}
fn inj_name(e: InjectError) -> &'static str {
    match e {
        InjectError::NotRunning => "InjectError::NotRunning",
        InjectError::Busy => "InjectError::Busy",
        InjectError::UnknownSlot => "InjectError::UnknownSlot",
        InjectError::StaleVersion => "InjectError::StaleVersion",
        InjectError::Oob => "InjectError::Oob",
        InjectError::BadArg => "InjectError::BadArg",
    }
}

const SMMU_MAP_OK: &[&str] = &[
    "MapError::NoMemoryCap",
    "MapError::BadRange",
    "MapError::Overlap",
    "MapError::TableFull",
    "MapError::CrossTenant",
    "MapError::StreamAbort",
    "MapError::SidBudget",
];
const SMMU_BIND_OK: &[&str] = &[
    "MapError::NoMemoryCap",
    "MapError::TableFull",
    "MapError::CrossTenant",
    "MapError::StreamAbort",
    "MapError::SidBudget",
];
const SMMU_LOOKUP_OK: &[&str] = &[
    "MapError::NotMapped",
    "MapError::WrongStream",
    "MapError::CrossTenant",
    "MapError::StreamAbort",
    "MapError::Stage2Fault",
];
const SMMU_SUBMIT_OK: &[&str] = &[
    "MapError::NotMapped",
    "MapError::WrongStream",
    "MapError::CrossTenant",
    "MapError::StreamAbort",
    "MapError::SubmitSid",
    "MapError::Stage2Fault",
];
const SMMU_SETSID_OK: &[&str] = &[
    "MapError::NoMemoryCap",
    "MapError::CrossTenant",
    "MapError::StreamAbort",
];
const SMMU_UNMAP_OK: &[&str] = &[
    "MapError::NoMemoryCap",
    "MapError::NotMapped",
    "MapError::CrossTenant",
    "MapError::WrongStream",
];
const GREEN_OK: &[&str] = &[
    "GreenCtxError::Exhausted",
    "GreenCtxError::Overcommit",
    "GreenCtxError::Unbound",
    "GreenCtxError::Busy",
    "GreenCtxError::BadArg",
];
const NOI_OK: &[&str] = &[
    "NoiError::BadArg",
    "NoiError::OverBudget",
    "NoiError::RingExhausted",
    "NoiError::Unbound",
];
const TL_OK: &[&str] = &[
    "PartitionError::Unbound",
    "PartitionError::CreditExhausted",
    "PartitionError::FenceNotReady",
];
const COLOR_OK: &[&str] = &[
    "ColorError::ForeignBank",
    "ColorError::ForeignTenant",
    "ColorError::Uncolored",
];
const INJ_OK: &[&str] = &[
    "InjectError::NotRunning",
    "InjectError::Busy",
    "InjectError::UnknownSlot",
    "InjectError::StaleVersion",
    "InjectError::Oob",
    "InjectError::BadArg",
];

impl World {
    fn new() -> Self {
        let mut io = IommuMap::new();
        let cap_a = mem_cap(1, TENANT_A, CapRights::MEM_FULL, CapKind::Memory);
        let cap_b = mem_cap(2, TENANT_B, CapRights::MEM_FULL, CapKind::Memory);
        // B holding a read-only Memory cap and an Endpoint cap (no MAP).
        let cap_b_ro = mem_cap(3, TENANT_B, CapRights(CapRights::READ), CapKind::Memory);
        let cap_b_ep = mem_cap(4, TENANT_B, CapRights::EP_FULL, CapKind::Endpoint);
        let sid_a = StreamId::accel(ChipletId(0), TileId(3), 1);
        let ra = io
            .map(
                &cap_a,
                MapRequest::pin_accel(PhysAddr(A_PA), A_LEN, sid_a),
            )
            .expect("tenant A canary pin");

        let mut pool = SoftGreenPool::new();
        let ctx_a = pool.create(SmWqBudget::split_30()).expect("A ctx");
        pool.bind(ctx_a, 0).expect("A queue 0");
        let ctx_a_budget = pool.ctx(ctx_a).expect("A ctx").budget;

        let mut noi = SoftNoI::new();
        noi.enable(true);
        noi.admit_class(TENANT_A, 300, FlowClass::Gradient)
            .expect("A noi");

        let pa = profile(1, 0);
        let pb = profile(2, 1);
        let mut ta = Timeline::new(PartitionId(1));
        ta.submit(&pa, None).expect("A fence");
        let tb = Timeline::new(PartitionId(2));

        let mut op_mem = [0u8; OP_MEM_LEN];
        for b in op_mem[OP_CANARY_LO..].iter_mut() {
            *b = OP_CANARY;
        }
        let mut sandbox = SidSandbox::new(StreamId::accel(ChipletId(1), TileId(4), 1));
        let _ = sandbox.push(SidRange::new(OP_BASE, OP_B_SPAN, true));

        Self {
            io,
            cap_b,
            cap_b_ro,
            cap_b_ep,
            sid_a,
            iova_a: ra.iova.0,
            pool,
            ctx_a,
            ctx_a_budget,
            b_ctx: [None; 4],
            noi,
            ta,
            tb,
            pa,
            pb,
            inj: OperatorInject::with_resident_memcpy_saxpy(),
            op_mem,
            sandbox,
            seen: Vec::new(),
            escapes: 0,
            unnamed: 0,
            b_ok: 0,
        }
    }

    fn note(&mut self, name: &'static str, allowed: &[&str]) {
        if !self.seen.contains(&name) {
            self.seen.push(name);
        }
        if !allowed.contains(&name) {
            self.unnamed += 1;
        }
    }

    fn a_key(&self) -> u32 {
        self.sid_a.stream_key()
    }

    /// True if `iova` falls in A's per-(STE, CD) IOVA window.
    fn in_a_iova_window(&self, iova: u64) -> bool {
        (iova >> 22) == (self.iova_a >> 22)
    }

    /// True if `pa` is inside A's canary PA window.
    fn in_a_pa(pa: u64) -> bool {
        pa >= A_PA && pa < A_PA + A_LEN
    }

    /// Invariant 2: everything A owns is unchanged. Real comparisons.
    fn a_intact(&self) -> bool {
        let sid = self.sid_a.raw();
        self.io.translate_result(sid, PhysAddr(A_PA), Some(TENANT_A)) == Ok(PhysAddr(self.iova_a))
            && self
                .io
                .resolve_result(sid, PhysAddr(self.iova_a), Some(TENANT_A))
                == Ok(PhysAddr(A_PA))
            && self.io.tenant_sid_count(TENANT_A) == 1
            && self
                .io
                .iter()
                .any(|r| r.tenant == TENANT_A && r.guest_pa.0 == A_PA && r.iova.0 == self.iova_a)
            && self.noi.occupant(TENANT_A)
                == Some(NoiOccupant {
                    tenant: TENANT_A,
                    demand: 300,
                    class: FlowClass::Gradient,
                })
            && self.pool.ctx_for_queue(0) == Some(self.ctx_a)
            && self.pool.ctx(self.ctx_a).map(|c| c.budget) == Some(self.ctx_a_budget)
            && self.ta.in_flight() == 1
            && self.ta.retired() == 0
            && self.op_mem[OP_CANARY_LO..].iter().all(|&b| b == OP_CANARY)
    }

    fn b_sid_pool(&self) -> [StreamId; 12] {
        let a = self.sid_a;
        [
            a,
            a.with_ssid(2),
            a.with_ssid(200),
            StreamId::accel(ChipletId(1), TileId(4), 1),
            StreamId::accel(ChipletId(1), TileId(4), 2),
            StreamId::accel(ChipletId(1), TileId(5), 1),
            StreamId::accel(ChipletId(2), TileId(6), 1),
            StreamId::accel(ChipletId(2), TileId(7), 1),
            StreamId::accel(ChipletId(3), TileId(8), 1),
            StreamId::accel(ChipletId(3), TileId(9), 9),
            StreamId::accel(ChipletId(0), TileId(3), 0),
            StreamId::from_raw(0),
        ]
    }

    /// B's own (non-A-family) SIDs, for untenanted `walk`.
    fn b_own(&self) -> [StreamId; 4] {
        [
            StreamId::accel(ChipletId(1), TileId(4), 1),
            StreamId::accel(ChipletId(1), TileId(5), 1),
            StreamId::accel(ChipletId(2), TileId(6), 1),
            StreamId::accel(ChipletId(3), TileId(8), 1),
        ]
    }

    fn pick_cap(&self, rng: &mut Xs) -> Capability {
        // Mostly the real B cap; sometimes a cap without MAP.
        match rng.below(8) {
            0 => self.cap_b_ro,
            1 => self.cap_b_ep,
            _ => self.cap_b,
        }
    }

    fn pick_iova(&self, rng: &mut Xs) -> u64 {
        let regions: Vec<u64> = self.io.iter().map(|r| r.iova.0).collect();
        match rng.below(6) {
            0 => self.iova_a,
            1 => self.iova_a + rng.below(A_LEN),
            2 if !regions.is_empty() => {
                let i = rng.below(regions.len() as u64) as usize;
                regions[i] + rng.below(0x800)
            }
            3 => rng.next(),
            4 => 0,
            _ => A_PA,
        }
    }

    fn step(&mut self, rng: &mut Xs) {
        match rng.below(16) {
            0..=2 => self.op_map(rng),
            3 => self.op_translate(rng),
            4 => self.op_walk_own(rng),
            5 => self.op_resolve(rng),
            6 => self.op_submit(rng),
            7 => self.op_unmap_for(rng),
            8 => self.op_unmap_for_a(rng),
            9 => self.op_bind(rng),
            10 => self.op_green(rng),
            11 => self.op_noi(rng),
            12 => self.op_timeline(rng),
            13 => self.op_color(rng),
            _ => self.op_opinject(rng),
        }
    }

    fn op_map(&mut self, rng: &mut Xs) {
        let sid = rng.pick(&self.b_sid_pool());
        let cap = self.pick_cap(rng);
        let pa = match rng.below(8) {
            0 => B_PA_LO + 0x1000 * rng.below(8),
            1 => u64::MAX - rng.below(0x2000),
            _ => B_PA_LO + 0x1000 * rng.below(64),
        };
        let len = rng.pick(&[0u64, 0x1000, 0x1000, 0x2000, 0x3000, 0x40_0000, 1 << 40, u64::MAX]);
        match self.io.map(&cap, MapRequest::pin_accel(PhysAddr(pa), len, sid)) {
            Ok(r) => {
                self.b_ok += 1;
                if r.tenant != TENANT_B
                    || r.stream_id != sid.raw()
                    || sid.stream_key() == self.a_key()
                    || r.guest_pa.0 < B_PA_LO
                    || self.in_a_iova_window(r.iova.0)
                {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(map_name(e), SMMU_MAP_OK),
        }
    }

    fn op_translate(&mut self, rng: &mut Xs) {
        let sid = rng.pick(&self.b_sid_pool());
        let pa = match rng.below(4) {
            0 => A_PA + rng.below(A_LEN),
            1 => B_PA_LO + 0x1000 * rng.below(64),
            2 => A_PA,
            _ => rng.next(),
        };
        match self
            .io
            .translate_result(sid.raw(), PhysAddr(pa), Some(TENANT_B))
        {
            Ok(iova) => {
                self.b_ok += 1;
                if sid.stream_key() == self.a_key() || self.in_a_iova_window(iova.0) {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(map_name(e), SMMU_LOOKUP_OK),
        }
    }

    fn op_walk_own(&mut self, rng: &mut Xs) {
        let sid = rng.pick(&self.b_own());
        let iova = self.pick_iova(rng);
        match self.io.walk(sid, PhysAddr(iova)) {
            Ok(w) => {
                self.b_ok += 1;
                if w.pa.0 < B_PA_LO || Self::in_a_pa(w.pa.0) {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(map_name(e), SMMU_LOOKUP_OK),
        }
    }

    fn op_resolve(&mut self, rng: &mut Xs) {
        let sid = rng.pick(&self.b_sid_pool());
        let iova = self.pick_iova(rng);
        match self
            .io
            .resolve_result(sid.raw(), PhysAddr(iova), Some(TENANT_B))
        {
            Ok(pa) => {
                self.b_ok += 1;
                if sid.stream_key() == self.a_key() || pa.0 < B_PA_LO || Self::in_a_pa(pa.0) {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(map_name(e), SMMU_LOOKUP_OK),
        }
    }

    fn op_submit(&mut self, rng: &mut Xs) {
        let sid = rng.pick(&self.b_sid_pool());
        // Sometimes arm SET_SID first (A's SID → CrossTenant).
        if rng.below(3) != 0 {
            let cap = self.pick_cap(rng);
            match self.io.set_sid(&cap, sid) {
                Ok(got) => {
                    self.b_ok += 1;
                    if got.stream_key() == self.a_key() {
                        self.escapes += 1;
                    }
                }
                Err(e) => self.note(map_name(e), SMMU_SETSID_OK),
            }
        }
        if let Some(armed) = self.io.submit_sid() {
            if armed.stream_key() == self.a_key() {
                self.escapes += 1;
            }
        }
        let iova = self.pick_iova(rng);
        match self
            .io
            .resolve_submit(sid.raw(), PhysAddr(iova), Some(TENANT_B))
        {
            Ok(pa) => {
                self.b_ok += 1;
                if sid.stream_key() == self.a_key() || pa.0 < B_PA_LO || Self::in_a_pa(pa.0) {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(map_name(e), SMMU_SUBMIT_OK),
        }
        if rng.below(4) == 0 {
            self.io.clear_submit_sid();
        }
    }

    /// One of the three cap-checked unmap forms (`unmap_for`, `unmap`,
    /// `unmap_stream` with a B-pool SID or A's raw SID).
    fn unmap_any(
        &mut self,
        rng: &mut Xs,
        cap: &Capability,
        iova: u64,
    ) -> Result<aether_core::iommu::MappedRegion, MapError> {
        match rng.below(3) {
            0 => self.io.unmap_for(cap, PhysAddr(iova)),
            1 => self.io.unmap(cap, PhysAddr(iova)),
            _ => {
                let raw = if rng.below(2) == 0 {
                    self.sid_a.raw()
                } else {
                    rng.pick(&self.b_sid_pool()).raw()
                };
                self.io.unmap_stream(cap, raw, PhysAddr(iova))
            }
        }
    }

    fn op_unmap_for(&mut self, rng: &mut Xs) {
        let cap = self.pick_cap(rng);
        let iova = self.pick_iova(rng);
        match self.unmap_any(rng, &cap, iova) {
            Ok(r) => {
                self.b_ok += 1;
                if r.tenant != TENANT_B || StreamId(r.stream_id).stream_key() == self.a_key() {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(map_name(e), SMMU_UNMAP_OK),
        }
    }

    /// Checked path against A's pin: `unmap_for(&cap_b, ..)` with the wrong
    /// tenant must be `CrossTenant` (or `NotMapped` if the offset misses A's
    /// window). Anything else — notably `Ok` — is an escape. Covers
    /// `unmap_for`, `unmap` and `unmap_stream` (A's raw SID or a B SID;
    /// issue #161).
    fn op_unmap_for_a(&mut self, rng: &mut Xs) {
        let off = rng.below(A_LEN);
        let cap_b = self.cap_b;
        match self.unmap_any(rng, &cap_b, self.iova_a + off) {
            Err(MapError::CrossTenant) => self.note("MapError::CrossTenant", SMMU_UNMAP_OK),
            Err(e) => {
                // A's window covers iova_a..iova_a+A_LEN, so only CrossTenant is right.
                self.escapes += 1;
                self.note(map_name(e), SMMU_UNMAP_OK);
            }
            Ok(_) => {
                self.b_ok += 1;
                self.escapes += 1;
            }
        }
    }

    fn op_bind(&mut self, rng: &mut Xs) {
        let sid = rng.pick(&self.b_sid_pool());
        let cap = self.pick_cap(rng);
        match self.io.bind_stream(&cap, sid) {
            Ok(_) => {
                self.b_ok += 1;
                if sid.stream_key() == self.a_key() {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(map_name(e), SMMU_BIND_OK),
        }
        if self.io.tenant_sid_count(TENANT_B) > aether_core::iommu::SID_BUDGET_PER_TENANT {
            self.escapes += 1;
        }
    }

    fn b_ctx_pick(&self, rng: &mut Xs) -> GreenCtxId {
        match rng.below(4) {
            0 => self.ctx_a,
            1 => GreenCtxId(rng.below(8) as u16),
            _ => {
                let i = rng.below(4) as usize;
                self.b_ctx[i].unwrap_or(GreenCtxId(rng.below(8) as u16))
            }
        }
    }

    fn op_green(&mut self, rng: &mut Xs) {
        // B owns queue indices >= 1; queue 0 is A's.
        let queue = rng.pick(&[1u16, 1, 1, 2, 3]);
        match rng.below(4) {
            0 => {
                let b = SmWqBudget {
                    sm: rng.below(12) as u16,
                    wq: rng.below(12) as u16,
                };
                match self.pool.create(b) {
                    Ok(id) => {
                        self.b_ok += 1;
                        if id == self.ctx_a {
                            self.escapes += 1;
                        }
                        let slot = self.b_ctx.iter().position(|c| c.is_none());
                        if let Some(s) = slot {
                            self.b_ctx[s] = Some(id);
                        }
                    }
                    Err(e) => self.note(green_name(e), GREEN_OK),
                }
            }
            1 => {
                let id = self.b_ctx_pick(rng);
                match self.pool.bind(id, queue) {
                    Ok(()) => self.b_ok += 1,
                    Err(e) => self.note(green_name(e), GREEN_OK),
                }
            }
            2 => {
                let dest = self.b_ctx_pick(rng);
                let sid = StreamId::accel(ChipletId(1), TileId(4), 1);
                match self.pool.migrate_to_yield(queue, dest, sid) {
                    Ok(got) => {
                        self.b_ok += 1;
                        if got != sid {
                            self.escapes += 1;
                        }
                    }
                    Err(e) => self.note(green_name(e), GREEN_OK),
                }
            }
            _ => {
                // B may only unbind a ctx it created.
                let i = rng.below(4) as usize;
                let id = self.b_ctx[i].unwrap_or(GreenCtxId(rng.below(8) as u16));
                if id == self.ctx_a {
                    return;
                }
                match self.pool.unbind(id) {
                    Ok(_) => self.b_ok += 1,
                    Err(e) => self.note(green_name(e), GREEN_OK),
                }
            }
        }
    }

    fn op_noi(&mut self, rng: &mut Xs) {
        if rng.below(8) == 0 {
            match self.noi.release(TENANT_B) {
                Ok(o) => {
                    self.b_ok += 1;
                    if o.tenant != TENANT_B {
                        self.escapes += 1;
                    }
                }
                Err(e) => self.note(noi_name(e), NOI_OK),
            }
            return;
        }
        let demand = rng.pick(&[0u32, 1, 100, 400, 401, 500, 800, 1200, 5000, u32::MAX]);
        let class = rng.pick(&[FlowClass::Gradient, FlowClass::Curl, FlowClass::Harmonic]);
        match self.noi.admit_class(TENANT_B, demand, class) {
            Ok(est) => {
                self.b_ok += 1;
                if est.tenant != TENANT_B {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(noi_name(e), NOI_OK),
        }
    }

    fn op_timeline(&mut self, rng: &mut Xs) {
        let fid = FenceId(rng.below(40));
        match rng.below(5) {
            0 | 1 => {
                // Sometimes present A's profile to B's timeline → Unbound.
                let prof = if rng.below(4) == 0 { self.pa } else { self.pb };
                let wf = if rng.below(2) == 0 { None } else { Some(fid) };
                match self.tb.submit(&prof, wf) {
                    Ok(f) => {
                        self.b_ok += 1;
                        if f.partition != PartitionId(2) {
                            self.escapes += 1;
                        }
                    }
                    Err(e) => self.note(part_name(e), TL_OK),
                }
            }
            2 => match self.tb.wait(fid) {
                Ok(_) => self.b_ok += 1,
                Err(e) => self.note(part_name(e), TL_OK),
            },
            3 => {
                let id = if rng.below(2) == 0 {
                    FenceId(self.tb.retired() + 1)
                } else {
                    fid
                };
                match self.tb.complete(id) {
                    Ok(_) => self.b_ok += 1,
                    Err(e) => self.note(part_name(e), TL_OK),
                }
            }
            _ => {
                let id = FenceId(self.tb.retired() + 1);
                match self.tb.timeout(id) {
                    Ok(_) => self.b_ok += 1,
                    Err(e) => self.note(part_name(e), TL_OK),
                }
            }
        }
    }

    fn op_color(&mut self, rng: &mut Xs) {
        let color = match rng.below(6) {
            0 => None,
            1 => Some(BankColor::new(TENANT_B, HOME_BANK)),
            2 => Some(BankColor::new(TENANT_A, HOME_BANK)),
            3 => Some(BankColor::new(TENANT_B, OTHER_BANK)),
            4 => Some(BankColor::unassigned(HOME_BANK)),
            _ => Some(BankColor::new(TENANT_A, OTHER_BANK)),
        };
        let phase = rng.pick(&[Phase::Compute, Phase::Exchange, Phase::Barrier]);
        let expect_ok = phase == Phase::Exchange
            || color.is_some_and(|c| c.is_assigned() && c.tenant == TENANT_B && c.bank == HOME_BANK);
        match admit_wave(TENANT_B.0, phase, color, HOME_BANK) {
            Ok(()) => {
                self.b_ok += 1;
                if !expect_ok {
                    self.escapes += 1;
                }
            }
            Err(e) => {
                if expect_ok {
                    self.escapes += 1;
                }
                self.note(color_name(e), COLOR_OK);
            }
        }
    }

    fn op_opinject(&mut self, rng: &mut Xs) {
        match rng.below(10) {
            0 => {
                match self.inj.start() {
                    Ok(()) => self.b_ok += 1,
                    Err(e) => self.note(inj_name(e), INJ_OK),
                }
                return;
            }
            1 => {
                self.inj.stop();
                return;
            }
            2 => {
                let slot = rng.below(6) as u8;
                let kind = rng.pick(&[InjectKind::Memcpy, InjectKind::Saxpy, InjectKind::Scale]);
                match self.inj.publish(slot, kind) {
                    Ok(_) => self.b_ok += 1,
                    Err(e) => self.note(inj_name(e), INJ_OK),
                }
                return;
            }
            3 => {
                match self.inj.hot_add_scale() {
                    Ok(_) => self.b_ok += 1,
                    Err(e) => self.note(inj_name(e), INJ_OK),
                }
                return;
            }
            _ => {}
        }
        let slot = rng.below(MAX_OP_SLOTS as u64 + 2) as u8;
        let kind = rng.pick(&[InjectKind::Memcpy, InjectKind::Saxpy, InjectKind::Scale]);
        let cur = self.inj.slot_version(slot).unwrap_or(0);
        let version = rng.pick(&[0u32, 1, 2, cur, cur, cur, 99]);
        let n = rng.pick(&[0u32, 1, 2, 4, 4, 8, 9, 0x7fff_ffff, u32::MAX]);
        let alpha = rng.next() as i32;
        let addrs = [
            0u64,
            0x10,
            0x1c,
            0x20,
            0x3c,
            0x40,
            0x5c,
            0x60,
            u64::MAX - 3,
            u64::MAX,
        ];
        let src = if rng.below(6) == 0 {
            rng.next()
        } else {
            rng.pick(&addrs)
        };
        let dst = if rng.below(6) == 0 {
            rng.next()
        } else {
            rng.pick(&addrs)
        };
        let call = OpCall::new(slot, kind, version, n, alpha, src, dst);
        let mut mem = FlatOpMem {
            base: OP_BASE,
            bytes: &mut self.op_mem,
        };
        match self.inj.submit(&call, &self.sandbox, &mut mem) {
            Ok(()) => {
                self.b_ok += 1;
                // Independent span check: both spans inside B's sandbox.
                let bytes = (n as u64).saturating_mul(4);
                let inside = |base: u64| {
                    base.checked_add(bytes)
                        .is_some_and(|e| base >= OP_BASE && e <= OP_BASE + OP_B_SPAN)
                };
                if !(inside(src) && inside(dst)) {
                    self.escapes += 1;
                }
            }
            Err(e) => self.note(inj_name(e), INJ_OK),
        }
    }
}

/// Run `ops` hostile tenant-B ops from `seed`. Deterministic.
pub fn run_tenant_fuzz_demo(seed: u64, ops: u32) -> TenantFuzzReport {
    let mut rng = Xs::new(seed);
    let mut w = World::new();
    if !w.a_intact() {
        w.escapes += 1;
    }
    for _ in 0..ops {
        // Invariant 1b: no panic. A caught panic counts as unnamed.
        let res = catch_unwind(AssertUnwindSafe(|| w.step(&mut rng)));
        if res.is_err() {
            w.unnamed += 1;
        }
        // Invariant 2 (and 3's A-side): A is unchanged after every op.
        if !w.a_intact() {
            w.escapes += 1;
        }
    }
    TenantFuzzReport {
        seed,
        ops,
        escapes: w.escapes,
        unnamed: w.unnamed,
        variants: w.seen.len() as u32,
        b_ok: w.b_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzz_is_clean_and_deterministic() {
        let a = run_tenant_fuzz_demo(FUZZ_SEED, FUZZ_OPS);
        let b = run_tenant_fuzz_demo(FUZZ_SEED, FUZZ_OPS);
        assert_eq!(a, b, "same seed ⇒ same report");
        assert_eq!(a.escapes, 0, "no escapes");
        assert_eq!(a.unnamed, 0, "no unnamed outcomes");
        assert!(a.variants >= 8, "non-vacuous variant coverage");
        assert!(a.b_ok > 0);
        assert!(a.all_ok());
    }

    /// Issue #161 repro: every unmap form with B's cap against A's pin is
    /// `CrossTenant` (including `unmap_stream` with A's raw SID) and A stays
    /// intact.
    #[test]
    fn unmap_forms_wrong_tenant_are_cross_tenant() {
        let mut w = World::new();
        let (cap_b, iova, sid_a) = (w.cap_b, w.iova_a, w.sid_a);
        assert_eq!(
            w.io.unmap_for(&cap_b, PhysAddr(iova)).map(|r| r.tenant),
            Err(MapError::CrossTenant)
        );
        assert_eq!(
            w.io.unmap(&cap_b, PhysAddr(iova)).map(|r| r.tenant),
            Err(MapError::CrossTenant)
        );
        assert_eq!(
            w.io.unmap_stream(&cap_b, sid_a.raw(), PhysAddr(iova))
                .map(|r| r.tenant),
            Err(MapError::CrossTenant)
        );
        assert!(w.a_intact());
    }

    #[test]
    fn other_seeds_are_clean() {
        for seed in [1u64, 2, 0xDEAD_BEEF, 0x5AE8, 0xFFFF_FFFF_FFFF] {
            let r = run_tenant_fuzz_demo(seed, 1024);
            assert_eq!(r.escapes, 0, "seed {seed:#x}");
            assert_eq!(r.unnamed, 0, "seed {seed:#x}");
        }
    }

    /// The A-canary check is a real comparison: tampering is detected.
    #[test]
    fn a_canary_check_detects_tamper() {
        let mut w = World::new();
        assert!(w.a_intact());
        let iova = w.iova_a;
        let cap_a = mem_cap(1, TENANT_A, CapRights::MEM_FULL, CapKind::Memory);
        w.io.unmap(&cap_a, PhysAddr(iova)).expect("owner unmap A");
        assert!(!w.a_intact(), "unmapped canary must be detected");

        let mut w = World::new();
        w.op_mem[OP_CANARY_LO] ^= 0xFF;
        assert!(!w.a_intact(), "opinject canary byte flip must be detected");

        let mut w = World::new();
        w.noi.release(TENANT_A).expect("release A");
        assert!(!w.a_intact(), "A noi occupant loss must be detected");

        let mut w = World::new();
        w.pool.unbind(w.ctx_a).expect("unbind A");
        assert!(!w.a_intact(), "A green binding loss must be detected");
    }

    /// Escape accounting is real: a forged "B got A's PA" outcome counts.
    #[test]
    fn escape_counter_counts_leaks() {
        let mut w = World::new();
        // A tenant-B bind on A's own SID must be refused, not counted.
        let before = w.escapes;
        let sid = w.sid_a;
        let cap = w.cap_b;
        assert_eq!(w.io.bind_stream(&cap, sid), Err(MapError::CrossTenant));
        assert_eq!(w.escapes, before);
        // A's PA is outside B's slice constant.
        assert!(World::in_a_pa(A_PA) && A_PA < B_PA_LO);
    }
}
