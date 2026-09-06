//! SparsifiedCollective — drop below-threshold harmonic before inject.
//!
//! Sibling of [`crate::opkernel::OperatorKernelHandle`]. This is a
//! **research kernel surface**, not a spectral compiler and not an
//! eigensolver. A Hodge-bound collective carries an integer milli
//! energy. Before inject:
//!
//! 1. Hodge refuse still wins: Tree+Harmonic is `HarmonicTreeReduce`,
//!    Tree+Curl is `CurlOnTree`. A below-threshold harmonic does **not**
//!    become a Drop that skips that policy.
//! 2. Harmonic energy strictly below the threshold is **dropped** (no
//!    enqueue, quota untouched).
//! 3. Harmonic at or above the threshold is **kept** and injected as
//!    Harmonic (no TREE_OFFLOAD unless the handle already set it — and
//!    a Tree+Harmonic handle cannot bind).
//! 4. Gradient and Curl pass through. Their energy is ignored.
//!
//! Caps stay on the `OperatorKernel` handle. No new `CapKind`, no new
//! syscall, no QEMU collective engine.

use crate::caps::{CPtr, CapRights, CapTable};
use crate::fabric::{EndpointId, Fabric};
use crate::hodge::{FlowClass, HodgeError};
use crate::opkernel::{
    compatible, require_opkernel_bind, CollectiveKind, OpKernelError, OpKernelId,
    OperatorKernelHandle,
};
use crate::types::TenantId;

/// Keep iff `energy_milli >= threshold`. Drop is strict `<`.
/// 1000 milli = 1.0 in the same fixed-point as Rayleigh helpers.
pub const DEFAULT_THRESHOLD_MILLI: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SparsifyAction {
    /// Inject the bound class as-is.
    Keep,
    /// Harmonic component is strictly below the threshold; do not enqueue.
    Drop,
}

/// Hodge-bound collective plus a spectral weight and threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SparsifiedCollective {
    pub handle: OperatorKernelHandle,
    pub energy_milli: u32,
    pub threshold_milli: u32,
}

impl SparsifiedCollective {
    pub const fn wrap(
        handle: OperatorKernelHandle,
        energy_milli: u32,
        threshold_milli: u32,
    ) -> Self {
        Self {
            handle,
            energy_milli,
            threshold_milli,
        }
    }

    /// Bind a topology × class (same refuse as [`OperatorKernelHandle::bind`]),
    /// then attach a weight / threshold. Tree+Harmonic cannot be constructed.
    pub fn from_header(
        id: OpKernelId,
        topology: CollectiveKind,
        flow: FlowClass,
        energy_milli: u32,
        threshold_milli: u32,
    ) -> Result<Self, HodgeError> {
        let handle = OperatorKernelHandle::bind(id, topology, flow)?;
        Ok(Self::wrap(handle, energy_milli, threshold_milli))
    }

    pub fn decide(self) -> Result<SparsifyAction, HodgeError> {
        decide_header(
            self.handle.topology,
            self.handle.flow,
            self.energy_milli,
            self.threshold_milli,
        )
    }

    /// Cap-checked inject. Drop is an authorized no-op (BIND+SUBMIT still
    /// required). Keep delegates to [`OperatorKernelHandle::inject`].
    pub fn inject(
        self,
        tab: &CapTable,
        cptr: CPtr,
        fabric: &mut Fabric,
        dest: EndpointId,
        tenant: TenantId,
        data: &[u8],
    ) -> Result<SparsifyAction, OpKernelError> {
        match self.decide().map_err(OpKernelError::Hodge)? {
            SparsifyAction::Drop => {
                require_authorized(self.handle, tab, cptr)?;
                Ok(SparsifyAction::Drop)
            }
            SparsifyAction::Keep => {
                self.handle.inject(tab, cptr, fabric, dest, tenant, data)?;
                Ok(SparsifyAction::Keep)
            }
        }
    }
}

/// Policy for a FlowClass header. Hodge refuse first, then threshold.
pub fn decide_header(
    topology: CollectiveKind,
    flow: FlowClass,
    energy_milli: u32,
    threshold_milli: u32,
) -> Result<SparsifyAction, HodgeError> {
    compatible(topology, flow)?;
    match flow {
        FlowClass::Harmonic if energy_milli < threshold_milli => Ok(SparsifyAction::Drop),
        FlowClass::Harmonic | FlowClass::Gradient | FlowClass::Curl => Ok(SparsifyAction::Keep),
    }
}

