//! Capability-secured message fabric.
//!
//! The fabric is the *only* IPC in Aether. There are no global ports, no
//! shared-memory "just because you know the PA", and no device MMIO outside
//! an accel-queue cap. Messages carry:
//!   - a destination endpoint
//!   - optional transferred caps (zero-copy tensor grants)
//!   - a chiplet route tag so a future mesh / EMIB / UALink hop can steer
//!     without parsing the payload

use crate::caps::Capability;
use crate::hodge::{FlowClass, HodgeError, HodgeQuota};
use crate::phase::Phase;
use crate::types::{TenantId, TileId};

pub const MAX_ENDPOINTS: usize = 16;
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
#[derive(Clone, Debug)]
pub struct Fabric {
    eps: [Option<Endpoint>; MAX_ENDPOINTS],
    next_id: u32,
    pub hodge: HodgeQuota,
}

impl Fabric {
    pub const fn new() -> Self {
        Self {
            eps: [None; MAX_ENDPOINTS],
            next_id: 1,
            hodge: HodgeQuota::generous(),
        }
    }

    pub fn create_endpoint(&mut self, owner: TenantId) -> Result<EndpointId, FabricError> {
        let slot = self
            .eps
            .iter()
            .position(|e| e.is_none())
            .ok_or(FabricError::EndpointLimit)?;
        let id = EndpointId(self.next_id);
        self.next_id += 1;
        self.eps[slot] = Some(Endpoint::new(id, owner));
        Ok(id)
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

    pub fn owner(&self, id: EndpointId) -> Result<TenantId, FabricError> {
        Ok(self.ep(id)?.owner)
    }

    /// Admit Hodge policy/quota on the virtual link, then enqueue.
    pub fn send(&mut self, msg: Message) -> Result<(), FabricError> {
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
        self.hodge.admit(flow, tree).map_err(FabricError::Hodge)?;
        self.ep_mut(dest)?.push(msg)
    }

    pub fn recv(&mut self, id: EndpointId) -> Result<Message, FabricError> {
        self.ep_mut(id)?.pop().ok_or(FabricError::WouldBlock)
    }

    pub fn pending(&self, id: EndpointId) -> Result<usize, FabricError> {
        Ok(self.ep(id)?.qlen)
    }

    pub fn close(&mut self, id: EndpointId) -> Result<(), FabricError> {
        self.ep_mut(id)?.closed = true;
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
/// Sell line `[redteam] attack=fabric-queue-full` — existing [`Fabric::send`]
/// gate only. A sender that floods one endpoint past [`MAX_QUEUE`] is refused
/// as [`FabricError::QueueFull`]; a send to a closed endpoint is refused as
/// [`FabricError::Closed`]. Both gates run **before** Hodge admit, so a refused
/// send enqueues nothing and burns no Hodge quota. The Hodge quota is
/// per-fabric (shared), not per-tenant; this clip claims endpoint
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
    let start = f.hodge.remain(flow);

    let mut fill_ok = true;
    for _ in 0..MAX_QUEUE {
        fill_ok &= flood_msg(victim, b).map(|m| f.send(m)) == Some(Ok(()));
    }
    fill_ok &= f.pending(victim) == Ok(MAX_QUEUE);
    let after_fill = f.hodge.remain(flow);

    let mut flood_refused = true;
    for _ in 0..3 {
        flood_refused &=
            flood_msg(victim, b).map(|m| f.send(m)) == Some(Err(FabricError::QueueFull));
    }
    flood_refused &= f.pending(victim) == Ok(MAX_QUEUE);
    let no_quota_burn =
        after_fill + MAX_QUEUE as u32 == start && f.hodge.remain(flow) == after_fill;

    let neighbor_ok = flood_msg(neighbor, a).map(|m| f.send(m)) == Some(Ok(()))
        && f.pending(neighbor) == Ok(1);

    let drained = f.recv(victim).is_ok();
    let drain_readmits = drained
        && flood_msg(victim, b).map(|m| f.send(m)) == Some(Ok(()))
        && f.pending(victim) == Ok(MAX_QUEUE);

    let before_close = f.hodge.remain(flow);
    let closed_refused = f.close(shut).is_ok()
        && flood_msg(shut, b).map(|m| f.send(m)) == Some(Err(FabricError::Closed))
        && f.pending(shut) == Ok(0)
        && f.hodge.remain(flow) == before_close;

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
        roundtrip_ok = f.pending(ep) == Ok(0)
            && f.send(m).is_ok()
            && match f.recv(ep) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CapKind, CapRights};

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
        f.send(msg).unwrap();
        let got = f.recv(ep).unwrap();
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
        f.send(msg).unwrap();
        let got = f.recv(ep).unwrap();
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
        f.send(msg).unwrap();
        let got = f.recv(ep).unwrap();
        assert_eq!(got.header.n_caps, 1);
        assert_eq!(got.caps[0].unwrap().object, 7);
    }

    #[test]
    fn queue_full_and_empty() {
        let mut f = Fabric::new();
        let ep = f.create_endpoint(TenantId(1)).unwrap();
        for _ in 0..MAX_QUEUE {
            f.send(
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
            f.send(
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
            f.recv(ep).unwrap();
        }
        assert_eq!(f.recv(ep).unwrap_err(), FabricError::WouldBlock);
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
            f.send(msg).unwrap_err(),
            FabricError::Hodge(crate::hodge::HodgeError::HarmonicTreeReduce)
        );
        assert_eq!(f.pending(ep).unwrap(), 0);
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
        assert_eq!(f.send(msg).unwrap_err(), FabricError::NoSuchEndpoint);
    }
}
