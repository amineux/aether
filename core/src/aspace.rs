//! Per-task address-space contract (x86_64 2 MiB identity + HH subset,
//! RISC-V Sv39, aarch64 TTBR0).
//!
//! The kernel clones the trampoline's 4 GiB identity map into a fresh
//! PML4 / PDPT / PD set and sets USER only on one 2 MiB window. Other
//! known user windows are unmapped (`P=0`). PML4[511] aliases the first
//! 2 GiB at the classic `-2 GiB` kernel map (`KERNEL_VMA + PA`). A
//! boot-time KASLR slide dual-maps an 8 MiB kernel span at
//! `KERNEL_VMA + slide + PA` on **separate** HH PDs so identity DMA /
//! SIPI / user windows do not move. The kernel is still linked at
//! `KERNEL_TEXT_VA` (`code-model=kernel`); the unused alias stays so
//! absolute symbols keep working. This is not PIE / reloc, not a
//! POSIX `mmap`. A documented **COW subset** maps one shared 4 KiB
//! USER page (`USER_COW_BASE`) read-only in `/init` and `/probe`; a
//! write fault copies the frame and sets RW on that aspace only.
//!
//! **KPTI subset:** `clone_user` does **not** copy PML4[511] or the
//! identity 4 GiB. User CR3 maps the task's 2 MiB ELF window (USER)
//! plus four supervisor 4 KiB trampoline pages at [`KPTI_TRAMP_VA`]
//! (syscall/IRQ entry). Kernel CR3 keeps identity + HH so SoftNPU
//! `IdentityDma`, AP SIPI, and page-table PA walks still work. Not a
//! Meltdown-complete KAISER claim (trampoline pages stay mapped).
//!
//! **PCID subset:** [`cr3_tagged`] / [`PcidAlloc`] are the host twin of
//! kernel CR3 tagging (kernel PCID 1, per-aspace user PCIDs from 2).
//! `SYS_CLONE` shares a PML4 and therefore a PCID. No PCID is a
//! full-flush `mov cr3` fallback — not a Meltdown claim.
//!
//! `SYS_CLONE` user threads share one of these maps; they do not get
//! a second PML4. After a COW break they see the same private page.

pub const PTE_P: u64 = 1;
pub const PTE_RW: u64 = 1 << 1;
pub const PTE_US: u64 = 1 << 2;
pub const PTE_PS: u64 = 1 << 7;
pub const PAGE_2M: u64 = 0x20_0000;
pub const PAGE_4K: u64 = 0x1000;

/// Classic x86_64 `-2 GiB` kernel map (Linux `__START_KERNEL_map`).
pub const KERNEL_VMA: u64 = 0xFFFF_FFFF_8000_0000;
/// Physical load address of the kernel image (trampoline copy).
pub const KERNEL_LMA: u64 = 0x40_0000;
/// Linked VA of kernel `_start` (`KERNEL_VMA + KERNEL_LMA`).
pub const KERNEL_TEXT_VA: u64 = KERNEL_VMA + KERNEL_LMA;
/// HH window covers PA `0..2 GiB` (the canonical `-2 GiB` hole).
pub const KERNEL_HH_SPAN: u64 = 0x8000_0000;

/// Boot-time slide stride. Three slots: 0, 16 MiB, 32 MiB.
/// All stay inside the last 2 GiB so `code-model=kernel` 32-bit
/// signed addresses still resolve on the canonical alias.
pub const KASLR_SLIDE_STRIDE: u64 = 0x0100_0000;
/// Number of legal slide indices (`kaslr=0|1|2`).
pub const KASLR_SLIDE_COUNT: u32 = 3;
/// Dual-mapped kernel span on the HH PDs (covers `.text` + BSS stack).
pub const KASLR_KERNEL_SPAN: u64 = 0x80_0000;
/// Trampoline mailbox: Multiboot magic at `+0`, info PA at `+4`,
/// selected slide bytes at `+8`.
pub const KASLR_MAILBOX: u64 = 0x7000;
/// Slide bytes written by the trampoline (u32).
pub const KASLR_MAILBOX_SLIDE: u64 = KASLR_MAILBOX + 8;
/// Dedicated HH PD0 (identity PDs stay at `0x3000`…).
pub const KASLR_HH_PD0: u64 = 0x7_1000;
/// Dedicated HH PD1.
pub const KASLR_HH_PD1: u64 = 0x7_2000;

/// Identity VA/PA of the KPTI trampoline (code + GDT/TSS + slots).
/// After HH PDs (`0x71000` / `0x72000`), before the kernel LMA.
pub const KPTI_TRAMP_VA: u64 = 0x7_3000;
/// Shadow IDT (256 × 16 bytes).
pub const KPTI_TRAMP_IDT: u64 = 0x7_4000;
/// Entry stack (two pages). Grows down from [`KPTI_TRAMP_STACK_TOP`].
pub const KPTI_TRAMP_STACK: u64 = 0x7_5000;
/// TSS.RSP0 / scratch stack top (16-byte aligned).
pub const KPTI_TRAMP_STACK_TOP: u64 = 0x7_7000;
/// Supervisor-only 4 KiB pages mapped in user CR3.
pub const KPTI_TRAMP_PAS: [u64; 4] = [0x7_3000, 0x7_4000, 0x7_5000, 0x7_6000];
/// Slots at the end of the trampoline code page (`kernel_cr3`, …).
pub const KPTI_SLOT_BASE: u64 = 0x7_3F00;

