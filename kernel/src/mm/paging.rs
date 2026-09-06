//! Page-table walk, per-task PML4 clone, and SMEP/SMAP.
//!
//! Boot identity-maps 4 GiB with 2 MiB leaves (trampoline PML4 at
//! 0x1000) and aliases the first 2 GiB at the classic `-2 GiB` kernel
//! map (`KERNEL_VMA + PA`) on dedicated HH PDs. A boot-time KASLR
//! slide dual-maps an 8 MiB kernel span at `KERNEL_VMA + slide + PA`
//! and `_start` runs at that RIP. The kernel is a static-PIE
//! (`relocation-model=pic`); the trampoline applies `.rela.dyn` and
//! unmaps the unused canonical alias when the slide is non-zero.
//! After `prove_higher_half`, `teardown_identity` drops the bulk of
//! the identity 4 GiB. Remaining islands: low 2 MiB (boot PTs,
//! Multiboot, AP SIPI, KPTI trampoline), virtio-blk DMA window,
//! APIC MMIO. SoftNPU / Soft-CP DMA goes through Soft SMMU IOVAs;
//! CPU tensor access uses `phys_to_kva` (HH), not identity. Dynamic
//! page tables are walked via HH (`phys_va`). That table stays the
//! **kernel CR3** (supervisor-only). Each ring-3 task gets a cloned
//! PML4: same identity islands + HH kernel mappings. Each ring-3
//! task gets a **KPTI** PML4: USER only on that task's 2 MiB ELF
//! window, no `PML4[511]` (no kernel HH), no identity, plus four
//! supervisor 4 KiB trampoline pages for syscall/IRQ entry. CR3
//! switches to the kernel map on enter and back on exit. SoftNPU
//! kthread-B stays on kernel CR3 and uses `KernelDma` + Soft SMMU.
//!
//! Documented KASLR subset (fixed 16 MiB slots + cmdline / entropy)
//! plus PIE reloc + unused-alias unmap, documented KPTI subset, plus
//! documented PCID subset (CR4.PCIDE when CPUID.1:ECX[17]; INVPCID
//! when CPUID.7:EBX[10]; else full-flush `mov cr3`). Documented COW
//! subset: one shared 4 KiB USER page at `USER_COW_BASE`, read-only
//! until a write fault copies the frame. Documented growable mmap:
//! `SYS_MMAP` allocates anonymous 4 KiB USER+RW pages in a reserved
//! window on the **user** CR3 / satp / TTBR0. SoftNPU stays on kernel
//! CR3. Soft SMMU is unchanged. Not Meltdown-complete, not POSIX
//! `mmap` / `fork`.

#[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[cfg(target_arch = "x86_64")]
use aether_core::aspace::{CR4_SMAP, CR4_SMEP};
use aether_core::types::PhysAddr;
#[cfg(target_arch = "x86_64")]
use aether_core::{
    cr3_tagged, identity_keep_2m, kaslr_slide_valid, kernel_text_va_slid, phys_to_kva,
    APIC_MMIO_BASE, CR4_PCIDE, INVPCID_SINGLE, KASLR_KERNEL_SPAN, KASLR_MAILBOX_RELOCS,
    KASLR_MAILBOX_SLIDE, KERNEL_LMA, KERNEL_TEXT_VA, KERNEL_VMA, KPTI_TRAMP_PAS, KPTI_TRAMP_VA,
    PCID_KERNEL, PCID_USER_BASE, USER_COW_BASE, USER_IMAGE_BASE, USER_MMAP_BASE,
    USER_PROBE_BASE, COW_TEMPLATE_WORD,
};
#[cfg(target_arch = "x86_64")]
use aether_core::sysnr::BLK_WINDOW_BASE;

#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
use crate::console::{self, write_hex, write_str, write_u64};
#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
use crate::mm::frame;
#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
use crate::println;
#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
use crate::syscall::SysError;

#[cfg(target_arch = "x86_64")]
const P: u64 = 1;
#[cfg(target_arch = "x86_64")]
const RW: u64 = 1 << 1;
#[cfg(target_arch = "x86_64")]
const US: u64 = 1 << 2;
#[cfg(target_arch = "x86_64")]
const PS: u64 = 1 << 7;

#[cfg(target_arch = "x86_64")]
static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
static KASLR_SLIDE: AtomicU64 = AtomicU64::new(u64::MAX);
#[cfg(target_arch = "x86_64")]
static SMAP_ON: AtomicBool = AtomicBool::new(false);
#[cfg(target_arch = "x86_64")]
static SMEP_ON: AtomicBool = AtomicBool::new(false);
#[cfg(target_arch = "x86_64")]
static CR3_SWITCH_LOGS: AtomicU32 = AtomicU32::new(0);
#[cfg(target_arch = "x86_64")]
static PCID_ON: AtomicBool = AtomicBool::new(false);
#[cfg(target_arch = "x86_64")]
static INVPCID_ON: AtomicBool = AtomicBool::new(false);
#[cfg(target_arch = "x86_64")]
static NEXT_USER_PCID: AtomicU32 = AtomicU32::new(PCID_USER_BASE as u32);
#[cfg(target_arch = "x86_64")]
const PCID_SLOTS: usize = 8;
#[cfg(target_arch = "x86_64")]
static PCID_CR3: [AtomicU64; PCID_SLOTS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
#[cfg(target_arch = "x86_64")]
static PCID_ID: [AtomicU32; PCID_SLOTS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
#[cfg(target_arch = "x86_64")]
static COW_TEMPLATE: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
static COW_PROBE_CR3: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
const PF_P: u64 = 1;
#[cfg(target_arch = "x86_64")]
const PF_W: u64 = 1 << 1;
#[cfg(target_arch = "x86_64")]
const PF_U: u64 = 1 << 2;

#[derive(Clone, Copy, Debug)]
pub struct Walk {
    pub pml4e: u64,
    pub pdpte: u64,
    pub pde: u64,
    pub phys: PhysAddr,
    pub huge_2m: bool,
    pub user: bool,
    pub writable: bool,
}

#[cfg(target_arch = "x86_64")]
pub unsafe fn cr3() -> u64 {
    let v: u64;
    core::arch::asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags));
    v
}

#[cfg(target_arch = "x86_64")]
pub fn kaslr_slide() -> u64 {
    let cached = KASLR_SLIDE.load(Ordering::Acquire);
    if cached != u64::MAX {
        return cached;
    }
    let raw = unsafe { core::ptr::read_volatile(KASLR_MAILBOX_SLIDE as *const u32) } as u64;
    let slide = if kaslr_slide_valid(raw) { raw } else { 0 };
    KASLR_SLIDE.store(slide, Ordering::Release);
    slide
}

#[cfg(target_arch = "x86_64")]
pub fn kernel_text_va() -> u64 {
    kernel_text_va_slid(kaslr_slide()).unwrap_or(KERNEL_TEXT_VA)
}

#[cfg(target_arch = "x86_64")]
pub fn capture_kernel_cr3() {
    let v = unsafe { cr3() & !0xFFF };
    KERNEL_CR3.store(v, Ordering::Release);
}

#[cfg(target_arch = "x86_64")]
pub fn kernel_cr3() -> u64 {
    let v = KERNEL_CR3.load(Ordering::Acquire);
    if v == 0 {
        unsafe { cr3() & !0xFFF }
    } else {
        v
    }
}

#[cfg(target_arch = "x86_64")]
pub fn pcid_enabled() -> bool {
    PCID_ON.load(Ordering::Acquire)
}

#[cfg(target_arch = "x86_64")]
pub fn invpcid_enabled() -> bool {
    INVPCID_ON.load(Ordering::Acquire)
}

#[cfg(target_arch = "x86_64")]
fn register_pcid(cr3_pa: u64) -> u16 {
    let pa = cr3_pa & !0xFFF;
    if pa == 0 || pa == kernel_cr3() {
        return PCID_KERNEL;
    }
    for i in 0..PCID_SLOTS {
        if PCID_CR3[i].load(Ordering::Acquire) == pa {
            return PCID_ID[i].load(Ordering::Acquire) as u16;
        }
    }
    let raw = NEXT_USER_PCID.fetch_add(1, Ordering::Relaxed);
    let id = if raw < PCID_USER_BASE as u32 || raw > 4095 {
        PCID_USER_BASE
    } else {
        raw as u16
    };
    for i in 0..PCID_SLOTS {
        if PCID_CR3[i]
            .compare_exchange(0, pa, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            PCID_ID[i].store(id as u32, Ordering::Release);
            return id;
        }
    }
    id
}

#[cfg(target_arch = "x86_64")]
pub fn pcid_of(cr3_pa: u64) -> u16 {
    let pa = cr3_pa & !0xFFF;
    if pa == 0 || pa == kernel_cr3() {
        return PCID_KERNEL;
    }
    for i in 0..PCID_SLOTS {
        if PCID_CR3[i].load(Ordering::Acquire) == pa {
            return PCID_ID[i].load(Ordering::Acquire) as u16;
        }
    }
    PCID_KERNEL
}

