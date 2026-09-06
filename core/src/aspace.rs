//! Per-task address-space contract (x86_64 2 MiB identity subset).
//!
//! The kernel clones the trampoline's 4 GiB identity map into a fresh
//! PML4 / PDPT / PD set and sets USER only on one 2 MiB window. Other
//! known user windows are unmapped (`P=0`). This module is the same
//! walk / flag logic the kernel uses, so host tests can prove
//! task-local USER leaves without QEMU.
//!
//! Honest limits: no higher-half, no KASLR, no PCID, no COW, no
//! POSIX `mmap`. Kernel mappings stay identity-mapped and
//! supervisor-only.

pub const PTE_P: u64 = 1;
pub const PTE_RW: u64 = 1 << 1;
pub const PTE_US: u64 = 1 << 2;
pub const PTE_PS: u64 = 1 << 7;
pub const PAGE_2M: u64 = 0x20_0000;

/// CR4.SMEP (Intel SDM Vol. 3A). Supervisor cannot execute USER pages.
pub const CR4_SMEP: u64 = 1 << 20;
/// CR4.SMAP. Supervisor cannot touch USER pages unless RFLAGS.AC (STAC).
pub const CR4_SMAP: u64 = 1 << 21;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walk {
    pub pde: u64,
    pub phys: u64,
    pub user: bool,
    pub present: bool,
    pub huge_2m: bool,
}

/// Software 4 GiB identity map (PML4[0] + 4× 1 GiB PDs of 2 MiB leaves).
#[derive(Clone, Debug)]
pub struct IdentityAs {
    pub pml4: [u64; 512],
    pub pdpt: [u64; 512],
    pub pd: [[u64; 512]; 4],
}

impl IdentityAs {
    pub fn empty() -> Self {
        Self {
            pml4: [0; 512],
            pdpt: [0; 512],
            pd: [[0; 512]; 4],
        }
    }

    /// Trampoline-shaped kernel map: 4 GiB identity, no USER bits.
    pub fn kernel() -> Self {
        let mut s = Self::empty();
        s.pml4[0] = 0x2000 | PTE_P | PTE_RW;
        for i in 0..4 {
            s.pdpt[i] = (0x3000 + i as u64 * 0x1000) | PTE_P | PTE_RW;
            for j in 0..512 {
                let phys = (i as u64 * 0x4000_0000) + (j as u64 * PAGE_2M);
                s.pd[i][j] = phys | PTE_P | PTE_RW | PTE_PS;
            }
        }
        s
    }

    /// Clone the kernel map. USER only on `[user_lo, user_hi)`. Each
    /// address in `unmap` has its 2 MiB leaf Present bit cleared.
    pub fn clone_user(&self, user_lo: u64, user_hi: u64, unmap: &[u64]) -> Self {
        let mut s = self.clone();
        s.pml4[0] |= PTE_US;
        let i3 = ((user_lo >> 30) & 0x1FF) as usize;
        if i3 < 4 {
            s.pdpt[i3] |= PTE_US;
        }
        let mut va = user_lo & !(PAGE_2M - 1);
        while va < user_hi {
            let gi = ((va >> 30) & 0x1FF) as usize;
            let i2 = ((va >> 21) & 0x1FF) as usize;
            if gi < 4 {
                s.pd[gi][i2] |= PTE_US;
            }
            va += PAGE_2M;
        }
        for &u in unmap {
            let gi = ((u >> 30) & 0x1FF) as usize;
            let i2 = ((u >> 21) & 0x1FF) as usize;
            if gi < 4 {
                s.pd[gi][i2] &= !PTE_P;
            }
        }
        s
    }

    pub fn walk(&self, va: u64) -> Option<Walk> {
        let i4 = ((va >> 39) & 0x1FF) as usize;
        let pml4e = self.pml4[i4];
        if pml4e & PTE_P == 0 {
            return None;
        }
        let i3 = ((va >> 30) & 0x1FF) as usize;
        if i3 >= 4 {
            return None;
        }
        let pdpte = self.pdpt[i3];
        if pdpte & PTE_P == 0 {
            return None;
        }
        let i2 = ((va >> 21) & 0x1FF) as usize;
        let pde = self.pd[i3][i2];
        if pde & PTE_P == 0 {
            return None;
        }
        Some(Walk {
            pde,
            phys: (pde & 0x000F_FFFF_FFE0_0000) | (va & 0x1F_FFFF),
            user: pml4e & PTE_US != 0 && pdpte & PTE_US != 0 && pde & PTE_US != 0,
            present: true,
            huge_2m: pde & PTE_PS != 0,
        })
    }

    pub fn user_mapped(&self, va: u64) -> bool {
        self.walk(va).map(|w| w.present && w.user).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysnr::{
        USER_IMAGE_BASE, USER_IMAGE_END, USER_PROBE_BASE, USER_PROBE_END,
    };

    #[test]
    fn cr4_bits_match_intel() {
        assert_eq!(CR4_SMEP, 1 << 20);
        assert_eq!(CR4_SMAP, 1 << 21);
        assert_ne!(CR4_SMEP, CR4_SMAP);
    }

    #[test]
    fn kernel_identity_has_no_user_leaves() {
        let k = IdentityAs::kernel();
        let w = k.walk(USER_IMAGE_BASE).unwrap();
        assert!(w.present && w.huge_2m);
        assert!(!w.user);
        assert!(!k.user_mapped(USER_IMAGE_BASE));
        assert!(!k.user_mapped(USER_PROBE_BASE));
        assert!(!k.user_mapped(0x400000));
        assert_eq!(k.walk(0x400000).unwrap().phys, 0x400000);
    }

    #[test]
    fn user_leaves_are_task_local() {
        let k = IdentityAs::kernel();
        let init = k.clone_user(USER_IMAGE_BASE, USER_IMAGE_END, &[USER_PROBE_BASE]);
        let probe = k.clone_user(USER_PROBE_BASE, USER_PROBE_END, &[USER_IMAGE_BASE]);

        assert!(init.user_mapped(USER_IMAGE_BASE));
        assert!(init.user_mapped(USER_IMAGE_END - 8));
        assert!(!init.user_mapped(USER_PROBE_BASE));
        assert!(init.walk(USER_PROBE_BASE).is_none());
        assert!(!init.user_mapped(0x400000));
        assert!(init.walk(0x400000).unwrap().present);

        assert!(probe.user_mapped(USER_PROBE_BASE));
        assert!(!probe.user_mapped(USER_IMAGE_BASE));
        assert!(probe.walk(USER_IMAGE_BASE).is_none());
        assert!(!probe.user_mapped(0x400000));
    }

    #[test]
    fn windows_do_not_overlap() {
        assert!(USER_IMAGE_END <= USER_PROBE_BASE);
        assert!(USER_IMAGE_END - USER_IMAGE_BASE == PAGE_2M);
        assert!(USER_PROBE_END - USER_PROBE_BASE == PAGE_2M);
    }
}
