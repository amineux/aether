//! Shared identifiers used across the fabric.

/// Physical address. Aether does not assume cache coherence on these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    pub const fn off(self, bytes: u64) -> Self {
        Self(self.0 + bytes)
    }

    pub const fn is_aligned(self, align: u64) -> bool {
        align != 0 && (self.0 & (align - 1)) == 0
    }
}

/// Memory bank / NUMA domain. On a real package this is a DRAM controller
/// or on-package HBM stack; tiles have affinity to banks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BankId(pub u8);

/// Compute tile: CPU core, NPU, GPU shader array, or custom ASIC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileId(pub u16);

/// Isolation domain. Caps minted for one tenant cannot be forged by another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TenantId(pub u32);

/// Page sizes the arena allocator understands.
pub const PAGE_4K: u64 = 4096;
pub const PAGE_2M: u64 = 2 * 1024 * 1024;

pub const MAX_TENANTS: usize = 8;
pub const MAX_TILES: usize = 8;
pub const MAX_BANKS: usize = 4;
pub const MAX_TASKS: usize = 8;