/// CR3 load value: PA, or PA|PCID|NOFLUSH when CR4.PCIDE is on.
#[cfg(target_arch = "x86_64")]
pub fn tagged_cr3(pa: u64) -> u64 {
    let pa = pa & !0xFFF;
    cr3_tagged(pa, pcid_of(pa), true, pcid_enabled())
}

#[cfg(target_arch = "x86_64")]
pub fn load_cr3(val: u64) {
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

/// Switch CR3 when the next thread's aspace differs. Logs the first few.
/// With PCID, this is `mov cr3` + NOFLUSH (tagged TLB). Without, full flush.
#[cfg(target_arch = "x86_64")]
pub fn switch_cr3(next: u64, from_tid: u32, to_tid: u32) {
    let want = if next == 0 { kernel_cr3() } else { next } & !0xFFF;
    let cur = unsafe { cr3() & !0xFFF };
    if cur == want {
        return;
    }
    load_cr3(tagged_cr3(want));
    let n = CR3_SWITCH_LOGS.fetch_add(1, Ordering::Relaxed);
    if n < 6 {
        write_str("[mm] cr3 switch tid=");
        write_u64(from_tid as u64);
        write_str("->");
        write_u64(to_tid as u64);
        write_str(" cr3=");
        write_hex(want);
        if pcid_enabled() {
            write_str(" pcid=");
            write_u64(pcid_of(want) as u64);
        }
        console::nl();
    }
}

/// CPU VA for a physical address after identity teardown.
///
/// x86: higher-half `KERNEL_VMA + PA` (or identity keep islands).
/// RISC-V / aarch64: identity (that *is* the kernel map).
#[inline]
pub fn phys_va(pa: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        phys_to_kva(pa).unwrap_or(pa)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        pa
    }
}

/// User VA → kernel load address via that aspace's walk, then [`phys_va`].
/// RISC-V / aarch64 keep identity, so `va` is already the load address.
pub fn user_kva_in(root: u64, va: u64) -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        if root & !0xFFF == 0 {
            return None;
        }
        unsafe { walk_in(root, va).map(|w| phys_va(w.phys.0)) }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = root;
        Some(va)
    }
}

/// [`user_kva_in`] for the current CPU's user CR3 (KPTI slot).
pub fn user_kva(va: u64) -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        user_kva_in(crate::arch::x86_64::kpti::user_cr3(), va)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Some(va)
    }
}

#[cfg(target_arch = "x86_64")]
fn read64(pa: u64) -> u64 {
    unsafe { core::ptr::read_volatile(phys_va(pa) as *const u64) }
}

#[cfg(target_arch = "x86_64")]
fn write64(pa: u64, v: u64) {
    unsafe {
        core::ptr::write_volatile(phys_va(pa) as *mut u64, v);
    }
}

/// Walk `va` in `root` (tables via [`phys_va`] / HH). USER requires US at
/// every level of the walk.
#[cfg(target_arch = "x86_64")]
pub unsafe fn walk_in(root: u64, va: u64) -> Option<Walk> {
    let pml4 = phys_va(root & !0xFFF) as *const u64;
    let i4 = ((va >> 39) & 0x1FF) as usize;
    let i3 = ((va >> 30) & 0x1FF) as usize;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let pml4e = core::ptr::read_volatile(pml4.add(i4));
    if pml4e & P == 0 {
        return None;
    }
    let pdpt = phys_va(pml4e & 0x000F_FFFF_FFFF_F000) as *const u64;
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
            user: pml4e & US != 0 && pdpte & US != 0,
            writable: pml4e & RW != 0 && pdpte & RW != 0,
        });
    }
    let pd = phys_va(pdpte & 0x000F_FFFF_FFFF_F000) as *const u64;
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
            user: pml4e & US != 0 && pdpte & US != 0 && pde & US != 0,
            writable: pml4e & RW != 0 && pdpte & RW != 0 && pde & RW != 0,
        });
    }
    let i1 = ((va >> 12) & 0x1FF) as usize;
    let pt = phys_va(pde & 0x000F_FFFF_FFFF_F000) as *const u64;
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
        user: pml4e & US != 0 && pdpte & US != 0 && pde & US != 0 && pte & US != 0,
        writable: pml4e & RW != 0 && pdpte & RW != 0 && pde & RW != 0 && pte & RW != 0,
    })
}

/// Walk `va` in the current address space (tables via [`phys_va`]).
#[cfg(target_arch = "x86_64")]
pub unsafe fn walk(va: u64) -> Option<Walk> {
    walk_in(cr3(), va)
}

#[cfg(target_arch = "x86_64")]
pub fn user_mapped(root: u64, va: u64) -> bool {
    unsafe {
        walk_in(root, va)
            .map(|w| w.user)
            .unwrap_or(false)
    }
}

#[cfg(target_arch = "x86_64")]
fn invlpg(va: u64) {
    unsafe {
        core::arch::asm!("invlpg [{0}]", in(reg) va, options(nostack, preserves_flags));
    }
}

/// INVPCID (66 0F 38 82 /r). Descriptor is { pcid, linear }.
#[cfg(target_arch = "x86_64")]
fn invpcid(ty: u64, pcid: u64, addr: u64) {
    #[repr(C, align(16))]
    struct Desc {
        pcid: u64,
        addr: u64,
    }
    let desc = Desc {
        pcid: pcid & 0xFFF,
        addr,
    };
    unsafe {
        core::arch::asm!(
            "invpcid {ty}, [{desc}]",
            ty = in(reg) ty,
            desc = in(reg) &desc,
            options(readonly, nostack, preserves_flags)
        );
    }
}

/// Flush one aspace (INVPCID single-context, or MOV-CR3 bit63=0, or full flush).
/// Used on remap and aspace teardown.
#[cfg(target_arch = "x86_64")]
pub fn invalidate_aspace(cr3_pa: u64) {
    let pa = cr3_pa & !0xFFF;
    let pcid = pcid_of(pa);
    if invpcid_enabled() {
        invpcid(INVPCID_SINGLE, pcid as u64, 0);
        return;
    }
    if pcid_enabled() {
        let ie = crate::arch::irq::save_disable();
        let cur = unsafe { cr3() };
        load_cr3(pa | (pcid as u64));
        load_cr3(tagged_cr3(cur));
        crate::arch::irq::restore(ie);
        return;
    }
}

/// Set USER on PML4[0] and PDPT[0] so ring-3 can walk the low 1 GiB.
/// Leaf pages stay supervisor-only until [`allow_user_2m`].
/// Operates on the **current** CR3 (the running user aspace during syscall).
#[cfg(target_arch = "x86_64")]
pub fn allow_user_walk_low() {
    unsafe {
        let pml4 = phys_va(cr3() & !0xFFF) as *mut u64;
        let pml4e = core::ptr::read_volatile(pml4);
        core::ptr::write_volatile(pml4, pml4e | US);
        let pdpt = phys_va(pml4e & 0x000F_FFFF_FFFF_F000) as *mut u64;
        let pdpte = core::ptr::read_volatile(pdpt);
        core::ptr::write_volatile(pdpt, pdpte | US);
    }
}

/// Mark the 2 MiB page covering `va` user-accessible on the **user**
/// CR3 (KPTI). Does not touch the kernel identity map.
#[cfg(target_arch = "x86_64")]
pub fn allow_user_2m(va: u64) {
    let root = crate::arch::x86_64::kpti::user_cr3();
    let root = if root == 0 { unsafe { cr3() } } else { root } & !0xFFF;
    let i3 = ((va >> 30) & 0x1FF) as usize;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    if i3 != 0 {
        return;
    }
    let pml4e = read64(root);
    if pml4e & P == 0 {
        return;
    }
    let pdpt = pml4e & 0x000F_FFFF_FFFF_F000;
    let pdpte = read64(pdpt);
    if pdpte & P == 0 || pdpte & PS != 0 {
        return;
    }
    write64(pdpt, pdpte | US);
    let pd = pdpte & 0x000F_FFFF_FFFF_F000;
    write64(pd + i2 as u64 * 8, (va & !0x1F_FFFF) | P | RW | US | PS);
    invlpg(va);
    invalidate_aspace(root);
}

#[cfg(target_arch = "x86_64")]
fn alloc_zeroed_page() -> Option<u64> {
    let p = frame::alloc()?;
    unsafe {
        core::ptr::write_bytes(phys_va(p.0) as *mut u8, 0, 4096);
    }
    Some(p.0)
}

/// Build a KPTI user PML4: USER 2 MiB on `[user_lo, user_hi)`, supervisor
/// 4 KiB trampoline pages, no HH, no identity 4 GiB. `unmap` clears those
/// 2 MiB leaves (they are not mapped unless they overlap the user window).
#[cfg(target_arch = "x86_64")]
pub fn clone_user_aspace(user_lo: u64, user_hi: u64, unmap: &[u64]) -> Option<u64> {
    let pml4 = alloc_zeroed_page()?;
    let pdpt = alloc_zeroed_page()?;
    let pd0 = alloc_zeroed_page()?;
    let pt = alloc_zeroed_page()?;

    write64(pml4, (pdpt & !0xFFF) | P | RW | US);
    write64(pdpt, (pd0 & !0xFFF) | P | RW | US);
    write64(pd0, (pt & !0xFFF) | P | RW);

    for &pa in &KPTI_TRAMP_PAS {
        let i = (pa >> 12) & 0x1FF;
        write64(pt + i * 8, (pa & !0xFFF) | P | RW);
    }

    let mut va = user_lo & !0x1F_FFFF;
    while va < user_hi {
        let i3 = (va >> 30) & 0x1FF;
        let i2 = (va >> 21) & 0x1FF;
        if i3 == 0 {
            write64(pd0 + i2 * 8, (va & !0x1F_FFFF) | P | RW | US | PS);
        }
        va += 0x20_0000;
    }

    for &u in unmap {
        let i3 = (u >> 30) & 0x1FF;
        let i2 = (u >> 21) & 0x1FF;
        if i3 == 0 {
            write64(pd0 + i2 * 8, 0);
        }
    }

    let _ = register_pcid(pml4);
    Some(pml4)
}

