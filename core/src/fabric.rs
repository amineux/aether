//! Capability-secured message fabric.
//!
//! The fabric is the *only* IPC in Aether. There are no global ports, no
//! shared-memory "just because you know the PA", and no device MMIO outside
//! an accel-queue cap. Messages carry:
//!   - a destination endpoint
//!   - optional transferred caps (zero-copy tensor grants)
//!   - a chiplet route tag so a future mesh / EMIB / UALink hop can steer
//!     without parsing the payload

use crate::caps::{CapKind, CapRights, CapTable, Capability};
use crate::hodge::{FlowClass, HodgeError, HodgeQuota};
use crate::phase::Phase;
use crate::types::{TenantId, TileId};

pub const MAX_ENDPOINTS: usize = 16;
/// Per-tenant cap on live (not closed) endpoints. A quarter of the shared
/// table, so no single tenant can take every slot. This is a bound, not a
/// reservation: enough distinct tenants can still fill the table together.
pub const MAX_ENDPOINTS_PER_TENANT: usize = 4;
/// Distinct sender tenants the per-tenant Hodge ledger tracks. A sender past
/// this many distinct tenants is refused as `Hodge(QuotaExceeded)`; entries
/// are never freed. This is a bound, not a reservation.
pub const MAX_QUOTA_TENANTS: usize = 16;
pub const MAX_QUEUE: usize = 8;
pub const MAX_MSG_BYTES: usize = 64;
pub const MAX_MSG_CAPS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointId(pub u32);

/// Chiplet-aware routing cookie. v0.1 is a single package; the header is
/// still populated so silicon bring-up does not change the ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChipletRoute {
    pub die: u8,
    pub chiplet: u8,
    pub tile: u8,
    pub hop_hint: u8,
}

impl ChipletRoute {
    pub const LOCAL: Self = Self {
        die: 0,
        chiplet: 0,
        tile: 0,
        hop_hint: 0,
    };

