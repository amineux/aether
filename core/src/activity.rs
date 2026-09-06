//! Fabric activities: every compute unit is an endpoint, not an ioctl device.
//!
//! A CPU tile and a virt accelerator are the same kind of object from the
//! fabric's point of view: an [`Activity`] behind an [`EndpointId`]. Drivers
//! may talk MMIO underneath; the kernel ABI never exposes `/dev/*` ioctls.

use crate::caps::{CapError, CapKind, CapRights, CapTable, Capability, CPtr};
use crate::fabric::EndpointId;
use crate::partition::PartitionId;

/// What kind of compute unit this activity is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    CpuTile,
    VirtAccel,
    SoftNpu,
    /// Reserved for a future real NPU / GPU / IPU backend.
    DeviceAccel,
}

impl ActivityKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::CpuTile => "CPU_TILE",
            Self::VirtAccel => "VIRT_ACCEL",
            Self::SoftNpu => "SOFT_NPU",
            Self::DeviceAccel => "DEVICE_ACCEL",
        }
    }
}

/// Stable activity identifier (namespace-local).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActivityId(pub u32);

/// A compute unit published on the capability fabric.
#[derive(Clone, Copy, Debug)]
pub struct Activity {
    pub id: ActivityId,
    pub kind: ActivityKind,
    pub endpoint: EndpointId,
    pub partition: Option<PartitionId>,
}

impl Activity {
    pub const fn new(id: ActivityId, kind: ActivityKind, endpoint: EndpointId) -> Self {
        Self {
            id,
            kind,
            endpoint,
            partition: None,
        }
    }

    pub const fn bind_partition(mut self, p: PartitionId) -> Self {
        self.partition = Some(p);
        self
    }

    /// Publish this activity as a fabric capability (uniform object, not ioctl).
    pub fn publish(&self, table: &mut CapTable) -> Result<CPtr, CapError> {
        table.mint(Capability {
            kind: CapKind::Activity,
            rights: CapRights::ACTIVITY_FULL,
            object: self.id.0,
            badge: self.endpoint.0 as u64,
            generation: 0,
            tenant: table.owner(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TenantId;

    #[test]
    fn activity_is_an_endpoint_not_an_ioctl() {
        let a = Activity::new(ActivityId(1), ActivityKind::VirtAccel, EndpointId(7));
        let mut t = CapTable::new(TenantId(1));
        let c = a.publish(&mut t).unwrap();
        let got = t.lookup(c).unwrap();
        assert_eq!(got.kind, CapKind::Activity);
        assert_eq!(got.badge, 7);
        assert!(got.rights.contains(CapRights::SUBMIT));
        assert_eq!(a.kind.name(), "VIRT_ACCEL");
    }
}