/// PTE address of the 4 KiB leaf at `va`, or `None` if the walk is a
/// huge page / missing. Tables are reached via [`phys_va`] (HH).
#[cfg(target_arch = "x86_64")]
fn cow_pte_pa(root: u64, va: u64) -> Option<u64> {
    let root = root & !0xFFF;
    let i3 = ((va >> 30) & 0x1FF) as usize;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let i1 = ((va >> 12) & 0x1FF) as usize;
    if i3 != 0 {
        return None;
    }
    let pml4e = read64(root);
    if pml4e & P == 0 {
        return None;
    }
    let pdpt = pml4e & 0x000F_FFFF_FFFF_F000;
    let pdpte = read64(pdpt);
    if pdpte & P == 0 || pdpte & PS != 0 {
        return None;
    }
    let pd = pdpte & 0x000F_FFFF_FFFF_F000;
    let pde = read64(pd + i2 as u64 * 8);
    if pde & P == 0 || pde & PS != 0 {
        return None;
    }
    let pt = pde & 0x000F_FFFF_FFFF_F000;
    Some(pt + i1 as u64 * 8)
}

/// Map one USER + present + !RW 4 KiB page at `va` in `root`.
#[cfg(target_arch = "x86_64")]
fn map_cow_4k(root: u64, va: u64, phys: u64) -> bool {
    let root = root & !0xFFF;
    let i3 = ((va >> 30) & 0x1FF) as usize;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let i1 = ((va >> 12) & 0x1FF) as usize;
    if i3 != 0 || i2 == 0 {
        return false;
    }
    let pml4e = read64(root);
    if pml4e & P == 0 {
        return false;
    }
    write64(root, pml4e | US);
    let pdpt = pml4e & 0x000F_FFFF_FFFF_F000;
    let pdpte = read64(pdpt);
    if pdpte & P == 0 || pdpte & PS != 0 {
        return false;
    }
    write64(pdpt, pdpte | US);
    let pd = pdpte & 0x000F_FFFF_FFFF_F000;
    let pde = read64(pd + i2 as u64 * 8);
    let pt = if pde & P != 0 && pde & PS == 0 {
        pde & 0x000F_FFFF_FFFF_F000
    } else {
        let Some(pt) = alloc_zeroed_page() else {
            return false;
        };
        write64(pd + i2 as u64 * 8, (pt & !0xFFF) | P | RW | US);
        pt
    };
    write64(pt + i1 as u64 * 8, (phys & !0xFFF) | P | US);
    invalidate_aspace(root);
    true
}

/// Map one USER + present + RW 4 KiB anonymous page at `va` in `root`.
/// KPTI: user CR3 only. Does not touch the kernel identity map.
#[cfg(target_arch = "x86_64")]
fn map_anon_4k(root: u64, va: u64, phys: u64) -> bool {
    let root = root & !0xFFF;
    let i3 = ((va >> 30) & 0x1FF) as usize;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let i1 = ((va >> 12) & 0x1FF) as usize;
    if i3 != 0 || i2 == 0 {
        return false;
    }
    let pml4e = read64(root);
    if pml4e & P == 0 {
        return false;
    }
    write64(root, pml4e | US);
    let pdpt = pml4e & 0x000F_FFFF_FFFF_F000;
    let pdpte = read64(pdpt);
    if pdpte & P == 0 || pdpte & PS != 0 {
        return false;
    }
    write64(pdpt, pdpte | US);
    let pd = pdpte & 0x000F_FFFF_FFFF_F000;
    let pde = read64(pd + i2 as u64 * 8);
    let pt = if pde & P != 0 && pde & PS == 0 {
        pde & 0x000F_FFFF_FFFF_F000
    } else if pde & P != 0 && pde & PS != 0 {
        return false;
    } else {
        let Some(pt) = alloc_zeroed_page() else {
            return false;
        };
        write64(pd + i2 as u64 * 8, (pt & !0xFFF) | P | RW | US);
        pt
    };
    let slot = pt + i1 as u64 * 8;
    if read64(slot) & P != 0 {
        return false;
    }
    write64(slot, (phys & !0xFFF) | P | RW | US);
    invalidate_aspace(root);
    true
}

/// Shared 4 KiB template in `/init` and optional `/probe`. Same PA,
/// USER + RO, until a write fault. SoftNPU stays on kernel CR3.
#[cfg(target_arch = "x86_64")]
pub fn install_shared_cow(init_cr3: u64, probe_cr3: Option<u64>) -> bool {
    let Some(pa) = alloc_zeroed_page() else {
        println!("[mm] cow FAIL (no frame for template)");
        return false;
    };
    unsafe {
        core::ptr::write_volatile(phys_va(pa) as *mut u64, COW_TEMPLATE_WORD);
    }
    if !map_cow_4k(init_cr3, USER_COW_BASE, pa) {
        println!("[mm] cow FAIL (map /init)");
        return false;
    }
    if let Some(p) = probe_cr3 {
        if !map_cow_4k(p, USER_COW_BASE, pa) {
            println!("[mm] cow FAIL (map /probe)");
            return false;
        }
    }
    COW_TEMPLATE.store(pa, Ordering::Release);
    COW_PROBE_CR3.store(probe_cr3.unwrap_or(0) & !0xFFF, Ordering::Release);
    write_str("[mm] cow shared va=");
    write_hex(USER_COW_BASE);
    write_str(" pa=");
    write_hex(pa);
    write_str(" ro (/init + /probe; not POSIX mmap)");
    console::nl();
    true
}

/// #PF from ring-3: present + write + user on the shared COW VA.
/// Copies the template, sets RW on this aspace only, resumes.
#[cfg(target_arch = "x86_64")]
pub fn handle_user_cow(cr2: u64, err: u64) -> bool {
    if err & (PF_P | PF_W | PF_U) != (PF_P | PF_W | PF_U) {
        return false;
    }
    if cr2 & !0xFFF != USER_COW_BASE {
        return false;
    }
    let root = crate::arch::x86_64::kpti::user_cr3();
    if root == 0 {
        return false;
    }
    handle_cow_fault(root, cr2)
}

#[cfg(target_arch = "x86_64")]
fn handle_cow_fault(root: u64, va: u64) -> bool {
    let Some(slot) = cow_pte_pa(root, va) else {
        return false;
    };
    let pte = read64(slot);
    if pte & P == 0 || pte & US == 0 || pte & RW != 0 {
        return false;
    }
    let old = pte & 0x000F_FFFF_FFFF_F000;
    let ie = crate::arch::irq::save_disable();
    let Some(new) = alloc_zeroed_page() else {
        crate::arch::irq::restore(ie);
        return false;
    };
    unsafe {
        core::ptr::copy_nonoverlapping(phys_va(old) as *const u8, phys_va(new) as *mut u8, 4096);
    }
    write64(slot, (new & !0xFFF) | P | RW | US);
    invalidate_aspace(root);
    crate::arch::irq::restore(ie);
    write_str("[mm] cow fault va=");
    write_hex(va & !0xFFF);
    write_str(" old=");
    write_hex(old);
    write_str(" new=");
    write_hex(new);
    console::nl();
    let _ = prove_cow(root, old, new);
    true
}

/// Serial proof: this aspace is private; `/probe` still names the template.
#[cfg(target_arch = "x86_64")]
fn prove_cow(broken_root: u64, old: u64, new: u64) -> bool {
    let template = COW_TEMPLATE.load(Ordering::Acquire);
    let probe = COW_PROBE_CR3.load(Ordering::Acquire);
    let init_w = unsafe { walk_in(broken_root, USER_COW_BASE) };
    let init_ok = init_w
        .as_ref()
        .map(|w| w.user && w.writable && w.phys.0 == new && new != old && new != template)
        .unwrap_or(false);
    let probe_ok = if probe == 0 {
        true
    } else {
        unsafe { walk_in(probe, USER_COW_BASE) }
            .map(|w| w.user && !w.writable && w.phys.0 == template && w.phys.0 == old)
            .unwrap_or(false)
    };
    let word_ok = unsafe { core::ptr::read_volatile(phys_va(new) as *const u64) } == COW_TEMPLATE_WORD;
    let k = kernel_cr3();
    let k_ok = !user_mapped(k, USER_COW_BASE);
    if init_ok && probe_ok && word_ok && k_ok {
        println!("[mm] cow ok (init private; probe still template; not fork / mmap)");
        true
    } else {
        println!("[mm] cow FAIL");
        false
    }
}