    pub const fn for_tile(tile: TileId) -> Self {
        Self {
            die: 0,
            chiplet: 0,
            tile: tile.0 as u8,
            hop_hint: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsgFlags(pub u16);

impl MsgFlags {
    pub const SYNC: u16 = 1 << 0;
    pub const ASYNC: u16 = 1 << 1;
    pub const GRANT: u16 = 1 << 2;
    pub const REPLY: u16 = 1 << 3;
    /// Gradient-only: use a spanning-tree / reduction-engine offload.
    pub const TREE_OFFLOAD: u16 = 1 << 4;
    /// Curl: reserve ring capacity on the virtual interconnect.
    pub const RING_RESERVE: u16 = 1 << 5;

    pub const fn is_sync(self) -> bool {
        self.0 & Self::SYNC != 0
    }
    pub const fn is_grant(self) -> bool {
        self.0 & Self::GRANT != 0
    }
    pub const fn tree_offload(self) -> bool {
        self.0 & Self::TREE_OFFLOAD != 0
    }
    pub const fn ring_reserve(self) -> bool {
        self.0 & Self::RING_RESERVE != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsgHeader {
    pub dest: EndpointId,
    pub badge: u64,
    pub flags: MsgFlags,
    pub n_caps: u8,
    pub payload_len: u8,
    pub route: ChipletRoute,
    pub sender_tenant: TenantId,
    pub flow: FlowClass,
    /// Named phase (Compute / Exchange / Barrier). Kernel does not fuse these.
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug)]
pub struct Message {
    pub header: MsgHeader,
    pub caps: [Option<Capability>; MAX_MSG_CAPS],
    pub payload: [u8; MAX_MSG_BYTES],
}

impl Message {
    pub fn new(
        dest: EndpointId,
        badge: u64,
        flags: MsgFlags,
        route: ChipletRoute,
        sender_tenant: TenantId,
        data: &[u8],
    ) -> Result<Self, FabricError> {
        if data.len() > MAX_MSG_BYTES {
            return Err(FabricError::PayloadTooLarge);
        }
        let mut payload = [0u8; MAX_MSG_BYTES];
        payload[..data.len()].copy_from_slice(data);
        Ok(Self {
            header: MsgHeader {
                dest,
                badge,
                flags,
                n_caps: 0,
                payload_len: data.len() as u8,
                route,
                sender_tenant,
                flow: FlowClass::Gradient,
                phase: Phase::Compute,
            },
            caps: [None; MAX_MSG_CAPS],
            payload,
        })
    }

    pub fn attach_cap(&mut self, cap: Capability) -> Result<(), FabricError> {
        if self.header.n_caps as usize >= MAX_MSG_CAPS {
            return Err(FabricError::TooManyCaps);
        }
        let i = self.header.n_caps as usize;
        self.caps[i] = Some(cap);
        self.header.n_caps += 1;
        Ok(())
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.header.payload_len as usize]
    }

    pub fn with_flow(mut self, flow: FlowClass) -> Self {
        self.header.flow = flow;
        self
    }

    pub fn with_phase(mut self, phase: Phase) -> Self {
        self.header.phase = phase;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabricError {
    NoSuchEndpoint,
    QueueFull,
    PayloadTooLarge,
    TooManyCaps,
    WouldBlock,
    Closed,
    EndpointLimit,
    Hodge(HodgeError),
}

#[derive(Clone, Copy, Debug)]
struct Endpoint {
    id: EndpointId,
    owner: TenantId,
    queue: [Option<Message>; MAX_QUEUE],
    qhead: usize,
    qlen: usize,
    closed: bool,
}

impl Endpoint {
    fn new(id: EndpointId, owner: TenantId) -> Self {
        Self {
            id,
            owner,
            queue: [None; MAX_QUEUE],
            qhead: 0,
            qlen: 0,
            closed: false,
        }
    }

    fn push(&mut self, msg: Message) -> Result<(), FabricError> {
        if self.closed {
            return Err(FabricError::Closed);
        }
        if self.qlen >= MAX_QUEUE {
            return Err(FabricError::QueueFull);
        }
        let i = (self.qhead + self.qlen) % MAX_QUEUE;
        self.queue[i] = Some(msg);
        self.qlen += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<Message> {
        if self.qlen == 0 {
            return None;
        }
        let msg = self.queue[self.qhead].take();
        self.qhead = (self.qhead + 1) % MAX_QUEUE;
        self.qlen -= 1;
        msg
    }
}

/// Global fabric: endpoint object table + send/recv.
///
/// Tenant-facing paths are [`Self::send_as`], [`Self::recv_as`],
/// [`Self::pending_as`] and [`Self::close_for`]. The `*_unchecked` entry
/// points skip every caller check and exist only for kernel-originated
/// messages, tests and host observers.
///
/// Endpoint ids come from one counter shared by all tenants, so the id a
/// create returns reveals how many endpoints were created before it. The
/// kernel ABI names endpoints by cptr only and never returns the id to user
/// code; host callers of [`Self::create_endpoint`] do see it.
#[derive(Clone, Debug)]
pub struct Fabric {
    eps: [Option<Endpoint>; MAX_ENDPOINTS],
    next_id: u32,
    /// Hodge budget each sender tenant starts with (per tenant, per class).
    pub tenant_budget: HodgeQuota,
    /// Per-sender-tenant remaining Hodge quota; created on first admitted send.
    quotas: [Option<(TenantId, HodgeQuota)>; MAX_QUOTA_TENANTS],
}

impl Fabric {
    pub const fn new() -> Self {
        Self {
            eps: [None; MAX_ENDPOINTS],
            next_id: 1,
            tenant_budget: HodgeQuota::generous(),
            quotas: [None; MAX_QUOTA_TENANTS],
        }
    }

    /// Remaining Hodge quota of sender `tenant` for `flow`.
    pub fn remain_for(&self, tenant: TenantId, flow: FlowClass) -> u32 {
        self.quotas
            .iter()
            .flatten()
            .find(|(t, _)| *t == tenant)
            .map_or(self.tenant_budget.remain(flow), |(_, q)| q.remain(flow))
    }

    /// Charge one message of `flow` to `sender`'s own quota. A refusal
    /// changes nothing (no ledger entry is created on refusal).
    fn admit_for(&mut self, sender: TenantId, flow: FlowClass, tree: bool) -> Result<(), FabricError> {
        let slot = match self.quotas.iter().position(|e| matches!(e, Some((t, _)) if *t == sender)) {
            Some(i) => i,
            None => match self.quotas.iter().position(|e| e.is_none()) {
                Some(i) => {
                    // Policy first, so a refused send creates no entry.
                    let mut q = self.tenant_budget;
                    q.admit(flow, tree).map_err(FabricError::Hodge)?;
                    self.quotas[i] = Some((sender, q));
                    return Ok(());
                }
                None => {
                    // Still apply policy errors (tree offload) before quota.
                    let mut probe = HodgeQuota::generous();
                    probe.admit(flow, tree).map_err(FabricError::Hodge)?;
                    return Err(FabricError::Hodge(HodgeError::QuotaExceeded));
                }
            },
        };
        match &mut self.quotas[slot] {
            Some((_, q)) => q.admit(flow, tree).map_err(FabricError::Hodge),
            None => Err(FabricError::Hodge(HodgeError::QuotaExceeded)),
        }
    }

    /// Create an endpoint owned by `owner`.
    ///
    /// Refused as [`FabricError::EndpointLimit`] when `owner` already holds
    /// [`MAX_ENDPOINTS_PER_TENANT`] live endpoints, when no slot is free, or
    /// when the endpoint-id space is used up. A slot is free when it is empty
    /// or holds a closed endpoint; a closed endpoint keeps answering
    /// [`FabricError::Closed`] until its slot is reused.
    ///
    /// Stale handles: every create issues a fresh [`EndpointId`] from a
    /// counter that never wraps (it refuses instead), and lookups match the
    /// full id. So an id whose slot was reused finds nothing
    /// ([`FabricError::NoSuchEndpoint`]); it can never reach the new owner.
    pub fn create_endpoint(&mut self, owner: TenantId) -> Result<EndpointId, FabricError> {
        if self.live_count(owner) >= MAX_ENDPOINTS_PER_TENANT {
            return Err(FabricError::EndpointLimit);
        }
        let slot = self
            .eps
            .iter()
            .position(|e| e.is_none())
            .or_else(|| self.eps.iter().position(|e| matches!(e, Some(ep) if ep.closed)))
            .ok_or(FabricError::EndpointLimit)?;
        let id = EndpointId(self.next_id);
        self.next_id = self.next_id.checked_add(1).ok_or(FabricError::EndpointLimit)?;
        self.eps[slot] = Some(Endpoint::new(id, owner));
        Ok(id)
    }

    /// Live (not closed) endpoints owned by `owner`.
    pub fn live_count(&self, owner: TenantId) -> usize {
        self.eps
            .iter()
            .flatten()
            .filter(|e| e.owner == owner && !e.closed)
            .count()
    }

    /// Table slot currently holding `id`, if any (host diagnostics / tests).
    pub fn slot_of(&self, id: EndpointId) -> Option<usize> {
        self.eps
            .iter()
            .position(|e| matches!(e, Some(ep) if ep.id == id))
    }

    fn ep_mut(&mut self, id: EndpointId) -> Result<&mut Endpoint, FabricError> {
        self.eps
            .iter_mut()
            .flatten()
            .find(|e| e.id == id)
            .ok_or(FabricError::NoSuchEndpoint)
    }

    fn ep(&self, id: EndpointId) -> Result<&Endpoint, FabricError> {
        self.eps
            .iter()
            .flatten()
            .find(|e| e.id == id)
            .ok_or(FabricError::NoSuchEndpoint)
    }

    /// Unchecked diagnostic: owner of `id`. Reveals whether `id` exists, so
    /// it is not a tenant-facing path (kernel / tests / host observers).
    pub fn owner(&self, id: EndpointId) -> Result<TenantId, FabricError> {
        Ok(self.ep(id)?.owner)
    }

    /// **Unchecked** send: no caller check; the Hodge quota is charged to
    /// `msg.header.sender_tenant` as given. Kernel-originated messages and
    /// tests only. Tenant-facing code uses [`Self::send_as`].
    pub fn send_unchecked(&mut self, msg: Message) -> Result<(), FabricError> {
        let dest = msg.header.dest;
        let flow = msg.header.flow;
        let tree = msg.header.flags.tree_offload();
        {
            let ep = self.ep_mut(dest)?;
            if ep.closed {
                return Err(FabricError::Closed);
            }
            if ep.qlen >= MAX_QUEUE {
                return Err(FabricError::QueueFull);
            }
        }
        self.admit_for(msg.header.sender_tenant, flow, tree)?;
        self.ep_mut(dest)?.push(msg)
    }

    /// Checked send on behalf of `tab`'s owner.
    ///
    /// The message's `sender_tenant` must be the caller, and the caller must
    /// either own the destination endpoint or hold an `Endpoint` cap with
    /// `WRITE` naming it in `tab`. Any failure (forged sender tag, no right,
    /// unknown or stale id) is [`FabricError::NoSuchEndpoint`], so a probe
    /// cannot tell a real endpoint it may not use from a missing one. Then
    /// `Closed` / `QueueFull`, then Hodge admit charged to the caller's own
    /// per-tenant quota.
    pub fn send_as(&mut self, tab: &CapTable, msg: Message) -> Result<(), FabricError> {
        let caller = tab.owner();
        let dest = msg.header.dest;
        if msg.header.sender_tenant != caller {
            return Err(FabricError::NoSuchEndpoint);
        }
        let owner = self.ep(dest)?.owner;
        if owner != caller && !tab.holds_rights(CapKind::Endpoint, dest.0, CapRights::WRITE) {
            return Err(FabricError::NoSuchEndpoint);
        }
        self.send_unchecked(msg)
    }

    /// **Unchecked** recv: anyone naming `id` dequeues. Tests / host only.
    pub fn recv_unchecked(&mut self, id: EndpointId) -> Result<Message, FabricError> {
        self.ep_mut(id)?.pop().ok_or(FabricError::WouldBlock)
    }

    /// Checked recv: only the endpoint's owner may dequeue. Another tenant's
    /// id, an unknown id and a stale id are all [`FabricError::NoSuchEndpoint`].
    pub fn recv_as(&mut self, caller: TenantId, id: EndpointId) -> Result<Message, FabricError> {
        let ep = self.ep_mut(id)?;
        if ep.owner != caller {
            return Err(FabricError::NoSuchEndpoint);
        }
        ep.pop().ok_or(FabricError::WouldBlock)
    }

    /// **Unchecked** queue depth. Tests / host observers only.
    pub fn pending_unchecked(&self, id: EndpointId) -> Result<usize, FabricError> {
        Ok(self.ep(id)?.qlen)
    }

    /// Checked queue depth: owner only, else [`FabricError::NoSuchEndpoint`].
    pub fn pending_as(&self, caller: TenantId, id: EndpointId) -> Result<usize, FabricError> {
        let ep = self.ep(id)?;
        if ep.owner != caller {
            return Err(FabricError::NoSuchEndpoint);
        }
        Ok(ep.qlen)
    }

    /// Close `id`. The endpoint stops admitting sends (`Closed`) and its slot
    /// becomes reusable by the next [`Self::create_endpoint`]. No tenant
    /// check: kernel-trust primitive; tenant-facing paths use
    /// [`Self::close_for`].
    pub fn close(&mut self, id: EndpointId) -> Result<(), FabricError> {
        self.ep_mut(id)?.closed = true;
        Ok(())
    }

    /// Tenant-checked close: only the owner may close. Another tenant's id,
    /// an unknown id and a stale id (slot reused) are all refused as
    /// [`FabricError::NoSuchEndpoint`], so the refusal does not reveal whether
    /// the id exists.
    pub fn close_for(&mut self, caller: TenantId, id: EndpointId) -> Result<(), FabricError> {
        let ep = self.ep_mut(id)?;
        if ep.owner != caller {
            return Err(FabricError::NoSuchEndpoint);
        }
        ep.closed = true;
        Ok(())
    }
}

impl Default for Fabric {
    fn default() -> Self {
        Self::new()
    }
}

/// Host red-team report for fabric endpoint flood refuse.
///
/// Sell line `[redteam] attack=fabric-queue-full` — existing
/// [`Fabric::send_as`] gates only (B holds an Endpoint+WRITE cap on A's
/// endpoints). A sender that floods one endpoint past [`MAX_QUEUE`] is refused
/// as [`FabricError::QueueFull`]; a send to a closed endpoint is refused as
/// [`FabricError::Closed`]. Both gates run **before** Hodge admit, so a refused
/// send enqueues nothing and burns no Hodge quota (the sender's own
/// per-tenant budget since Round 25); this clip claims endpoint
/// back-pressure only. **Not** hodge-quota (`QuotaExceeded`) / Hodge
/// policy / CapTable; no new opcodes; software path only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricQueueFullReport {
    /// Control: `MAX_QUEUE` sends to one endpoint all admit.
    pub fill_ok: bool,
    /// Send `MAX_QUEUE + 1` → `QueueFull`; pending stays `MAX_QUEUE`.
    pub flood_refused: bool,
    /// Refused sends burn no Hodge quota (exactly `MAX_QUEUE` charged).
    pub no_quota_burn: bool,
    /// A neighbor endpoint still admits while the flooded one is full.
    pub neighbor_ok: bool,
    /// Drain one message → the next send admits again (back-pressure, not wedge).
    pub drain_readmits: bool,
    /// Closed endpoint → `Closed`; nothing enqueued, no quota charged.
    pub closed_refused: bool,
}

impl FabricQueueFullReport {
    pub fn all_ok(&self) -> bool {
        self.fill_ok
            && self.flood_refused
            && self.no_quota_burn
            && self.neighbor_ok
            && self.drain_readmits
            && self.closed_refused
    }
}

/// Caller cap table holding `Endpoint` + `WRITE` caps on `eps` (what the
/// kernel would mint when an endpoint owner grants send rights).
fn send_tab(t: TenantId, eps: &[EndpointId]) -> CapTable {
    let mut tab = CapTable::new(t);
    for ep in eps {
        let _ = tab.mint(Capability::new(CapKind::Endpoint, CapRights(CapRights::WRITE), ep.0, t));
    }
    tab
}

fn flood_msg(dest: EndpointId, tenant: TenantId) -> Option<Message> {
    Message::new(
        dest,
        0,
        MsgFlags(MsgFlags::ASYNC),
        ChipletRoute::LOCAL,
        tenant,
        b"flood",
    )
    .ok()
}

/// Endpoint flood → [`FabricError::QueueFull`]; closed endpoint →
/// [`FabricError::Closed`]. Refused sends charge no Hodge quota.
pub fn run_fabric_queue_full_demo() -> FabricQueueFullReport {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut f = Fabric::new();
    let (Ok(victim), Ok(neighbor), Ok(shut)) = (
        f.create_endpoint(a),
        f.create_endpoint(b),
        f.create_endpoint(a),
    ) else {
        return FabricQueueFullReport {
            fill_ok: false,
            flood_refused: false,
            no_quota_burn: false,
            neighbor_ok: false,
            drain_readmits: false,
            closed_refused: false,
        };
    };
    let flow = FlowClass::Gradient;
    let (tab_a, tab_b) = (send_tab(a, &[neighbor]), send_tab(b, &[victim, shut]));
    let start = f.remain_for(b, flow);

    let mut fill_ok = true;
    for _ in 0..MAX_QUEUE {
        fill_ok &= flood_msg(victim, b).map(|m| f.send_as(&tab_b, m)) == Some(Ok(()));
    }
    fill_ok &= f.pending_as(a, victim) == Ok(MAX_QUEUE);
    let after_fill = f.remain_for(b, flow);

    let mut flood_refused = true;
    for _ in 0..3 {
        flood_refused &=
            flood_msg(victim, b).map(|m| f.send_as(&tab_b, m)) == Some(Err(FabricError::QueueFull));
    }
    flood_refused &= f.pending_as(a, victim) == Ok(MAX_QUEUE);
    let no_quota_burn =
        after_fill + MAX_QUEUE as u32 == start && f.remain_for(b, flow) == after_fill;

    let neighbor_ok = flood_msg(neighbor, a).map(|m| f.send_as(&tab_a, m)) == Some(Ok(()))
        && f.pending_as(b, neighbor) == Ok(1);

    let drained = f.recv_as(a, victim).is_ok();
    let drain_readmits = drained
        && flood_msg(victim, b).map(|m| f.send_as(&tab_b, m)) == Some(Ok(()))
        && f.pending_as(a, victim) == Ok(MAX_QUEUE);

    let before_close = f.remain_for(b, flow);
    let closed_refused = f.close_for(a, shut).is_ok()
        && flood_msg(shut, b).map(|m| f.send_as(&tab_b, m)) == Some(Err(FabricError::Closed))
        && f.pending_as(a, shut) == Ok(0)
        && f.remain_for(b, flow) == before_close;

    FabricQueueFullReport {
        fill_ok,
        flood_refused,
        no_quota_burn,
        neighbor_ok,
        drain_readmits,
        closed_refused,
    }
}

/// Host red-team report for oversized fabric message refuse.
///
/// Sell lines `[redteam] attack=fabric-payload-too-large` and
/// `[redteam] attack=fabric-too-many-caps` — existing [`Message::new`] /
/// [`Message::attach_cap`] gates only. A payload longer than
/// [`MAX_MSG_BYTES`] is refused as [`FabricError::PayloadTooLarge`] (never
/// truncated); a cap past [`MAX_MSG_CAPS`] is refused as
/// [`FabricError::TooManyCaps`] with the message's existing caps untouched.
/// A maximal message (exactly `MAX_MSG_BYTES` bytes, `MAX_MSG_CAPS` caps)
/// still admits and round-trips intact. **Not** fabric-queue-full
/// (`QueueFull`) / hodge-quota / CapTable; no new opcodes; no ABI or wire
/// change; software path only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricOversizedMsgReport {
    /// Control: a payload of exactly `MAX_MSG_BYTES` builds.
    pub max_payload_ok: bool,
    /// `MAX_MSG_BYTES + 1` and a 4 KiB payload → `PayloadTooLarge`.
    pub payload_refused: bool,
    /// Control: `MAX_MSG_CAPS` caps attach.
    pub max_caps_ok: bool,
    /// Cap `MAX_MSG_CAPS + 1` → `TooManyCaps`; `n_caps` and stored caps unchanged.
    pub caps_refused: bool,
    /// The maximal message sends and receives with payload and caps intact.
    pub roundtrip_ok: bool,
}

impl FabricOversizedMsgReport {
    pub fn payload_too_large_ok(&self) -> bool {
        self.max_payload_ok && self.payload_refused && self.roundtrip_ok
    }

    pub fn too_many_caps_ok(&self) -> bool {
        self.max_caps_ok && self.caps_refused && self.roundtrip_ok
    }

    pub fn all_ok(&self) -> bool {
        self.payload_too_large_ok() && self.too_many_caps_ok()
    }
}

/// Oversized payload → [`FabricError::PayloadTooLarge`]; cap past
/// [`MAX_MSG_CAPS`] → [`FabricError::TooManyCaps`]. A maximal message admits.
pub fn run_fabric_oversized_msg_demo() -> FabricOversizedMsgReport {
    use crate::caps::{CapKind, CapRights};

    let t = TenantId(1);
    let mk = |dest: EndpointId, data: &[u8]| {
        Message::new(
            dest,
            0,
            MsgFlags(MsgFlags::ASYNC | MsgFlags::GRANT),
            ChipletRoute::LOCAL,
            t,
            data,
        )
    };
    let mut f = Fabric::new();
    let Ok(ep) = f.create_endpoint(t) else {
        return FabricOversizedMsgReport {
            max_payload_ok: false,
            payload_refused: false,
            max_caps_ok: false,
            caps_refused: false,
            roundtrip_ok: false,
        };
    };

    let mut full = [0u8; MAX_MSG_BYTES];
    for (i, b) in full.iter_mut().enumerate() {
        *b = i as u8 ^ 0x5A;
    }
    let max_msg = mk(ep, &full);
    let max_payload_ok = matches!(&max_msg, Ok(m) if m.payload() == &full[..]);

    let over = [0xA5u8; MAX_MSG_BYTES + 1];
    let huge = [0xA5u8; 4096];
    let payload_refused = mk(ep, &over).err() == Some(FabricError::PayloadTooLarge)
        && mk(ep, &huge).err() == Some(FabricError::PayloadTooLarge);

    let cap = |obj: u32| Capability::new(CapKind::Memory, CapRights(CapRights::READ), obj, t);
    let (mut max_caps_ok, mut caps_refused, mut roundtrip_ok) = (false, false, false);
    if let Ok(mut m) = max_msg {
        max_caps_ok = (0..MAX_MSG_CAPS as u32).all(|i| m.attach_cap(cap(100 + i)).is_ok())
            && m.header.n_caps as usize == MAX_MSG_CAPS;
        let before = m.caps;
        caps_refused = m.attach_cap(cap(999)) == Err(FabricError::TooManyCaps)
            && m.attach_cap(cap(998)) == Err(FabricError::TooManyCaps)
            && m.header.n_caps as usize == MAX_MSG_CAPS
            && m.caps == before
            && !m.caps.iter().flatten().any(|c| c.object >= 998);
        let tab = CapTable::new(t);
        roundtrip_ok = f.pending_as(t, ep) == Ok(0)
            && f.send_as(&tab, m).is_ok()
            && match f.recv_as(t, ep) {
                Ok(r) => {
                    r.payload() == &full[..]
                        && r.header.n_caps as usize == MAX_MSG_CAPS
                        && r.caps == before
                }
                Err(_) => false,
            };
    }

    FabricOversizedMsgReport {
        max_payload_ok,
        payload_refused,
        max_caps_ok,
        caps_refused,
        roundtrip_ok,
    }
}

/// Host red-team report for fabric endpoint-table exhaustion.
///
/// Sell line `[redteam] attack=fabric-endpoint-limit`. It uses only the
/// [`Fabric::create_endpoint`] gate. Four tenants each take their
/// [`MAX_ENDPOINTS_PER_TENANT`] endpoints, filling all [`MAX_ENDPOINTS`]
/// slots; a fifth tenant's create is then refused as
/// [`FabricError::EndpointLimit`]. The refusal adds no partial endpoint, and
/// every existing endpoint keeps its owner and still round-trips a message.
///
/// Honesty: the table is still **global** (per [`Fabric`]). The per-tenant
/// quota stops one tenant from taking every slot, but enough distinct tenants
/// together can still fill it; that is a resource bound, **not** a per-tenant
/// reservation. Closing an endpoint frees its slot (checked here). **Not**
/// fabric-queue-full (`QueueFull`) / payload-too-large / hodge-quota /
/// CapTable; no new opcodes; software path only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricEndpointLimitReport {
    /// Control: `MAX_ENDPOINTS` creates across four tenants admit, distinct ids.
    pub fill_ok: bool,
    /// Create on a full table → `EndpointLimit` (repeated), nothing added.
    pub limit_refused: bool,
    /// After the refusals every endpoint keeps its owner and round-trips.
    pub existing_intact: bool,
    /// Honesty check: the table is shared, so a full table also refuses a
    /// tenant that holds no endpoint at all.
    pub table_is_global: bool,
    /// Closing one endpoint frees its slot: the refused tenant now admits.
    pub close_frees_slot: bool,
}

impl FabricEndpointLimitReport {
    pub fn all_ok(&self) -> bool {
        self.fill_ok
            && self.limit_refused
            && self.existing_intact
            && self.table_is_global
            && self.close_frees_slot
    }
}

/// Endpoint table full → [`FabricError::EndpointLimit`]; existing endpoints
/// keep working; close frees a slot. The table is global, so this is a
/// bound, not isolation.
pub fn run_fabric_endpoint_limit_demo() -> FabricEndpointLimitReport {
    let owners = |i: usize| TenantId(1 + (i / MAX_ENDPOINTS_PER_TENANT) as u32);
    let late = TenantId(99);
    let mut f = Fabric::new();
    let mut eps = [EndpointId(0); MAX_ENDPOINTS];
    let mut fill_ok = true;
    for (i, slot) in eps.iter_mut().enumerate() {
        match f.create_endpoint(owners(i)) {
            Ok(id) => *slot = id,
            Err(_) => fill_ok = false,
        }
    }
    fill_ok &= eps
        .iter()
        .enumerate()
        .all(|(i, x)| x.0 != 0 && eps[..i].iter().all(|y| y != x));

    // Owner 1 is at quota; a fresh tenant hits the full table.
    let limit_refused = (0..3).all(|_| f.create_endpoint(late) == Err(FabricError::EndpointLimit))
        && f.live_count(late) == 0;

    let existing_intact = fill_ok
        && eps.iter().enumerate().all(|(i, &ep)| {
            f.owner(ep) == Ok(owners(i))
                && flood_msg(ep, owners(i)).map(|m| f.send_as(&CapTable::new(owners(i)), m)) == Some(Ok(()))
                && f.recv_as(owners(i), ep).map(|m| m.payload() == b"flood") == Ok(true)
                && f.pending_as(owners(i), ep) == Ok(0)
        });

    let table_is_global = f.create_endpoint(TenantId(100)) == Err(FabricError::EndpointLimit);

    let close_frees_slot = f.close_for(owners(0), eps[0]).is_ok()
        && matches!(f.create_endpoint(late), Ok(n) if f.slot_of(n) == Some(0) && f.owner(n) == Ok(late))
        && f.owner(eps[0]) == Err(FabricError::NoSuchEndpoint);

    FabricEndpointLimitReport {
        fill_ok,
        limit_refused,
        existing_intact,
        table_is_global,
        close_frees_slot,
    }
}

/// Host red-team report for one tenant exhausting the endpoint table.
///
/// Sell line `[redteam] attack=fabric-slot-exhaust`. Before this fix the
/// table was shared with no per-tenant bound and `close` never freed a slot,
/// so one tenant could take all [`MAX_ENDPOINTS`] slots (or churn
/// create/close until none were left) and lock every other tenant out. Now a
/// tenant past [`MAX_ENDPOINTS_PER_TENANT`] live endpoints is refused as the
/// existing [`FabricError::EndpointLimit`], a refusal consumes no slot, and
/// closed endpoints free their slots. **Not** a reservation (many tenants
/// together can still fill the table) / fabric-endpoint-limit (table full) /
/// hodge-quota; no new opcodes, errors or ABI; software path only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricSlotExhaustReport {
    /// Control: the attacker's first `MAX_ENDPOINTS_PER_TENANT` creates admit.
    pub quota_fill_ok: bool,
    /// Further creates → `EndpointLimit` (repeated); no slot consumed.
    pub over_quota_refused: bool,
    /// Other tenants still create endpoints after the attacker is refused.
    pub others_admit: bool,
    /// 1000 create/close cycles never leak a slot or lift the quota.
    pub churn_no_leak: bool,
    /// The attacker cannot close another tenant's endpoint (`NoSuchEndpoint`).
    pub foreign_close_refused: bool,
}

impl FabricSlotExhaustReport {
    pub fn all_ok(&self) -> bool {
        self.quota_fill_ok
            && self.over_quota_refused
            && self.others_admit
            && self.churn_no_leak
            && self.foreign_close_refused
    }
}

fn free_slots(f: &Fabric) -> usize {
    f.eps.iter().filter(|e| e.as_ref().map_or(true, |ep| ep.closed)).count()
}

/// One tenant tries to take every endpoint slot → [`FabricError::EndpointLimit`]
/// at its quota; other tenants keep admitting.
pub fn run_fabric_slot_exhaust_demo() -> FabricSlotExhaustReport {
    let atk = TenantId(3);
    let a = TenantId(1);
    let b = TenantId(2);
    let mut f = Fabric::new();
    let mut held = [EndpointId(0); MAX_ENDPOINTS_PER_TENANT];
    let mut quota_fill_ok = true;
    for h in held.iter_mut() {
        match f.create_endpoint(atk) {
            Ok(id) => *h = id,
            Err(_) => quota_fill_ok = false,
        }
    }
    quota_fill_ok &= f.live_count(atk) == MAX_ENDPOINTS_PER_TENANT;

    let free_before = free_slots(&f);
    let over_quota_refused = (0..MAX_ENDPOINTS)
        .all(|_| f.create_endpoint(atk) == Err(FabricError::EndpointLimit))
        && f.live_count(atk) == MAX_ENDPOINTS_PER_TENANT
        && free_slots(&f) == free_before
        && free_before == MAX_ENDPOINTS - MAX_ENDPOINTS_PER_TENANT;

    let (ea, eb) = (f.create_endpoint(a), f.create_endpoint(b));
    let others_admit = matches!((ea, eb), (Ok(x), Ok(y)) if f.owner(x) == Ok(a) && f.owner(y) == Ok(b));

    // Churn: close one, create one, 1000 times. Quota and free count hold.
    let mut churn_no_leak = quota_fill_ok;
    let free_mid = free_slots(&f);
    for i in 0..1000usize {
        let k = i % MAX_ENDPOINTS_PER_TENANT;
        churn_no_leak &= f.close_for(atk, held[k]).is_ok();
        match f.create_endpoint(atk) {
            Ok(id) => held[k] = id,
            Err(_) => churn_no_leak = false,
        }
        churn_no_leak &= f.create_endpoint(atk) == Err(FabricError::EndpointLimit)
            && f.live_count(atk) == MAX_ENDPOINTS_PER_TENANT;
    }
    churn_no_leak &= free_slots(&f) == free_mid
        && matches!(f.create_endpoint(a), Ok(x) if f.owner(x) == Ok(a));

    let foreign_close_refused = match eb {
        Ok(y) => {
            f.close_for(atk, y) == Err(FabricError::NoSuchEndpoint)
                && flood_msg(y, b).map(|m| f.send_as(&CapTable::new(b), m)) == Some(Ok(()))
                && f.pending_as(b, y) == Ok(1)
        }
        Err(_) => false,
    };

    FabricSlotExhaustReport {
        quota_fill_ok,
        over_quota_refused,
        others_admit,
        churn_no_leak,
        foreign_close_refused,
    }
}

/// Host red-team report for a stale endpoint handle after slot reuse.
///
/// Sell line `[redteam] attack=fabric-stale-endpoint`. Tenant A closes an
/// endpoint; its slot is reused by tenant V. A's old [`EndpointId`] is then
/// refused as the existing [`FabricError::NoSuchEndpoint`] on send, recv,
/// pending, owner and close: ids are never reissued (the counter refuses
/// rather than wraps), and lookups match the full id, so the stale handle
/// cannot reach V's endpoint or read its queued message. Refused sends charge
/// no Hodge quota. Before reuse, the closed endpoint answers `Closed`.
/// **Not** fabric-queue-full / fabric-slot-exhaust / CapTable; no new
/// opcodes, errors or ABI; software path only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricStaleEndpointReport {
    /// Before reuse, a send to the closed endpoint → `Closed`.
    pub closed_before_reuse: bool,
    /// V's new endpoint lands in A's old slot, with a different id.
    pub slot_reused: bool,
    /// A's stale id → `NoSuchEndpoint` on send/recv/pending/owner/close.
    pub stale_refused: bool,
    /// V's endpoint is untouched: owner, pending, payload, still open.
    pub new_owner_intact: bool,
    /// Refused stale sends charged no Hodge quota.
    pub no_quota_burn: bool,
}

impl FabricStaleEndpointReport {
    pub fn all_ok(&self) -> bool {
        self.closed_before_reuse
            && self.slot_reused
            && self.stale_refused
            && self.new_owner_intact
            && self.no_quota_burn
    }
}

/// Stale endpoint id after its slot is reused → [`FabricError::NoSuchEndpoint`];
/// the new owner's endpoint is unreachable through it.
pub fn run_fabric_stale_endpoint_demo() -> FabricStaleEndpointReport {
    let a = TenantId(1);
    let v = TenantId(5);
    let mut f = Fabric::new();
    let fail = FabricStaleEndpointReport {
        closed_before_reuse: false,
        slot_reused: false,
        stale_refused: false,
        new_owner_intact: false,
        no_quota_burn: false,
    };
    let Ok(old) = f.create_endpoint(a) else { return fail };
    // Fill the rest of the table so reuse must take A's old slot.
    for i in 1..MAX_ENDPOINTS {
        let owner = TenantId(1 + (i / MAX_ENDPOINTS_PER_TENANT) as u32);
        if f.create_endpoint(owner).is_err() {
            return fail;
        }
    }
    let old_slot = f.slot_of(old);
    let flow = FlowClass::Gradient;

    let (tab_a, tab_v) = (CapTable::new(a), CapTable::new(v));
    let closed_before_reuse = f.close_for(a, old).is_ok()
        && flood_msg(old, a).map(|m| f.send_as(&tab_a, m)) == Some(Err(FabricError::Closed));

    let Ok(fresh) = f.create_endpoint(v) else { return fail };
    let slot_reused = old_slot.is_some() && f.slot_of(fresh) == old_slot && fresh != old;

    let secret = Message::new(fresh, 0x5EC, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, v, b"v-secret");
    let queued = secret.map(|m| f.send_as(&tab_v, m)) == Ok(Ok(()));
    let before = f.remain_for(a, flow);

    let nse = Err(FabricError::NoSuchEndpoint);
    let stale_refused = (0..3).all(|_| flood_msg(old, a).map(|m| f.send_as(&tab_a, m)) == Some(nse))
        && f.recv_as(a, old).map(|_| ()) == nse
        && f.pending_as(a, old).map(|_| ()) == nse
        && f.owner(old).map(|_| ()) == nse
        && f.close_for(a, old) == nse
        && f.close(old) == nse;
    let no_quota_burn = f.remain_for(a, flow) == before;

    let new_owner_intact = queued
        && f.owner(fresh) == Ok(v)
        && f.pending_as(v, fresh) == Ok(1)
        && f.recv_as(v, fresh).map(|m| m.payload() == b"v-secret" && m.header.badge == 0x5EC) == Ok(true)
        && flood_msg(fresh, v).map(|m| f.send_as(&tab_v, m)) == Some(Ok(()));

    FabricStaleEndpointReport {
        closed_before_reuse,
        slot_reused,
        stale_refused,
        new_owner_intact,
        no_quota_burn,
    }
}

/// Host red-team report for receiving from another tenant's endpoint.
///
/// Sell line `[redteam] attack=fabric-recv-foreign`. Tenant B names tenant
/// A's endpoint id on [`Fabric::recv_as`] and [`Fabric::pending_as`]: both
/// are refused as the existing [`FabricError::NoSuchEndpoint`], the same
/// answer an id that does not exist gets, so B learns neither the message nor
/// whether the endpoint exists. A's queued message stays queued and A
/// receives it intact. Before Round 25 `recv` / `pending` had no caller
/// check. **Not** fabric-send-no-cap / fabric-stale-endpoint / CapTable; no
/// new opcodes, errors or ABI; software path only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricRecvForeignReport {
    /// B's recv / pending on A's endpoint → `NoSuchEndpoint` (repeated).
    pub foreign_refused: bool,
    /// The refusal is identical to the one for an id that does not exist.
    pub indistinguishable: bool,
    /// A still has its message queued and receives it intact.
    pub owner_intact: bool,
}

