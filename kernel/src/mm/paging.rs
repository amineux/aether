//! Page-table walk, per-task PML4 clone, and SMEP/SMAP.
//!
//! Boot still identity-maps 4 GiB with 2 MiB leaves (trampoline PML4
//! at 0x1000). That table stays the **kernel CR3** (supervisor-only).
//! Each ring-3 task gets a cloned PML4: same identity kernel mappings,
//! USER only on that task's 2 MiB ELF window, other known user windows
//! unmapped. Not a higher-half / KPTI / POSIX MM.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use aether_core::aspace::{CR4_SMAP, CR4_SMEP};
use aether_core::types::PhysAddr;
use aether_core::{USER_IMAGE_BASE, USER_PROBE_BASE};

use crate::console::{self, write_hex, write_str, write_u64};
use crate::mm::frame;
use crate::println;

#[cfg(target_arch = "x86_64")]
const P: u64 = 1;
#[cfg(target_arch = "x86_64")]
const RW: u64 = 1 << 1;
#[cfg(target_arch = "x86_64")]
const US: u64 = 1 << 2;
#[cfg(target_arch = "x86_64")]
const PS: u64 = 1 << 7;

static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);
static SMAP_ON: AtomicBool = AtomicBool::new(false);
static SMEP_ON: AtomicBool = AtomicBool::new(false);
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

/// Clone the kernel identity map into a new PML4. USER only on
/// `[user_lo, user_hi)`; each VA in `unmap` loses Present on its 2 MiB leaf.
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
    write_str(" (supervisor identity, no USER leaves)");
    console::nl();

    write_str("[mm] /init  CR3=");
    write_hex(init_cr3);
    write_str(" ");
    print_walk("USER ", init_cr3, USER_IMAGE_BASE);
    write_str(" ");
    print_walk("probe ", init_cr3, USER_PROBE_BASE);
    console::nl();

    let init_ok = user_mapped(init_cr3, USER_IMAGE_BASE)
        && !user_mapped(init_cr3, USER_PROBE_BASE)
        && unsafe { walk_in(init_cr3, USER_PROBE_BASE) }.is_none()
        && !user_mapped(k, USER_IMAGE_BASE);

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
        println!("[mm] aspace isolate ok (task-local USER leaves + SMEP/SMAP)");
    } else {
        println!("[mm] aspace isolate FAIL");
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
            user: false,
        });
    }
    None
}
