//! Capability tables bound by trusted boot code to the two current images.
//! Root identity comes from the scheduler, never a user-supplied task/tenant id.
//! Clones share a root/table; unknown roots have no fallback to bootstrap caps.
use aether_core::caps::{CPtr, CapError, CapKind, CapRights, CapTable, Capability};

struct ProbeSpace {
    root: u64,
    caps: CapTable,
}

pub struct CapSpaces {
    bootstrap: CapTable,
    init_root: Option<u64>,
    probe: Option<ProbeSpace>,
}

impl CapSpaces {
    pub fn new(bootstrap: CapTable) -> Self {
        Self {
            bootstrap,
            init_root: None,
            probe: None,
        }
    }

    /// Bind once, before entering userspace. Roots cannot yet be recycled.
    pub fn bind_init(&mut self, root: u64) -> Result<(), ()> {
        if root == 0 || self.init_root.is_some() {
            return Err(());
        }
        self.init_root = Some(root);
        Ok(())
    }

    /// Trusted boot policy for /probe: one private endpoint, no GRANT, memory,
    /// accelerator or allocator authority. Both boot images remain tenant 1.
    pub fn bind_probe_endpoint(&mut self, root: u64, endpoint: u32) -> Result<(), ()> {
        if root == 0
            || self.init_root.is_none()
            || self.init_root == Some(root)
            || self.probe.is_some()
        {
            return Err(());
        }
        let tenant = self.bootstrap.owner();
        let mut caps = CapTable::new(tenant);
        let slot = caps
            .mint(
                Capability::new(
                    CapKind::Endpoint,
                    CapRights(CapRights::READ | CapRights::WRITE),
                    endpoint,
                    tenant,
                )
                .with_badge(0xB4),
            )
            .map_err(|_| ())?;
        assert_eq!(slot, CPtr(0));
        self.probe = Some(ProbeSpace { root, caps });
        Ok(())
    }

    /// Copy an attenuated endpoint cap between the two registered spaces.
    /// The caller supplies trusted scheduler roots. No move, GRANT propagation,
    /// memory or queue sharing; a refused transfer leaves both tables intact.
    pub fn grant_endpoint(
        &mut self,
        source_root: u64,
        target_root: u64,
        source: CPtr,
        rights: u64,
    ) -> Result<CPtr, CapError> {
        let init = self.init_root.ok_or(CapError::InvalidCptr)?;
        let probe = self.probe.as_mut().ok_or(CapError::InvalidCptr)?;
        let (from, to) = if source_root == init && target_root == probe.root {
            (&mut self.bootstrap, &mut probe.caps)
        } else if source_root == probe.root && target_root == init {
            (&mut probe.caps, &mut self.bootstrap)
        } else {
            return Err(CapError::InvalidCptr);
        };
        from.require(source, CapKind::Endpoint, CapRights::GRANT)?;
        let bits = u16::try_from(rights).map_err(|_| CapError::WouldEscalate)?;
        if bits == 0 || bits & !(CapRights::READ | CapRights::WRITE) != 0 {
            return Err(CapError::WouldEscalate);
        }
        from.transfer(source, to, CapRights(bits), false)
    }

    /// Revoke endpoint descendants across every registered boot space. Keep
    /// the source root so its owner can grant again after reclaiming slots.
    pub fn revoke_endpoint(&mut self, root: u64, source: CPtr) -> Result<(), CapError> {
        self.get(root).ok_or(CapError::InvalidCptr)?.require(
            source,
            CapKind::Endpoint,
            CapRights::GRANT,
        )?;
        if self.init_root == Some(root) {
            if let Some(probe) = self.probe.as_mut() {
                self.bootstrap
                    .revoke_descendants_in(source, &mut [&mut probe.caps])
            } else {
                self.bootstrap.revoke_descendants_in(source, &mut [])
            }
        } else {
            self.probe
                .as_mut()
                .ok_or(CapError::InvalidCptr)?
                .caps
                .revoke_descendants_in(source, &mut [&mut self.bootstrap])
        }
    }

    /// Blocked receives are authorized by a currently held READ cap on the
    /// endpoint, not solely by the numeric endpoint id saved in the scheduler.
    pub fn can_receive(&self, root: u64, endpoint: u32) -> bool {
        self.get(root)
            .is_some_and(|caps| caps.holds_rights(CapKind::Endpoint, endpoint, CapRights::READ))
    }

    pub fn get(&self, root: u64) -> Option<&CapTable> {
        if self.init_root == Some(root) {
            Some(&self.bootstrap)
        } else {
            self.probe
                .as_ref()
                .filter(|p| p.root == root)
                .map(|p| &p.caps)
        }
    }

    pub fn get_mut(&mut self, root: u64) -> Option<&mut CapTable> {
        if self.init_root == Some(root) {
            Some(&mut self.bootstrap)
        } else {
            self.probe
                .as_mut()
                .filter(|p| p.root == root)
                .map(|p| &mut p.caps)
        }
    }

    /// SYS_ARENA_ALLOC has no allocator-cap ABI yet: preserve init-only policy.
    pub fn can_allocate(&self, root: u64) -> bool {
        self.init_root == Some(root)
    }

    /// Kernel-internal ping only. User syscall lookups must use get(root).
    pub fn bootstrap(&self) -> &CapTable {
        &self.bootstrap
    }
}
