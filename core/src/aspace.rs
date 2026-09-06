//! Per-task address-space contract (x86_64 2 MiB identity + HH subset).
//!
//! The kernel clones the trampoline's 4 GiB identity map into a fresh
//! PML4 / PDPT / PD set and sets USER only on one 2 MiB window. Other
//! known user windows are unmapped (`P=0`). PML4[511] aliases the first
//! 2 GiB at the classic `-2 GiB` kernel map (`KERNEL_VMA + PA`). This
//! module is the same walk / flag logic the kernel uses, so host tests
//! can prove task-local USER leaves and the HH alias without QEMU.
//!
//! Honest limits: no KASLR, no KPTI, no PCID, no COW, no POSIX `mmap`.
//! The identity 4 GiB stays mapped on purpose (SoftNPU DMA, page-table
//! walks, AP SIPI, user ELF windows).

pub const PTE_P: u64 = 1;
pub const PTE_RW: u64 = 1 << 1;
pub const PTE_US: u64 = 1 << 2;
pub const PTE_PS: u64 = 1 << 7;
pub const PAGE_2M: u64 = 0x20_0000;

/// Classic x86_64 `-2 GiB` kernel map (Linux `__START_KERNEL_map`).
pub const KERNEL_VMA: u64 = 0xFFFF_FFFF_8000_0000;
/// Physical load address of the kernel image (trampoline copy).
pub const KERNEL_LMA: u64 = 0x40_0000;
/// Linked VA of kernel `_start` (`KERNEL_VMA + KERNEL_LMA`).
pub const KERNEL_TEXT_VA: u64 = KERNEL_VMA + KERNEL_LMA;
/// HH window covers PA `0..2 GiB` (the canonical `-2 GiB` hole).
pub const KERNEL_HH_SPAN: u64 = 0x8000_0000;

/// `PA → KERNEL_VMA + PA` when `PA` fits in the HH 2 GiB window.
pub fn phys_to_hh(pa: u64) -> Option<u64> {
    if pa < KERNEL_HH_SPAN {
        Some(KERNEL_VMA + pa)
    } else {
        None
    }
}

