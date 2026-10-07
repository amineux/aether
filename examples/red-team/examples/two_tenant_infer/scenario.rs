//! Two-tenant inference isolation scenario. Host only, software only.
//!
//! Tenants A and B each run a tiny i32 MLP on one shared software NPU
//! (`aether_core::accel::SoftNpu`) with existing ops only:
//! `Wave` (MatMul + bias) → `Relu` → `MatMul` → `Add` → `Max`. Every DMA
//! address is a Soft-SMMU IOVA, resolved per job on the submit path
//! (`IommuMap::set_sid` then `resolve_submit(sid, iova, Some(tenant))`).
//! Buffers come from `ArenaAllocator` arenas, mapped with each tenant's own
//! Memory+MAP capability (PA taken from the arena, as the kernel's
//! `SYS_MAP` does when `vaddr == 0`).
//!
//! Between the honest layers a third tenant C runs named attacks through
//! existing refuse paths only. Each attack counts as refused only if it
//! returns the expected named error **and** both honest tenants' arena
//! bytes, Soft-SMMU mappings and arena owners are unchanged.
//!
//! Cross-tenant unmap uses the checked `IommuMap::unmap_for`. The demo does
//! not rely on `unmap` / `unmap_stream` (issue #161). [`Config::leaked_cap`]
//! is a negative control: C presents A's own capability, the unmap
//! succeeds, and the harness must report it.
//!
//! Not hardware isolation, not MIG, no performance numbers.

use std::cell::Cell;

use aether_core::accel::{AccelError, AccelJobDesc, AccelOp, DmaView, SliceMem, SoftNpu};
use aether_core::arena::{ArenaAllocator, ArenaId, ArenaRequest};
use aether_core::caps::{CapKind, CapRights, Capability};
use aether_core::hodge::{FlowClass, HodgeQuota};
use aether_core::iommu::{IommuMap, MapError, MapRequest, StreamId};
use aether_core::opkernel::{CollectiveKind, OpKernelId, OperatorKernelHandle};
use aether_core::sysnr::{user_pages_ok, USER_MMAP_BASE, USER_PAGE};
use aether_core::types::{BankId, ChipletId, PhysAddr, TenantId, TileId, PAGE_4K};

/// Guest RAM window backing the arena bank.
pub const RAM_BASE: u64 = 0x0100_0000;
pub const RAM_LEN: usize = 64 * 1024;

// Byte offsets inside each tenant's 4 KiB arena.
const X: u64 = 0; // input 2×4
const W1: u64 = 64; // 4×4
const B1: u64 = 128; // 4
const H: u64 = 192; // 2×4
const R: u64 = 256; // 2×4
const W2: u64 = 320; // 4×2
const Y: u64 = 384; // 2×2
const B2: u64 = 448; // 2×2 (row-replicated bias)
const Z: u64 = 512; // 2×2
const FLOOR: u64 = 576; // 2×2 (replicated clamp floor)
const OUT: u64 = 640; // 2×2

/// One tenant's model and input. Small integers: exact, no overflow.
#[derive(Clone, Copy, Debug)]
pub struct Model {
    pub x: [i32; 8],
    pub w1: [i32; 16],
    pub b1: [i32; 4],
    pub w2: [i32; 8],
    pub b2: [i32; 2],
    pub floor: i32,
}

pub const MODEL_A: Model = Model {
    x: [1, 2, 3, 4, -1, 0, 2, 1],
    w1: [1, 0, -1, 2, 0, 1, 1, -1, 2, -1, 0, 1, -1, 2, 1, 0],
    b1: [1, -2, 0, 3],
    w2: [1, -1, 2, 0, -1, 1, 0, 2],
    b2: [-3, 1],
    floor: -4,
};

pub const MODEL_B: Model = Model {
    x: [3, -2, 1, 0, 2, 2, -1, 1],
    w1: [2, 1, 0, -1, -1, 3, 1, 0, 0, -2, 2, 1, 1, 0, -1, 2],
    b1: [0, 1, -1, 2],
    w2: [3, 0, -2, 1, 1, 1, 0, -1],
    b2: [2, -5],
    floor: -2,
};

