//! Page-table walk helpers on the trampoline's identity map.
//!
//! Adding a non-identity mapping is how a future ELF loader isolates
//! tenants. v0.1 only walks and reports.

use aether_core::types::PhysAddr;

#[cfg(target_arch = "x86_64")]
const P: u64 = 1;
#[cfg(target_arch = "x86_64")]
const RW: u64 = 1 << 1;
#[cfg(target_arch = "x86_64")]
const US: u64 = 1 << 2;
#[cfg(target_arch = "x86_64")]
const PS: u64 = 1 << 7;

#[derive(Clone, Copy, Debug)]
pub struct Walk {
    pub pml4e: u64,
    pub pdpte: u64,
    pub pde: u64,
    pub phys: PhysAddr,
    pub huge_2m: bool,
}

#[cfg(target_arch = "x86_64")]
pub unsafe fn cr3() -> u64 {
    let v: u64;
    core::arch::asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags));
    v
}

/// Walk `va` in the current address space (identity-mapped tables).
#[cfg(target_arch = "x86_64")]
pub unsafe fn walk(va: u64) -> Option<Walk> {
    let pml4 = (cr3() & !0xFFF) as *const u64;
    let i4 = ((va >> 39) & 0x1FF) as usize;
    let i3 = ((va >> 30) & 0x1FF) as usize;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let pml4e = core::ptr::read_volatile(pml4.add(i4));
    if pml4e & P == 0 {
        return None;
    }
    let pdpt = (pml4e & 0x000F_FFFF_FFFF_F000) as *const u64;
    let pdpte = core::ptr::read_volatile(pdpt.add(i3));
    if pdpte & P == 0 {
        return None;
    }
    if pdpte & PS != 0 {
        let phys = (pdpte & 0x000F_FFFF_C000_0000) | (va & 0x3FFF_FFFF);
        return Some(Walk {
            pml4e,
            pdpte,
            pde: 0,
            phys: PhysAddr(phys),
            huge_2m: false,
        });
    }
    let pd = (pdpte & 0x000F_FFFF_FFFF_F000) as *const u64;
    let pde = core::ptr::read_volatile(pd.add(i2));
    if pde & P == 0 {
        return None;
    }
    if pde & PS != 0 {
        let phys = (pde & 0x000F_FFFF_FFE0_0000) | (va & 0x1F_FFFF);
        return Some(Walk {
            pml4e,
            pdpte,
            pde,
            phys: PhysAddr(phys),
            huge_2m: true,
        });
    }
    let i1 = ((va >> 12) & 0x1FF) as usize;
    let pt = (pde & 0x000F_FFFF_FFFF_F000) as *const u64;
    let pte = core::ptr::read_volatile(pt.add(i1));
    if pte & P == 0 {
        return None;
    }
    let phys = (pte & 0x000F_FFFF_FFFF_F000) | (va & 0xFFF);
    Some(Walk {
        pml4e,
        pdpte,
        pde,
        phys: PhysAddr(phys),
        huge_2m: false,
    })
}

#[cfg(target_arch = "x86_64")]
fn invlpg(va: u64) {
    unsafe {
        core::arch::asm!("invlpg [{0}]", in(reg) va, options(nostack, preserves_flags));
    }
}

/// Set USER on PML4[0] and PDPT[0] so ring-3 can walk the low 1 GiB.
/// Leaf pages stay supervisor-only until [`allow_user_2m`].
#[cfg(target_arch = "x86_64")]
pub fn allow_user_walk_low() {
    unsafe {
        let pml4 = (cr3() & !0xFFF) as *mut u64;
        let pml4e = core::ptr::read_volatile(pml4);
        core::ptr::write_volatile(pml4, pml4e | US);
        let pdpt = (pml4e & 0x000F_FFFF_FFFF_F000) as *mut u64;
        let pdpte = core::ptr::read_volatile(pdpt);
        core::ptr::write_volatile(pdpt, pdpte | US);
    }
}

/// Mark the 2 MiB page covering `va` user-accessible (identity map).
#[cfg(target_arch = "x86_64")]
pub fn allow_user_2m(va: u64) {
    unsafe {
        let pml4 = (cr3() & !0xFFF) as *mut u64;
        let pml4e = core::ptr::read_volatile(pml4);
        if pml4e & P == 0 {
            return;
        }
        let pdpt = (pml4e & 0x000F_FFFF_FFFF_F000) as *mut u64;
        let i3 = ((va >> 30) & 0x1FF) as usize;
        let pdpte = core::ptr::read_volatile(pdpt.add(i3));
        if pdpte & P == 0 || pdpte & PS != 0 {
            return;
        }
        core::ptr::write_volatile(pdpt.add(i3), pdpte | US);
        let pd = (pdpte & 0x000F_FFFF_FFFF_F000) as *mut u64;
        let i2 = ((va >> 21) & 0x1FF) as usize;
        let pde = core::ptr::read_volatile(pd.add(i2));
        if pde & P == 0 {
            return;
        }
        core::ptr::write_volatile(pd.add(i2), pde | US);
        invlpg(va);
    }
}

#[cfg(target_arch = "x86_64")]
pub fn flags_rw() -> u64 {
    P | RW
}

#[cfg(target_arch = "riscv64")]
const PTE_V: u64 = 1;
#[cfg(target_arch = "riscv64")]
const PTE_R: u64 = 1 << 1;
#[cfg(target_arch = "riscv64")]
const PTE_LEAF: u64 = PTE_R;

#[cfg(target_arch = "riscv64")]
pub unsafe fn satp() -> u64 {
    let v: u64;
    core::arch::asm!("csrr {v}, satp", v = out(reg) v, options(nomem, nostack));
    v
}

/// Sv39 walk. A 1 GiB identity leaf is reported as `huge_2m = true`.
#[cfg(target_arch = "riscv64")]
pub unsafe fn walk(va: u64) -> Option<Walk> {
    let satp = satp();
    let mode = satp >> 60;
    if mode != 8 {
        return None;
    }
    let root = ((satp & 0x0000_0FFF_FFFF_FFFF) << 12) as *const u64;
    let i2 = ((va >> 30) & 0x1FF) as usize;
    let pte = core::ptr::read_volatile(root.add(i2));
    if pte & PTE_V == 0 {
        return None;
    }
    if pte & PTE_LEAF != 0 {
        let ppn = (pte >> 10) & 0x0FFF_FFFF_FFFF;
        let phys = (ppn << 12) | (va & 0x3FFF_FFFF);
        return Some(Walk {
            pml4e: pte,
            pdpte: 0,
            pde: 0,
            phys: PhysAddr(phys),
            huge_2m: true,
        });
    }
    None
}
