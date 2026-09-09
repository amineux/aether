//! SoftChipletSync: scoped timelines + hierarchical counters + SoftCCT.
//!
//! SpectraScout post-M2 leftover (after M3 SID-at-submit and M4 XQueue).
//! SoftChipletSync (PR #51) is the scoped-timeline layer. **SoftCCT** is
//! the elision layer on top: Soft-CP buffer labels + last-writer chiplet.
//! **SoftNoI-IS** is the admit layer: a fake shared NoI advertises a
//! per-tenant Interference Score; Soft-CP / XQueue refuse when projected
//! IS > budget. Descriptors may carry a software [`crate::hodge::FlowClass`]
//! tag; Curl also needs reserved ring capacity. PARL/NoI inspiration;
//! **admit control, not topology synthesis** (see [`crate::noi`]).
//!
//! **Inspiration (not a port, not a product):**
//! - Fleet hierarchical event counters (wave / CU / chiplet / package):
//!   workers increment a chiplet-local counter with **no** package fence;
//!   only the last worker on a participating chiplet issues a package-scope
//!   fence. Chiplet-local signal is free; package-scope costs more.
//! - CPElide (MICRO’24) Chiplet Coherence Table: last-writer chiplet per
//!   buffer label. Targeted acquire/release vs all-chiplet (broadcast)
//!   fences. A consumer on that same chiplet **elides** the package fence.
//!
//! SoftCCT issues a SoftChipletSync package-scope fence **only** when the
//! table says a cross-chiplet hazard. Single-chiplet CCT is a **no-op**
//! (no inter-chiplet hazard to elide; host tests that claim a CCT win
//! use ≥2 fake chiplets).
//!
//! **Not claimed.** This is not a Vulkan / ROCm product, not UCIe sync,
//! not a full coherence protocol, and not ChipletFleet **placement** (that
//! stub lives in [`crate::sched::ChipletTaskScope`] and stays
//! KILL-as-calendar). Host tests measure fence **counts**. Latency wins
//! need a multi-chiplet sim — single-die QEMU / host numbers are not
//! partner proof.

use crate::fence::{Fence, FenceId, Timeline, TimelineId, MAX_IN_FLIGHT};
use crate::hodge::FlowClass;
use crate::noi::{IsEstimate, NoiError, SoftNoI};
use crate::partition::{
    BlastRadius, PartitionError, PartitionId, PartitionProfile, QosBudget, SpatialSlice,
};
use crate::types::{ChipletId, TenantId};

/// Fake-package width. Software cap, not a silicon XCD count.
pub const MAX_SYNC_CHIPLETS: usize = 8;

/// CCT rows. Software table in the CP, not a directory cache.
pub const MAX_CCT_ENTRIES: usize = 16;

/// Canonical hierarchical clip: two fake chiplets × this many workers.
/// Naive global fence = 16; hierarchical package fences = 2.
pub const DEMO_WORKERS_PER_CHIPLET: u32 = 8;

/// Visibility scope. Narrowest first (Fleet-shaped).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SyncScope {
    /// Wavefront-local. Chiplet-local signal; no package fence.
    Wave = 0,
    /// Compute-unit local. Chiplet-local signal; no package fence.
    Cu = 1,
    /// Chiplet / L2-local. Free in this model (no package fence).
    Chiplet = 2,
    /// Package / GPU-scope. Last worker per participating chiplet pays.
    Package = 3,
}

impl SyncScope {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Wave => "wave",
            Self::Cu => "cu",
            Self::Chiplet => "chiplet",
            Self::Package => "package",
        }
    }

    /// Package fence cost of one signal at this scope (before hierarchy / CCT).
    pub const fn naive_package_cost(self) -> u32 {
        match self {
            Self::Package => 1,
            _ => 0,
        }
    }

    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Wave),
            1 => Some(Self::Cu),
            2 => Some(Self::Chiplet),
            3 => Some(Self::Package),
            _ => None,
        }
    }
}

/// Named data structure the CCT tracks. Not an IOVA and not a Vulkan handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferLabel(pub u32);

/// What a scoped arrive / wait did. PackageFence is the expensive one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalKind {
    /// Wave / CU / chiplet, or a non-last worker on a chiplet.
    ChipletLocal,
    /// Last worker; CCT on — package fence deferred until [`SoftChipletSync::wait`].
    PendingPackage,
    /// A package-scope fence was issued (or a cross-chiplet wait observed one).
    PackageFence,
    /// CCT: last-writer chiplet matches the consumer. No package fence.
    Elided,
}