/// Plain-Rust reference (i64 accumulate, checked narrowing). Independent
/// of `SoftNpu`.
pub fn cpu_reference(m: &Model) -> [i32; 4] {
    let mut h = [0i64; 8];
    for i in 0..2 {
        for j in 0..4 {
            let mut acc = m.b1[j] as i64;
            for k in 0..4 {
                acc += m.x[i * 4 + k] as i64 * m.w1[k * 4 + j] as i64;
            }
            h[i * 4 + j] = acc.max(0);
        }
    }
    let mut out = [0i32; 4];
    for i in 0..2 {
        for j in 0..2 {
            let mut acc = 0i64;
            for k in 0..4 {
                acc += h[i * 4 + k] * m.w2[k * 2 + j] as i64;
            }
            let z = acc + m.b2[j] as i64;
            out[i * 2 + j] = i32::try_from(z.max(m.floor as i64)).expect("reference fits i32");
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Config {
    /// Run tenant C's attacks between the honest layers.
    pub attacker: bool,
    /// Negative control: attack 1 presents A's own Memory+MAP cap (as if
    /// it had leaked to C), so `unmap_for` succeeds.
    pub leaked_cap: bool,
}

impl Config {
    pub const fn with_attacker() -> Self {
        Self {
            attacker: true,
            leaked_cap: false,
        }
    }
    pub const fn honest_only() -> Self {
        Self {
            attacker: false,
            leaked_cap: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackResult {
    pub name: &'static str,
    pub expected: String,
    pub got: String,
    /// Expected named error **and** honest state unchanged.
    pub refused: bool,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub log: Vec<String>,
    /// Tenant A, tenant B outputs as read back from guest RAM.
    pub outputs: [[i32; 4]; 2],
    pub output_bytes: [[u8; 16]; 2],
    pub cpu: [[i32; 4]; 2],
    pub honest_jobs_ok: bool,
    pub attacks: Vec<AttackResult>,
}

impl Report {
    pub fn refused(&self) -> usize {
        self.attacks.iter().filter(|a| a.refused).count()
    }
    pub fn outputs_match_cpu(&self) -> bool {
        self.honest_jobs_ok && self.outputs == self.cpu
    }
}

#[derive(Clone, Copy, Debug)]
struct Tenant {
    name: &'static str,
    id: TenantId,
    tile: u16,
    sid: StreamId,
    cap: Capability,
    arena: ArenaId,
    pa: u64,
    iova: u64,
}

/// SoftNpu DMA view for one submit: every address is an IOVA resolved on
/// the armed SID for this tenant. The first Soft-SMMU refusal is kept so
/// the log can name it (SoftNpu itself only reports a failed job).
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
    t: [Tenant; 3],
    log: Vec<String>,
}

const A: usize = 0;
const B: usize = 1;
const C: usize = 2;

fn err<E: core::fmt::Debug>(ty: &str, e: E) -> String {
    format!("{ty}::{e:?}")
}

impl World {
    fn new() -> Self {
        let mut arenas = ArenaAllocator::new(&[(BankId(0), PhysAddr(RAM_BASE), RAM_LEN as u64)])
            .expect("arena bank");
        let mut iommu = IommuMap::new();
        let mut log = Vec::new();
        let mk = |arenas: &mut ArenaAllocator,
                  iommu: &mut IommuMap,
                  name: &'static str,
                  id: u32,
                  tile: u16| {
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
            Tenant {
                name,
                id: tenant,
                tile,
                sid,
                cap,
                arena: arena.id,
                pa: arena.base.0,
                iova: region.iova.0,
            }
        };
        let a = mk(&mut arenas, &mut iommu, "A", 1, 1);
        let b = mk(&mut arenas, &mut iommu, "B", 2, 2);
        let c = mk(&mut arenas, &mut iommu, "C", 3, 3);
        for t in [&a, &b, &c] {
            log.push(format!(
                "[demo] setup tenant={} id={} tile={} sid={:#010x} arena=#{} pa={:#x} iova={:#x}",
                t.name,
                t.id.0,
                t.tile,
                t.sid.raw(),
                t.arena.0,
                t.pa,
                t.iova
            ));
        }
        Self {
            ram: vec![0u8; RAM_LEN],
            arenas,
            iommu,
            npu: SoftNpu::new(),
            t: [a, b, c],
            log,
        }
    }

    fn off(&self, pa: u64) -> usize {
        (pa - RAM_BASE) as usize
    }

    /// Host loader: copy a model into the tenant's own arena (by PA).
    fn load_model(&mut self, t: usize, m: &Model) {
        let base = self.t[t].pa;
        let mut put = |at: u64, vals: &[i32]| {
            for (i, v) in vals.iter().enumerate() {
                let o = (base + at - RAM_BASE) as usize + i * 4;
                self.ram[o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
        };
        put(X, &m.x);
        put(W1, &m.w1);
        put(B1, &m.b1);
        put(W2, &m.w2);
        put(B2, &[m.b2[0], m.b2[1], m.b2[0], m.b2[1]]);
        put(FLOOR, &[m.floor; 4]);
    }

    fn arena_bytes(&self, t: usize) -> Vec<u8> {
        let o = self.off(self.t[t].pa);
        self.ram[o..o + PAGE_4K as usize].to_vec()
    }

    fn read_out(&self, t: usize) -> ([i32; 4], [u8; 16]) {
        let o = self.off(self.t[t].pa + OUT);
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&self.ram[o..o + 16]);
        let mut v = [0i32; 4];
        for (i, x) in v.iter_mut().enumerate() {
            *x = i32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        }
        (v, bytes)
    }

    /// Layer `n` (1..=5) of the MLP for tenant `t`, all addresses IOVAs.
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

    /// SET_SID with the tenant's own cap, execute, drop the latch.
    fn submit(&mut self, t: usize, job: &AccelJobDesc) -> Result<u32, String> {
        let ten = self.t[t];
        self.iommu
            .set_sid(&ten.cap, ten.sid)
            .map_err(|e| err("MapError", e))?;
        let mut mem = SliceMem {
            base: PhysAddr(RAM_BASE),
            bytes: &mut self.ram,
        };
        let mut dma = TenantDma {
            iommu: &self.iommu,
            sid: ten.sid,
            tenant: ten.id,
            mem: &mut mem,
            fault: Cell::new(None),
        };
        let r = self.npu.execute(job, &mut dma);
        let fault = dma.fault.get();
        self.iommu.clear_submit_sid();
        match (r, fault) {
            (Ok(c), _) => Ok(c.job_seq),
            (Err(_), Some(f)) => Err(err("MapError", f)),
            (Err(e), None) => Err(err("AccelError", e)),
        }
    }

    /// Honest A and B state the attacker must not move.
    fn honest_snapshot(&self) -> Vec<(Vec<u8>, Option<PhysAddr>, Option<u16>)> {
        [A, B]
            .iter()
            .map(|&t| {
                let ten = &self.t[t];
                (
                    self.arena_bytes(t),
                    self.iommu.resolve_stream(ten.sid.raw(), PhysAddr(ten.iova)),
                    self.arenas.get(ten.arena).ok().and_then(|a| a.owner_tile),
                )
            })
            .collect()
    }

    fn attack(&mut self, i: usize, cfg: Config) -> AttackResult {
        let (a, c) = (self.t[A], self.t[C]);
        let before = self.honest_snapshot();
        let c_before = self.arena_bytes(C);
        let ok = |r: Result<(), String>| r.err().unwrap_or_else(|| "accepted".into());
        let (name, expected, got): (&'static str, String, String) = match i {
            1 => (
                "cross-tenant-unmap",
                err("MapError", MapError::CrossTenant),
                {
                    let cap = if cfg.leaked_cap { a.cap } else { c.cap };
                    ok(self
                        .iommu
                        .unmap_for(&cap, PhysAddr(a.iova))
                        .map(|_| ())
                        .map_err(|e| err("MapError", e)))
                },
            ),
            2 => (
                "bind-foreign-sid",
                err("MapError", MapError::CrossTenant),
                ok(self.iommu.bind_stream(&c.cap, a.sid).map(|_| ()).map_err(|e| err("MapError", e))),
            ),
            3 => (
                "stamp-foreign-sid",
                err("MapError", MapError::CrossTenant),
                {
                    let r = self.iommu.set_sid(&c.cap, a.sid).map(|_| ());
                    self.iommu.clear_submit_sid();
                    ok(r.map_err(|e| err("MapError", e)))
                },
            ),
            4 => (
                "wrong-stream-submit",
                err("MapError", MapError::WrongStream),
                {
                    let armed = self.iommu.set_sid(&c.cap, c.sid);
                    let r = self
                        .iommu
                        .resolve_submit(a.sid.raw(), PhysAddr(a.iova + W1), Some(c.id));
                    self.iommu.clear_submit_sid();
                    match armed {
                        Err(e) => err("MapError", e),
                        Ok(_) => ok(r.map(|_| ()).map_err(|e| err("MapError", e))),
                    }
                },
            ),
            5 => (
                "dma-read-foreign-weights",
                err("MapError", MapError::WrongStream),
                {
                    // C's job names A's W1 IOVA as its A operand.
                    let job = AccelJobDesc::matmul_i32(
                        4,
                        4,
                        4,
                        PhysAddr(a.iova + W1),
                        PhysAddr(c.iova + W1),
                        PhysAddr(c.iova + H),
                        c.id.0,
                    );
                    ok(self.submit(C, &job).map(|_| ()))
                },
            ),
            6 => (
                "dma-write-foreign-activations",
                err("MapError", MapError::WrongStream),
                {
                    // C's job stores into A's hidden activations.
                    let job = AccelJobDesc::add_i32(
                        2,
                        4,
                        PhysAddr(c.iova + X),
                        PhysAddr(c.iova + X),
                        PhysAddr(a.iova + H),
                        c.id.0,
                    );
                    ok(self.submit(C, &job).map(|_| ()))
                },
            ),
            7 => (
                "foreign-arena",
                err("ArenaError", aether_core::arena::ArenaError::NotOwner),
                {
                    let take = self
                        .arenas
                        .transfer_owner(a.arena, Some(c.tile), c.tile, c.id.0);
                    let reclaim = self.arenas.transfer_owner(a.arena, None, c.tile, c.id.0);
                    match (take, reclaim) {
                        (Err(e1), Err(e2)) if e1 == e2 => err("ArenaError", e1),
                        (Err(e1), Err(_)) => err("ArenaError", e1),
                        _ => "accepted".into(),
                    }
                },
            ),
            8 => (
                "user-copy-straddle",
                format!("user_pages_ok::Err({:#x})", USER_MMAP_BASE + USER_PAGE),
                {
                    // C's aspace maps one page; the next page is A's and is
                    // not mapped for C. 8 + 8 bytes across the boundary.
                    let c_mapped = |va: u64| va / USER_PAGE == USER_MMAP_BASE / USER_PAGE;
                    match user_pages_ok(USER_MMAP_BASE + USER_PAGE - 8, 16, c_mapped) {
                        Ok(()) => "accepted".into(),
                        Err(p) => format!("user_pages_ok::Err({p:#x})"),
                    }
                },
            ),
            _ => (
                "opkernel-wrong-class",
                err("OpKernelError", aether_core::opkernel::OpKernelError::ClassMismatch),
                {
                    let mut q = HodgeQuota::generous();
                    let q0 = q;
                    match OperatorKernelHandle::bind(
                        OpKernelId(c.id.0),
                        CollectiveKind::Tree,
                        FlowClass::Gradient,
                    ) {
                        Err(e) => err("HodgeError", e),
                        Ok(h) => match h.admit_as(&mut q, FlowClass::Harmonic) {
                            Ok(()) => "accepted".into(),
                            Err(e) if q.remain == q0.remain => err("OpKernelError", e),
                            Err(e) => format!("OpKernelError::{e:?} (quota moved)"),
                        },
                    }
                },
            ),
        };
        let after = self.honest_snapshot();
        // Judged per attack: this call must not move A's or B's bytes,
        // Soft-SMMU translation or arena owner.
        let honest_intact = before == after;
        // Refused DMA jobs must not write C's own output either.
        let c_quiet = !(5..=6).contains(&i) || self.arena_bytes(C) == c_before;
        let refused = got == expected && honest_intact && c_quiet;
        self.log.push(format!(
            "[demo] attack={name} by=C target=A err={got} honest_state_unchanged={} result={}",
            honest_intact && c_quiet,
            if refused { "refused" } else { "NOT-REFUSED" }
        ));
        AttackResult {
            name,
            expected,
            got,
            refused,
        }
    }
}

/// Attacks fired after each honest layer pair (layer → attack ids).
const SCHEDULE: [&[usize]; 5] = [&[1, 2], &[3, 4], &[5, 6], &[7, 8], &[9]];

pub fn run(cfg: Config) -> Report {
    let mut w = World::new();
    w.load_model(A, &MODEL_A);
    w.load_model(B, &MODEL_B);
    let mut honest_jobs_ok = true;
    let mut attacks = Vec::new();
    for (layer, slot) in (1..=5u32).zip(SCHEDULE) {
        for t in [A, B] {
            let job = w.layer_job(t, layer);
            let op = job.op;
            let r = w.submit(t, &job);
            honest_jobs_ok &= r.is_ok();
            let line = match r {
                Ok(seq) => format!("[demo] tenant={} layer={layer} op={op:?} ok seq={seq}", w.t[t].name),
                Err(e) => format!("[demo] tenant={} layer={layer} op={op:?} FAILED err={e}", w.t[t].name),
            };
            w.log.push(line);
        }
        if cfg.attacker {
            for &i in slot {
                attacks.push(w.attack(i, cfg));
            }
        }
    }
    let cpu = [cpu_reference(&MODEL_A), cpu_reference(&MODEL_B)];
    let (oa, ba) = w.read_out(A);
    let (ob, bb) = w.read_out(B);
    for (t, out, refv) in [(A, oa, cpu[0]), (B, ob, cpu[1])] {
        w.log.push(format!(
            "[demo] tenant={} out={out:?} cpu={refv:?} match={}",
            w.t[t].name,
            out == refv
        ));
    }
    Report {
        log: w.log,
        outputs: [oa, ob],
        output_bytes: [ba, bb],
        cpu,
        honest_jobs_ok,
        attacks,
    }
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub run: Report,
    pub attacks: usize,
    pub refused: usize,
    pub outputs_match_cpu: bool,
    /// Two fresh runs: same output bytes and same full log.
    pub deterministic: bool,
    /// Honest outputs equal an attacker-free run.
    pub unperturbed: bool,
}

impl Summary {
    pub fn line(&self) -> String {
        format!(
            "[demo] tenants=2 attacker=1 attacks={} refused={} outputs_match_cpu={} deterministic={} unperturbed={}",
            self.attacks, self.refused, self.outputs_match_cpu, self.deterministic, self.unperturbed
        )
    }

    pub fn all_ok(&self) -> bool {
        self.attacks > 0
            && self.refused == self.attacks
            && self.outputs_match_cpu
            && self.deterministic
            && self.unperturbed
    }
}

pub fn summarize(cfg: Config) -> Summary {
    let r1 = run(cfg);
    let r2 = run(cfg);
    let r0 = run(Config::honest_only());
    Summary {
        attacks: r1.attacks.len(),
        refused: r1.refused(),
        outputs_match_cpu: r1.outputs_match_cpu(),
        deterministic: r1.log == r2.log && r1.output_bytes == r2.output_bytes,
        unperturbed: r1.output_bytes == r0.output_bytes,
        run: r1,
    }
}

pub const SCOPE_LINE: &str = "[demo] scope: host software model (core SoftNpu + Soft SMMU + arenas + caps); not hardware isolation, not MIG, no performance claim";
