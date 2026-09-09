//! OperatorKernelHandle — a capability for a compiled collective.
//!
//! This is a **research kernel surface**, not a compiler. The handle stores
//! a collective topology (`Tree` / `Ring` / `Torus`) and a bound Hodge
//! class. Bind and inject reuse [`crate::hodge`] policy: Gradient may
//! tree-offload; Curl and Harmonic must not.
//!
//! Caps (`CapKind::OperatorKernel`) are minted through the existing table.
//! Derive / GRANT-copy record a CDT parent; revoke of that parent empties
//! descendants the same way as Memory.

use crate::caps::{CPtr, CapError, CapKind, CapRights, CapTable, Capability};
use crate::fabric::{ChipletRoute, EndpointId, Fabric, FabricError, Message, MsgFlags};
use crate::hodge::{FlowClass, HodgeError, HodgeQuota};
use crate::phase::Phase;
use crate::types::TenantId;

/// Namespace-local handle id (not a CPtr).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpKernelId(pub u32);

/// Collective interconnect shape. Not an IR — a topology cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CollectiveKind {
    /// Spanning-tree / reduction-engine offload. Gradient only.
    Tree = 0,
    /// Cyclic ring. Natural home of Curl; no TREE_OFFLOAD.
    Ring = 1,
    /// 2-D cyclic mesh. Natural home of Harmonic; no TREE_OFFLOAD.
    Torus = 2,
}

