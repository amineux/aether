//! Kernel-side invariant self-check (same `run_boot_demo` as `cargo test`).
//! The COMPLETE banner and SoftNPU verify are printed by user `/init`
//! (x86 ring-3 / RISC-V U-mode / aarch64 EL0).

use aether_core::blast::run_blast_demo;
use aether_core::chipsync::{run_chipsync_demo, run_softcct_demo};
use aether_core::cut::AffinityGraph;
use aether_core::demo::run_boot_demo;
use aether_core::greenctx::run_greenctx_demo;
use aether_core::laplacian::AffinityLaplacian;
use aether_core::opinject::run_opinject_demo;
use aether_core::sid::run_sid_submit_demo;
use aether_core::softsfi::run_softsfi_demo;
use aether_core::sva::run_sva_demo;
use aether_drivers::softnpu::KernelDma;
use aether_drivers::{run_firewall_demo, IreeShapedCp, SoftCommandProcessor};
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

    write_str("[fence] timeline seq#");
    write_u64(report.fence_id);
    write_str(" submit -> wait -> complete  credits/partition  ");
    write_str(flag(report.fence_ok));
    console::nl();

    write_str("[map] Soft SMMU pin + Memory-cap refuse  ");
    write_str(flag(report.map_ok));
    console::nl();

    write_str("[window] TypedWindow CxlMemStub map + wrong-SID refuse + CrossCut foreign-tenant  ");
    write_str(flag(report.window_ok));
    write_str(" (stub; not CXL.mem)");
    console::nl();

    {
        let mut cp = SoftCommandProcessor::new(KernelDma);
        match cp.probe() {
            Ok(info) => {
                write_str("[accel] SoftCommandProcessor probe backend=");
                write_u64(info.backend as u64);
                write_str(" ");
                write_str(cp.name());
                write_str(" n_queues=");
                write_u64(info.n_queues as u64);
                write_str(" sm=");
                write_u64(info.sm_count as u64);
                write_str(" wq=");
                write_u64(info.wq_count as u64);
                write_str(" (XQueue + SoftGreenCtx; not MIG; QEMU demo stays SoftNPU)");
                console::nl();
            }
            Err(_) => {
                write_str("[accel] SoftCommandProcessor probe FAIL");
                console::nl();
            }
        }
    }

    {
        let mut hal = IreeShapedCp::new(KernelDma);
        match hal.probe() {
            Ok(info) => {
                write_str("[accel] IreeShapedCp probe backend=");
                write_u64(info.backend as u64);
                write_str(" ");
                write_str(hal.name());
                write_str(" (IREE HAL packet; not a vendor)");
                console::nl();
            }
            Err(_) => {
                write_str("[accel] IreeShapedCp probe FAIL");
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

    write_str("[sparsify] below-threshold DROP + above KEEP + harmonic-tree REFUSE  ");
    write_str(flag(report.sparsify_ok));
    console::nl();

    write_str("[accel] SoftNPU F32/F16 soft-float 2x2 ");
    write_str(flag(report.dtype_ok));
    write_str(" (software IEEE; not a tensor ISA)");
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

    // Diligence clip: sequential so IommuMap does not share the stack
    // with run_boot_demo.
    let blast = run_blast_demo();
    write_str("[blast] tenant A=1 B=2 Memory/Activity/Cut refuse  ");
    write_str(flag(blast.mem_ok && blast.activity_ok && blast.cut_ok));
    console::nl();
    write_str("[blast] SpectralCut CrossCut refuse  ");
    write_str(flag(blast.cut_ok));
    console::nl();
    write_str("[blast] Soft SMMU wrong SID abort  ");
    write_str(flag(blast.smmu_ok));
    console::nl();
    if blast.all_ok() {
        println!("[blast] two-tenant blast radius sealed");
    } else {
        println!("[blast] FAIL -- two-tenant refuse");
    }

    // Sequential: IommuMap is large; do not share the stack with blast.
    let sid = run_sid_submit_demo();
    write_str("[sid] SET_SID-at-submit two tenants  ");
    write_str(flag(sid.two_sids && sid.cross_tenant));
    console::nl();
    write_str("[sid] Soft SMMU refuse until submit SID  ");
    write_str(flag(sid.abort_until_set && sid.wrong_sid && sid.budget_ok));
    console::nl();
    if sid.all_ok() {
        println!("[sid] two-SID Host1x-shaped submit sealed");
    } else {
        println!("[sid] FAIL -- SET_SID-at-submit");
    }

    let chipsync = run_chipsync_demo();
    write_str("[chipsync] package fences=");
    write_u64(chipsync.hierarchical_fences as u64);
    write_str(" naive=");
    write_u64(chipsync.naive_fences as u64);
    write_str(" (Fleet hierarchical; not UCIe)  ");
    write_str(flag(chipsync.package_lt_naive));
    console::nl();
    write_str("[chipsync] CCT elide last-writer=consumer  ");
    write_str(flag(chipsync.cct_elide));
    console::nl();
    if chipsync.all_ok() {
        println!("[chipsync] two-chiplet producer/consumer scoped timelines sealed");
    } else {
        println!("[chipsync] FAIL -- SoftChipletSync");
    }

    let softcct = run_softcct_demo();
    write_str("[softcct] package fences=");
    write_u64(softcct.cct_fences as u64);
    write_str(" broadcast=");
    write_u64(softcct.broadcast_fences as u64);
    write_str(" (CPElide CCT; not a coherence protocol)  ");
    write_str(flag(softcct.cct_lt_broadcast));
    console::nl();
    write_str("[softcct] incorrect elision refused  ");
    write_str(flag(softcct.incorrect_elision_refused));
    console::nl();
    write_str("[softcct] single-chiplet no-op  ");
    write_str(flag(softcct.single_chiplet_noop));
    console::nl();
    if softcct.all_ok() {
        println!("[softcct] two-chiplet producer/consumer elision sealed");
    } else {
        println!("[softcct] FAIL -- SoftCCT");
    }

    // Sequential blocks: IommuMap in the firewall clip must not share
    // the stack with chipsync / sid / softcct / SoftGreenCtx.
    // SoftSFI uses a 128-byte flat clip (no IommuMap).
    let firewall_ok = {
        let firewall = run_firewall_demo();
        write_str("[firewall] copy-then-validate Host1x race  sneak=");
        write_str(flag(firewall.sneak_without));
        write_str(" hold=");
        write_str(flag(firewall.hold_with));
        write_str(" (cmd-stream integrity; not confidential GPU)  ");
        write_str(flag(firewall.all_ok()));
        console::nl();
        if firewall.all_ok() {
            println!("[firewall] copy-then-validate race sealed");
        } else {
            println!("[firewall] FAIL -- SoftCmdFirewall");
        }
        firewall.all_ok()
    };

    let green_ok = {
        let green = run_greenctx_demo();
        write_str("[greenctx] SM/WQ pool split 70/30  part70_bw=");
        write_u64(green.part70_bw as u64);
        write_str(" unpart=");
        write_u64(green.unpart_bw as u64);
        write_str(" (Green Contexts / DetShare; not MIG)  ");
        write_str(flag(green.split_ok && green.interference_ok));
        console::nl();
        write_str("[greenctx] migrate-to-yield A 30->70 SID unchanged  ");
        write_str(flag(green.migrate_ok && green.not_mig));
        console::nl();
        if green.all_ok() {
            println!("[greenctx] two-queue SoftGreenCtx sealed");
        } else {
            println!("[greenctx] FAIL -- SoftGreenCtx");
        }
        green.all_ok()
    };

    let softsfi = run_softsfi_demo();
    write_str("[softsfi] in-bounds accept / OOB+unmodeled reject  ");
    write_str(flag(
        softsfi.in_bounds && softsfi.oob_reject && softsfi.unmodeled_reject,
    ));
    console::nl();
    write_str("[softsfi] skip-verify no cross-read  ");
    write_str(flag(softsfi.no_cross_read));
    console::nl();
    if softsfi.all_ok() {
        println!("[softsfi] two-tenant SFI+SID sandbox sealed");
    } else {
        println!("[softsfi] FAIL -- SoftSFI");
    }

    // Sequential: IommuMap in the SVA clip must not share the stack
    // with SoftSFI / SoftGreenCtx / firewall.
    let sva = run_sva_demo();
    write_str("[sva] bind mm↔ssid DMA VA  ");
    write_str(flag(sva.bind_ok && sva.dma_va && sva.sid_submit));
    write_str(" (Linux SVA/PASID inspiration; not hardware)");
    console::nl();
    write_str("[sva] unmap invalidates SSID TLB; stale translate faults  ");
    write_str(flag(sva.unmap_inv && sva.stale_fault));
    console::nl();
    if sva.all_ok() {
        println!("[sva] mm↔ssid Soft-SMMU SVA sealed");
    } else {
        println!("[sva] FAIL -- PASID/SVA");
    }

    let opinject = run_opinject_demo();
    write_str("[opinject] resident memcpy+saxpy  hot-add scale no-relaunch  ");
    write_str(flag(
        opinject.memcpy_ok
            && opinject.saxpy_ok
            && opinject.hot_add_no_relaunch
            && opinject.scale_ok,
    ));
    console::nl();
    write_str("[opinject] SID-at-submit refuse + scale unpublished  ");
    write_str(flag(opinject.sid_oob && opinject.scale_refused_before));
    console::nl();
    if opinject.all_ok() {
        println!("[opinject] resident worker + hot-add sealed");
    } else {
        println!("[opinject] FAIL -- OperatorInject");
    }

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

    if !report.all_ok()
        || !blast.all_ok()
        || !sid.all_ok()
        || !chipsync.all_ok()
        || !softcct.all_ok()
        || !firewall_ok
        || !green_ok
        || !softsfi.all_ok()
        || !sva.all_ok()
        || !opinject.all_ok()
    {
        println!("[kcheck] FAIL -- self-check");
        crate::arch::exit_qemu(false);
        crate::arch::idle();
    }
    println!("[kcheck] boot demo all_ok -- loading /init");
}
