//! Page-table walk, per-task PML4 clone, and SMEP/SMAP.
//!
//! Boot identity-maps 4 GiB with 2 MiB leaves (trampoline PML4 at
//! 0x1000) and aliases the first 2 GiB at the classic `-2 GiB` kernel
//! map (`KERNEL_VMA + PA`) on dedicated HH PDs. A boot-time KASLR
//! slide dual-maps an 8 MiB kernel span at `KERNEL_VMA + slide + PA`
//! and `_start` runs at that RIP. The canonical alias stays so
//! `code-model=kernel` absolute symbols still resolve. The identity
//! 4 GiB stays mapped on purpose: SoftNPU DMA, page-table walks
//! (tables are still addressed by PA), AP SIPI, Multiboot mailbox,
//! and user ELF windows. That table stays the **kernel CR3**
//! (supervisor-only). Each ring-3 task gets a cloned PML4: same
//! identity + HH kernel mappings, USER only on that task's 2 MiB ELF
//! window, other known user windows unmapped.
//!
//! Documented KASLR subset (fixed 16 MiB slots + cmdline / entropy).
//! Not PIE / reloc (cannot unmap the unused alias), not KPTI, not
//! PCID, not COW, not a POSIX MM.

#[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[cfg(target_arch = "x86_64")]
use aether_core::aspace::{CR4_SMAP, CR4_SMEP};
use aether_core::types::PhysAddr;
#[cfg(target_arch = "x86_64")]
use aether_core::{
    kaslr_slide_valid, kernel_text_va_slid, KASLR_KERNEL_SPAN, KASLR_MAILBOX_SLIDE, KERNEL_LMA,
    KERNEL_TEXT_VA, KERNEL_VMA, USER_IMAGE_BASE, USER_PROBE_BASE,
};

#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
use crate::console::{self, write_hex, write_str, write_u64};
#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
use crate::mm::frame;
#[cfg(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64"))]
use crate::println;

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

#[derive(Clone, Copy, Debug)]
pub struct Walk {
    pub pml4e: u64,
    pub pdpte: u64,
    pub pde: u64,
    pub phys: PhysAddr,
    pub huge_2m: bool,
    pub user: bool,
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
pub fn load_cr3(val: u64) {
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

/// Switch CR3 when the next thread's aspace differs. Logs the first few.
#[cfg(target_arch = "x86_64")]
pub fn switch_cr3(next: u64, from_tid: u32, to_tid: u32) {
    let want = if next == 0 { kernel_cr3() } else { next } & !0xFFF;
    let cur = unsafe { cr3() & !0xFFF };
    if cur == want {
        return;
    }
    load_cr3(want);
    let n = CR3_SWITCH_LOGS.fetch_add(1, Ordering::Relaxed);
    if n < 6 {
        write_str("[mm] cr3 switch tid=");
        write_u64(from_tid as u64);
        write_str("->");
        write_u64(to_tid as u64);
        write_str(" cr3=");
        write_hex(want);
        console::nl();
    }
}

#[cfg(target_arch = "x86_64")]
fn read64(pa: u64) -> u64 {
    unsafe { core::ptr::read_volatile(pa as *const u64) }
}

#[cfg(target_arch = "x86_64")]
fn write64(pa: u64, v: u64) {
    unsafe {
        core::ptr::write_volatile(pa as *mut u64, v);
    }
}

/// Walk `va` in `root` (identity-mapped tables). USER requires US at
/// every level of the walk.
#[cfg(target_arch = "x86_64")]
pub unsafe fn walk_in(root: u64, va: u64) -> Option<Walk> {
    let pml4 = (root & !0xFFF) as *const u64;
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
            user: pml4e & US != 0 && pdpte & US != 0,
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
            user: pml4e & US != 0 && pdpte & US != 0 && pde & US != 0,
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
        user: pml4e & US != 0 && pdpte & US != 0 && pde & US != 0 && pte & US != 0,
    })
}

/// Walk `va` in the current address space (identity-mapped tables).
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

/// Set USER on PML4[0] and PDPT[0] so ring-3 can walk the low 1 GiB.
/// Leaf pages stay supervisor-only until [`allow_user_2m`].
/// Operates on the **current** CR3 (the running user aspace during syscall).
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
fn alloc_zeroed_page() -> Option<u64> {
    let p = frame::alloc()?;
    unsafe {
        core::ptr::write_bytes(p.0 as *mut u8, 0, 4096);
    }
    Some(p.0)
}