#[cfg(target_arch = "x86_64")]
fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            ebx = out(reg) ebx,
            inout("ecx") sub => ecx,
            out("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

#[cfg(target_arch = "x86_64")]
fn read_cr4() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

/// Enable CR4.SMEP and CR4.SMAP when CPUID advertises them.
#[cfg(target_arch = "x86_64")]
pub fn enable_smep_smap() {
    let (_eax, ebx, _ecx, _edx) = cpuid(7, 0);
    let have_smep = ebx & (1 << 7) != 0;
    let have_smap = ebx & (1 << 20) != 0;
    let mut cr4 = read_cr4();
    if have_smep {
        cr4 |= CR4_SMEP;
    }
    if have_smap {
        cr4 |= CR4_SMAP;
    }
    unsafe {
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
    let now = read_cr4();
    let smep = now & CR4_SMEP != 0;
    let smap = now & CR4_SMAP != 0;
    SMEP_ON.store(smep, Ordering::Release);
    SMAP_ON.store(smap, Ordering::Release);
    write_str("[mm] SMEP+SMAP cr4=");
    write_hex(now);
    write_str(" smep=");
    write_u64(smep as u64);
    write_str(" smap=");
    write_u64(smap as u64);
    if !have_smep || !have_smap {
        write_str(" (CPUID missing bit; try -cpu qemu64,+smep,+smap)");
    }
    console::nl();
}

/// Enable CR4.PCIDE when CPUID.1:ECX[17]. INVPCID is a separate bit
/// (CPUID.7:EBX[10]). Stock `qemu64` often has neither — that is the
/// documented full-flush fallback (`-cpu qemu64,+pcid,+invpcid` arms it).
#[cfg(target_arch = "x86_64")]
pub fn enable_pcid() {
    let ie = crate::arch::irq::save_disable();
    let (_eax, _ebx, ecx, _edx) = cpuid(1, 0);
    let have_pcid = ecx & (1 << 17) != 0;
    let (_eax7, ebx7, _ecx7, _edx7) = cpuid(7, 0);
    let have_invpcid = ebx7 & (1 << 10) != 0;
    if have_pcid {
        // SDM: CR3[11:0] must be 0 when setting CR4.PCIDE.
        let pa = unsafe { cr3() & !0xFFF };
        load_cr3(pa);
        let mut cr4 = read_cr4();
        cr4 |= CR4_PCIDE;
        unsafe {
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
        }
        let now = read_cr4();
        let on = now & CR4_PCIDE != 0;
        PCID_ON.store(on, Ordering::Release);
        INVPCID_ON.store(on && have_invpcid, Ordering::Release);
        if on {
            load_cr3(tagged_cr3(kernel_cr3()));
            crate::arch::x86_64::kpti::sync_cr3_slots();
        }
    } else {
        PCID_ON.store(false, Ordering::Release);
        INVPCID_ON.store(false, Ordering::Release);
    }
    crate::arch::irq::restore(ie);
    let now = read_cr4();
    write_str("[mm] pcid cr4=");
    write_hex(now);
    write_str(" pcide=");
    write_u64((now & CR4_PCIDE != 0) as u64);
    write_str(" invpcid=");
    write_u64(invpcid_enabled() as u64);
    if !have_pcid {
        write_str(" (CPUID.PCID=0; mov cr3 still full-flush; try -cpu qemu64,+pcid,+invpcid)");
    } else if !have_invpcid {
        write_str(" (INVPCID missing; remap flushes via mov cr3 bit63=0)");
    }
    console::nl();
}

#[cfg(target_arch = "x86_64")]
pub fn prove_pcid(init_cr3: u64, probe_cr3: Option<u64>) {
    let k = kernel_cr3();
    write_str("[mm] pcid kernel=");
    write_u64(pcid_of(k) as u64);
    write_str(" init=");
    write_u64(pcid_of(init_cr3) as u64);
    if let Some(p) = probe_cr3 {
        write_str(" probe=");
        write_u64(pcid_of(p) as u64);
    }
    console::nl();
    if pcid_enabled() {
        let cr4_on = read_cr4() & CR4_PCIDE != 0;
        let distinct = pcid_of(init_cr3) != PCID_KERNEL
            && probe_cr3
                .map(|p| pcid_of(p) != pcid_of(init_cr3) && pcid_of(p) != PCID_KERNEL)
                .unwrap_or(true);
        if cr4_on && distinct {
            println!("[mm] pcid ok (tagged TLB; KPTI mov cr3 is not a full flush)");
        } else {
            println!("[mm] pcid FAIL");
        }
    } else {
        println!(
            "[mm] pcid fallback (CPUID.PCID=0; mov cr3 still full-flush; try -cpu qemu64,+pcid,+invpcid)"
        );
    }
}

#[cfg(target_arch = "x86_64")]
pub fn smap_enabled() -> bool {
    SMAP_ON.load(Ordering::Acquire)
}

#[cfg(target_arch = "x86_64")]
pub fn smep_enabled() -> bool {
    SMEP_ON.load(Ordering::Acquire)
}

/// STAC — allow supervisor access to USER pages while SMAP is on.
#[cfg(target_arch = "x86_64")]
pub fn stac() {
    if SMAP_ON.load(Ordering::Relaxed) {
        unsafe {
            core::arch::asm!(".byte 0x0f, 0x01, 0xcb", options(nomem, nostack, preserves_flags));
        }
    }
}

/// CLAC — re-arm SMAP.
#[cfg(target_arch = "x86_64")]
pub fn clac() {
    if SMAP_ON.load(Ordering::Relaxed) {
        unsafe {
            core::arch::asm!(".byte 0x0f, 0x01, 0xca", options(nomem, nostack, preserves_flags));
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub fn with_user_access<T>(f: impl FnOnce() -> T) -> T {
    stac();
    let r = f();
    clac();
    r
}

#[cfg(target_arch = "x86_64")]
fn print_walk(label: &str, root: u64, va: u64) {
    write_str(label);
    write_hex(va);
    match unsafe { walk_in(root, va) } {
        Some(w) => {
            write_str(" present=1 user=");
            write_u64(w.user as u64);
        }
        None => {
            write_str(" present=0 user=0");
        }
    }
}

/// Serial proof: USER leaves are task-local; kernel CR3 has none.
#[cfg(target_arch = "x86_64")]
pub fn prove_aspace(init_cr3: u64, probe_cr3: Option<u64>) -> bool {
    let k = kernel_cr3();
    write_str("[mm] kernel CR3=");
    write_hex(k);
    write_str(" (supervisor identity islands+HH, no USER leaves)");
    console::nl();

    write_str("[mm] /init  CR3=");
    write_hex(init_cr3);
    write_str(" ");
    print_walk("USER ", init_cr3, USER_IMAGE_BASE);
    write_str(" ");
    print_walk("probe ", init_cr3, USER_PROBE_BASE);
    console::nl();

    let slid = kernel_text_va();
    let hh_in_user = unsafe { walk_in(init_cr3, slid) }.is_some();
    let canon_in_user = unsafe { walk_in(init_cr3, KERNEL_TEXT_VA) }.is_some();
    let lma_in_user = unsafe { walk_in(init_cr3, KERNEL_LMA) }.is_some();
    let dma_in_user = unsafe { walk_in(init_cr3, 0x0100_0000) }.is_some();
    let tramp = unsafe { walk_in(init_cr3, KPTI_TRAMP_VA) };
    let tramp_ok = tramp
        .as_ref()
        .map(|w| w.phys.0 == KPTI_TRAMP_VA && !w.user)
        .unwrap_or(false);
    let k_keeps_sipi = unsafe { walk_in(k, 0x8000) }
        .map(|w| w.phys.0 == 0x8000 && !w.user)
        .unwrap_or(false);
    let k_no_lma_id = unsafe { walk_in(k, KERNEL_LMA) }.is_none();
    let k_no_arena_id = unsafe { walk_in(k, 0x0100_0000) }.is_none();
    let k_arena_hh = unsafe { walk_in(k, KERNEL_VMA + 0x0100_0000) }
        .map(|w| w.phys.0 == 0x0100_0000 && !w.user)
        .unwrap_or(false);
    let k_keeps_hh = unsafe { walk_in(k, slid) }
        .map(|w| w.phys.0 == KERNEL_LMA && !w.user)
        .unwrap_or(false);

    write_str("[mm] kpti user tramp=");
    print_walk("", init_cr3, KPTI_TRAMP_VA);
    write_str(" hh=");
    write_u64(hh_in_user as u64);
    write_str(" identity=");
    write_u64(lma_in_user as u64);
    console::nl();

    let init_ok = user_mapped(init_cr3, USER_IMAGE_BASE)
        && !user_mapped(init_cr3, USER_PROBE_BASE)
        && unsafe { walk_in(init_cr3, USER_PROBE_BASE) }.is_none()
        && !user_mapped(k, USER_IMAGE_BASE)
        && !hh_in_user
        && !canon_in_user
        && !lma_in_user
        && !dma_in_user
        && tramp_ok
        && k_keeps_sipi
        && k_no_lma_id
        && k_no_arena_id
        && k_arena_hh
        && k_keeps_hh;

    let probe_ok = if let Some(p) = probe_cr3 {
        write_str("[mm] /probe CR3=");
        write_hex(p);
        write_str(" ");
        print_walk("USER ", p, USER_PROBE_BASE);
        write_str(" ");
        print_walk("init ", p, USER_IMAGE_BASE);
        console::nl();
        user_mapped(p, USER_PROBE_BASE)
            && !user_mapped(p, USER_IMAGE_BASE)
            && unsafe { walk_in(p, USER_IMAGE_BASE) }.is_none()
            && unsafe { walk_in(p, slid) }.is_none()
            && unsafe { walk_in(p, KPTI_TRAMP_VA) }
                .map(|w| !w.user)
                .unwrap_or(false)
    } else {
        true
    };

    let ok = init_ok && probe_ok && smep_enabled() && smap_enabled();
    if ok {
        println!(
            "[mm] aspace isolate ok (task-local USER leaves + SMEP/SMAP + KPTI trampoline)"
        );
        println!(
            "[mm] kpti ok (user CR3: no HH, no identity DMA; trampoline only; not Meltdown-complete)"
        );
        prove_pcid(init_cr3, probe_cr3);
    } else {
        println!("[mm] aspace isolate FAIL");
    }
    ok
}

/// Serial proof: RIP is at the slid HH text; unused link VA is gone
/// when slide != 0; identity still names the LMA (pre-teardown).
#[cfg(target_arch = "x86_64")]
pub fn prove_higher_half() -> bool {
    let rip: u64;
    unsafe {
        core::arch::asm!("lea {0}, [rip]", out(reg) rip, options(nomem, nostack, preserves_flags));
    }
    let slide = kaslr_slide();
    let slid_text = kernel_text_va();
    let relocs = unsafe { core::ptr::read_volatile(KASLR_MAILBOX_RELOCS as *const u32) } as u64;
    let w_hh = unsafe { walk(slid_text) };
    let w_canon = unsafe { walk(KERNEL_TEXT_VA) };
    let w_id = unsafe { walk(KERNEL_LMA) };
    let hh_ok = w_hh
        .as_ref()
        .map(|w| w.phys.0 == KERNEL_LMA && w.huge_2m && !w.user)
        .unwrap_or(false);
    let canon_ok = if slide == 0 {
        w_canon
            .as_ref()
            .map(|w| w.phys.0 == KERNEL_LMA && w.huge_2m && !w.user)
            .unwrap_or(false)
    } else {
        w_canon.is_none()
    };
    let id_ok = w_id
        .as_ref()
        .map(|w| w.phys.0 == KERNEL_LMA && w.huge_2m && !w.user)
        .unwrap_or(false);
    let rip_ok = rip >= slid_text && rip < slid_text + KASLR_KERNEL_SPAN && rip >= KERNEL_VMA;
    let reloc_ok = relocs > 0;
    write_str("[mm] pie reloc n=");
    write_u64(relocs);
    write_str(" applied (R_X86_64_RELATIVE)");
    console::nl();
    write_str("[mm] kaslr slide=");
    write_hex(slide);
    write_str(" idx=");
    write_u64(slide / aether_core::KASLR_SLIDE_STRIDE);
    write_str(" (PIE + dual-map HH; identity still up pre-teardown)");
    console::nl();
    if slide == 0 {
        println!("[mm] kaslr unused alias kept (slide=0; link VA is the map)");
    } else {
        println!("[mm] kaslr unused alias unmapped (link VA not usable)");
    }
    write_str("[mm] higher-half kernel VA=");
    write_hex(slid_text);
    write_str(" linked=");
    write_hex(KERNEL_TEXT_VA);
    write_str(" PA=");
    write_hex(KERNEL_LMA);
    write_str(" rip=");
    write_hex(rip);
    write_str(" identity=");
    write_hex(KERNEL_LMA);
    console::nl();
    if hh_ok && canon_ok && id_ok && rip_ok && reloc_ok {
        println!(
            "[mm] higher-half ok (ffffffff80000000+PA + slide; unused alias unmapped; identity still up pre-teardown)"
        );
        true
    } else {
        println!("[mm] higher-half FAIL");
        false
    }
}

/// Drop identity 2 MiB leaves except SIPI / mailbox / trampoline,
/// virtio-blk, and APIC. HH PDs stay; SoftNPU / PT walks use [`phys_va`].
#[cfg(target_arch = "x86_64")]
pub fn teardown_identity() {
    const PDS: [u64; 4] = [0x3000, 0x4000, 0x5000, 0x6000];
    for i in 0..4u64 {
        let pd = PDS[i as usize];
        for j in 0..512u64 {
            let phys = i * 0x4000_0000 + j * 0x20_0000;
            if identity_keep_2m(phys) {
                continue;
            }
            write64(pd + j * 8, 0);
            invlpg(phys);
        }
    }
    let k = kernel_cr3();
    if k != 0 {
        load_cr3(tagged_cr3(k));
    }
}

/// Serial proof: identity islands remain; SoftNPU arena / user ELF /
/// kernel LMA are HH-only.
#[cfg(target_arch = "x86_64")]
pub fn prove_identity_teardown() -> bool {
    let sipi = unsafe { walk(0x8000) }
        .map(|w| w.phys.0 == 0x8000 && !w.user)
        .unwrap_or(false);
    let mailbox = unsafe { walk(0x7000) }
        .map(|w| w.phys.0 == 0x7000 && !w.user)
        .unwrap_or(false);
    let tramp = unsafe { walk(KPTI_TRAMP_VA) }
        .map(|w| w.phys.0 == KPTI_TRAMP_VA && !w.user)
        .unwrap_or(false);
    let blk = unsafe { walk(BLK_WINDOW_BASE) }
        .map(|w| w.phys.0 == BLK_WINDOW_BASE && !w.user)
        .unwrap_or(false);
    let apic = unsafe { walk(APIC_MMIO_BASE) }
        .map(|w| w.phys.0 == APIC_MMIO_BASE && !w.user)
        .unwrap_or(false);
    let no_arena_id = unsafe { walk(0x0100_0000) }.is_none();
    let no_user_id = unsafe { walk(USER_IMAGE_BASE) }.is_none();
    let no_mmap_id = unsafe { walk(USER_MMAP_BASE) }.is_none();
    let no_lma_id = unsafe { walk(KERNEL_LMA) }.is_none();
    let arena_hh = unsafe { walk(KERNEL_VMA + 0x0100_0000) }
        .map(|w| w.phys.0 == 0x0100_0000 && !w.user)
        .unwrap_or(false);
    let lma_hh = unsafe { walk(kernel_text_va()) }
        .map(|w| w.phys.0 == KERNEL_LMA && !w.user)
        .unwrap_or(false);
    let ok = sipi
        && mailbox
        && tramp
        && blk
        && apic
        && no_arena_id
        && no_user_id
        && no_mmap_id
        && no_lma_id
        && arena_hh
        && lma_hh;
    if ok {
        println!(
            "[mm] identity teardown ok (islands: low 2MiB + virtio-blk + APIC; SoftNPU via Soft SMMU + HH)"
        );
    } else {
        println!("[mm] identity teardown FAIL");
    }
    ok
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
const PTE_W: u64 = 1 << 2;
#[cfg(target_arch = "riscv64")]
const PTE_X: u64 = 1 << 3;
#[cfg(target_arch = "riscv64")]
const PTE_U: u64 = 1 << 4;
#[cfg(target_arch = "riscv64")]
const PTE_G: u64 = 1 << 5;
#[cfg(target_arch = "riscv64")]
const PTE_A: u64 = 1 << 6;
#[cfg(target_arch = "riscv64")]
const PTE_D: u64 = 1 << 7;
#[cfg(target_arch = "riscv64")]
const PTE_LEAF: u64 = PTE_R;
#[cfg(target_arch = "riscv64")]
const SATP_SV39: u64 = 8 << 60;
#[cfg(target_arch = "riscv64")]
const SSTATUS_SUM: u64 = 1 << 18;

#[cfg(target_arch = "riscv64")]
static KERNEL_SATP_ROOT: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "riscv64")]
static SATP_SWITCH_LOGS: AtomicU32 = AtomicU32::new(0);
#[cfg(target_arch = "riscv64")]
static SUM_ARMED: AtomicBool = AtomicBool::new(true);

#[cfg(target_arch = "riscv64")]
pub unsafe fn satp() -> u64 {
    let v: u64;
    core::arch::asm!("csrr {v}, satp", v = out(reg) v, options(nomem, nostack));
    v
}

#[cfg(target_arch = "riscv64")]
pub fn satp_root() -> u64 {
    unsafe { (satp() & 0x0000_0FFF_FFFF_FFFF) << 12 }
}

#[cfg(target_arch = "riscv64")]
pub fn capture_kernel_satp() {
    KERNEL_SATP_ROOT.store(satp_root(), Ordering::Release);
}

#[cfg(target_arch = "riscv64")]
pub fn kernel_cr3() -> u64 {
    let v = KERNEL_SATP_ROOT.load(Ordering::Acquire);
    if v == 0 {
        satp_root()
    } else {
        v
    }
}

#[cfg(target_arch = "riscv64")]
pub fn load_satp(root: u64) {
    let satp = SATP_SV39 | ((root & !0xFFF) >> 12);
    unsafe {
        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",
            in(reg) satp,
            options(nostack)
        );
    }
}

/// Switch satp when the next thread's aspace differs. Logs the first few.
#[cfg(target_arch = "riscv64")]
pub fn switch_cr3(next: u64, from_tid: u32, to_tid: u32) {
    let want = if next == 0 { kernel_cr3() } else { next } & !0xFFF;
    let cur = satp_root();
    if cur == want {
        return;
    }
    load_satp(want);
    let n = SATP_SWITCH_LOGS.fetch_add(1, Ordering::Relaxed);
    if n < 6 {
        write_str("[mm] satp switch tid=");
        write_u64(from_tid as u64);
        write_str("->");
        write_u64(to_tid as u64);
        write_str(" satp=");
        write_hex(want);
        console::nl();
    }
}

#[cfg(target_arch = "riscv64")]
pub fn write_sscratch(v: u64) {
    unsafe {
        core::arch::asm!("csrw sscratch, {0}", in(reg) v, options(nostack));
    }
}

#[cfg(target_arch = "riscv64")]
fn read64(pa: u64) -> u64 {
    unsafe { core::ptr::read_volatile(pa as *const u64) }
}

#[cfg(target_arch = "riscv64")]
fn write64(pa: u64, v: u64) {
    unsafe {
        core::ptr::write_volatile(pa as *mut u64, v);
    }
}

#[cfg(target_arch = "riscv64")]
fn alloc_zeroed_page() -> Option<u64> {
    let p = frame::alloc()?;
    unsafe {
        core::ptr::write_bytes(phys_va(p.0) as *mut u8, 0, 4096);
    }
    Some(p.0)
}

#[cfg(target_arch = "riscv64")]
fn meg_pte(phys: u64, user: bool) -> u64 {
    let mut flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
    if user {
        flags |= PTE_U;
    } else {
        flags |= PTE_G;
    }
    (phys >> 2) | flags
}

/// Walk `va` in `root` (identity-mapped Sv39 tables).
#[cfg(target_arch = "riscv64")]
pub unsafe fn walk_in(root: u64, va: u64) -> Option<Walk> {
    let l2 = (root & !0xFFF) as *const u64;
    let i2 = ((va >> 30) & 0x1FF) as usize;
    let pte2 = core::ptr::read_volatile(l2.add(i2));
    if pte2 & PTE_V == 0 {
        return None;
    }
    if pte2 & PTE_LEAF != 0 {
        let ppn = (pte2 >> 10) & 0x0FFF_FFFF_FFFF;
        let phys = (ppn << 12) | (va & 0x3FFF_FFFF);
        return Some(Walk {
            pml4e: pte2,
            pdpte: 0,
            pde: 0,
            phys: PhysAddr(phys),
            huge_2m: true,
            user: pte2 & PTE_U != 0,
            writable: pte2 & PTE_W != 0,
        });
    }
    let l1 = ((pte2 >> 10) << 12) as *const u64;
    let i1 = ((va >> 21) & 0x1FF) as usize;
    let pte1 = core::ptr::read_volatile(l1.add(i1));
    if pte1 & PTE_V == 0 {
        return None;
    }
    if pte1 & PTE_LEAF != 0 {
        let ppn = (pte1 >> 10) & 0x0FFF_FFFF_FFFF;
        let phys = (ppn << 12) | (va & 0x1F_FFFF);
        return Some(Walk {
            pml4e: pte2,
            pdpte: pte1,
            pde: 0,
            phys: PhysAddr(phys),
            huge_2m: true,
            user: pte1 & PTE_U != 0,
            writable: pte1 & PTE_W != 0,
        });
    }
    let l0 = ((pte1 >> 10) << 12) as *const u64;
    let i0 = ((va >> 12) & 0x1FF) as usize;
    let pte0 = core::ptr::read_volatile(l0.add(i0));
    if pte0 & PTE_V == 0 {
        return None;
    }
    let ppn = (pte0 >> 10) & 0x0FFF_FFFF_FFFF;
    let phys = (ppn << 12) | (va & 0xFFF);
    Some(Walk {
        pml4e: pte2,
        pdpte: pte1,
        pde: pte0,
        phys: PhysAddr(phys),
        huge_2m: false,
        user: pte0 & PTE_U != 0,
        writable: pte0 & PTE_W != 0,
    })
}

/// Sv39 walk of the current satp. A 1 GiB or 2 MiB identity leaf is
/// `huge_2m = true`.
#[cfg(target_arch = "riscv64")]
pub unsafe fn walk(va: u64) -> Option<Walk> {
    let satp = satp();
    if satp >> 60 != 8 {
        return None;
    }
    walk_in((satp & 0x0000_0FFF_FFFF_FFFF) << 12, va)
}

#[cfg(target_arch = "riscv64")]
pub fn user_mapped(root: u64, va: u64) -> bool {
    unsafe { walk_in(root, va).map(|w| w.user).unwrap_or(false) }
}

/// Clone the kernel identity map into a new satp root. U only on
/// `[user_lo, user_hi)`; each VA in `unmap` loses V on its 2 MiB leaf.
#[cfg(target_arch = "riscv64")]
pub fn clone_user_aspace(user_lo: u64, user_hi: u64, unmap: &[u64]) -> Option<u64> {
    let kroot = kernel_cr3();
    let new_l2 = alloc_zeroed_page()?;
    unsafe {
        core::ptr::copy_nonoverlapping(kroot as *const u8, new_l2 as *mut u8, 4096);
    }
    let vpn2 = ((user_lo >> 30) & 0x1FF) as usize;
    let l2e = read64(new_l2 + vpn2 as u64 * 8);
    if l2e & PTE_V == 0 {
        return None;
    }
    let l1 = alloc_zeroed_page()?;
    if l2e & PTE_LEAF != 0 {
        let gphys = ((l2e >> 10) & 0x0FFF_FFFF_FFFF) << 12;
        let gphys = gphys & !0x3FFF_FFFF;
        for j in 0..512u64 {
            write64(l1 + j * 8, meg_pte(gphys + j * 0x20_0000, false));
        }
    } else {
        let old_l1 = ((l2e >> 10) & 0x0FFF_FFFF_FFFF) << 12;
        unsafe {
            core::ptr::copy_nonoverlapping(old_l1 as *const u8, l1 as *mut u8, 4096);
        }
        for j in 0..512u64 {
            let e = read64(l1 + j * 8);
            write64(l1 + j * 8, (e & !PTE_U) | PTE_G);
        }
    }
    let mut va = user_lo & !0x1F_FFFF;
    while va < user_hi {
        if ((va >> 30) & 0x1FF) as usize == vpn2 {
            let i1 = ((va >> 21) & 0x1FF) as usize;
            write64(l1 + i1 as u64 * 8, meg_pte(va, true));
        }
        va += 0x20_0000;
    }
    for &u in unmap {
        if ((u >> 30) & 0x1FF) as usize == vpn2 {
            let i1 = ((u >> 21) & 0x1FF) as usize;
            write64(l1 + i1 as u64 * 8, read64(l1 + i1 as u64 * 8) & !PTE_V);
        }
    }
    write64(new_l2 + vpn2 as u64 * 8, ((l1 >> 12) << 10) | PTE_V);
    Some(new_l2)
}

/// Map one U+RW 4 KiB anonymous page. Splits a 2 MiB leaf if needed.
#[cfg(target_arch = "riscv64")]
fn map_anon_4k(root: u64, va: u64, phys: u64) -> bool {
    let root = root & !0xFFF;
    let i2 = ((va >> 30) & 0x1FF) as usize;
    let i1 = ((va >> 21) & 0x1FF) as usize;
    let i0 = ((va >> 12) & 0x1FF) as usize;
    let pte2 = read64(root + i2 as u64 * 8);
    if pte2 & PTE_V == 0 || pte2 & PTE_LEAF != 0 {
        return false;
    }
    let l1 = ((pte2 >> 10) & 0x0FFF_FFFF_FFFF) << 12;
    let pte1 = read64(l1 + i1 as u64 * 8);
    let l0 = if pte1 & PTE_V != 0 && pte1 & PTE_LEAF == 0 {
        ((pte1 >> 10) & 0x0FFF_FFFF_FFFF) << 12
    } else if pte1 & PTE_V != 0 && pte1 & PTE_U != 0 {
        return false;
    } else {
        let Some(l0) = alloc_zeroed_page() else {
            return false;
        };
        write64(l1 + i1 as u64 * 8, ((l0 >> 12) << 10) | PTE_V);
        l0
    };
    let slot = l0 + i0 as u64 * 8;
    if read64(slot) & PTE_V != 0 {
        return false;
    }
    write64(
        slot,
        (phys >> 2) | PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D,
    );
    unsafe {
        core::arch::asm!("sfence.vma", options(nostack));
    }
    true
}

/// Mark the 2 MiB page covering `va` user-accessible (current satp).
#[cfg(target_arch = "riscv64")]
pub fn allow_user_2m(va: u64) {
    let root = satp_root();
    let i2 = ((va >> 30) & 0x1FF) as usize;
    let l2e = read64(root + i2 as u64 * 8);
    if l2e & PTE_V == 0 || l2e & PTE_LEAF != 0 {
        return;
    }
    let l1 = ((l2e >> 10) & 0x0FFF_FFFF_FFFF) << 12;
    let i1 = ((va >> 21) & 0x1FF) as usize;
    let e = read64(l1 + i1 as u64 * 8);
    if e & PTE_V == 0 {
        return;
    }
    write64(l1 + i1 as u64 * 8, (e | PTE_U) & !PTE_G);
    unsafe {
        core::arch::asm!("sfence.vma", options(nostack));
    }
}

#[cfg(target_arch = "riscv64")]
pub fn with_user_access<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        core::arch::asm!("csrs sstatus, {0}", in(reg) SSTATUS_SUM, options(nostack));
    }
    let r = f();
    unsafe {
        core::arch::asm!("csrc sstatus, {0}", in(reg) SSTATUS_SUM, options(nostack));
    }
    r
}

#[cfg(target_arch = "riscv64")]
fn print_walk(label: &str, root: u64, va: u64) {
    write_str(label);
    write_hex(va);
    match unsafe { walk_in(root, va) } {
        Some(w) => {
            write_str(" present=1 user=");
            write_u64(w.user as u64);
        }
        None => {
            write_str(" present=0 user=0");
        }
    }
}

/// Serial proof: U leaves are task-local; kernel satp has none.
#[cfg(target_arch = "riscv64")]
pub fn prove_aspace(init_root: u64, _probe: Option<u64>) -> bool {
    let k = kernel_cr3();
    write_str("[mm] kernel satp=");
    write_hex(k);
    write_str(" (supervisor identity, no U leaves)");
    console::nl();

    write_str("[mm] /init  satp=");
    write_hex(init_root);
    write_str(" ");
    print_walk("USER ", init_root, aether_core::USER_RV_IMAGE_BASE);
    write_str(" ");
    print_walk("ktext ", init_root, crate::arch::riscv64::KERNEL_VA);
    console::nl();

    let init_ok = user_mapped(init_root, aether_core::USER_RV_IMAGE_BASE)
        && !user_mapped(init_root, crate::arch::riscv64::KERNEL_VA)
        && unsafe { walk_in(init_root, crate::arch::riscv64::KERNEL_VA) }.is_some()
        && !user_mapped(k, aether_core::USER_RV_IMAGE_BASE);

    SUM_ARMED.store(true, Ordering::Release);
    let ok = init_ok;
    if ok {
        println!("[mm] aspace isolate ok (task-local U leaves + SUM off)");
    } else {
        println!("[mm] aspace isolate FAIL");
    }
    ok
}

#[cfg(target_arch = "aarch64")]
const PTE_VALID: u64 = 1;
#[cfg(target_arch = "aarch64")]
const PTE_TABLE: u64 = 1 << 1;
#[cfg(target_arch = "aarch64")]
const PTE_ATTR_NORMAL: u64 = 1 << 2;
#[cfg(target_arch = "aarch64")]
const PTE_AP_EL0: u64 = 1 << 6;
#[cfg(target_arch = "aarch64")]
const PTE_SH_ISH: u64 = 3 << 8;
#[cfg(target_arch = "aarch64")]
const PTE_AF: u64 = 1 << 10;
#[cfg(target_arch = "aarch64")]
const PTE_PXN: u64 = 1 << 53;
#[cfg(target_arch = "aarch64")]
const PTE_UXN: u64 = 1 << 54;

#[cfg(target_arch = "aarch64")]
static KERNEL_TTBR0: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static TTBR_SWITCH_LOGS: AtomicU32 = AtomicU32::new(0);

#[cfg(target_arch = "aarch64")]
pub unsafe fn ttbr0() -> u64 {
    let v: u64;
    core::arch::asm!("mrs {v}, ttbr0_el1", v = out(reg) v, options(nomem, nostack));
    v
}

#[cfg(target_arch = "aarch64")]
pub fn ttbr0_root() -> u64 {
    unsafe { ttbr0() & 0x0000_FFFF_FFFF_F000 }
}

#[cfg(target_arch = "aarch64")]
pub fn capture_kernel_ttbr() {
    KERNEL_TTBR0.store(ttbr0_root(), Ordering::Release);
}

#[cfg(target_arch = "aarch64")]
pub fn kernel_cr3() -> u64 {
    let v = KERNEL_TTBR0.load(Ordering::Acquire);
    if v == 0 {
        ttbr0_root()
    } else {
        v
    }
}

#[cfg(target_arch = "aarch64")]
pub fn load_ttbr0(root: u64) {
    let root = root & 0x0000_FFFF_FFFF_F000;
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "msr ttbr0_el1, {0}",
            "isb",
            "tlbi vmalle1",
            "dsb sy",
            "isb",
            in(reg) root,
            options(nostack)
        );
    }
}

