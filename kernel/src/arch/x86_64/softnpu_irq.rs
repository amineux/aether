//! SoftNPU used-ring doorbell via local APIC self-IPI (path B).
//!
//! SoftNPU stays the in-kernel [`aether_drivers::mmio::AccelMmio`] BAR.
//! Completions need a **real interrupt path** on this HAL. RISC-V uses
//! UART THRE → PLIC; on x86 the doorbell is a **LAPIC self-IPI** on
//! vector 49 (in-tree APIC plumbing). The KPTI shadow IDT must carry
//! a dedicated stub for that vector — the generic trampoline gate
//! pushes `0xFF` and would drop the claim.
//!
//! Still path B: not virtio-mmio, not MSI-X device IRQ, not HW SMMU.

use super::apic;
use crate::println;

/// Dedicated SoftNPU completion vector (fixed delivery, BSP self-IPI).
/// Distinct from SMP IPI vector 48.
pub const SOFTNPU_VEC: u8 = 49;

pub fn init() {
    println!("[boot] APIC SoftNPU doorbell = self-IPI vec 49 (path B BAR)");
}

/// Kick SoftNPU completion. Call after AccelMmio doorbell (APIC must be on).
pub fn raise_softnpu_doorbell() {
    apic::ipi_self(SOFTNPU_VEC);
}

pub fn ack_softnpu_doorbell() {}