/// Clone the kernel identity + HH map into a new PML4. USER only on
/// `[user_lo, user_hi)`; each VA in `unmap` loses Present on its 2 MiB leaf.
/// PML4[511] is copied with the rest of the kernel PML4 (shared HH PDPT).
#[cfg(target_arch = "x86_64")]
pub fn clone_user_aspace(user_lo: u64, user_hi: u64, unmap: &[u64]) -> Option<u64> {
    let kcr3 = kernel_cr3();
    let pml4 = alloc_zeroed_page()?;
    let pdpt = alloc_zeroed_page()?;
    unsafe {
        core::ptr::copy_nonoverlapping(kcr3 as *const u8, pml4 as *mut u8, 4096);
    }
    let k_pml4e = read64(kcr3);
    if k_pml4e & P == 0 {
        return None;
    }
    let k_pdpt = k_pml4e & 0x000F_FFFF_FFFF_F000;
    unsafe {
        core::ptr::copy_nonoverlapping(k_pdpt as *const u8, pdpt as *mut u8, 4096);
    }

    let mut new_pds = [0u64; 4];
    for i in 0..4 {
        let k_pdpte = read64(k_pdpt + i as u64 * 8);
        if k_pdpte & P == 0 {
            continue;
        }
        let pd = alloc_zeroed_page()?;
        let k_pd = k_pdpte & 0x000F_FFFF_FFFF_F000;
        unsafe {
            core::ptr::copy_nonoverlapping(k_pd as *const u8, pd as *mut u8, 4096);
        }
        for j in 0..512u64 {
            let e = read64(pd + j * 8);
            write64(pd + j * 8, e & !US);
        }
        write64(
            pdpt + i as u64 * 8,
            (pd & !0xFFF) | (k_pdpte & (PS | 0xFFF) & !US) | P | RW,
        );
        new_pds[i] = pd;
    }

    write64(pml4, (pdpt & !0xFFF) | P | RW | US);

    let gi = ((user_lo >> 30) & 0x1FF) as usize;
    if gi < 4 {
        write64(pdpt + gi as u64 * 8, read64(pdpt + gi as u64 * 8) | US);
    }

    let mut va = user_lo & !0x1F_FFFF;
    while va < user_hi {
        let i3 = ((va >> 30) & 0x1FF) as usize;
        let i2 = ((va >> 21) & 0x1FF) as usize;
        if i3 < 4 && new_pds[i3] != 0 {
            write64(new_pds[i3] + i2 as u64 * 8, read64(new_pds[i3] + i2 as u64 * 8) | US);
        }
        va += 0x20_0000;
    }

    for &u in unmap {
        let i3 = ((u >> 30) & 0x1FF) as usize;
        let i2 = ((u >> 21) & 0x1FF) as usize;
        if i3 < 4 && new_pds[i3] != 0 {
            write64(new_pds[i3] + i2 as u64 * 8, read64(new_pds[i3] + i2 as u64 * 8) & !P);
        }
    }

    Some(pml4)
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
    write_str(" (supervisor identity+HH, no USER leaves)");
    console::nl();

    write_str("[mm] /init  CR3=");
    write_hex(init_cr3);
    write_str(" ");
    print_walk("USER ", init_cr3, USER_IMAGE_BASE);
    write_str(" ");
    print_walk("probe ", init_cr3, USER_PROBE_BASE);
    console::nl();

    let slid = kernel_text_va();
    let hh_in_user = unsafe { walk_in(init_cr3, slid) }
        .map(|w| w.phys.0 == KERNEL_LMA && !w.user)
        .unwrap_or(false);
    let canon_in_user = unsafe { walk_in(init_cr3, KERNEL_TEXT_VA) }
        .map(|w| w.phys.0 == KERNEL_LMA && !w.user)
        .unwrap_or(false);

    let init_ok = user_mapped(init_cr3, USER_IMAGE_BASE)
        && !user_mapped(init_cr3, USER_PROBE_BASE)
        && unsafe { walk_in(init_cr3, USER_PROBE_BASE) }.is_none()
        && !user_mapped(k, USER_IMAGE_BASE)
        && hh_in_user
        && canon_in_user;

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
    } else {
        true
    };

    let ok = init_ok && probe_ok && smep_enabled() && smap_enabled();
    if ok {
        println!("[mm] aspace isolate ok (task-local USER leaves + SMEP/SMAP + HH in user CR3)");
    } else {
        println!("[mm] aspace isolate FAIL");
    }
    ok
}

/// Serial proof: RIP is at the slid HH text; identity still names the LMA.
#[cfg(target_arch = "x86_64")]
pub fn prove_higher_half() -> bool {
    let rip: u64;
    unsafe {
        core::arch::asm!("lea {0}, [rip]", out(reg) rip, options(nomem, nostack, preserves_flags));
    }
    let slide = kaslr_slide();
    let slid_text = kernel_text_va();
    let w_hh = unsafe { walk(slid_text) };
    let w_canon = unsafe { walk(KERNEL_TEXT_VA) };
    let w_id = unsafe { walk(KERNEL_LMA) };
    let hh_ok = w_hh
        .as_ref()
        .map(|w| w.phys.0 == KERNEL_LMA && w.huge_2m && !w.user)
        .unwrap_or(false);
    let canon_ok = w_canon
        .as_ref()
        .map(|w| w.phys.0 == KERNEL_LMA && w.huge_2m && !w.user)
        .unwrap_or(false);
    let id_ok = w_id
        .as_ref()
        .map(|w| w.phys.0 == KERNEL_LMA && w.huge_2m && !w.user)
        .unwrap_or(false);
    let rip_ok = rip >= slid_text && rip < slid_text + KASLR_KERNEL_SPAN && rip >= KERNEL_VMA;
    write_str("[mm] kaslr slide=");
    write_hex(slide);
    write_str(" idx=");
    write_u64(slide / aether_core::KASLR_SLIDE_STRIDE);
    write_str(" (dual-map HH; identity 4 GiB kept)");
    console::nl();
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
    if hh_ok && canon_ok && id_ok && rip_ok {
        println!(
            "[mm] higher-half ok (ffffffff80000000+PA + slide; identity 4 GiB kept for DMA)"
        );
        true
    } else {
        println!("[mm] higher-half FAIL");
        false
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
        core::ptr::write_bytes(p.0 as *mut u8, 0, 4096);
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
        core::ptr::write_bytes(p.0 as *mut u8, 0, 4096);
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
        });
    }
    None
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