/// Producer / consumer annotation a Soft-CP or IreeShapedCp job may carry.
/// Default submit leaves this unset (SID / XQueue / mailbox path unchanged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopedWork {
    pub scope: SyncScope,
    pub write: Option<BufferLabel>,
    pub read: Option<BufferLabel>,
}

/// A completed pulse on a scoped timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopedFence {
    pub fence: Fence,
    pub scope: SyncScope,
    pub chiplet: ChipletId,
    pub kind: SignalKind,
}

/// Chiplet Coherence Table: last-writer chiplet per [`BufferLabel`].
///
/// CPElide-shaped software table housed in the command processor.
/// Not a cache-coherence directory and not silicon.
#[derive(Clone, Copy, Debug)]
pub struct ChipletCoherenceTable {
    labels: [u32; MAX_CCT_ENTRIES],
    writers: [u8; MAX_CCT_ENTRIES],
    used: [bool; MAX_CCT_ENTRIES],
}

impl ChipletCoherenceTable {
    pub const fn new() -> Self {
        Self {
            labels: [0; MAX_CCT_ENTRIES],
            writers: [0; MAX_CCT_ENTRIES],
            used: [false; MAX_CCT_ENTRIES],
        }
    }

    pub fn record(&mut self, label: BufferLabel, writer: ChipletId) -> Result<(), PartitionError> {
        for i in 0..MAX_CCT_ENTRIES {
            if self.used[i] && self.labels[i] == label.0 {
                self.writers[i] = writer.0;
                return Ok(());
            }
        }
        for i in 0..MAX_CCT_ENTRIES {
            if !self.used[i] {
                self.used[i] = true;
                self.labels[i] = label.0;
                self.writers[i] = writer.0;
                return Ok(());
            }
        }
        Err(PartitionError::CreditExhausted)
    }

    pub fn last_writer(&self, label: BufferLabel) -> Option<ChipletId> {
        for i in 0..MAX_CCT_ENTRIES {
            if self.used[i] && self.labels[i] == label.0 {
                return Some(ChipletId(self.writers[i]));
            }
        }
        None
    }

    /// Table-level match: last-writer chiplet == consumer.
    ///
    /// SoftCCT policy ([`SoftCct::should_elide`]) also requires ≥2
    /// participating chiplets. A single-chiplet match is a no-op.
    pub fn elide_package_fence(&self, label: BufferLabel, consumer: ChipletId) -> bool {
        self.last_writer(label) == Some(consumer)
    }