/// `PA → KERNEL_VMA + PA` when `PA` fits in the HH 2 GiB window.
pub fn phys_to_hh(pa: u64) -> Option<u64> {
    if pa < KERNEL_HH_SPAN {
        Some(KERNEL_VMA + pa)
    } else {
        None
    }
}

/// Inverse of [`phys_to_hh`] (canonical alias only; ignores a slide).
pub fn hh_to_phys(va: u64) -> Option<u64> {
    va.checked_sub(KERNEL_VMA).filter(|&p| p < KERNEL_HH_SPAN)
}

/// Slide bytes for index `0..KASLR_SLIDE_COUNT`, or `None`.
pub fn kaslr_slide(index: u32) -> Option<u64> {
    if index < KASLR_SLIDE_COUNT {
        Some(index as u64 * KASLR_SLIDE_STRIDE)
    } else {
        None
    }
}

/// True when `slide` is one of the three CI-legal offsets.
pub fn kaslr_slide_valid(slide: u64) -> bool {
    slide % KASLR_SLIDE_STRIDE == 0 && slide / KASLR_SLIDE_STRIDE < KASLR_SLIDE_COUNT as u64
}

/// `PA → KERNEL_VMA + slide + PA` when both fit the HH window.
pub fn phys_to_hh_slid(pa: u64, slide: u64) -> Option<u64> {
    if pa < KERNEL_HH_SPAN && kaslr_slide_valid(slide) {
        KERNEL_VMA.checked_add(slide)?.checked_add(pa)
    } else {
        None
    }
}

/// Linked `_start` plus a legal slide.
pub fn kernel_text_va_slid(slide: u64) -> Option<u64> {
    if kaslr_slide_valid(slide) {
        KERNEL_TEXT_VA.checked_add(slide)
    } else {
        None
    }
}

/// Parse a Multiboot cmdline for `kaslr=0|1|2|off`.
///
/// `None` means the trampoline should pick entropy (RDRAND / TSC).
/// Unknown values (`kaslr=9`) are also `None`. `kaslr=off` is index 0.
pub fn parse_kaslr_cmdline(cmd: &[u8]) -> Option<u32> {
    let needle = b"kaslr=";
    let mut i = 0;
    while i + needle.len() <= cmd.len() {
        let at_token = i == 0 || cmd[i - 1] == b' ' || cmd[i - 1] == b'\t';
        if at_token && cmd[i..].starts_with(needle) {
            let rest = &cmd[i + needle.len()..];
            if rest.starts_with(b"off") {
                let after = rest.get(3).copied().unwrap_or(0);
                if after == 0 || after == b' ' || after == b'\t' {
                    return Some(0);
                }
            }
            if let Some(&b) = rest.first() {
                if (b == b'0' || b == b'1' || b == b'2')
                    && rest.get(1).map(|c| !c.is_ascii_digit()).unwrap_or(true)
                {
                    return Some((b - b'0') as u32);
                }
            }
            return None;
        }
        i += 1;
    }
    None
}

/// CR4.SMEP (Intel SDM Vol. 3A). Supervisor cannot execute USER pages.
pub const CR4_SMEP: u64 = 1 << 20;
/// CR4.SMAP. Supervisor cannot touch USER pages unless RFLAGS.AC (STAC).
pub const CR4_SMAP: u64 = 1 << 21;
/// CR4.PCIDE. CR3[11:0] is a PCID; CR3[63] is the no-flush bit.
pub const CR4_PCIDE: u64 = 1 << 17;
/// CR3 PCID field (12 bits) when CR4.PCIDE is set.
pub const CR3_PCID_MASK: u64 = 0xFFF;
/// MOV-to-CR3 no-invalidate bit (Intel SDM Vol. 3A §4.10.4.1).
pub const CR3_NOFLUSH: u64 = 1 << 63;
/// Kernel aspace PCID. 0 is reserved (must be current when enabling PCIDE).
pub const PCID_KERNEL: u16 = 1;
/// First user aspace PCID (`/init`). `/probe` and later maps increment.
pub const PCID_USER_BASE: u16 = 2;
/// INVPCID type 0: one linear address in one PCID.
pub const INVPCID_INDIV: u64 = 0;
/// INVPCID type 1: all translations for one PCID (not global).
pub const INVPCID_SINGLE: u64 = 1;
/// INVPCID type 2: all non-global translations, all PCIDs.
pub const INVPCID_ALL: u64 = 2;
/// INVPCID type 3: all translations including global.
pub const INVPCID_ALL_GLOBAL: u64 = 3;

/// Pack a PML4 PA + PCID, optionally setting the no-flush bit.
///
/// When `pcid_on` is false this is just the page-aligned PA (full-flush
/// `mov cr3`). Host twin of the kernel tagged-CR3 helper.
pub fn cr3_tagged(pa: u64, pcid: u16, noflush: bool, pcid_on: bool) -> u64 {
    let pa = pa & !CR3_PCID_MASK;
    if !pcid_on {
        return pa;
    }
    let mut v = pa | (pcid as u64 & CR3_PCID_MASK);
    if noflush {
        v |= CR3_NOFLUSH;
    }
    v
}

/// PML4 physical address from a (possibly tagged) CR3 value.
pub fn cr3_pa(cr3: u64) -> u64 {
    cr3 & !CR3_PCID_MASK & !CR3_NOFLUSH
}

/// PCID field from a tagged CR3 value (`0` when the value is PA-only).
pub fn cr3_pcid(cr3: u64) -> u16 {
    (cr3 & CR3_PCID_MASK) as u16
}

