//! Kernel-side invariant self-check (same `run_boot_demo` as `cargo test`).
//! The COMPLETE banner and SoftNPU verify are printed by ring-3 `/init`.

use aether_core::cut::AffinityGraph;
use aether_core::demo::run_boot_demo;
use aether_core::laplacian::AffinityLaplacian;
use aether_drivers::softnpu::IdentityDma;
use aether_drivers::SoftCommandProcessor;
use aether_hal::AccelDevice;

use crate::arch::irq;
use crate::console::{self, write_hex, write_i32, write_str, write_u64};
use crate::mm::paging;
use crate::println;

fn flag(b: bool) -> &'static str {
    if b {
        "ok"
    } else {
        "FAIL"
    }
}

pub fn run_kernel_selfcheck() {
    println!("[kcheck] host-identical boot demo (caps/IPC/arena/sched/NPU)");
    let report = run_boot_demo();

    write_str("[fabric] IPC ");
    write_str(flag(report.ipc_ok));
    write_str("  arena ");
    write_str(flag(report.arena_ok));
    write_str(" bank");
    write_u64(report.arena_bank as u64);
    write_str(" @ ");
    write_hex(report.arena_base);
    write_str("  sched ");
    write_str(flag(report.sched_ok));
    write_str("  isolation ");
    write_str(flag(report.isolation_ok));
    console::nl();

    write_str("[cut] bind SpectralCut cap  Phi=");
    write_u64(report.cut_phi_milli as u64);
    write_str(" milli  bound=400  ");
    write_str(flag(report.cut_ok));
    write_str(" (place NPU+bank0 ok, tile1+bank0 CrossCut refuse)");
    console::nl();

    write_str("[hodge] gradient tree-offload + curl ring + harmonic-tree REFUSE  ");
    write_str(flag(report.hodge_ok));
    console::nl();

    write_str("[space] TILE_SRAM place (chiplet0,tile2)  UNIFIED=never  ");
    write_str(flag(report.space_ok));
    console::nl();

    write_str("[activity] VIRT_ACCEL fabric endpoint (not ioctl)  ");
    write_str(flag(report.activity_ok));
    console::nl();

    write_str("[fence] submit#");
    write_u64(report.fence_id);
    write_str(" -> complete  phase=COMPUTE  credits/partition  ");
    write_str(flag(report.fence_ok));
    console::nl();

    write_str("[map] Soft SMMU pin + Memory-cap refuse  ");
    write_str(flag(report.map_ok));
    console::nl();

    {
        let mut cp = SoftCommandProcessor::new(IdentityDma);
        match cp.probe() {
            Ok(info) => {
                write_str("[accel] SoftCommandProcessor probe backend=");
                write_u64(info.backend as u64);
                write_str(" ");
                write_str(cp.name());
                write_str(" (host CP path; QEMU demo stays SoftNPU)");
                console::nl();
            }
            Err(_) => {
                write_str("[accel] SoftCommandProcessor probe FAIL");
                console::nl();
            }
        }
    }

    write_str("[color] tenant/bank paint  Compute foreign refuse + Exchange ok  ");
    write_str(flag(report.color_ok));
    console::nl();

    write_str("[cdt] revoke descendants ");
    write_str(flag(report.revoke_ok));
    console::nl();

    write_str("[opkernel] tree+gradient inject + harmonic-tree REFUSE  ");
    write_str(flag(report.opkernel_ok));
    console::nl();

    {
        let g = AffinityGraph::qemu_package();
        let lap = AffinityLaplacian::from_graph(&g);
        let mask = lap.fiedler_mask();
        let chiplet = 0b000111u32;
        let split = mask == chiplet || mask == (!chiplet & 0b111111);
        write_str("[laplace] L=D-A n=");
        write_u64(lap.n as u64);
        write_str(" fiedler-mask=");
        write_hex(mask as u64);
        write_str(" chiplet-split=");
        write_str(flag(split));
        console::nl();
    }

    write_str("[fabric] SoftNPU job#");
    write_u64(report.job_seq as u64);
    write_str(" C[0,0]=");
    write_i32(report.c00);
    write_str(" C[1,1]=");
    write_i32(report.c11);
    write_str(" events=");
    write_u64(report.events as u64);
    console::nl();

    unsafe {
        if let Some(w) = paging::walk(crate::arch::kernel_text_va()) {
            write_str("[mm] walk kernel _start: PA ");
            write_hex(w.phys.0);
            write_str(" huge=");
            write_str(if w.huge_2m { "true" } else { "false" });
            write_str(" ticks=");
            write_u64(irq::ticks());
            console::nl();
        }
    }

    if !report.all_ok() {
        println!("[kcheck] FAIL -- self-check");
        crate::arch::exit_qemu(false);
        crate::arch::idle();
    }
    #[cfg(target_arch = "x86_64")]
    println!("[kcheck] boot demo all_ok -- loading /init");
    #[cfg(target_arch = "riscv64")]
    println!("[kcheck] boot demo all_ok -- RISC-V thin port (no /init)");
    #[cfg(target_arch = "aarch64")]
    println!("[kcheck] boot demo all_ok -- aarch64 thin port (no EL0)");
}