    /// Buggy policy: elide whenever the label is known, ignoring writer chiplet.
    /// The incorrect-elision host test must fail this on a cross-chiplet consume.
    pub fn incorrect_elide(&self, label: BufferLabel) -> bool {
        self.last_writer(label).is_some()
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

/// SoftCCT: elision policy on [`ChipletCoherenceTable`].
///
/// Soft-CP / IreeShapedCp jobs carry [`BufferLabel`]s. SoftCCT records the
/// last-writer chiplet and tells SoftChipletSync to issue a package fence
/// only on a cross-chiplet hazard. Not a directory cache, not a coherence
/// protocol, not a Vulkan / ROCm product.
#[derive(Clone, Copy, Debug)]
pub struct SoftCct {
    table: ChipletCoherenceTable,
    /// Bit i set if chiplet i has been expected, arrived, or waited.
    seen: u8,
}

impl SoftCct {
    pub const fn new() -> Self {
        Self {
            table: ChipletCoherenceTable::new(),
            seen: 0,
        }
    }

    pub const fn table(&self) -> &ChipletCoherenceTable {
        &self.table
    }

    /// Distinct chiplets named on this open event.
    pub const fn package_width(&self) -> u32 {
        self.seen.count_ones()
    }

    /// Single-chiplet CCT has nothing to elide.
    pub const fn is_noop(&self) -> bool {
        self.package_width() < 2
    }

    pub fn note_chiplet(&mut self, chiplet: ChipletId) -> Result<(), PartitionError> {
        let i = idx(chiplet)?;
        self.seen |= 1u8 << i;
        Ok(())
    }

    pub fn record(&mut self, label: BufferLabel, writer: ChipletId) -> Result<(), PartitionError> {
        self.note_chiplet(writer)?;
        self.table.record(label, writer)
    }

    pub fn last_writer(&self, label: BufferLabel) -> Option<ChipletId> {
        self.table.last_writer(label)
    }

    /// Elide only on a multi-chiplet package when last-writer == consumer.
    pub fn should_elide(&self, label: BufferLabel, consumer: ChipletId) -> bool {
        !self.is_noop() && self.table.elide_package_fence(label, consumer)
    }

    /// Reset participating chiplets for a new event. Table rows persist (CP).
    pub fn clear_event(&mut self) {
        self.seen = 0;
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

impl Default for SoftCct {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for ChipletCoherenceTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Scoped SoftChipletSync object.
///
/// Four CP-shaped [`Timeline`]s (wave / CU / chiplet / package) plus
/// Fleet-shaped two-level counters and SoftCCT elision. Distinct from
/// [`crate::sched::ChipletTaskScope`] (placement / steal affinity).
pub struct SoftChipletSync {
    partition: PartitionId,
    profile: PartitionProfile,
    timelines: [Timeline; 4],
    cct: SoftCct,
    cct_on: bool,
    scope: SyncScope,
    expected: [u32; MAX_SYNC_CHIPLETS],
    arrived: [u32; MAX_SYNC_CHIPLETS],
    pending: [bool; MAX_SYNC_CHIPLETS],
    naive_package: u32,
    package_fences: u32,
    elided: u32,
    /// All-chiplet fence baseline: one package fence per labeled wait.
    broadcast_package: u32,
    /// Fake NoI admit policy. Off by default (SID / XQueue / CCT unchanged).
    noi: SoftNoI,
}

impl SoftChipletSync {
    pub const fn new(partition: PartitionId) -> Self {
        Self {
            partition,
            profile: PartitionProfile::new(
                partition,
                SpatialSlice {
                    chiplet_lo: ChipletId(0),
                    chiplet_hi: ChipletId((MAX_SYNC_CHIPLETS - 1) as u8),
                    tile_mask: u32::MAX,
                    bank_mask: 0b1,
                },
                QosBudget {
                    bw_mbps: 0,
                    credits: MAX_IN_FLIGHT,
                },
                BlastRadius {
                    max_nodes: MAX_SYNC_CHIPLETS as u16,
                    max_hops: 2,
                },
            ),
            timelines: [
                Timeline::named(TimelineId(0), partition),
                Timeline::named(TimelineId(1), partition),
                Timeline::named(TimelineId(2), partition),
                Timeline::named(TimelineId(3), partition),
            ],
            cct: SoftCct::new(),
            cct_on: false,
            scope: SyncScope::Chiplet,
            expected: [0; MAX_SYNC_CHIPLETS],
            arrived: [0; MAX_SYNC_CHIPLETS],
            pending: [false; MAX_SYNC_CHIPLETS],
            naive_package: 0,
            package_fences: 0,
            elided: 0,
            broadcast_package: 0,
            noi: SoftNoI::new(),
        }
    }

    pub const fn partition(&self) -> PartitionId {
        self.partition
    }

    pub const fn scope(&self) -> SyncScope {
        self.scope
    }

    pub const fn cct_enabled(&self) -> bool {
        self.cct_on
    }

    /// Package-scope fences **issued** this event (after hierarchy + CCT).
    pub const fn package_fences(&self) -> u32 {
        self.package_fences
    }

    /// Naive global fence: one package fence per worker arrive.
    pub const fn naive_package_fences(&self) -> u32 {
        self.naive_package
    }

    pub const fn elided(&self) -> u32 {
        self.elided
    }

    /// Broadcast / all-chiplet fence baseline (one per labeled consumer wait).
    pub const fn broadcast_package_fences(&self) -> u32 {
        self.broadcast_package
    }

    pub fn cct(&self) -> &ChipletCoherenceTable {
        self.cct.table()
    }

    pub fn softcct(&self) -> &SoftCct {
        &self.cct
    }

    pub fn noi(&self) -> &SoftNoI {
        &self.noi
    }

    pub fn noi_mut(&mut self) -> &mut SoftNoI {
        &mut self.noi
    }

    /// Enable SoftNoI-IS admit. Off by default (SID / XQueue path unchanged).
    pub fn enable_noi(&mut self, on: bool) {
        self.noi.enable(on);
    }

    /// Per-tenant IS estimate advertised by this fabric. `None` if empty.
    pub fn advertised_is_milli(&self, tenant: TenantId) -> Option<u32> {
        self.noi.tenant_is_milli(tenant)
    }

    /// Project + admit onto the fake NoI. Refuse when IS > budget.
    pub fn admit_noi(&mut self, tenant: TenantId, demand: u32) -> Result<IsEstimate, NoiError> {
        self.noi.admit(tenant, demand)
    }

    /// Admit with a fabric class tag (Curl also needs reserved ring capacity).
    pub fn admit_noi_class(
        &mut self,
        tenant: TenantId,
        demand: u32,
        class: FlowClass,
    ) -> Result<IsEstimate, NoiError> {
        self.noi.admit_class(tenant, demand, class)
    }

    pub fn release_noi(&mut self, tenant: TenantId) -> Result<(), NoiError> {
        self.noi.release(tenant).map(|_| ())
    }

    pub fn timeline(&self, scope: SyncScope) -> &Timeline {
        &self.timelines[scope as usize]
    }

    /// Enable SoftCCT elision. Off by default (Fleet counters only).
    /// Single-chiplet packages stay a no-op even when this is on.
    pub fn enable_cct(&mut self, on: bool) {
        self.cct_on = on;
    }

    /// Set the open event's scope without resetting counters.
    pub fn set_scope(&mut self, scope: SyncScope) {
        self.scope = scope;
    }

    /// Apply a CP job's scoped annotation (arrive and/or wait).
    pub fn note(
        &mut self,
        chiplet: ChipletId,
        work: ScopedWork,
    ) -> Result<SignalKind, PartitionError> {
        self.set_scope(work.scope);
        let mut kind = SignalKind::ChipletLocal;
        if work.write.is_some() || work.read.is_none() {
            kind = self.arrive(chiplet, work.write)?.kind;
        }
        if work.read.is_some() {
            kind = self.wait(chiplet, work.read)?;
        }
        Ok(kind)
    }

    /// Open a new event. CCT rows persist (the table lives in the CP).
    pub fn open(&mut self, scope: SyncScope) {
        self.scope = scope;
        self.expected = [0; MAX_SYNC_CHIPLETS];
        self.arrived = [0; MAX_SYNC_CHIPLETS];
        self.pending = [false; MAX_SYNC_CHIPLETS];
        self.naive_package = 0;
        self.package_fences = 0;
        self.elided = 0;
        self.broadcast_package = 0;
        self.cct.clear_event();
    }

    /// How many workers on `chiplet` participate in the open event.
    /// `0` means "first arrive implies 1" (single-job default).
    pub fn expect(&mut self, chiplet: ChipletId, n_workers: u32) -> Result<(), PartitionError> {
        let i = idx(chiplet)?;
        self.expected[i] = n_workers;
        if n_workers > 0 {
            self.cct.note_chiplet(chiplet)?;
        }
        Ok(())
    }

    /// Worker completion. Chiplet-local counter is free. Last worker on a
    /// package-scope chiplet issues (or defers) one package fence.
    pub fn arrive(
        &mut self,
        chiplet: ChipletId,
        write: Option<BufferLabel>,
    ) -> Result<ScopedFence, PartitionError> {
        let i = idx(chiplet)?;
        if self.expected[i] == 0 {
            self.expected[i] = 1;
        }
        if self.arrived[i] >= self.expected[i] {
            return Err(PartitionError::Unbound);
        }
        self.arrived[i] += 1;
        self.naive_package = self.naive_package.saturating_add(1);

        self.cct.note_chiplet(chiplet)?;
        if let Some(label) = write {
            self.cct.record(label, chiplet)?;
        }

        let local = if self.scope < SyncScope::Chiplet {
            self.scope
        } else {
            SyncScope::Chiplet
        };
        let pulse = self.pulse(local, chiplet)?;
        let last = self.arrived[i] == self.expected[i];

        if self.scope != SyncScope::Package {
            return Ok(ScopedFence {
                fence: pulse,
                scope: local,
                chiplet,
                kind: SignalKind::ChipletLocal,
            });
        }
        if !last {
            return Ok(ScopedFence {
                fence: pulse,
                scope: local,
                chiplet,
                kind: SignalKind::ChipletLocal,
            });
        }

        if self.cct_on {
            self.pending[i] = true;
            return Ok(ScopedFence {
                fence: pulse,
                scope: SyncScope::Package,
                chiplet,
                kind: SignalKind::PendingPackage,
            });
        }

        self.package_fences = self.package_fences.saturating_add(1);
        let pkg = self.pulse(SyncScope::Package, chiplet)?;
        Ok(ScopedFence {
            fence: pkg,
            scope: SyncScope::Package,
            chiplet,
            kind: SignalKind::PackageFence,
        })
    }

    /// Consumer wait. SoftCCT elides when last-writer chiplet == consumer
    /// on a ≥2-chiplet package. Cross-chiplet hazard still package-fences.
    pub fn wait(
        &mut self,
        consumer: ChipletId,
        read: Option<BufferLabel>,
    ) -> Result<SignalKind, PartitionError> {
        let _ = idx(consumer)?;
        self.cct.note_chiplet(consumer)?;
        if self.cct_on {
            if read.is_some() {
                self.broadcast_package = self.broadcast_package.saturating_add(1);
            }
            if let Some(label) = read {
                if self.cct.should_elide(label, consumer) {
                    // Keep pending[writer]: a later cross-chiplet wait still
                    // needs the deferred package fence.
                    self.elided = self.elided.saturating_add(1);
                    let _ = self.pulse(SyncScope::Chiplet, consumer)?;
                    return Ok(SignalKind::Elided);
                }
            }
        }

        let mut issued_now = 0u32;
        for i in 0..MAX_SYNC_CHIPLETS {
            if self.pending[i] {
                self.pending[i] = false;
                self.package_fences = self.package_fences.saturating_add(1);
                issued_now += 1;
                let _ = self.pulse(SyncScope::Package, ChipletId(i as u8))?;
            }
        }

        let cross = match read.and_then(|l| self.cct.last_writer(l)) {
            Some(w) => w != consumer,
            None => false,
        };
        if issued_now > 0 || (cross && self.package_fences > 0) {
            if issued_now == 0 {
                let _ = self.pulse(SyncScope::Package, consumer)?;
            }
            Ok(SignalKind::PackageFence)
        } else {
            let _ = self.pulse(SyncScope::Chiplet, consumer)?;
            Ok(SignalKind::ChipletLocal)
        }
    }

    /// True when issued package fences are a strict fraction of naive global.
    ///
    /// Host-test measurable. **Not** a latency claim and not partner proof
    /// on a single die.
    pub const fn package_lt_naive(&self) -> bool {
        let issued = self.package_fences;
        let naive = self.naive_package;
        naive > 0 && issued < naive && (issued == 0 || issued.saturating_mul(4) < naive)
    }

    /// SoftCCT issued package fences ≪ all-chiplet broadcast baseline.
    ///
    /// Requires CCT on and at least one labeled wait. Single-chiplet is
    /// a no-op (issued == broadcast), so this is false there on purpose.
    pub const fn cct_lt_broadcast(&self) -> bool {
        let issued = self.package_fences;
        let bcast = self.broadcast_package;
        self.cct_on
            && bcast > 0
            && issued < bcast
            && (issued == 0 || issued.saturating_mul(4) < bcast)
    }

    /// Poll a scoped timeline watermark (the seq a CP would retire).
    pub fn wait_seq(&self, scope: SyncScope, id: FenceId) -> Result<Fence, PartitionError> {
        self.timelines[scope as usize].wait(id)
    }

    fn pulse(&mut self, scope: SyncScope, chiplet: ChipletId) -> Result<Fence, PartitionError> {
        let _ = chiplet;
        let t = &mut self.timelines[scope as usize];
        if t.partition() != self.partition {
            return Err(PartitionError::Unbound);
        }
        let profile = self.profile;
        let f = t.submit(&profile, None)?;
        t.complete(f.id)
    }
}

fn idx(c: ChipletId) -> Result<usize, PartitionError> {
    let i = c.0 as usize;
    if i >= MAX_SYNC_CHIPLETS {
        Err(PartitionError::OutsideSlice)
    } else {
        Ok(i)
    }
}

/// Host-identical clip. Kernel prints `[chipsync] …`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChipletSyncReport {
    pub package_lt_naive: bool,
    pub hierarchical_fences: u32,
    pub naive_fences: u32,
    pub cct_elide: bool,
    pub two_chiplet: bool,
}

impl ChipletSyncReport {
    pub fn all_ok(&self) -> bool {
        self.package_lt_naive && self.cct_elide && self.two_chiplet
    }
}

/// SoftCCT clip. Kernel prints `[softcct] …`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftCctReport {
    pub cct_lt_broadcast: bool,
    pub cct_fences: u32,
    pub broadcast_fences: u32,
    pub same_chiplet_elide: bool,
    pub cross_chiplet_fence: bool,
    pub single_chiplet_noop: bool,
    pub incorrect_elision_refused: bool,
}

impl SoftCctReport {
    pub fn all_ok(&self) -> bool {
        self.cct_lt_broadcast
            && self.same_chiplet_elide
            && self.cross_chiplet_fence
            && self.single_chiplet_noop
            && self.incorrect_elision_refused
    }
}

/// Two fake chiplets, hierarchical package fences ≪ naive, CCT elision
/// on same-chiplet consume, cross-chiplet producer/consumer cannot elide.
pub fn run_chipsync_demo() -> ChipletSyncReport {
    let buf = BufferLabel(1);
    let a = ChipletId(0);
    let b = ChipletId(1);

    let mut sync = SoftChipletSync::new(PartitionId(1));

    // Fleet hierarchical counters, CCT off: 2 chiplets × 8 workers.
    sync.open(SyncScope::Package);
    let _ = sync.expect(a, DEMO_WORKERS_PER_CHIPLET);
    let _ = sync.expect(b, DEMO_WORKERS_PER_CHIPLET);
    for _ in 0..DEMO_WORKERS_PER_CHIPLET {
        let _ = sync.arrive(a, Some(buf));
        let _ = sync.arrive(b, Some(buf));
    }
    let hierarchical_fences = sync.package_fences();
    let naive_fences = sync.naive_package_fences();
    let package_lt_naive = sync.package_lt_naive()
        && hierarchical_fences == 2
        && naive_fences == DEMO_WORKERS_PER_CHIPLET * 2;

    // SoftCCT elision: ≥2 fake chiplets, last-writer matches consumer.
    sync.enable_cct(true);
    sync.open(SyncScope::Package);
    let _ = sync.expect(a, DEMO_WORKERS_PER_CHIPLET);
    let _ = sync.expect(b, DEMO_WORKERS_PER_CHIPLET);
    for _ in 0..DEMO_WORKERS_PER_CHIPLET {
        let _ = sync.arrive(a, Some(buf));
    }
    let elide = sync.wait(a, Some(buf));
    let cct_elide = elide == Ok(SignalKind::Elided)
        && sync.package_fences() == 0
        && sync.elided() == 1
        && sync.cct().last_writer(buf) == Some(a)
        && sync.softcct().should_elide(buf, a)
        && !sync.softcct().is_noop();

    // Producer on chiplet 0, consumer on chiplet 1: cannot elide.
    sync.open(SyncScope::Package);
    let _ = sync.expect(a, DEMO_WORKERS_PER_CHIPLET);
    for _ in 0..DEMO_WORKERS_PER_CHIPLET {
        let _ = sync.arrive(a, Some(buf));
    }
    let cross = sync.wait(b, Some(buf));
    let two_chiplet = cross == Ok(SignalKind::PackageFence)
        && sync.package_fences() == 1
        && sync.elided() == 0
        && sync.cct().last_writer(buf) == Some(a)
        && !sync.cct().elide_package_fence(buf, b)
        && !sync.softcct().should_elide(buf, b)
        && sync.package_lt_naive();

    ChipletSyncReport {
        package_lt_naive,
        hierarchical_fences,
        naive_fences,
        cct_elide,
        two_chiplet,
    }
}

/// SoftCCT vs broadcast, incorrect elision refused, single-chiplet no-op.
pub fn run_softcct_demo() -> SoftCctReport {
    let buf = BufferLabel(2);
    let a = ChipletId(0);
    let b = ChipletId(1);

    // 10 arrives, then 8 same-chiplet waits + 2 cross waits.
    let mut sync = SoftChipletSync::new(PartitionId(1));
    sync.enable_cct(true);
    sync.open(SyncScope::Package);
    let _ = sync.expect(a, 10);
    let _ = sync.expect(b, 1);
    for _ in 0..10 {
        let _ = sync.arrive(a, Some(buf));
    }
    for _ in 0..8 {
        let _ = sync.wait(a, Some(buf));
    }
    for _ in 0..2 {
        let _ = sync.wait(b, Some(buf));
    }
    let cct_fences = sync.package_fences();
    let broadcast_fences = sync.broadcast_package_fences();
    let cct_lt_broadcast =
        sync.cct_lt_broadcast() && cct_fences == 1 && broadcast_fences == 10 && sync.elided() == 8;
    let same_chiplet_elide = sync.elided() == 8 && sync.softcct().should_elide(buf, a);
    let cross_chiplet_fence = !sync.softcct().should_elide(buf, b) && cct_fences == 1;

    // Incorrect policy would elide the cross-chiplet consume.
    let incorrect_elision_refused = sync.cct().incorrect_elide(buf)
        && !sync.softcct().should_elide(buf, b)
        && !sync.cct().elide_package_fence(buf, b);

    // Single-chiplet CCT is a no-op: same fence count as CCT-off.
    let mut one = SoftChipletSync::new(PartitionId(1));
    one.enable_cct(true);
    one.open(SyncScope::Package);
    let _ = one.expect(a, DEMO_WORKERS_PER_CHIPLET);
    for _ in 0..DEMO_WORKERS_PER_CHIPLET {
        let _ = one.arrive(a, Some(buf));
    }
    let one_wait = one.wait(a, Some(buf));
    let single_chiplet_noop = one.softcct().is_noop()
        && one_wait == Ok(SignalKind::PackageFence)
        && one.package_fences() == 1
        && one.elided() == 0
        && one.broadcast_package_fences() == 1
        && !one.cct_lt_broadcast()
        && !one.softcct().should_elide(buf, a);

    SoftCctReport {
        cct_lt_broadcast,
        cct_fences,
        broadcast_fences,
        same_chiplet_elide,
        cross_chiplet_fence,
        single_chiplet_noop,
        incorrect_elision_refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chiplet_local_signal_is_free() {
        for scope in [SyncScope::Wave, SyncScope::Cu, SyncScope::Chiplet] {
            let mut s = SoftChipletSync::new(PartitionId(1));
            s.open(scope);
            let _ = s.expect(ChipletId(0), 8);
            for _ in 0..8 {
                let f = s.arrive(ChipletId(0), None).unwrap();
                assert_eq!(f.kind, SignalKind::ChipletLocal);
                assert_eq!(scope.naive_package_cost(), 0);
            }
            assert_eq!(s.package_fences(), 0);
            assert_eq!(s.naive_package_fences(), 8);
            assert!(s.package_lt_naive());
            let local = if scope < SyncScope::Chiplet {
                scope
            } else {
                SyncScope::Chiplet
            };
            let id = FenceId(s.timeline(local).retired());
            assert!(s.wait_seq(local, id).unwrap().completed);
        }
    }

    #[test]
    fn package_scope_fence_count_much_less_than_naive() {
        let mut s = SoftChipletSync::new(PartitionId(1));
        s.open(SyncScope::Package);
        s.expect(ChipletId(0), 8).unwrap();
        s.expect(ChipletId(1), 8).unwrap();
        let mut last_kind = SignalKind::ChipletLocal;
        for w in 0..8 {
            let a = s.arrive(ChipletId(0), Some(BufferLabel(7))).unwrap();
            let b = s.arrive(ChipletId(1), Some(BufferLabel(7))).unwrap();
            if w < 7 {
                assert_eq!(a.kind, SignalKind::ChipletLocal);
                assert_eq!(b.kind, SignalKind::ChipletLocal);
            } else {
                assert_eq!(a.kind, SignalKind::PackageFence);
                assert_eq!(b.kind, SignalKind::PackageFence);
                last_kind = b.kind;
            }
        }
        assert_eq!(last_kind, SignalKind::PackageFence);
        assert_eq!(s.package_fences(), 2);
        assert_eq!(s.naive_package_fences(), 16);
        assert!(s.package_lt_naive());
        assert_eq!(s.timeline(SyncScope::Package).retired(), 2);
        assert!(
            s.wait_seq(
                SyncScope::Package,
                FenceId(s.timeline(SyncScope::Package).retired())
            )
            .unwrap()
            .completed
        );
    }

    #[test]
    fn cct_elides_when_last_writer_matches_consumer() {
        let mut s = SoftChipletSync::new(PartitionId(1));
        s.enable_cct(true);
        s.open(SyncScope::Package);
        s.expect(ChipletId(0), 4).unwrap();
        s.expect(ChipletId(1), 1).unwrap();
        for _ in 0..3 {
            assert_eq!(
                s.arrive(ChipletId(0), Some(BufferLabel(3))).unwrap().kind,
                SignalKind::ChipletLocal
            );
        }
        assert_eq!(
            s.arrive(ChipletId(0), Some(BufferLabel(3))).unwrap().kind,
            SignalKind::PendingPackage
        );
        assert_eq!(s.package_fences(), 0);
        assert_eq!(s.cct().last_writer(BufferLabel(3)), Some(ChipletId(0)));
        assert!(s.cct().elide_package_fence(BufferLabel(3), ChipletId(0)));
        assert!(s.softcct().should_elide(BufferLabel(3), ChipletId(0)));
        assert!(!s.softcct().is_noop());
        assert_eq!(
            s.wait(ChipletId(0), Some(BufferLabel(3))).unwrap(),
            SignalKind::Elided
        );
        assert_eq!(s.package_fences(), 0);
        assert_eq!(s.elided(), 1);
        assert_eq!(s.naive_package_fences(), 4);
        assert!(s.package_lt_naive());
    }

    #[test]
    fn cct_cannot_elide_cross_chiplet_consumer() {
        let mut s = SoftChipletSync::new(PartitionId(1));
        s.enable_cct(true);
        s.open(SyncScope::Package);
        s.expect(ChipletId(0), 8).unwrap();
        for _ in 0..8 {
            let _ = s.arrive(ChipletId(0), Some(BufferLabel(9)));
        }
        assert!(!s.cct().elide_package_fence(BufferLabel(9), ChipletId(1)));
        assert_eq!(
            s.wait(ChipletId(1), Some(BufferLabel(9))).unwrap(),
            SignalKind::PackageFence
        );
        assert_eq!(s.package_fences(), 1);
        assert_eq!(s.elided(), 0);
        assert_eq!(s.naive_package_fences(), 8);
        assert!(s.package_lt_naive());
    }

    #[test]
    fn extra_arrive_refused() {
        let mut s = SoftChipletSync::new(PartitionId(1));
        s.open(SyncScope::Chiplet);
        s.expect(ChipletId(0), 1).unwrap();
        s.arrive(ChipletId(0), None).unwrap();
        assert_eq!(
            s.arrive(ChipletId(0), None).unwrap_err(),
            PartitionError::Unbound
        );
        assert_eq!(
            s.expect(ChipletId(9), 1).unwrap_err(),
            PartitionError::OutsideSlice
        );
    }

    #[test]
    fn chipsync_demo_two_chiplet_producer_consumer() {
        let r = run_chipsync_demo();
        assert!(r.package_lt_naive, "hierarchical ≪ naive");
        assert_eq!(r.hierarchical_fences, 2);
        assert_eq!(r.naive_fences, 16);
        assert!(r.cct_elide, "CCT same-chiplet elide");
        assert!(r.two_chiplet, "cross-chiplet cannot elide");
        assert!(r.all_ok());
    }

    #[test]
    fn softcct_package_fence_lt_broadcast_two_chiplet() {
        let r = run_softcct_demo();
        assert!(r.cct_lt_broadcast, "CCT ≪ broadcast");
        assert_eq!(r.cct_fences, 1);
        assert_eq!(r.broadcast_fences, 10);
        assert!(r.same_chiplet_elide);
        assert!(r.cross_chiplet_fence);
        assert!(r.single_chiplet_noop);
        assert!(r.incorrect_elision_refused);
        assert!(r.all_ok());
    }

    #[test]
    fn softcct_incorrect_elision_fails() {
        let mut s = SoftChipletSync::new(PartitionId(1));
        s.enable_cct(true);
        s.open(SyncScope::Package);
        s.expect(ChipletId(0), 4).unwrap();
        s.expect(ChipletId(1), 1).unwrap();
        for _ in 0..4 {
            let _ = s.arrive(ChipletId(0), Some(BufferLabel(5)));
        }
        // Buggy policy: "label is known" would elide chiplet0 → chiplet1.
        assert!(
            s.cct().incorrect_elide(BufferLabel(5)),
            "incorrect policy elides any known label"
        );
        assert!(
            !s.softcct().should_elide(BufferLabel(5), ChipletId(1)),
            "SoftCCT must not elide a cross-chiplet hazard"
        );
        assert_eq!(
            s.wait(ChipletId(1), Some(BufferLabel(5))).unwrap(),
            SignalKind::PackageFence
        );
        assert_eq!(s.elided(), 0);
        assert_eq!(s.package_fences(), 1);
        assert_eq!(s.broadcast_package_fences(), 1);
    }

    #[test]
    fn softcct_single_chiplet_is_noop() {
        let buf = BufferLabel(6);
        let mut off = SoftChipletSync::new(PartitionId(1));
        off.open(SyncScope::Package);
        off.expect(ChipletId(0), 8).unwrap();
        for _ in 0..8 {
            let _ = off.arrive(ChipletId(0), Some(buf));
        }
        assert_eq!(off.package_fences(), 1);

        let mut on = SoftChipletSync::new(PartitionId(1));
        on.enable_cct(true);
        on.open(SyncScope::Package);
        on.expect(ChipletId(0), 8).unwrap();
        for _ in 0..8 {
            let _ = on.arrive(ChipletId(0), Some(buf));
        }
        assert!(on.softcct().is_noop());
        assert!(!on.softcct().should_elide(buf, ChipletId(0)));
        assert_eq!(
            on.wait(ChipletId(0), Some(buf)).unwrap(),
            SignalKind::PackageFence
        );
        assert_eq!(on.package_fences(), 1);
        assert_eq!(on.elided(), 0);
        assert_eq!(on.broadcast_package_fences(), 1);
        assert!(!on.cct_lt_broadcast());
        assert_eq!(on.package_fences(), off.package_fences());
    }

    #[test]
    fn not_chiplet_fleet_placement() {
        // SoftChipletSync is fence domains. ChipletTaskScope (sched) is
        // placement / steal affinity and stays a killed calendar stub.
        let s = SoftChipletSync::new(PartitionId(7));
        assert_eq!(s.partition(), PartitionId(7));
        assert_eq!(s.timeline(SyncScope::Wave).id(), TimelineId(0));
        assert_eq!(s.timeline(SyncScope::Package).id(), TimelineId(3));
        assert!(!s.cct_enabled());
        assert!(s.softcct().is_noop());
        assert!(!s.noi().enabled());
        assert_eq!(s.advertised_is_milli(crate::types::TenantId(1)), None);
    }
}