fn require_authorized(
    handle: OperatorKernelHandle,
    tab: &CapTable,
    cptr: CPtr,
) -> Result<(), OpKernelError> {
    require_opkernel_bind(tab, cptr).map_err(|_| OpKernelError::NotBound)?;
    let cap = tab.lookup(cptr).map_err(OpKernelError::Cap)?;
    let named = OperatorKernelHandle::from_cap(cap)?;
    if named != handle {
        return Err(OpKernelError::NotBound);
    }
    if !cap.rights.contains(CapRights::SUBMIT) {
        return Err(OpKernelError::NotBound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{CapKind, CapRights, CapTable, Capability};
    use crate::hodge::HodgeQuota;
    use crate::opkernel::CollectiveKind;
    use crate::types::TenantId;

    fn tab() -> CapTable {
        CapTable::new(TenantId(1))
    }

    fn torus_harmonic() -> OperatorKernelHandle {
        OperatorKernelHandle::bind(OpKernelId(1), CollectiveKind::Torus, FlowClass::Harmonic)
            .unwrap()
    }

    #[test]
    fn below_threshold_harmonic_dropped() {
        let mut fabric = crate::fabric::Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let mut caps = tab();
        let h = torus_harmonic();
        let cptr = h.mint(&mut caps).unwrap();
        let remain = fabric.hodge.remain(FlowClass::Harmonic);
        let s = SparsifiedCollective::wrap(h, 499, 500);
        assert_eq!(s.decide().unwrap(), SparsifyAction::Drop);
        assert_eq!(
            s.inject(&caps, cptr, &mut fabric, ep, TenantId(1), b"tiny")
                .unwrap(),
            SparsifyAction::Drop
        );
        assert_eq!(fabric.pending(ep).unwrap(), 0);
        assert_eq!(fabric.hodge.remain(FlowClass::Harmonic), remain);
    }

    #[test]
    fn above_threshold_harmonic_kept() {
        let mut fabric = crate::fabric::Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let mut caps = tab();
        let h = torus_harmonic();
        let cptr = h.mint(&mut caps).unwrap();
        let s = SparsifiedCollective::wrap(h, 500, 500);
        assert_eq!(s.decide().unwrap(), SparsifyAction::Keep);
        assert_eq!(
            s.inject(&caps, cptr, &mut fabric, ep, TenantId(1), b"cycle")
                .unwrap(),
            SparsifyAction::Keep
        );
        let got = fabric.recv(ep).unwrap();
        assert_eq!(got.payload(), b"cycle");
        assert_eq!(got.header.flow, FlowClass::Harmonic);
        assert!(!got.header.flags.tree_offload());
    }

    #[test]
    fn gradient_and_curl_ignore_energy() {
        let mut fabric = crate::fabric::Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let mut caps = tab();

        let tree =
            OperatorKernelHandle::bind(OpKernelId(2), CollectiveKind::Tree, FlowClass::Gradient)
                .unwrap();
        let tp = tree.mint(&mut caps).unwrap();
        let g = SparsifiedCollective::wrap(tree, 0, DEFAULT_THRESHOLD_MILLI);
        assert_eq!(g.decide().unwrap(), SparsifyAction::Keep);
        g.inject(&caps, tp, &mut fabric, ep, TenantId(1), b"tree")
            .unwrap();
        let got = fabric.recv(ep).unwrap();
        assert_eq!(got.header.flow, FlowClass::Gradient);
        assert!(got.header.flags.tree_offload());

        let ring = OperatorKernelHandle::bind(OpKernelId(3), CollectiveKind::Ring, FlowClass::Curl)
            .unwrap();
        let rp = ring.mint(&mut caps).unwrap();
        let c = SparsifiedCollective::wrap(ring, 0, DEFAULT_THRESHOLD_MILLI);
        assert_eq!(c.decide().unwrap(), SparsifyAction::Keep);
        c.inject(&caps, rp, &mut fabric, ep, TenantId(1), b"ring")
            .unwrap();
        let got = fabric.recv(ep).unwrap();
        assert_eq!(got.header.flow, FlowClass::Curl);
        assert!(got.header.flags.ring_reserve());
        assert!(!got.header.flags.tree_offload());
    }

    #[test]
    fn harmonic_tree_still_refused_even_below_threshold() {
        assert_eq!(
            decide_header(CollectiveKind::Tree, FlowClass::Harmonic, 1, 500).unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );
        assert_eq!(
            decide_header(CollectiveKind::Tree, FlowClass::Harmonic, 9999, 1).unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );
        assert_eq!(
            SparsifiedCollective::from_header(
                OpKernelId(9),
                CollectiveKind::Tree,
                FlowClass::Harmonic,
                1,
                500
            )
            .unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );

        let illegal = SparsifiedCollective::wrap(
            OperatorKernelHandle {
                id: OpKernelId(9),
                topology: CollectiveKind::Tree,
                flow: FlowClass::Harmonic,
            },
            1,
            500,
        );
        assert_eq!(
            illegal.decide().unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );
        let mut fabric = crate::fabric::Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let caps = tab();
        assert_eq!(
            illegal
                .inject(&caps, CPtr(0), &mut fabric, ep, TenantId(1), b"nope")
                .unwrap_err(),
            OpKernelError::Hodge(HodgeError::HarmonicTreeReduce)
        );
        assert_eq!(fabric.pending(ep).unwrap(), 0);
    }

    #[test]
    fn curl_on_tree_still_refused() {
        assert_eq!(
            decide_header(CollectiveKind::Tree, FlowClass::Curl, 0, 1).unwrap_err(),
            HodgeError::CurlOnTree
        );
    }

    #[test]
    fn missing_bind_refuses_even_on_drop() {
        let mut fabric = crate::fabric::Fabric::new();
        let ep = fabric.create_endpoint(TenantId(1)).unwrap();
        let mut caps = tab();
        let h = torus_harmonic();
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
        let s = SparsifiedCollective::wrap(h, 1, 500);
        assert_eq!(
            s.inject(&caps, weak, &mut fabric, ep, TenantId(1), b"tiny")
                .unwrap_err(),
            OpKernelError::NotBound
        );
        assert_eq!(fabric.pending(ep).unwrap(), 0);
    }

    #[test]
    fn drop_does_not_charge_quota() {
        let q = HodgeQuota::generous();
        let start = q.remain(FlowClass::Harmonic);
        let s = SparsifiedCollective::from_header(
            OpKernelId(4),
            CollectiveKind::Torus,
            FlowClass::Harmonic,
            10,
            100,
        )
        .unwrap();
        assert_eq!(s.decide().unwrap(), SparsifyAction::Drop);
        assert_eq!(q.remain(FlowClass::Harmonic), start);
    }
}