impl FabricRecvForeignReport {
    pub fn all_ok(&self) -> bool {
        self.foreign_refused && self.indistinguishable && self.owner_intact
    }
}

/// B's recv / pending on A's endpoint → [`FabricError::NoSuchEndpoint`].
pub fn run_fabric_recv_foreign_demo() -> FabricRecvForeignReport {
    let a = TenantId(1);
    let b = TenantId(2);
    let mut f = Fabric::new();
    let (Ok(ep_a), Ok(_ep_b)) = (f.create_endpoint(a), f.create_endpoint(b)) else {
        return FabricRecvForeignReport { foreign_refused: false, indistinguishable: false, owner_intact: false };
    };
    let tab_a = CapTable::new(a);
    let queued = Message::new(ep_a, 0xA1, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, a, b"a-private")
        .map(|m| f.send_as(&tab_a, m))
        == Ok(Ok(()));
    let nse = Err(FabricError::NoSuchEndpoint);
    let foreign_refused = queued
        && (0..3).all(|_| f.recv_as(b, ep_a).map(|_| ()) == nse)
        && f.pending_as(b, ep_a).map(|_| ()) == nse;
    let missing = EndpointId(0xDEAD);
    let indistinguishable = f.recv_as(b, missing).map(|_| ()) == f.recv_as(b, ep_a).map(|_| ())
        && f.pending_as(b, missing) == f.pending_as(b, ep_a);
    let owner_intact = f.pending_as(a, ep_a) == Ok(1)
        && f.recv_as(a, ep_a).map(|m| m.payload() == b"a-private" && m.header.sender_tenant == a) == Ok(true)
        && f.pending_as(a, ep_a) == Ok(0);
    FabricRecvForeignReport { foreign_refused, indistinguishable, owner_intact }
}