/// Tiny PCID allocator: kernel = 1; each distinct CR3 PA gets the next
/// id from [`PCID_USER_BASE`]. Clone threads that share a PML4 share a
/// PCID. Host twin of the kernel table in `paging.rs`.
#[derive(Clone, Debug)]
pub struct PcidAlloc {
    next: u16,
    slots: [(u64, u16); 8],
    n: usize,
}

impl PcidAlloc {
    pub fn new() -> Self {
        Self {
            next: PCID_USER_BASE,
            slots: [(0, 0); 8],
            n: 0,
        }
    }

    pub fn kernel() -> u16 {
        PCID_KERNEL
    }

    /// Assign (or look up) a PCID for `cr3_pa`. Kernel PA → 1.
    pub fn assign(&mut self, cr3_pa: u64, kernel_pa: u64) -> u16 {
        let pa = cr3_pa & !CR3_PCID_MASK;
        let k = kernel_pa & !CR3_PCID_MASK;
        if pa == 0 || pa == k {
            return PCID_KERNEL;
        }
        for i in 0..self.n {
            if self.slots[i].0 == pa {
                return self.slots[i].1;
            }
        }
        let id = self.next;
        self.next = if id >= 4095 { PCID_USER_BASE } else { id + 1 };
        if self.n < self.slots.len() {
            self.slots[self.n] = (pa, id);
            self.n += 1;
        }
        id
    }