/// Switch TTBR0 when the next thread's aspace differs. Logs the first few.
#[cfg(target_arch = "aarch64")]
pub fn switch_cr3(next: u64, from_tid: u32, to_tid: u32) {
    let want = if next == 0 { kernel_cr3() } else { next } & 0x0000_FFFF_FFFF_F000;
    let cur = ttbr0_root();
    if cur == want {
        return;
    }
    load_ttbr0(want);
    let n = TTBR_SWITCH_LOGS.fetch_add(1, Ordering::Relaxed);
    if n < 6 {
        write_str("[mm] ttbr0 switch tid=");
        write_u64(from_tid as u64);
        write_str("->");
        write_u64(to_tid as u64);
        write_str(" ttbr0=");
        write_hex(want);
        console::nl();
    }
}

#[cfg(target_arch = "aarch64")]
pub fn write_tpidr(v: u64) {
    unsafe {
        core::arch::asm!("msr tpidr_el1, {0}", in(reg) v, options(nostack));
    }
}

#[cfg(target_arch = "aarch64")]
fn read64(pa: u64) -> u64 {
    unsafe { core::ptr::read_volatile(pa as *const u64) }
}

#[cfg(target_arch = "aarch64")]
fn write64(pa: u64, v: u64) {
    unsafe {
        core::ptr::write_volatile(pa as *mut u64, v);
    }
}