/// Host red-team report for sending without the right to send.
///
/// Sell line `[redteam] attack=fabric-send-no-cap`. Tenant B sends to tenant
/// A's endpoint through [`Fabric::send_as`] with: no cap, an `Endpoint` cap
/// on another endpoint, an `Endpoint` cap without `WRITE`, and a forged
/// sender tag (A's). Each is refused as the existing
/// [`FabricError::NoSuchEndpoint`] (same answer as a missing id), nothing is
/// queued and neither tenant's Hodge quota moves. With an `Endpoint` +
/// `WRITE` cap on A's endpoint the send admits and is charged to B. Before
/// Round 25 any tenant that knew an id could send. **Not** fabric-recv-foreign
/// / fabric-queue-full / CapTable; no new opcodes, errors or ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricSendNoCapReport {
    /// No cap / wrong object / no WRITE / forged tag → `NoSuchEndpoint`.
    pub unauthorized_refused: bool,
    /// Nothing queued; A's and B's quotas unchanged.
    pub no_side_effect: bool,
    /// Control: with Endpoint+WRITE on A's endpoint, B's send admits,
    /// charged to B's quota only.
    pub granted_ok: bool,
}

impl FabricSendNoCapReport {
    pub fn all_ok(&self) -> bool {
        self.unauthorized_refused && self.no_side_effect && self.granted_ok
    }
}