/// Inverse of [`phys_to_hh`].
pub fn hh_to_phys(va: u64) -> Option<u64> {
    va.checked_sub(KERNEL_VMA).filter(|&p| p < KERNEL_HH_SPAN)
}

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

    /// Trampoline-shaped kernel map: 4 GiB identity + HH alias, no USER bits.
    pub fn kernel() -> Self {
        let mut s = Self::empty();
        s.pml4[0] = 0x2000 | PTE_P | PTE_RW;
        s.pml4[511] = 0x2000 | PTE_P | PTE_RW;
        for i in 0..4 {
            s.pdpt[i] = (0x3000 + i as u64 * 0x1000) | PTE_P | PTE_RW;
            for j in 0..512 {
                let phys = (i as u64 * 0x4000_0000) + (j as u64 * PAGE_2M);
                s.pd[i][j] = phys | PTE_P | PTE_RW | PTE_PS;
            }
        }
        // -2 GiB window aliases the first 2 GiB of identity PDs.
        s.pdpt[510] = 0x3000 | PTE_P | PTE_RW;
        s.pdpt[511] = 0x4000 | PTE_P | PTE_RW;
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

    fn pd_slot(i3: usize) -> Option<usize> {
        match i3 {
            0..=3 => Some(i3),
            510 => Some(0),
            511 => Some(1),
            _ => None,
        }
    }

    pub fn walk(&self, va: u64) -> Option<Walk> {
        let i4 = ((va >> 39) & 0x1FF) as usize;
        let pml4e = self.pml4[i4];
        if pml4e & PTE_P == 0 {
            return None;
        }
        let i3 = ((va >> 30) & 0x1FF) as usize;
        let Some(pd_i) = Self::pd_slot(i3) else {
            return None;
        };
        let pdpte = self.pdpt[i3];
        if pdpte & PTE_P == 0 {
            return None;
        }
        let i2 = ((va >> 21) & 0x1FF) as usize;
        let pde = self.pd[pd_i][i2];
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
        assert!(!k.user_mapped(KERNEL_LMA));
        assert_eq!(k.walk(KERNEL_LMA).unwrap().phys, KERNEL_LMA);
    }

    #[test]
    fn higher_half_aliases_low_phys() {
        assert_eq!(phys_to_hh(KERNEL_LMA), Some(KERNEL_TEXT_VA));
        assert_eq!(hh_to_phys(KERNEL_TEXT_VA), Some(KERNEL_LMA));
        assert!(phys_to_hh(KERNEL_HH_SPAN).is_none());

        let k = IdentityAs::kernel();
        let w = k.walk(KERNEL_TEXT_VA).unwrap();
        assert!(w.present && w.huge_2m);
        assert!(!w.user);
        assert_eq!(w.phys, KERNEL_LMA);
        assert!(!k.user_mapped(KERNEL_TEXT_VA));
        assert_eq!(k.walk(KERNEL_LMA).unwrap().phys, KERNEL_LMA);

        let init = k.clone_user(USER_IMAGE_BASE, USER_IMAGE_END, &[USER_PROBE_BASE]);
        assert!(!init.user_mapped(KERNEL_TEXT_VA));
        assert_eq!(init.walk(KERNEL_TEXT_VA).unwrap().phys, KERNEL_LMA);
        assert!(init.user_mapped(USER_IMAGE_BASE));
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

/// Sv39 PTE bits (privileged spec). U is only meaningful on a leaf.
pub const SV39_V: u64 = 1;
pub const SV39_R: u64 = 1 << 1;
pub const SV39_W: u64 = 1 << 2;
pub const SV39_X: u64 = 1 << 3;
pub const SV39_U: u64 = 1 << 4;
pub const SV39_G: u64 = 1 << 5;
pub const SV39_A: u64 = 1 << 6;
pub const SV39_D: u64 = 1 << 7;
pub const SV39_LEAF: u64 = SV39_R;
pub const PAGE_1G: u64 = 0x4000_0000;

/// Software Sv39 identity map (4× 1 GiB root leaves). `clone_user`
/// splits the 1 GiB that holds the user window into 2 MiB pages and
/// sets U only there. Host-tested twin of the RISC-V kernel walk.
#[derive(Clone, Debug)]
pub struct Sv39As {
    pub l2: [u64; 512],
    pub l1: [u64; 512],
    pub split_vpn2: usize,
}

impl Sv39As {
    pub fn empty() -> Self {
        Self {
            l2: [0; 512],
            l1: [0; 512],
            split_vpn2: usize::MAX,
        }
    }

    fn gig_pte(i: u64) -> u64 {
        (i << 28) | SV39_V | SV39_R | SV39_W | SV39_X | SV39_G | SV39_A | SV39_D
    }

    fn meg_pte(phys: u64, user: bool) -> u64 {
        let mut flags = SV39_V | SV39_R | SV39_W | SV39_X | SV39_A | SV39_D;
        if user {
            flags |= SV39_U;
        } else {
            flags |= SV39_G;
        }
        (phys >> 2) | flags
    }

    /// Trampoline-shaped kernel map: 4 GiB identity, no U bits.
    pub fn kernel() -> Self {
        let mut s = Self::empty();
        for i in 0..4u64 {
            s.l2[i as usize] = Self::gig_pte(i);
        }
        s
    }

    /// Clone the kernel map. U only on `[user_lo, user_hi)` 2 MiB
    /// leaves. Each address in `unmap` has its 2 MiB V bit cleared.
    pub fn clone_user(&self, user_lo: u64, user_hi: u64, unmap: &[u64]) -> Self {
        let mut s = self.clone();
        let vpn2 = ((user_lo >> 30) & 0x1FF) as usize;
        s.split_vpn2 = vpn2;
        let gphys = (vpn2 as u64) * PAGE_1G;
        for j in 0..512u64 {
            s.l1[j as usize] = Self::meg_pte(gphys + j * PAGE_2M, false);
        }
        s.l2[vpn2] = SV39_V;
        let mut va = user_lo & !(PAGE_2M - 1);
        while va < user_hi {
            if ((va >> 30) & 0x1FF) as usize == vpn2 {
                let i1 = ((va >> 21) & 0x1FF) as usize;
                s.l1[i1] = Self::meg_pte(va & !(PAGE_2M - 1), true);
            }
            va += PAGE_2M;
        }
        for &u in unmap {
            if ((u >> 30) & 0x1FF) as usize == vpn2 {
                let i1 = ((u >> 21) & 0x1FF) as usize;
                s.l1[i1] &= !SV39_V;
            }
        }
        s
    }

    pub fn walk(&self, va: u64) -> Option<Walk> {
        let i2 = ((va >> 30) & 0x1FF) as usize;
        let pte2 = self.l2[i2];
        if pte2 & SV39_V == 0 {
            return None;
        }
        if pte2 & SV39_LEAF != 0 {
            return Some(Walk {
                pde: pte2,
                phys: (pte2 << 2) & !0x3FFF_FFFF | (va & 0x3FFF_FFFF),
                user: pte2 & SV39_U != 0,
                present: true,
                huge_2m: true,
            });
        }
        if i2 != self.split_vpn2 {
            return None;
        }
        let i1 = ((va >> 21) & 0x1FF) as usize;
        let pte1 = self.l1[i1];
        if pte1 & SV39_V == 0 {
            return None;
        }
        Some(Walk {
            pde: pte1,
            phys: (pte1 << 2) & !0x1F_FFFF | (va & 0x1F_FFFF),
            user: pte1 & SV39_U != 0,
            present: true,
            huge_2m: pte1 & SV39_LEAF != 0,
        })
    }

    pub fn user_mapped(&self, va: u64) -> bool {
        self.walk(va).map(|w| w.present && w.user).unwrap_or(false)
    }
}

#[cfg(test)]
mod sv39_tests {
    use super::*;
    use crate::sysnr::{USER_RV_IMAGE_BASE, USER_RV_IMAGE_END};

    #[test]
    fn kernel_identity_has_no_u_leaves() {
        let k = Sv39As::kernel();
        let w = k.walk(USER_RV_IMAGE_BASE).unwrap();
        assert!(w.present && w.huge_2m);
        assert!(!w.user);
        assert!(!k.user_mapped(USER_RV_IMAGE_BASE));
        assert!(!k.user_mapped(0x8020_0000));
        assert_eq!(k.walk(0x8020_0000).unwrap().phys, 0x8020_0000);
        assert_eq!(k.walk(0x1000_0000).unwrap().phys, 0x1000_0000);
    }

    #[test]
    fn user_leaves_are_task_local() {
        let k = Sv39As::kernel();
        let init = k.clone_user(USER_RV_IMAGE_BASE, USER_RV_IMAGE_END, &[]);

        assert!(init.user_mapped(USER_RV_IMAGE_BASE));
        assert!(init.user_mapped(USER_RV_IMAGE_END - 8));
        assert!(!init.user_mapped(0x8020_0000));
        assert!(init.walk(0x8020_0000).unwrap().present);
        assert!(!init.walk(0x8020_0000).unwrap().user);
        assert!(!init.user_mapped(0x1000_0000));
        assert!(init.walk(0x1000_0000).unwrap().present);
        assert!(!k.user_mapped(USER_RV_IMAGE_BASE));
    }

    #[test]
    fn rv_window_is_2m_in_ram() {
        assert_eq!(USER_RV_IMAGE_END - USER_RV_IMAGE_BASE, PAGE_2M);
        assert!(USER_RV_IMAGE_BASE >= 0x8000_0000);
        assert!(USER_RV_IMAGE_BASE < 0x8800_0000);
    }
}