impl CollectiveKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Tree),
            1 => Some(Self::Ring),
            2 => Some(Self::Torus),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Ring => "ring",
            Self::Torus => "torus",
        }
    }

    /// Tree is the only topology that sets `MsgFlags::TREE_OFFLOAD`.
    pub const fn tree_offload(self) -> bool {
        matches!(self, Self::Tree)
    }

    pub const fn ring_reserve(self) -> bool {
        matches!(self, Self::Ring)
    }

    /// SoftNoI / DMA tag at submit. Software enum, not a vendor header.
    ///
    /// Tree (allreduce) → Gradient; Ring (ring-exchange) → Curl;
    /// Torus (persistent) → Harmonic.
    pub const fn fabric_class(self) -> FlowClass {
        match self {
            Self::Tree => FlowClass::Gradient,
            Self::Ring => FlowClass::Curl,
            Self::Torus => FlowClass::Harmonic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKernelError {
    Hodge(HodgeError),
    Cap(CapError),
    Fabric(FabricError),
    /// Inject asked for a FlowClass other than the one bound on the handle.
    ClassMismatch,
    /// Cap missing, wrong kind, or missing BIND/SUBMIT.
    NotBound,
}

/// Compiled collective bound to one Hodge class.
///
/// Badge packing (minted into the cap): `topology` in bits 0..7,
/// `flow` in bits 8..15. Object id is [`OpKernelId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperatorKernelHandle {
    pub id: OpKernelId,
    pub topology: CollectiveKind,
    pub flow: FlowClass,
}

impl OperatorKernelHandle {
    pub fn bind(
        id: OpKernelId,
        topology: CollectiveKind,
        flow: FlowClass,
    ) -> Result<Self, HodgeError> {
        compatible(topology, flow)?;
        Ok(Self { id, topology, flow })
    }

    pub const fn badge(self) -> u64 {
        (self.topology as u8 as u64) | ((self.flow as u8 as u64) << 8)
    }

    pub fn from_badge(id: OpKernelId, badge: u64) -> Option<Self> {
        let topology = CollectiveKind::from_u8(badge as u8)?;
        let flow = FlowClass::from_u8((badge >> 8) as u8)?;
        Self::bind(id, topology, flow).ok()
    }

    pub fn from_cap(cap: &Capability) -> Result<Self, OpKernelError> {
        if cap.kind != CapKind::OperatorKernel {
            return Err(OpKernelError::NotBound);
        }
        Self::from_badge(OpKernelId(cap.object), cap.badge).ok_or(OpKernelError::NotBound)
    }

    pub fn mint(&self, table: &mut CapTable) -> Result<CPtr, CapError> {
        table.mint(
            Capability::new(
                CapKind::OperatorKernel,
                CapRights::OPKERNEL_FULL,
                self.id.0,
                table.owner(),
            )
            .with_badge(self.badge()),
        )
    }

    pub fn inject_flags(self) -> MsgFlags {
        let mut bits = MsgFlags::ASYNC;
        if self.topology.tree_offload() {
            bits |= MsgFlags::TREE_OFFLOAD;
        }
        if self.topology.ring_reserve() {
            bits |= MsgFlags::RING_RESERVE;
        }
        MsgFlags(bits)
    }

    /// Admit against the virtual-link quota using this handle's topology.
    pub fn admit(self, quota: &mut HodgeQuota) -> Result<(), HodgeError> {
        quota.admit(self.flow, self.topology.tree_offload())
    }

    /// Attach a milli energy / threshold. See [`crate::sparsify`].
    pub const fn sparsify(
        self,
        energy_milli: u32,
        threshold_milli: u32,
    ) -> crate::sparsify::SparsifiedCollective {
        crate::sparsify::SparsifiedCollective::wrap(self, energy_milli, threshold_milli)
    }

    /// Same as [`Self::admit`] but the caller names a class. Mismatch refuses
    /// before Hodge policy runs.
    pub fn admit_as(self, quota: &mut HodgeQuota, flow: FlowClass) -> Result<(), OpKernelError> {
        if flow != self.flow {
            return Err(OpKernelError::ClassMismatch);
        }
        self.admit(quota).map_err(OpKernelError::Hodge)
    }

    /// Bind-checked inject: header flow + TREE_OFFLOAD / RING_RESERVE come
    /// from the handle, then [`Fabric::send`] runs Hodge admit.
    pub fn inject(
        self,
        tab: &CapTable,
        cptr: CPtr,
        fabric: &mut Fabric,
        dest: EndpointId,
        tenant: TenantId,
        data: &[u8],
    ) -> Result<(), OpKernelError> {
        self.inject_as(tab, cptr, fabric, dest, tenant, data, self.flow)
    }

    pub fn inject_as(
        self,
        tab: &CapTable,
        cptr: CPtr,
        fabric: &mut Fabric,
        dest: EndpointId,
        tenant: TenantId,
        data: &[u8],
        flow: FlowClass,
    ) -> Result<(), OpKernelError> {
        if flow != self.flow {
            return Err(OpKernelError::ClassMismatch);
        }
        require_opkernel_bind(tab, cptr).map_err(|_| OpKernelError::NotBound)?;
        let cap = tab.lookup(cptr).map_err(OpKernelError::Cap)?;
        let named = Self::from_cap(cap)?;
        if named != self {
            return Err(OpKernelError::NotBound);
        }
        if !cap.rights.contains(CapRights::SUBMIT) {
            return Err(OpKernelError::NotBound);
        }
        let msg = Message::new(
            dest,
            cap.badge,
            self.inject_flags(),
            ChipletRoute::LOCAL,
            tenant,
            data,
        )
        .map_err(OpKernelError::Fabric)?
        .with_flow(self.flow)
        .with_phase(Phase::Exchange);
        fabric.send(msg).map_err(OpKernelError::Fabric)
    }
}

/// Tree + Curl / Harmonic is the existing Hodge refuse. Ring and torus
/// never set TREE_OFFLOAD, so any class may bind to them.
pub fn compatible(topology: CollectiveKind, flow: FlowClass) -> Result<(), HodgeError> {
    if topology.tree_offload() {
        match flow {
            FlowClass::Gradient => Ok(()),
            FlowClass::Curl => Err(HodgeError::CurlOnTree),
            FlowClass::Harmonic => Err(HodgeError::HarmonicTreeReduce),
        }
    } else {
        Ok(())
    }
}

/// BIND is required. Without it the handle is inert (same as SpectralCut).
pub fn require_opkernel_bind(tab: &CapTable, cptr: CPtr) -> Result<&Capability, CapError> {
    tab.require(cptr, CapKind::OperatorKernel, CapRights::BIND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::CapTable;
    use crate::types::TenantId;

    fn tab() -> CapTable {
        CapTable::new(TenantId(1))
    }

    #[test]
    fn fabric_class_from_collective_type() {
        assert_eq!(CollectiveKind::Tree.fabric_class(), FlowClass::Gradient);
        assert_eq!(CollectiveKind::Ring.fabric_class(), FlowClass::Curl);
        assert_eq!(CollectiveKind::Torus.fabric_class(), FlowClass::Harmonic);
    }

    #[test]
    fn tree_binds_gradient_only() {
        assert!(OperatorKernelHandle::bind(
            OpKernelId(1),
            CollectiveKind::Tree,
            FlowClass::Gradient
        )
        .is_ok());
        assert_eq!(
            OperatorKernelHandle::bind(OpKernelId(1), CollectiveKind::Tree, FlowClass::Curl)
                .unwrap_err(),
            HodgeError::CurlOnTree
        );
        assert_eq!(
            OperatorKernelHandle::bind(OpKernelId(1), CollectiveKind::Tree, FlowClass::Harmonic)
                .unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );
    }

    #[test]
    fn ring_and_torus_accept_all_classes() {
        for kind in [CollectiveKind::Ring, CollectiveKind::Torus] {
            for flow in [FlowClass::Gradient, FlowClass::Curl, FlowClass::Harmonic] {
                OperatorKernelHandle::bind(OpKernelId(2), kind, flow).unwrap();
            }
        }
    }

    #[test]
    fn admit_respects_hodge_tree_policy() {
        let tree =
            OperatorKernelHandle::bind(OpKernelId(1), CollectiveKind::Tree, FlowClass::Gradient)
                .unwrap();
        let mut q = HodgeQuota::generous();
        tree.admit(&mut q).unwrap();
        assert_eq!(q.remain(FlowClass::Gradient), 63);

        let harm = OperatorKernelHandle {
            id: OpKernelId(9),
            topology: CollectiveKind::Tree,
            flow: FlowClass::Harmonic,
        };
        assert_eq!(
            harm.admit(&mut HodgeQuota::generous()).unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );
        let curl = OperatorKernelHandle {
            id: OpKernelId(8),
            topology: CollectiveKind::Tree,
            flow: FlowClass::Curl,
        };
        assert_eq!(
            curl.admit(&mut HodgeQuota::generous()).unwrap_err(),
            HodgeError::CurlOnTree
        );
    }

    #[test]
    fn torus_harmonic_admits_without_tree() {
        let h =
            OperatorKernelHandle::bind(OpKernelId(3), CollectiveKind::Torus, FlowClass::Harmonic)
                .unwrap();
        let mut q = HodgeQuota::generous();
        h.admit(&mut q).unwrap();
        assert!(!h.inject_flags().tree_offload());
        assert_eq!(q.remain(FlowClass::Harmonic), 63);
    }

    #[test]
    fn wrong_class_refused_before_quota() {
        let h =
            OperatorKernelHandle::bind(OpKernelId(1), CollectiveKind::Tree, FlowClass::Gradient)
                .unwrap();
        let mut q = HodgeQuota::generous();
        assert_eq!(
            h.admit_as(&mut q, FlowClass::Harmonic).unwrap_err(),
            OpKernelError::ClassMismatch
        );
        assert_eq!(q.remain(FlowClass::Gradient), 64);
        assert_eq!(q.remain(FlowClass::Harmonic), 64);
    }

    #[test]
    fn inject_tree_gradient_ok_wrong_class_refused() {
        let mut fabric = Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let mut caps = tab();
        let h =
            OperatorKernelHandle::bind(OpKernelId(4), CollectiveKind::Tree, FlowClass::Gradient)
                .unwrap();
        let cptr = h.mint(&mut caps).unwrap();
        h.inject(&caps, cptr, &mut fabric, ep, TenantId(1), b"allreduce")
            .unwrap();
        let got = fabric.recv(ep).unwrap();
        assert_eq!(got.payload(), b"allreduce");
        assert_eq!(got.header.flow, FlowClass::Gradient);
        assert!(got.header.flags.tree_offload());

        assert_eq!(
            h.inject_as(
                &caps,
                cptr,
                &mut fabric,
                ep,
                TenantId(1),
                b"nope",
                FlowClass::Harmonic,
            )
            .unwrap_err(),
            OpKernelError::ClassMismatch
        );
        assert_eq!(fabric.pending(ep).unwrap(), 0);
    }

    #[test]
    fn inject_ring_curl_and_torus_harmonic() {
        let mut fabric = Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let mut caps = tab();

        let ring = OperatorKernelHandle::bind(OpKernelId(5), CollectiveKind::Ring, FlowClass::Curl)
            .unwrap();
        let rp = ring.mint(&mut caps).unwrap();
        ring.inject(&caps, rp, &mut fabric, ep, TenantId(1), b"ring")
            .unwrap();
        let got = fabric.recv(ep).unwrap();
        assert_eq!(got.header.flow, FlowClass::Curl);
        assert!(got.header.flags.ring_reserve());
        assert!(!got.header.flags.tree_offload());

        let torus =
            OperatorKernelHandle::bind(OpKernelId(6), CollectiveKind::Torus, FlowClass::Harmonic)
                .unwrap();
        let tp = torus.mint(&mut caps).unwrap();
        torus
            .inject(&caps, tp, &mut fabric, ep, TenantId(1), b"cycle")
            .unwrap();
        let got = fabric.recv(ep).unwrap();
        assert_eq!(got.header.flow, FlowClass::Harmonic);
        assert!(!got.header.flags.tree_offload());
    }

    #[test]
    fn missing_bind_or_foreign_table_refuses() {
        let mut fabric = Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let mut caps = tab();
        let h = OperatorKernelHandle::bind(OpKernelId(7), CollectiveKind::Ring, FlowClass::Curl)
            .unwrap();
        let weak = caps
            .mint(
                Capability::new(
                    CapKind::OperatorKernel,
                    CapRights(CapRights::READ),
                    h.id.0,
                    TenantId(1),
                )
                .with_badge(h.badge()),
            )
            .unwrap();
        assert_eq!(
            h.inject(&caps, weak, &mut fabric, ep, TenantId(1), &[])
                .unwrap_err(),
            OpKernelError::NotBound
        );

        let other = CapTable::new(TenantId(2));
        assert_eq!(
            h.inject(&other, CPtr(0), &mut fabric, ep, TenantId(2), &[])
                .unwrap_err(),
            OpKernelError::NotBound
        );
        assert!(!other.holds(CapKind::OperatorKernel, h.id.0));
    }

    #[test]
    fn mint_derive_revoke_parent_empties_child() {
        let mut caps = tab();
        let h =
            OperatorKernelHandle::bind(OpKernelId(10), CollectiveKind::Tree, FlowClass::Gradient)
                .unwrap();
        let parent = h.mint(&mut caps).unwrap();
        let child = caps
            .derive(parent, CapRights(CapRights::READ | CapRights::BIND))
            .unwrap();
        assert_eq!(caps.lookup(child).unwrap().kind, CapKind::OperatorKernel);
        assert_eq!(
            caps.lookup(child).unwrap().parent,
            Some(caps.lookup(parent).unwrap().cdt())
        );
        caps.revoke(parent).unwrap();
        assert_eq!(caps.lookup(parent).unwrap_err(), CapError::EmptySlot);
        assert_eq!(
            require_opkernel_bind(&caps, child).unwrap_err(),
            CapError::EmptySlot
        );
        assert!(!caps.holds(CapKind::OperatorKernel, 10));
    }

    #[test]
    fn from_u8_roundtrip() {
        assert_eq!(CollectiveKind::from_u8(0), Some(CollectiveKind::Tree));
        assert_eq!(CollectiveKind::from_u8(1), Some(CollectiveKind::Ring));
        assert_eq!(CollectiveKind::from_u8(2), Some(CollectiveKind::Torus));
        assert_eq!(CollectiveKind::from_u8(3), None);
        let h =
            OperatorKernelHandle::bind(OpKernelId(1), CollectiveKind::Torus, FlowClass::Harmonic)
                .unwrap();
        assert_eq!(
            OperatorKernelHandle::from_badge(OpKernelId(1), h.badge()),
            Some(h)
        );
    }
}