/// Send to another tenant's endpoint without an Endpoint+WRITE cap →
/// [`FabricError::NoSuchEndpoint`].
pub fn run_fabric_send_no_cap_demo() -> FabricSendNoCapReport {
    let a = TenantId(1);
    let b = TenantId(2);
    let flow = FlowClass::Gradient;
    let mut f = Fabric::new();
    let (Ok(ep_a), Ok(ep_b)) = (f.create_endpoint(a), f.create_endpoint(b)) else {
        return FabricSendNoCapReport { unauthorized_refused: false, no_side_effect: false, granted_ok: false };
    };
    let empty = CapTable::new(b);
    let wrong_obj = send_tab(b, &[ep_b]);
    let mut read_only = CapTable::new(b);
    let _ = read_only.mint(Capability::new(CapKind::Endpoint, CapRights(CapRights::READ), ep_a.0, b));
    let granted = send_tab(b, &[ep_a]);
    let (qa, qb) = (f.remain_for(a, flow), f.remain_for(b, flow));
    let nse = Some(Err(FabricError::NoSuchEndpoint));
    let unauthorized_refused = flood_msg(ep_a, b).map(|m| f.send_as(&empty, m)) == nse
        && flood_msg(ep_a, b).map(|m| f.send_as(&wrong_obj, m)) == nse
        && flood_msg(ep_a, b).map(|m| f.send_as(&read_only, m)) == nse
        // Forged sender tag (A's) with a table B owns, even one holding WRITE.
        && flood_msg(ep_a, a).map(|m| f.send_as(&granted, m)) == nse
        && flood_msg(EndpointId(0xDEAD), b).map(|m| f.send_as(&empty, m)) == nse;
    let no_side_effect = f.pending_as(a, ep_a) == Ok(0)
        && f.remain_for(a, flow) == qa
        && f.remain_for(b, flow) == qb;
    let granted_ok = flood_msg(ep_a, b).map(|m| f.send_as(&granted, m)) == Some(Ok(()))
        && f.pending_as(a, ep_a) == Ok(1)
        && f.remain_for(b, flow) + 1 == qb
        && f.remain_for(a, flow) == qa
        && f.recv_as(a, ep_a).map(|m| m.header.sender_tenant == b) == Ok(true);
    FabricSendNoCapReport { unauthorized_refused, no_side_effect, granted_ok }
}