#[cfg(target_arch = "aarch64")]
fn alloc_zeroed_page() -> Option<u64> {
    let p = frame::alloc()?;
    unsafe {
        core::ptr::write_bytes(phys_va(p.0) as *mut u8, 0, 4096);
    }
    Some(p.0)
}

#[cfg(target_arch = "aarch64")]
fn is_el0(pte: u64) -> bool {
    (pte >> 6) & 3 == 1
}

#[cfg(target_arch = "aarch64")]
fn meg_block(phys: u64, user: bool) -> u64 {
    let mut e = PTE_VALID | PTE_AF | PTE_ATTR_NORMAL | PTE_SH_ISH | (phys & !0x1F_FFFF);
    if user {
        e |= PTE_AP_EL0 | PTE_PXN;
    } else {
        e |= PTE_UXN;
    }
    e
}

/// Walk `va` in `root` (identity-mapped TTBR0 tables).
#[cfg(target_arch = "aarch64")]
pub unsafe fn walk_in(root: u64, va: u64) -> Option<Walk> {
    let l1 = (root & 0x0000_FFFF_FFFF_F000) as *const u64;
    let i1 = ((va >> 30) & 0x1FF) as usize;
    let pte1 = core::ptr::read_volatile(l1.add(i1));
    if pte1 & PTE_VALID == 0 {
        return None;
    }
    if pte1 & PTE_TABLE == 0 {
        let phys = (pte1 & 0x0000_FFFF_C000_0000) | (va & 0x3FFF_FFFF);
        return Some(Walk {
            pml4e: pte1,
            pdpte: 0,
            pde: 0,
            phys: PhysAddr(phys),
            huge_2m: true,
            user: is_el0(pte1),
            writable: pte1 & (1 << 7) == 0,
        });
    }
    let l2 = (pte1 & 0x0000_FFFF_FFFF_F000) as *const u64;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let pte2 = core::ptr::read_volatile(l2.add(i2));
    if pte2 & PTE_VALID == 0 {
        return None;
    }
    if pte2 & PTE_TABLE == 0 {
        let phys = (pte2 & 0x0000_FFFF_FFE0_0000) | (va & 0x1F_FFFF);
        return Some(Walk {
            pml4e: pte1,
            pdpte: pte2,
            pde: 0,
            phys: PhysAddr(phys),
            huge_2m: true,
            user: is_el0(pte2),
            writable: pte2 & (1 << 7) == 0,
        });
    }
    let l3 = (pte2 & 0x0000_FFFF_FFFF_F000) as *const u64;
    let i3 = ((va >> 12) & 0x1FF) as usize;
    let pte3 = core::ptr::read_volatile(l3.add(i3));
    if pte3 & PTE_VALID == 0 {
        return None;
    }
    let phys = (pte3 & 0x0000_FFFF_FFFF_F000) | (va & 0xFFF);
    Some(Walk {
        pml4e: pte1,
        pdpte: pte2,
        pde: pte3,
        phys: PhysAddr(phys),
        huge_2m: false,
        user: is_el0(pte3),
        writable: pte3 & (1 << 7) == 0,
    })
}

