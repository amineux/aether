//! FlowHodgeQuota — classify fabric traffic by Hodge component.
//!
//! Interconnect flow on a package is not just "bytes". The Hodge
//! decomposition of a flow on the chiplet graph splits it into:
//!
//! - **Gradient** — tree-like (broadcast, reduce, allreduce-tree). May use
//!   tree offload (a spanning-tree credit path / on-die reduction engine).
//! - **Curl** — a simple cyclic ring. Reserves ring capacity; must not be
//!   collapsed onto a tree.
//! - **Harmonic** — persistent collective cycles (homology). **Must not**
//!   be tree-reduced: mapping a non-trivial cycle onto a spanning tree
//!   identifies distinct edges, so two harmonic collectives can share a
//!   tree buffer and wait for each other (credit deadlock). It also
//!   destroys the homology class the collective's progress depends on.
//!
//! v0.1: one virtual interconnect, integer quotas, real refusal rules.
//! Caps (`CapKind::FlowQuota`) authorize which classes a tenant may send.

use crate::caps::{CapKind, CapRights, Capability};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FlowClass {
    Gradient = 0,
    Curl = 1,
    Harmonic = 2,
}

impl FlowClass {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Gradient),
            1 => Some(Self::Curl),
            2 => Some(Self::Harmonic),
            _ => None,
        }
    }

    pub const fn bit(self) -> u64 {
        1u64 << (self as u8)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gradient => "gradient",
            Self::Curl => "curl",
            Self::Harmonic => "harmonic",
        }
    }
}

pub const CLASS_GRADIENT: u64 = 1;
pub const CLASS_CURL: u64 = 2;
pub const CLASS_HARMONIC: u64 = 4;
pub const CLASS_ALL: u64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HodgeError {
    QuotaExceeded,
    /// Harmonic + TREE_OFFLOAD: deadlock / homology collapse (see module docs).
    HarmonicTreeReduce,
    /// Curl is a ring; tree offload is the wrong topology.
    CurlOnTree,
    ClassNotAuthorized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HodgeQuota {
    pub remain: [u32; 3],
}

impl HodgeQuota {
    pub const fn generous() -> Self {
        Self {
            remain: [64, 64, 64],
        }
    }

    pub const fn empty() -> Self {
        Self { remain: [0, 0, 0] }
    }

    pub fn remain(&self, c: FlowClass) -> u32 {
        self.remain[c as usize]
    }

    /// Policy then quota. Called from `Fabric::send` on the only virtual link.
    pub fn admit(&mut self, flow: FlowClass, tree_offload: bool) -> Result<(), HodgeError> {
        if tree_offload {
            match flow {
                FlowClass::Gradient => {}
                FlowClass::Curl => return Err(HodgeError::CurlOnTree),
                FlowClass::Harmonic => return Err(HodgeError::HarmonicTreeReduce),
            }
        }
        let i = flow as usize;
        if self.remain[i] == 0 {
            return Err(HodgeError::QuotaExceeded);
        }
        self.remain[i] -= 1;
        Ok(())
    }
}

/// Cap surface: `FlowQuota` badge is a class bitmask. WRITE required to send.
pub fn authorize(cap: &Capability, flow: FlowClass) -> Result<(), HodgeError> {
    if cap.kind != CapKind::FlowQuota {
        return Err(HodgeError::ClassNotAuthorized);
    }
    if !cap.rights.contains(CapRights::WRITE) {
        return Err(HodgeError::ClassNotAuthorized);
    }
    if cap.badge & flow.bit() == 0 {
        return Err(HodgeError::ClassNotAuthorized);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::Capability;
    use crate::types::TenantId;

    #[test]
    fn gradient_tree_ok_curl_tree_refused() {
        let mut q = HodgeQuota::generous();
        assert!(q.admit(FlowClass::Gradient, true).is_ok());
        assert_eq!(
            q.admit(FlowClass::Curl, true).unwrap_err(),
            HodgeError::CurlOnTree
        );
        assert_eq!(
            q.admit(FlowClass::Harmonic, true).unwrap_err(),
            HodgeError::HarmonicTreeReduce
        );
    }

    #[test]
    fn curl_ring_and_harmonic_plain_ok() {
        let mut q = HodgeQuota::generous();
        q.admit(FlowClass::Curl, false).unwrap();
        q.admit(FlowClass::Harmonic, false).unwrap();
        assert_eq!(q.remain(FlowClass::Curl), 63);
        assert_eq!(q.remain(FlowClass::Harmonic), 63);
    }

    #[test]
    fn quota_zero() {
        let mut q = HodgeQuota::empty();
        assert_eq!(
            q.admit(FlowClass::Gradient, false).unwrap_err(),
            HodgeError::QuotaExceeded
        );
    }

    #[test]
    fn cap_authorizes_classes() {
        let cap = Capability::new(CapKind::FlowQuota, CapRights::HODGE_FULL, 1, TenantId(1))
            .with_badge(CLASS_GRADIENT | CLASS_CURL)
            .with_generation(1);
        assert!(authorize(&cap, FlowClass::Gradient).is_ok());
        assert!(authorize(&cap, FlowClass::Curl).is_ok());
        assert_eq!(
            authorize(&cap, FlowClass::Harmonic).unwrap_err(),
            HodgeError::ClassNotAuthorized
        );
    }
}