/// Host red-team report for one sender draining the fabric's Hodge quota.
///
/// Sell line `[redteam] attack=fabric-quota-drain`. Before Round 25 the
/// fabric had one Hodge quota (64 messages per class) shared by every
/// sender, so one tenant could use it up and every other tenant's sends were
/// refused. Now each sender tenant has its own budget per class
/// ([`Fabric::tenant_budget`]): tenant C sends to its own endpoint until its
/// Gradient budget is spent, its next sends are refused as the existing
/// `FabricError::Hodge(HodgeError::QuotaExceeded)`, and tenants A and B still
/// have their full budgets and still send. C's other classes are untouched
/// (per tenant and class). The ledger tracks [`MAX_QUOTA_TENANTS`] distinct
/// senders; past that a new sender is refused as `QuotaExceeded` (a bound,
/// not a reservation). Budgets do not refill. **Not** hodge-quota (empty
/// quota) / fabric-queue-full; no new opcodes, errors or ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FabricQuotaDrainReport {
    /// Control: C's first `tenant_budget` Gradient sends admit.
    pub budget_ok: bool,
    /// Further C Gradient sends → `Hodge(QuotaExceeded)`, nothing queued.
    pub drain_refused: bool,
    /// A and B keep their full budgets and still send.
    pub others_unaffected: bool,
    /// C's Curl budget is separate and still admits.
    pub per_class: bool,
}

impl FabricQuotaDrainReport {
    pub fn all_ok(&self) -> bool {
        self.budget_ok && self.drain_refused && self.others_unaffected && self.per_class
    }
}