/// 4K / T0SZ=25 walk. A 1 GiB L1 or 2 MiB L2 identity block is
/// `huge_2m = true`.
#[cfg(target_arch = "aarch64")]
pub unsafe fn walk(va: u64) -> Option<Walk> {
    walk_in(ttbr0_root(), va)
}

#[cfg(target_arch = "aarch64")]
pub fn user_mapped(root: u64, va: u64) -> bool {
    unsafe { walk_in(root, va).map(|w| w.user).unwrap_or(false) }
}

/// Clone the kernel identity map into a new TTBR0. AP_EL0 only on
/// `[user_lo, user_hi)`; each VA in `unmap` loses valid on its 2 MiB leaf.
#[cfg(target_arch = "aarch64")]
pub fn clone_user_aspace(user_lo: u64, user_hi: u64, unmap: &[u64]) -> Option<u64> {
    let kroot = kernel_cr3();
    let new_l1 = alloc_zeroed_page()?;
    unsafe {
        core::ptr::copy_nonoverlapping(kroot as *const u8, new_l1 as *mut u8, 4096);
    }
    let i1 = ((user_lo >> 30) & 0x1FF) as usize;
    let l1e = read64(new_l1 + i1 as u64 * 8);
    if l1e & PTE_VALID == 0 {
        return None;
    }
    let l2 = alloc_zeroed_page()?;
    if l1e & PTE_TABLE == 0 {
        let gphys = user_lo & !0x3FFF_FFFF;
        for j in 0..512u64 {
            write64(l2 + j * 8, meg_block(gphys + j * 0x20_0000, false));
        }
    } else {
        let old_l2 = l1e & 0x0000_FFFF_FFFF_F000;
        unsafe {
            core::ptr::copy_nonoverlapping(old_l2 as *const u8, l2 as *mut u8, 4096);
        }
        for j in 0..512u64 {
            let e = read64(l2 + j * 8);
            write64(l2 + j * 8, (e & !PTE_AP_EL0) | PTE_UXN);
        }
    }
    let mut va = user_lo & !0x1F_FFFF;
    while va < user_hi {
        if ((va >> 30) & 0x1FF) as usize == i1 {
            let i2 = ((va >> 21) & 0x1FF) as usize;
            write64(l2 + i2 as u64 * 8, meg_block(va, true));
        }
        va += 0x20_0000;
    }
    for &u in unmap {
        if ((u >> 30) & 0x1FF) as usize == i1 {
            let i2 = ((u >> 21) & 0x1FF) as usize;
            write64(l2 + i2 as u64 * 8, read64(l2 + i2 as u64 * 8) & !PTE_VALID);
        }
    }
    write64(new_l1 + i1 as u64 * 8, (l2 & 0x0000_FFFF_FFFF_F000) | PTE_VALID | PTE_TABLE);
    Some(new_l1)
}

