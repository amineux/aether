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
        self.hodge
            .admit(flow, tree)
            .map_err(FabricError::Hodge)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CapKind, CapRights};

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
        msg.attach_cap(Capability {
            kind: CapKind::Memory,
            rights: CapRights::MEM_FULL,
            object: 7,
            badge: 0,
            generation: 1,
            tenant: TenantId(2),
        })
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