/// One sender drains its own Hodge budget → `Hodge(QuotaExceeded)`; other
/// tenants keep theirs.
pub fn run_fabric_quota_drain_demo() -> FabricQuotaDrainReport {
    let (a, b, c) = (TenantId(1), TenantId(2), TenantId(3));
    let flow = FlowClass::Gradient;
    let mut f = Fabric::new();
    let (Ok(ep_a), Ok(ep_b), Ok(ep_c)) = (f.create_endpoint(a), f.create_endpoint(b), f.create_endpoint(c)) else {
        return FabricQuotaDrainReport { budget_ok: false, drain_refused: false, others_unaffected: false, per_class: false };
    };
    let (tab_a, tab_b, tab_c) = (CapTable::new(a), CapTable::new(b), CapTable::new(c));
    let budget = f.tenant_budget.remain(flow);
    let full_a = f.remain_for(a, flow);
    let mut budget_ok = budget > 0;
    for _ in 0..budget {
        budget_ok &= flood_msg(ep_c, c).map(|m| f.send_as(&tab_c, m)) == Some(Ok(()));
        let _ = f.recv_as(c, ep_c);
    }
    budget_ok &= f.remain_for(c, flow) == 0;
    let refused = Some(Err(FabricError::Hodge(HodgeError::QuotaExceeded)));
    let drain_refused = (0..MAX_QUEUE + 3).all(|_| flood_msg(ep_c, c).map(|m| f.send_as(&tab_c, m)) == refused)
        && f.pending_as(c, ep_c) == Ok(0);
    let others_unaffected = f.remain_for(a, flow) == full_a
        && f.remain_for(b, flow) == full_a
        && flood_msg(ep_a, a).map(|m| f.send_as(&tab_a, m)) == Some(Ok(()))
        && flood_msg(ep_b, b).map(|m| f.send_as(&tab_b, m)) == Some(Ok(()))
        && f.remain_for(a, flow) + 1 == full_a;
    let per_class = flood_msg(ep_c, c)
        .map(|m| f.send_as(&tab_c, m.with_flow(FlowClass::Curl)))
        == Some(Ok(()));
    FabricQuotaDrainReport { budget_ok, drain_refused, others_unaffected, per_class }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CPtr;

    #[test]
    fn fabric_recv_foreign_demo_all_ok() {
        let r = run_fabric_recv_foreign_demo();
        assert!(r.foreign_refused, "{r:?}");
        assert!(r.indistinguishable, "{r:?}");
        assert!(r.owner_intact, "{r:?}");
    }

    #[test]
    fn fabric_send_no_cap_demo_all_ok() {
        let r = run_fabric_send_no_cap_demo();
        assert!(r.unauthorized_refused, "{r:?}");
        assert!(r.no_side_effect, "{r:?}");
        assert!(r.granted_ok, "{r:?}");
    }

    #[test]
    fn fabric_quota_drain_demo_all_ok() {
        let r = run_fabric_quota_drain_demo();
        assert!(r.budget_ok, "{r:?}");
        assert!(r.drain_refused, "{r:?}");
        assert!(r.others_unaffected, "{r:?}");
        assert!(r.per_class, "{r:?}");
    }

    /// Regression: `recv` used to dequeue for any caller naming the id.
    #[test]
    fn recv_as_refuses_non_owner() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        let m = Message::new(ep, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, TenantId(1), b"x").unwrap();
        f.send_as(&CapTable::new(TenantId(1)), m).unwrap();
        assert_eq!(f.recv_as(TenantId(2), ep).unwrap_err(), FabricError::NoSuchEndpoint);
        assert_eq!(f.pending_as(TenantId(2), ep), Err(FabricError::NoSuchEndpoint));
        assert_eq!(f.pending_as(TenantId(1), ep), Ok(1));
    }

    /// Regression: `send` used to admit from anyone naming the id.
    #[test]
    fn send_as_requires_owner_or_write_cap() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        let mk = |t| Message::new(ep, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, t, b"x").unwrap();
        assert_eq!(f.send_as(&CapTable::new(TenantId(2)), mk(TenantId(2))), Err(FabricError::NoSuchEndpoint));
        let mut tab = CapTable::new(TenantId(2));
        tab.mint(Capability::new(CapKind::Endpoint, CapRights(CapRights::WRITE), ep.0, TenantId(2))).unwrap();
        assert_eq!(f.send_as(&tab, mk(TenantId(1))), Err(FabricError::NoSuchEndpoint), "forged sender tag");
        assert_eq!(f.send_as(&tab, mk(TenantId(2))), Ok(()));
        let cptr = CPtr(0);
        tab.revoke(cptr).unwrap();
        assert_eq!(f.send_as(&tab, mk(TenantId(2))), Err(FabricError::NoSuchEndpoint), "revoked cap");
    }

    /// Regression: one sender used to drain the single shared Hodge quota.
    #[test]
    fn hodge_quota_is_per_sender_tenant() {
        let mut f = Fabric::new();
        f.tenant_budget = HodgeQuota { remain: [2, 2, 2] };
        let ep = f.create_endpoint(TenantId(3)).unwrap();
        let mine = f.create_endpoint(TenantId(1)).unwrap();
        let tab3 = CapTable::new(TenantId(3));
        for _ in 0..2 {
            let m = Message::new(ep, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, TenantId(3), b"").unwrap();
            f.send_as(&tab3, m).unwrap();
        }
        let m = Message::new(ep, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, TenantId(3), b"").unwrap();
        assert_eq!(f.send_as(&tab3, m), Err(FabricError::Hodge(HodgeError::QuotaExceeded)));
        let m = Message::new(mine, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, TenantId(1), b"").unwrap();
        assert_eq!(f.send_as(&CapTable::new(TenantId(1)), m), Ok(()));
        assert_eq!(f.remain_for(TenantId(1), FlowClass::Gradient), 1);
    }

    /// The ledger tracks MAX_QUOTA_TENANTS senders; a refused send or a
    /// policy refusal creates no entry.
    #[test]
    fn quota_ledger_bound_and_no_entry_on_refusal() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        for t in 0..MAX_QUOTA_TENANTS as u32 {
            let m = Message::new(ep, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, TenantId(100 + t), b"").unwrap();
            f.send_unchecked(m).unwrap();
            f.recv_unchecked(ep).unwrap();
        }
        let m = Message::new(ep, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, TenantId(999), b"").unwrap();
        assert_eq!(f.send_unchecked(m), Err(FabricError::Hodge(HodgeError::QuotaExceeded)));
        let harm = Message::new(ep, 0, MsgFlags(MsgFlags::TREE_OFFLOAD), ChipletRoute::LOCAL, TenantId(999), b"")
            .unwrap()
            .with_flow(FlowClass::Harmonic);
        assert_eq!(f.send_unchecked(harm), Err(FabricError::Hodge(HodgeError::HarmonicTreeReduce)));
        let mut g = Fabric::new();
        g.tenant_budget = HodgeQuota::empty();
        let ep = g.create_endpoint(TenantId(1)).unwrap();
        let m = Message::new(ep, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, TenantId(1), b"").unwrap();
        assert!(g.send_unchecked(m).is_err());
        assert!(g.quotas.iter().all(|e| e.is_none()));
    }

    #[test]
    fn fabric_endpoint_limit_demo_all_ok() {
        let r = run_fabric_endpoint_limit_demo();
        assert!(r.fill_ok, "MAX_ENDPOINTS creates admit: {r:?}");
        assert!(r.limit_refused, "create past MAX_ENDPOINTS → EndpointLimit");
        assert!(r.existing_intact, "existing endpoints keep owner and round-trip");
        assert!(r.table_is_global, "table is shared: other tenant also EndpointLimit");
        assert!(r.close_frees_slot, "close frees a slot for another tenant");
        assert!(r.all_ok());
    }

    #[test]
    fn create_endpoint_past_limit_is_endpoint_limit() {
        let mut f = Fabric::new();
        for i in 0..MAX_ENDPOINTS {
            f.create_endpoint(TenantId(1 + (i / MAX_ENDPOINTS_PER_TENANT) as u32)).unwrap();
        }
        assert_eq!(f.create_endpoint(TenantId(77)), Err(FabricError::EndpointLimit));
    }

    #[test]
    fn fabric_slot_exhaust_demo_all_ok() {
        let r = run_fabric_slot_exhaust_demo();
        assert!(r.quota_fill_ok, "quota creates admit: {r:?}");
        assert!(r.over_quota_refused, "past quota → EndpointLimit, no slot consumed");
        assert!(r.others_admit, "other tenants still create");
        assert!(r.churn_no_leak, "create/close churn leaks no slot");
        assert!(r.foreign_close_refused, "close_for on a foreign endpoint → NoSuchEndpoint");
        assert!(r.all_ok());
    }

    #[test]
    fn fabric_stale_endpoint_demo_all_ok() {
        let r = run_fabric_stale_endpoint_demo();
        assert!(r.closed_before_reuse, "closed endpoint → Closed before reuse: {r:?}");
        assert!(r.slot_reused, "new endpoint reuses the closed slot, new id");
        assert!(r.stale_refused, "stale id → NoSuchEndpoint everywhere");
        assert!(r.new_owner_intact, "new owner's endpoint untouched");
        assert!(r.no_quota_burn, "stale sends charge no Hodge quota");
        assert!(r.all_ok());
    }

    /// Regression: one tenant used to be able to take all 16 slots.
    #[test]
    fn one_tenant_cannot_take_every_slot() {
        let mut f = Fabric::new();
        let atk = TenantId(9);
        let mut ok = 0;
        for _ in 0..MAX_ENDPOINTS {
            if f.create_endpoint(atk).is_ok() {
                ok += 1;
            }
        }
        assert_eq!(ok, MAX_ENDPOINTS_PER_TENANT);
        assert_eq!(f.create_endpoint(atk), Err(FabricError::EndpointLimit));
        for t in 1..=3u32 {
            for _ in 0..MAX_ENDPOINTS_PER_TENANT {
                f.create_endpoint(TenantId(t)).unwrap();
            }
        }
    }

    /// Regression: close used to keep its slot forever, so create/close
    /// churn drained the table.
    #[test]
    fn close_frees_slot_under_churn() {
        let mut f = Fabric::new();
        let t = TenantId(1);
        for _ in 0..10 * MAX_ENDPOINTS {
            let ep = f.create_endpoint(t).unwrap();
            f.close_for(t, ep).unwrap();
        }
        assert_eq!(f.live_count(t), 0);
        for i in 0..MAX_ENDPOINTS {
            f.create_endpoint(TenantId(2 + (i / MAX_ENDPOINTS_PER_TENANT) as u32)).unwrap();
        }
    }

    #[test]
    fn stale_id_cannot_reach_reused_slot() {
        let mut f = Fabric::new();
        let a = TenantId(1);
        let v = TenantId(2);
        let old = f.create_endpoint(a).unwrap();
        // Empty slots are taken first; fill them so reuse must hit slot 0.
        for i in 1..MAX_ENDPOINTS {
            f.create_endpoint(TenantId(10 + (i / MAX_ENDPOINTS_PER_TENANT) as u32)).unwrap();
        }
        f.close_for(a, old).unwrap();
        let fresh = f.create_endpoint(v).unwrap();
        assert_eq!(f.slot_of(fresh), Some(0));
        assert_ne!(fresh, old);
        let m = Message::new(old, 0, MsgFlags(MsgFlags::ASYNC), ChipletRoute::LOCAL, a, b"x").unwrap();
        assert_eq!(f.send_unchecked(m), Err(FabricError::NoSuchEndpoint));
        assert_eq!(f.recv_unchecked(old).unwrap_err(), FabricError::NoSuchEndpoint);
        assert_eq!(f.close_for(a, old), Err(FabricError::NoSuchEndpoint));
        assert_eq!(f.pending_unchecked(fresh), Ok(0));
        assert_eq!(f.owner(fresh), Ok(v));
    }

    #[test]
    fn close_for_refuses_foreign_owner() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        assert_eq!(f.close_for(TenantId(2), ep), Err(FabricError::NoSuchEndpoint));
        assert_eq!(f.live_count(TenantId(1)), 1);
        f.close_for(TenantId(1), ep).unwrap();
        assert_eq!(f.live_count(TenantId(1)), 0);
    }

    /// The id counter refuses instead of wrapping, so an id is never reissued.
    #[test]
    fn endpoint_id_never_wraps() {
        let mut f = Fabric::new();
        f.next_id = u32::MAX - 1;
        let last = f.create_endpoint(TenantId(1)).unwrap();
        assert_eq!(last, EndpointId(u32::MAX - 1));
        f.close(last).unwrap();
        assert_eq!(f.create_endpoint(TenantId(1)), Err(FabricError::EndpointLimit));
        assert_eq!(f.create_endpoint(TenantId(1)), Err(FabricError::EndpointLimit));
        assert_eq!(f.slot_of(last), Some(0));
    }

    #[test]
    fn fabric_oversized_msg_demo_names_both_refusals() {
        let r = run_fabric_oversized_msg_demo();
        assert!(r.max_payload_ok, "MAX_MSG_BYTES payload builds: {r:?}");
        assert!(r.payload_refused, "payload past MAX_MSG_BYTES → PayloadTooLarge");
        assert!(r.max_caps_ok, "MAX_MSG_CAPS caps attach");
        assert!(r.caps_refused, "cap past MAX_MSG_CAPS → TooManyCaps, caps unchanged");
        assert!(r.roundtrip_ok, "maximal message round-trips intact");
        assert!(r.all_ok());
    }

    #[test]
    fn attach_cap_past_limit_is_too_many_caps() {
        let t = TenantId(1);
        let mut m = Message::new(
            EndpointId(1),
            0,
            MsgFlags(MsgFlags::GRANT),
            ChipletRoute::LOCAL,
            t,
            b"g",
        )
        .unwrap();
        let c = Capability::new(CapKind::Memory, CapRights(CapRights::READ), 7, t);
        for _ in 0..MAX_MSG_CAPS {
            m.attach_cap(c).unwrap();
        }
        assert_eq!(m.attach_cap(c).unwrap_err(), FabricError::TooManyCaps);
        assert_eq!(m.header.n_caps as usize, MAX_MSG_CAPS);
    }

    #[test]
    fn fabric_queue_full_demo_all_ok() {
        let r = run_fabric_queue_full_demo();
        assert!(r.fill_ok, "MAX_QUEUE sends admit: {r:?}");
        assert!(r.flood_refused, "send past MAX_QUEUE → QueueFull");
        assert!(r.no_quota_burn, "refused sends charge no Hodge quota");
        assert!(r.neighbor_ok, "neighbor endpoint still admits");
        assert!(r.drain_readmits, "drain one → next send admits");
        assert!(r.closed_refused, "closed endpoint → Closed, nothing enqueued");
        assert!(r.all_ok());
    }

    #[test]
    fn send_recv_roundtrip() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        let msg = Message::new(
            ep,
            0xA3,
            MsgFlags(MsgFlags::ASYNC),
            ChipletRoute::LOCAL,
            TenantId(1),
            b"hello",
        )
        .unwrap();
        f.send_unchecked(msg).unwrap();
        let got = f.recv_unchecked(ep).unwrap();
        assert_eq!(got.payload(), b"hello");
        assert_eq!(got.header.badge, 0xA3);
        assert_eq!(got.header.route, ChipletRoute::LOCAL);
    }

    #[test]
    fn sync_flag_preserved() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        let msg = Message::new(
            ep,
            1,
            MsgFlags(MsgFlags::SYNC),
            ChipletRoute::for_tile(TileId(3)),
            TenantId(1),
            &[],
        )
        .unwrap();
        f.send_unchecked(msg).unwrap();
        let got = f.recv_unchecked(ep).unwrap();
        assert!(got.header.flags.is_sync());
        assert_eq!(got.header.route.tile, 3);
    }

    #[test]
    fn cap_grant_in_message() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(2)).unwrap();
        let mut msg = Message::new(
            ep,
            0,
            MsgFlags(MsgFlags::GRANT | MsgFlags::ASYNC),
            ChipletRoute::LOCAL,
            TenantId(1),
            b"tensor",
        )
        .unwrap();
        msg.attach_cap(
            Capability::new(CapKind::Memory, CapRights::MEM_FULL, 7, TenantId(2))
                .with_generation(1),
        )
        .unwrap();
        f.send_unchecked(msg).unwrap();
        let got = f.recv_unchecked(ep).unwrap();
        assert_eq!(got.header.n_caps, 1);
        assert_eq!(got.caps[0].unwrap().object, 7);
    }

    #[test]
    fn queue_full_and_empty() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        for _ in 0..MAX_QUEUE {
            f.send_unchecked(
                Message::new(
                    ep,
                    0,
                    MsgFlags(MsgFlags::ASYNC),
                    ChipletRoute::LOCAL,
                    TenantId(1),
                    &[],
                )
                .unwrap(),
            )
            .unwrap();
        }
        assert_eq!(
            f.send_unchecked(
                Message::new(
                    ep,
                    0,
                    MsgFlags(MsgFlags::ASYNC),
                    ChipletRoute::LOCAL,
                    TenantId(1),
                    &[],
                )
                .unwrap()
            )
            .unwrap_err(),
            FabricError::QueueFull
        );
        for _ in 0..MAX_QUEUE {
            f.recv_unchecked(ep).unwrap();
        }
        assert_eq!(f.recv_unchecked(ep).unwrap_err(), FabricError::WouldBlock);
    }

    #[test]
    fn payload_limit() {
        let big = [0u8; MAX_MSG_BYTES + 1];
        assert_eq!(
            Message::new(
                EndpointId(1),
                0,
                MsgFlags(0),
                ChipletRoute::LOCAL,
                TenantId(1),
                &big
            )
            .unwrap_err(),
            FabricError::PayloadTooLarge
        );
    }

    #[test]
    fn hodge_refuses_harmonic_tree() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        let msg = Message::new(
            ep,
            0,
            MsgFlags(MsgFlags::ASYNC | MsgFlags::TREE_OFFLOAD),
            ChipletRoute::LOCAL,
            TenantId(1),
            b"harm",
        )
        .unwrap()
        .with_flow(crate::hodge::FlowClass::Harmonic);
        assert_eq!(
            f.send_unchecked(msg).unwrap_err(),
            FabricError::Hodge(crate::hodge::HodgeError::HarmonicTreeReduce)
        );
        assert_eq!(f.pending_unchecked(ep).unwrap(), 0);
    }

    #[test]
    fn unknown_endpoint() {
        let mut f = Fabric::new();
        let msg = Message::new(
            EndpointId(99),
            0,
            MsgFlags(MsgFlags::ASYNC),
            ChipletRoute::LOCAL,
            TenantId(1),
            &[],
        )
        .unwrap();
        assert_eq!(f.send_unchecked(msg).unwrap_err(), FabricError::NoSuchEndpoint);
    }
}