/// Map one EL0+RW 4 KiB anonymous page. Splits a 2 MiB block if needed.
#[cfg(target_arch = "aarch64")]
fn map_anon_4k(root: u64, va: u64, phys: u64) -> bool {
    let root = root & 0x0000_FFFF_FFFF_F000;
    let i1 = ((va >> 30) & 0x1FF) as usize;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let i3 = ((va >> 12) & 0x1FF) as usize;
    let pte1 = read64(root + i1 as u64 * 8);
    if pte1 & PTE_VALID == 0 || pte1 & PTE_TABLE == 0 {
        return false;
    }
    let l2 = pte1 & 0x0000_FFFF_FFFF_F000;
    let pte2 = read64(l2 + i2 as u64 * 8);
    let l3 = if pte2 & PTE_VALID != 0 && pte2 & PTE_TABLE != 0 {
        pte2 & 0x0000_FFFF_FFFF_F000
    } else if pte2 & PTE_VALID != 0 && is_el0(pte2) {
        return false;
    } else {
        let Some(l3) = alloc_zeroed_page() else {
            return false;
        };
        write64(l2 + i2 as u64 * 8, (l3 & 0x0000_FFFF_FFFF_F000) | PTE_VALID | PTE_TABLE);
        l3
    };
    let slot = l3 + i3 as u64 * 8;
    if read64(slot) & PTE_VALID != 0 {
        return false;
    }
    write64(
        slot,
        PTE_VALID
            | PTE_TABLE
            | PTE_AF
            | PTE_ATTR_NORMAL
            | PTE_SH_ISH
            | PTE_AP_EL0
            | PTE_PXN
            | (phys & !0xFFF),
    );
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "tlbi vmalle1",
            "dsb sy",
            "isb",
            options(nostack)
        );
    }
    true
}

/// Mark the 2 MiB page covering `va` EL0-accessible (current TTBR0).
#[cfg(target_arch = "aarch64")]
pub fn allow_user_2m(va: u64) {
    let root = ttbr0_root();
    let i1 = ((va >> 30) & 0x1FF) as usize;
    let l1e = read64(root + i1 as u64 * 8);
    if l1e & PTE_VALID == 0 || l1e & PTE_TABLE == 0 {
        return;
    }
    let l2 = l1e & 0x0000_FFFF_FFFF_F000;
    let i2 = ((va >> 21) & 0x1FF) as usize;
    let e = read64(l2 + i2 as u64 * 8);
    if e & PTE_VALID == 0 {
        return;
    }
    write64(
        l2 + i2 as u64 * 8,
        (e | PTE_AP_EL0 | PTE_PXN) & !PTE_UXN,
    );
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "tlbi vmalle1",
            "dsb sy",
            "isb",
            options(nostack)
        );
    }
}

/// cortex-a72 has no PAN. EL1 can already load AP_EL0 pages.
#[cfg(target_arch = "aarch64")]
pub fn with_user_access<T>(f: impl FnOnce() -> T) -> T {
    f()
}

#[cfg(target_arch = "aarch64")]
fn print_walk(label: &str, root: u64, va: u64) {
    write_str(label);
    write_hex(va);
    match unsafe { walk_in(root, va) } {
        Some(w) => {
            write_str(" present=1 user=");
            write_u64(w.user as u64);
        }
        None => {
            write_str(" present=0 user=0");
        }
    }
}

/// Serial proof: AP_EL0 leaves are task-local; kernel TTBR0 has none.
#[cfg(target_arch = "aarch64")]
pub fn prove_aspace(init_root: u64, _probe: Option<u64>) -> bool {
    let k = kernel_cr3();
    write_str("[mm] kernel ttbr0=");
    write_hex(k);
    write_str(" (EL1 identity, no AP_EL0 leaves)");
    console::nl();

    write_str("[mm] /init  ttbr0=");
    write_hex(init_root);
    write_str(" ");
    print_walk("USER ", init_root, aether_core::USER_AA_IMAGE_BASE);
    write_str(" ");
    print_walk("ktext ", init_root, crate::arch::aarch64::KERNEL_VA);
    console::nl();

    let init_ok = user_mapped(init_root, aether_core::USER_AA_IMAGE_BASE)
        && !user_mapped(init_root, crate::arch::aarch64::KERNEL_VA)
        && unsafe { walk_in(init_root, crate::arch::aarch64::KERNEL_VA) }.is_some()
        && !user_mapped(k, aether_core::USER_AA_IMAGE_BASE);

    let ok = init_ok;
    if ok {
        println!("[mm] aspace isolate ok (task-local EL0 leaves; no PAN on cortex-a72)");
    } else {
        println!("[mm] aspace isolate FAIL");
    }
    ok
}

/// Allocate frames and map `[va, va+len)` USER+RW in `root`.
/// SoftNPU / kernel CR3 are not rewritten. Soft SMMU is unchanged.
#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
pub fn map_anon_pages(root: u64, va: u64, len: u64) -> Result<u64, SysError> {
    if va & 0xFFF != 0 || len & 0xFFF != 0 || len == 0 {
        return Err(SysError::Inval);
    }
    let ie = crate::arch::irq::save_disable();
    let mut off = 0u64;
    while off < len {
        let Some(pa) = alloc_zeroed_page() else {
            crate::arch::irq::restore(ie);
            return Err(SysError::Again);
        };
        if !map_anon_4k(root, va + off, pa) {
            frame::free(PhysAddr(pa));
            crate::arch::irq::restore(ie);
            return Err(SysError::Fault);
        }
        off += 0x1000;
    }
    crate::arch::irq::restore(ie);
    let w = unsafe { walk_in(root, va) };
    let user_ok = w
        .as_ref()
        .map(|w| w.user && w.writable && !w.huge_2m)
        .unwrap_or(false);
    let k = kernel_cr3();
    let k_ok = !user_mapped(k, va);
    write_str("[mm] mmap grow va=");
    write_hex(va);
    write_str(" pages=");
    write_u64(len / 0x1000);
    if user_ok && k_ok {
        write_str(" user (anon RW; kernel CR3 no USER; not POSIX)");
        console::nl();
        Ok(va)
    } else {
        write_str(" FAIL");
        console::nl();
        Err(SysError::Fault)
    }
}