    pub fn of(&self, cr3_pa: u64, kernel_pa: u64) -> u16 {
        let pa = cr3_pa & !CR3_PCID_MASK;
        let k = kernel_pa & !CR3_PCID_MASK;
        if pa == 0 || pa == k {
            return PCID_KERNEL;
        }
        for i in 0..self.n {
            if self.slots[i].0 == pa {
                return self.slots[i].1;
            }
        }
        PCID_KERNEL
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walk {
    pub pde: u64,
    pub phys: u64,
    pub user: bool,
    pub present: bool,
    pub huge_2m: bool,
    pub writable: bool,
}

/// Software 4 GiB identity map (PML4[0] + 4× 1 GiB PDs of 2 MiB leaves).
/// HH uses its own PDs (`hh_pd`) so a KASLR dual-map cannot disturb
/// identity DMA / SIPI / user windows.
#[derive(Clone, Debug)]
pub struct IdentityAs {
    pub pml4: [u64; 512],
    pub pdpt: [u64; 512],
    pub pd: [[u64; 512]; 4],
    pub hh_pd: [[u64; 512]; 2],
    /// Boot-time slide applied to the HH kernel span (bytes).
    pub slide: u64,
    /// KPTI user map: first 2 MiB is a 4 KiB PT (trampoline only).
    /// `None` on the kernel CR3 (2 MiB identity leaves).
    pub tramp_pt: Option<[u64; 512]>,
    /// Optional 4 KiB PT for the shared COW 2 MiB slot (`USER_COW_BASE`).
    pub cow_pt: Option<[u64; 512]>,
    /// PD index (`va >> 21`) that `cow_pt` covers, or `usize::MAX`.
    pub cow_i2: usize,
}

impl IdentityAs {
    pub fn empty() -> Self {
        Self {
            pml4: [0; 512],
            pdpt: [0; 512],
            pd: [[0; 512]; 4],
            hh_pd: [[0; 512]; 2],
            slide: 0,
            tramp_pt: None,
            cow_pt: None,
            cow_i2: usize::MAX,
        }
    }

    /// Trampoline-shaped kernel map: 4 GiB identity + HH alias, no USER bits.
    pub fn kernel() -> Self {
        Self::kernel_with_slide(0)
    }

    /// Same as [`kernel`], plus a dual-map of [`KASLR_KERNEL_SPAN`] at
    /// `KERNEL_VMA + slide + PA`. Identity PDs are never rewritten.
    pub fn kernel_with_slide(slide: u64) -> Self {
        let slide = if kaslr_slide_valid(slide) { slide } else { 0 };
        let mut s = Self::empty();
        s.slide = slide;
        s.pml4[0] = 0x2000 | PTE_P | PTE_RW;
        s.pml4[511] = 0x2000 | PTE_P | PTE_RW;
        for i in 0..4 {
            s.pdpt[i] = (0x3000 + i as u64 * 0x1000) | PTE_P | PTE_RW;
            for j in 0..512 {
                let phys = (i as u64 * 0x4000_0000) + (j as u64 * PAGE_2M);
                s.pd[i][j] = phys | PTE_P | PTE_RW | PTE_PS;
            }
        }
        s.hh_pd[0] = s.pd[0];
        s.hh_pd[1] = s.pd[1];
        // Dedicated HH PDs (trampoline: 0x71000 / 0x72000).
        s.pdpt[510] = KASLR_HH_PD0 | PTE_P | PTE_RW;
        s.pdpt[511] = KASLR_HH_PD1 | PTE_P | PTE_RW;
        if slide != 0 {
            let src = (KERNEL_LMA / PAGE_2M) as usize;
            let dest = src + (slide / PAGE_2M) as usize;
            let n = (KASLR_KERNEL_SPAN / PAGE_2M) as usize;
            for i in 0..n {
                if dest + i < 512 {
                    s.hh_pd[0][dest + i] = s.hh_pd[0][src + i];
                }
            }
        }
        s
    }

    /// KPTI user map: USER only on `[user_lo, user_hi)`, supervisor 4 KiB
    /// trampoline pages, no HH (`PML4[511]=0`), no identity 4 GiB.
    /// Each address in `unmap` has its 2 MiB leaf Present bit cleared.
    pub fn clone_user(&self, user_lo: u64, user_hi: u64, unmap: &[u64]) -> Self {
        let _ = self;
        let mut s = Self::empty();
        s.pml4[0] = 0x2000 | PTE_P | PTE_RW | PTE_US;
        s.pdpt[0] = 0x3000 | PTE_P | PTE_RW | PTE_US;
        let mut va = user_lo & !(PAGE_2M - 1);
        while va < user_hi {
            let gi = ((va >> 30) & 0x1FF) as usize;
            let i2 = ((va >> 21) & 0x1FF) as usize;
            if gi < 4 {
                s.pd[gi][i2] = (va & !(PAGE_2M - 1)) | PTE_P | PTE_RW | PTE_US | PTE_PS;
                if gi != 0 {
                    s.pdpt[gi] = (0x3000 + gi as u64 * 0x1000) | PTE_P | PTE_RW | PTE_US;
                }
            }
            va += PAGE_2M;
        }
        for &u in unmap {
            let gi = ((u >> 30) & 0x1FF) as usize;
            let i2 = ((u >> 21) & 0x1FF) as usize;
            if gi < 4 {
                s.pd[gi][i2] = 0;
            }
        }
        let mut pt = [0u64; 512];
        for &pa in &KPTI_TRAMP_PAS {
            pt[((pa >> 12) & 0x1FF) as usize] = pa | PTE_P | PTE_RW;
        }
        s.tramp_pt = Some(pt);
        // First 2 MiB: table (no PS, no US). Trampoline only.
        s.pd[0][0] = PTE_P | PTE_RW;
        s
    }

    fn pd_leaf(&self, i4: usize, i3: usize) -> Option<&[u64; 512]> {
        match (i4, i3) {
            (0, 0..=3) => Some(&self.pd[i3]),
            (511, 510) => Some(&self.hh_pd[0]),
            (511, 511) => Some(&self.hh_pd[1]),
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
        let Some(pd) = self.pd_leaf(i4, i3) else {
            return None;
        };
        let pdpte = self.pdpt[i3];
        if pdpte & PTE_P == 0 {
            return None;
        }
        let i2 = ((va >> 21) & 0x1FF) as usize;
        let pde = pd[i2];
        if pde & PTE_P == 0 {
            return None;
        }
        if pde & PTE_PS == 0 {
            if i4 == 0 && i3 == 0 && i2 == 0 {
                if let Some(pt) = &self.tramp_pt {
                    let i1 = ((va >> 12) & 0x1FF) as usize;
                    let pte = pt[i1];
                    if pte & PTE_P == 0 {
                        return None;
                    }
                    return Some(Walk {
                        pde: pte,
                        phys: (pte & 0x000F_FFFF_FFFF_F000) | (va & 0xFFF),
                        user: pml4e & PTE_US != 0
                            && pdpte & PTE_US != 0
                            && pde & PTE_US != 0
                            && pte & PTE_US != 0,
                        present: true,
                        huge_2m: false,
                        writable: pml4e & PTE_RW != 0
                            && pdpte & PTE_RW != 0
                            && pde & PTE_RW != 0
                            && pte & PTE_RW != 0,
                    });
                }
            }
            if i4 == 0 && i3 == 0 && i2 == self.cow_i2 {
                if let Some(pt) = &self.cow_pt {
                    let i1 = ((va >> 12) & 0x1FF) as usize;
                    let pte = pt[i1];
                    if pte & PTE_P == 0 {
                        return None;
                    }
                    return Some(Walk {
                        pde: pte,
                        phys: (pte & 0x000F_FFFF_FFFF_F000) | (va & 0xFFF),
                        user: pml4e & PTE_US != 0
                            && pdpte & PTE_US != 0
                            && pde & PTE_US != 0
                            && pte & PTE_US != 0,
                        present: true,
                        huge_2m: false,
                        writable: pml4e & PTE_RW != 0
                            && pdpte & PTE_RW != 0
                            && pde & PTE_RW != 0
                            && pte & PTE_RW != 0,
                    });
                }
            }
            return None;
        }
        Some(Walk {
            pde,
            phys: (pde & 0x000F_FFFF_FFE0_0000) | (va & 0x1F_FFFF),
            user: pml4e & PTE_US != 0 && pdpte & PTE_US != 0 && pde & PTE_US != 0,
            present: true,
            huge_2m: pde & PTE_PS != 0,
            writable: pml4e & PTE_RW != 0 && pdpte & PTE_RW != 0 && pde & PTE_RW != 0,
        })
    }

    /// Map one 4 KiB USER read-only page at `va` (shared COW template).
    /// Splits the covering 2 MiB slot into a PT. Host twin of the kernel
    /// `map_cow_4k`.
    pub fn map_cow_ro(&mut self, va: u64, phys: u64) -> bool {
        if va & (PAGE_4K - 1) != 0 || phys & (PAGE_4K - 1) != 0 {
            return false;
        }
        let i4 = ((va >> 39) & 0x1FF) as usize;
        let i3 = ((va >> 30) & 0x1FF) as usize;
        let i2 = ((va >> 21) & 0x1FF) as usize;
        let i1 = ((va >> 12) & 0x1FF) as usize;
        if i4 != 0 || i3 != 0 || i2 == 0 {
            return false;
        }
        self.pml4[0] |= PTE_P | PTE_RW | PTE_US;
        self.pdpt[0] |= PTE_P | PTE_RW | PTE_US;
        let mut pt = self.cow_pt.unwrap_or([0; 512]);
        pt[i1] = (phys & !0xFFF) | PTE_P | PTE_US;
        self.cow_pt = Some(pt);
        self.cow_i2 = i2;
        self.pd[0][i2] = PTE_P | PTE_RW | PTE_US;
        true
    }

    /// Write-fault break: if the leaf is USER + present + !RW, retarget
    /// it to `new_phys` and set RW. Returns the old frame PA.
    pub fn try_cow_break(&mut self, va: u64, new_phys: u64) -> Option<u64> {
        if new_phys & (PAGE_4K - 1) != 0 {
            return None;
        }
        let i2 = ((va >> 21) & 0x1FF) as usize;
        let i1 = ((va >> 12) & 0x1FF) as usize;
        if i2 != self.cow_i2 {
            return None;
        }
        let pt = self.cow_pt.as_mut()?;
        let pte = pt[i1];
        if pte & PTE_P == 0 || pte & PTE_US == 0 || pte & PTE_RW != 0 {
            return None;
        }
        let old = pte & 0x000F_FFFF_FFFF_F000;
        pt[i1] = (new_phys & !0xFFF) | PTE_P | PTE_RW | PTE_US;
        Some(old)
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
        assert_eq!(CR4_PCIDE, 1 << 17);
        assert_ne!(CR4_SMEP, CR4_SMAP);
        assert_ne!(CR4_PCIDE, CR4_SMEP);
        assert_eq!(CR3_NOFLUSH, 1u64 << 63);
        assert_eq!(INVPCID_INDIV, 0);
        assert_eq!(INVPCID_SINGLE, 1);
        assert_eq!(INVPCID_ALL, 2);
        assert_eq!(INVPCID_ALL_GLOBAL, 3);
    }

    #[test]
    fn cr3_tagged_is_pa_only_without_pcid() {
        let pa = 0xABC000u64;
        assert_eq!(cr3_tagged(pa | 0xF00, 7, true, false), pa);
        assert_eq!(cr3_pa(cr3_tagged(pa, 1, true, true)), pa);
        assert_eq!(cr3_pcid(cr3_tagged(pa, PCID_KERNEL, true, true)), PCID_KERNEL);
        let tagged = cr3_tagged(pa, 2, true, true);
        assert_eq!(tagged & CR3_PCID_MASK, 2);
        assert_eq!(tagged & CR3_NOFLUSH, CR3_NOFLUSH);
        let flush = cr3_tagged(pa, 2, false, true);
        assert_eq!(flush & CR3_NOFLUSH, 0);
        assert_eq!(flush & CR3_PCID_MASK, 2);
    }

    #[test]
    fn pcid_kernel_vs_user_and_clone_share() {
        let kpa = 0x1000u64;
        let init = 0xA000u64;
        let probe = 0xB000u64;
        let mut a = PcidAlloc::new();
        assert_eq!(a.assign(kpa, kpa), PCID_KERNEL);
        assert_eq!(a.assign(init, kpa), PCID_USER_BASE);
        assert_eq!(a.assign(probe, kpa), PCID_USER_BASE + 1);
        // SYS_CLONE shares the /init PML4 → same PCID.
        assert_eq!(a.assign(init, kpa), PCID_USER_BASE);
        assert_eq!(a.of(init, kpa), PCID_USER_BASE);
        assert_ne!(a.of(init, kpa), a.of(probe, kpa));
        assert_ne!(a.of(init, kpa), PCID_KERNEL);
        let kcr3 = cr3_tagged(kpa, a.of(kpa, kpa), true, true);
        let ucr3 = cr3_tagged(init, a.of(init, kpa), true, true);
        assert_eq!(cr3_pa(kcr3), kpa);
        assert_eq!(cr3_pa(ucr3), init);
        assert_eq!(cr3_pcid(kcr3), PCID_KERNEL);
        assert_eq!(cr3_pcid(ucr3), PCID_USER_BASE);
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
        assert!(init.walk(KERNEL_TEXT_VA).is_none());
        assert!(init.user_mapped(USER_IMAGE_BASE));
    }

    #[test]
    fn kaslr_slides_are_legal_and_in_last_2gib() {
        assert_eq!(kaslr_slide(0), Some(0));
        assert_eq!(kaslr_slide(1), Some(KASLR_SLIDE_STRIDE));
        assert_eq!(kaslr_slide(2), Some(2 * KASLR_SLIDE_STRIDE));
        assert!(kaslr_slide(3).is_none());
        assert!(kaslr_slide_valid(0));
        assert!(kaslr_slide_valid(KASLR_SLIDE_STRIDE));
        assert!(!kaslr_slide_valid(PAGE_2M));
        let slid = kernel_text_va_slid(2 * KASLR_SLIDE_STRIDE).unwrap();
        assert!(slid >= KERNEL_VMA);
        assert!(slid > KERNEL_TEXT_VA);
        // Last 2 GiB is [KERNEL_VMA, u64::MAX]; +2 GiB wraps.
        assert_eq!(
            phys_to_hh_slid(KERNEL_LMA, KASLR_SLIDE_STRIDE),
            Some(KERNEL_TEXT_VA + KASLR_SLIDE_STRIDE)
        );
    }

    #[test]
    fn parse_kaslr_cmdline_tokens() {
        assert_eq!(parse_kaslr_cmdline(b"kaslr=0"), Some(0));
        assert_eq!(parse_kaslr_cmdline(b"kaslr=off"), Some(0));
        assert_eq!(parse_kaslr_cmdline(b"console=tty kaslr=1 extra"), Some(1));
        assert_eq!(parse_kaslr_cmdline(b"kaslr=2"), Some(2));
        assert!(parse_kaslr_cmdline(b"").is_none());
        assert!(parse_kaslr_cmdline(b"kaslr=9").is_none());
        assert!(parse_kaslr_cmdline(b"nokaslr=1").is_none());
    }

    #[test]
    fn kaslr_dual_map_keeps_identity() {
        let slide = KASLR_SLIDE_STRIDE;
        let k = IdentityAs::kernel_with_slide(slide);
        let slid_text = KERNEL_TEXT_VA + slide;
        assert_eq!(k.walk(KERNEL_TEXT_VA).unwrap().phys, KERNEL_LMA);
        assert_eq!(k.walk(slid_text).unwrap().phys, KERNEL_LMA);
        assert!(!k.user_mapped(slid_text));
        // Identity of the overwritten HH slot is untouched.
        let id_va = KERNEL_LMA + slide;
        assert_eq!(k.walk(id_va).unwrap().phys, id_va);
        assert_eq!(k.walk(KERNEL_LMA).unwrap().phys, KERNEL_LMA);
        // Canonical HH of that RAM page now aliases the kernel (documented).
        assert_eq!(k.walk(KERNEL_VMA + id_va).unwrap().phys, KERNEL_LMA);

        let init = k.clone_user(USER_IMAGE_BASE, USER_IMAGE_END, &[USER_PROBE_BASE]);
        assert!(!init.user_mapped(slid_text));
        assert!(init.walk(slid_text).is_none());
        assert!(init.user_mapped(USER_IMAGE_BASE));
        assert_eq!(init.walk(USER_IMAGE_BASE).unwrap().phys, USER_IMAGE_BASE);
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
        assert!(init.walk(0x400000).is_none());

        assert!(probe.user_mapped(USER_PROBE_BASE));
        assert!(!probe.user_mapped(USER_IMAGE_BASE));
        assert!(probe.walk(USER_IMAGE_BASE).is_none());
        assert!(!probe.user_mapped(0x400000));
        assert!(probe.walk(0x400000).is_none());
    }

    #[test]
    fn windows_do_not_overlap() {
        assert!(USER_IMAGE_END <= USER_PROBE_BASE);
        assert!(USER_IMAGE_END - USER_IMAGE_BASE == PAGE_2M);
        assert!(USER_PROBE_END - USER_PROBE_BASE == PAGE_2M);
    }

    #[test]
    fn clone_threads_share_one_aspace() {
        // SYS_CLONE does not clone_user() a second PML4. Two threads
        // on /init walk the same map: image + stack USER, /probe unmapped.
        let k = IdentityAs::kernel();
        let shared = k.clone_user(USER_IMAGE_BASE, USER_IMAGE_END, &[USER_PROBE_BASE]);
        let parent_sp = USER_IMAGE_END - 16;
        let child_sp = USER_IMAGE_END - 0x2000;
        assert!(shared.user_mapped(USER_IMAGE_BASE));
        assert!(shared.user_mapped(parent_sp));
        assert!(shared.user_mapped(child_sp));
        assert_eq!(
            shared.walk(parent_sp).unwrap().pde,
            shared.walk(child_sp).unwrap().pde
        );
        assert_eq!(shared.walk(parent_sp).unwrap().phys, parent_sp);
        assert_eq!(shared.walk(child_sp).unwrap().phys, child_sp);
        assert!(shared.walk(USER_PROBE_BASE).is_none());
        assert!(!shared.user_mapped(KERNEL_TEXT_VA));
        assert!(shared.walk(KERNEL_TEXT_VA).is_none());
    }

    #[test]
    fn kpti_user_has_no_hh_or_identity_dma() {
        let slide = KASLR_SLIDE_STRIDE;
        let k = IdentityAs::kernel_with_slide(slide);
        let u = k.clone_user(USER_IMAGE_BASE, USER_IMAGE_END, &[USER_PROBE_BASE]);

        assert!(k.walk(KERNEL_TEXT_VA).is_some());
        assert_eq!(k.walk(KERNEL_LMA).unwrap().phys, KERNEL_LMA);
        assert_eq!(k.walk(0x0100_0000).unwrap().phys, 0x0100_0000);
        assert_eq!(k.walk(0x8000).unwrap().phys, 0x8000);

        assert!(u.walk(KERNEL_TEXT_VA).is_none());
        assert!(u.walk(KERNEL_TEXT_VA + slide).is_none());
        assert!(u.walk(KERNEL_LMA).is_none());
        assert!(u.walk(0x0100_0000).is_none());
        assert!(u.walk(0x1000).is_none());
        assert!(u.pml4[511] == 0);

        let tw = u.walk(KPTI_TRAMP_VA).unwrap();
        assert!(tw.present && !tw.user && !tw.huge_2m);
        assert_eq!(tw.phys, KPTI_TRAMP_VA);
        assert!(!u.user_mapped(KPTI_TRAMP_VA));
        assert!(!u.user_mapped(KPTI_TRAMP_IDT));
        assert!(u.walk(KPTI_TRAMP_IDT).unwrap().present);
        assert!(u.walk(KPTI_TRAMP_STACK).unwrap().present);
        assert!(u.user_mapped(USER_IMAGE_BASE));
        assert!(u.walk(USER_PROBE_BASE).is_none());
    }

    #[test]
    fn cow_shared_until_write() {
        use crate::sysnr::{USER_COW_BASE, USER_COW_END};

        assert_eq!(USER_COW_END - USER_COW_BASE, PAGE_4K);
        assert!(USER_COW_BASE >= USER_PROBE_END);
        assert_ne!((USER_COW_BASE >> 21) & 0x1FF, 0);

        let k = IdentityAs::kernel();
        let mut init = k.clone_user(USER_IMAGE_BASE, USER_IMAGE_END, &[USER_PROBE_BASE]);
        let mut probe = k.clone_user(USER_PROBE_BASE, USER_PROBE_END, &[USER_IMAGE_BASE]);
        let template = 0x9_0000u64;
        let private = 0xA_0000u64;
        assert!(init.map_cow_ro(USER_COW_BASE, template));
        assert!(probe.map_cow_ro(USER_COW_BASE, template));

        let iw = init.walk(USER_COW_BASE).unwrap();
        let pw = probe.walk(USER_COW_BASE).unwrap();
        assert!(iw.present && iw.user && !iw.writable && !iw.huge_2m);
        assert!(pw.present && pw.user && !pw.writable);
        assert_eq!(iw.phys, template);
        assert_eq!(pw.phys, template);
        assert!(!k.user_mapped(USER_COW_BASE));
        assert!(init.user_mapped(USER_IMAGE_BASE));
        assert!(init.walk(USER_PROBE_BASE).is_none());
        assert!(probe.walk(USER_IMAGE_BASE).is_none());

        let old = init.try_cow_break(USER_COW_BASE, private).unwrap();
        assert_eq!(old, template);
        let iw = init.walk(USER_COW_BASE).unwrap();
        let pw = probe.walk(USER_COW_BASE).unwrap();
        assert!(iw.writable && iw.user);
        assert_eq!(iw.phys, private);
        assert!(!pw.writable);
        assert_eq!(pw.phys, template);
        // Second write is already private — not a COW fault.
        assert!(init.try_cow_break(USER_COW_BASE, 0xB_0000).is_none());
        assert_eq!(init.walk(USER_COW_BASE).unwrap().phys, private);
    }

    #[test]
    fn clone_threads_share_cow_break() {
        use crate::sysnr::USER_COW_BASE;

        // SYS_CLONE shares the /init PML4, so a COW break is visible
        // to the sibling — they are not a second isolation domain.
        let k = IdentityAs::kernel();
        let mut shared = k.clone_user(USER_IMAGE_BASE, USER_IMAGE_END, &[USER_PROBE_BASE]);
        assert!(shared.map_cow_ro(USER_COW_BASE, 0x9_0000));
        assert_eq!(
            shared.try_cow_break(USER_COW_BASE, 0xA_0000).unwrap(),
            0x9_0000
        );
        assert_eq!(shared.walk(USER_COW_BASE).unwrap().phys, 0xA_0000);
        assert!(shared.walk(USER_COW_BASE).unwrap().writable);
        assert!(shared.user_mapped(USER_IMAGE_END - 0x2000));
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
                writable: pte2 & SV39_W != 0,
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
            writable: pte1 & SV39_W != 0,
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

    #[test]
    fn clone_threads_share_one_sv39() {
        let k = Sv39As::kernel();
        let shared = k.clone_user(USER_RV_IMAGE_BASE, USER_RV_IMAGE_END, &[]);
        assert!(shared.user_mapped(USER_RV_IMAGE_BASE));
        assert!(shared.user_mapped(USER_RV_IMAGE_END - 16));
        assert!(shared.user_mapped(USER_RV_IMAGE_END - 0x2000));
        assert!(!shared.user_mapped(0x8020_0000));
    }
}

/// AArch64 4K / T0SZ=25 descriptor bits (ARM ARM D5). AP[2:1] = 01
/// is EL1+EL0 RW. UXN is Unprivileged Execute Never.
pub const AA_VALID: u64 = 1;
pub const AA_TABLE: u64 = 1 << 1;
pub const AA_ATTR_NORMAL: u64 = 1 << 2;
pub const AA_AP_EL0: u64 = 1 << 6;
pub const AA_SH_ISH: u64 = 3 << 8;
pub const AA_AF: u64 = 1 << 10;
pub const AA_PXN: u64 = 1 << 53;
pub const AA_UXN: u64 = 1 << 54;

/// Software TTBR0 identity map (4× 1 GiB L1 blocks). `clone_user`
/// splits the 1 GiB that holds the user window into 2 MiB pages and
/// sets AP_EL0 only there. Host-tested twin of the aarch64 kernel walk.
/// Not GICv3, not a second product MM, not PAN (cortex-a72 is v8.0).
#[derive(Clone, Debug)]
pub struct Ttbr0As {
    pub l1: [u64; 512],
    pub l2: [u64; 512],
    pub split_i1: usize,
}

impl Ttbr0As {
    pub fn empty() -> Self {
        Self {
            l1: [0; 512],
            l2: [0; 512],
            split_i1: usize::MAX,
        }
    }

    fn gig_block(phys: u64, normal: bool) -> u64 {
        let mut e = AA_VALID | AA_AF | AA_UXN | (phys & !0x3FFF_FFFF);
        if normal {
            e |= AA_ATTR_NORMAL | AA_SH_ISH;
        }
        e
    }

    fn meg_block(phys: u64, user: bool) -> u64 {
        let mut e = AA_VALID | AA_AF | AA_ATTR_NORMAL | AA_SH_ISH | (phys & !0x1F_FFFF);
        if user {
            e |= AA_AP_EL0 | AA_PXN;
        } else {
            e |= AA_UXN;
        }
        e
    }

    fn is_user(pte: u64) -> bool {
        (pte >> 6) & 3 == 1
    }

    /// Trampoline-shaped kernel map: 4 GiB identity, no AP_EL0 bits.
    pub fn kernel() -> Self {
        let mut s = Self::empty();
        s.l1[0] = Self::gig_block(0, false);
        s.l1[1] = Self::gig_block(PAGE_1G, true);
        s.l1[2] = Self::gig_block(2 * PAGE_1G, false);
        s.l1[3] = Self::gig_block(3 * PAGE_1G, false);
        s
    }

    /// Clone the kernel map. AP_EL0 only on `[user_lo, user_hi)` 2 MiB
    /// leaves. Each address in `unmap` has its 2 MiB valid bit cleared.
    pub fn clone_user(&self, user_lo: u64, user_hi: u64, unmap: &[u64]) -> Self {
        let mut s = self.clone();
        let i1 = ((user_lo >> 30) & 0x1FF) as usize;
        s.split_i1 = i1;
        let gphys = (i1 as u64) * PAGE_1G;
        for j in 0..512u64 {
            s.l2[j as usize] = Self::meg_block(gphys + j * PAGE_2M, false);
        }
        s.l1[i1] = AA_VALID | AA_TABLE;
        let mut va = user_lo & !(PAGE_2M - 1);
        while va < user_hi {
            if ((va >> 30) & 0x1FF) as usize == i1 {
                let i2 = ((va >> 21) & 0x1FF) as usize;
                s.l2[i2] = Self::meg_block(va & !(PAGE_2M - 1), true);
            }
            va += PAGE_2M;
        }
        for &u in unmap {
            if ((u >> 30) & 0x1FF) as usize == i1 {
                let i2 = ((u >> 21) & 0x1FF) as usize;
                s.l2[i2] &= !AA_VALID;
            }
        }
        s
    }

    pub fn walk(&self, va: u64) -> Option<Walk> {
        let i1 = ((va >> 30) & 0x1FF) as usize;
        let pte1 = self.l1[i1];
        if pte1 & AA_VALID == 0 {
            return None;
        }
        if pte1 & AA_TABLE == 0 {
            return Some(Walk {
                pde: pte1,
                phys: (pte1 & 0x0000_FFFF_C000_0000) | (va & 0x3FFF_FFFF),
                user: Self::is_user(pte1),
                present: true,
                huge_2m: true,
                writable: pte1 & (1 << 7) == 0,
            });
        }
        if i1 != self.split_i1 {
            return None;
        }
        let i2 = ((va >> 21) & 0x1FF) as usize;
        let pte2 = self.l2[i2];
        if pte2 & AA_VALID == 0 {
            return None;
        }
        Some(Walk {
            pde: pte2,
            phys: (pte2 & 0x0000_FFFF_FFE0_0000) | (va & 0x1F_FFFF),
            user: Self::is_user(pte2),
            present: true,
            huge_2m: pte2 & AA_TABLE == 0,
            writable: pte2 & (1 << 7) == 0,
        })
    }

    pub fn user_mapped(&self, va: u64) -> bool {
        self.walk(va).map(|w| w.present && w.user).unwrap_or(false)
    }
}

#[cfg(test)]
mod ttbr0_tests {
    use super::*;
    use crate::sysnr::{USER_AA_IMAGE_BASE, USER_AA_IMAGE_END};

    #[test]
    fn kernel_identity_has_no_el0_leaves() {
        let k = Ttbr0As::kernel();
        let w = k.walk(USER_AA_IMAGE_BASE).unwrap();
        assert!(w.present && w.huge_2m);
        assert!(!w.user);
        assert!(!k.user_mapped(USER_AA_IMAGE_BASE));
        assert!(!k.user_mapped(0x4008_0000));
        assert_eq!(k.walk(0x4008_0000).unwrap().phys, 0x4008_0000);
        assert_eq!(k.walk(0x0900_0000).unwrap().phys, 0x0900_0000);
    }

    #[test]
    fn user_leaves_are_task_local() {
        let k = Ttbr0As::kernel();
        let init = k.clone_user(USER_AA_IMAGE_BASE, USER_AA_IMAGE_END, &[]);

        assert!(init.user_mapped(USER_AA_IMAGE_BASE));
        assert!(init.user_mapped(USER_AA_IMAGE_END - 8));
        assert!(!init.user_mapped(0x4008_0000));
        assert!(init.walk(0x4008_0000).unwrap().present);
        assert!(!init.walk(0x4008_0000).unwrap().user);
        assert!(!init.user_mapped(0x0900_0000));
        assert!(init.walk(0x0900_0000).unwrap().present);
        assert!(!k.user_mapped(USER_AA_IMAGE_BASE));
    }

    #[test]
    fn aa_window_is_2m_in_ram() {
        assert_eq!(USER_AA_IMAGE_END - USER_AA_IMAGE_BASE, PAGE_2M);
        assert!(USER_AA_IMAGE_BASE >= 0x4000_0000);
        assert!(USER_AA_IMAGE_BASE < 0x4800_0000);
    }

    #[test]
    fn clone_threads_share_one_ttbr0() {
        let k = Ttbr0As::kernel();
        let shared = k.clone_user(USER_AA_IMAGE_BASE, USER_AA_IMAGE_END, &[]);
        assert!(shared.user_mapped(USER_AA_IMAGE_BASE));
        assert!(shared.user_mapped(USER_AA_IMAGE_END - 16));
        assert!(shared.user_mapped(USER_AA_IMAGE_END - 0x2000));
        assert!(!shared.user_mapped(0x4008_0000));
    }
}
