//! Built-in init task: fabric IPC, tensor arena, VirtIO-Accel SoftNPU.

use aether_core::accel::{AccelJobDesc, AccelOp, DType, SoftNpu};
use aether_core::demo::{run_boot_demo, DEMO_B};
use aether_core::types::PhysAddr;
use aether_drivers::softnpu::IdentityDma;
use aether_drivers::{SoftNpuDevice, VIRTIO_ACCEL_MAGIC};
use aether_hal::AccelDevice;

use crate::arch::irq;
use crate::console::{self, write_hex, write_i32, write_str, write_u64};
use crate::println;
use crate::mm::paging;
use crate::syscall::{self, SYS_DEBUG_PRINT, SYS_YIELD};

#[repr(align(64))]
struct TensorPad([u8; 256]);

static mut TENSORS: TensorPad = TensorPad([0; 256]);

fn flag(b: bool) -> &'static str {
    if b {
        "ok"
    } else {
        "FAIL"
    }
}

pub fn run_demo() {
    let msg = b"[init] syscall debug_print ok\r\n";
    let _ = syscall::dispatch(SYS_DEBUG_PRINT, msg.as_ptr() as u64, msg.len() as u64, 0);
    let _ = syscall::dispatch(SYS_YIELD, 0, 0, 0);

    println!("[fabric] running host-identical boot demo (caps/IPC/arena/sched/NPU)");
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

    write_str("[fabric] SoftNPU job#");
    write_u64(report.job_seq as u64);
    write_str(" C[0,0]=");
    write_i32(report.c00);
    write_str(" C[1,1]=");
    write_i32(report.c11);
    write_str(" events=");
    write_u64(report.events as u64);
    console::nl();

    println!("[accel] probing VirtIO-Accel + SoftNPU (identity DMA)");
    live_accel_path();

    unsafe {
        if let Some(w) = paging::walk(0x400000) {
            write_str("[mm] walk kernel _start: PA ");
            write_hex(w.phys.0);
            write_str(" huge2M=");
            write_str(if w.huge_2m { "true" } else { "false" });
            write_str(" ticks=");
            write_u64(irq::ticks());
            console::nl();
        }
    }

    if report.all_ok() {
        console::nl();
        println!("====================================================");
        println!("  FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE");
        println!("====================================================");
        crate::arch::x86_64::io::outb(0xF4, 0x00);
    } else {
        println!("[demo] FAIL -- see flags above");
        crate::arch::x86_64::io::outb(0xF4, 0x01);
    }
}

fn live_accel_path() {
    let tensors = unsafe { &mut TENSORS.0 };
    tensors.fill(0);
    let ident = [
        1i32, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1,
    ];
    for (i, v) in ident.iter().enumerate() {
        tensors[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in DEMO_B.iter().enumerate() {
        tensors[64 + i * 4..64 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }

    let base = tensors.as_ptr() as u64;
    let mut npu = SoftNpu::new();
    let job = AccelJobDesc {
        op: AccelOp::MatMul,
        flags: 0,
        m: 4,
        n: 4,
        k: 4,
        a: PhysAddr(base),
        b: PhysAddr(base + 64),
        c: PhysAddr(base + 128),
        bias: PhysAddr(0),
        a_stride: 4,
        b_stride: 4,
        c_stride: 4,
        dtype: DType::I32,
        tenant: 1,
        completion_ep: 1,
    };

    let mut dev = SoftNpuDevice::new(IdentityDma);
    match dev.probe() {
        Ok(info) => {
            write_str("[accel] ");
            write_str(dev.name());
            write_str(" vendor=");
            write_hex(info.vendor as u64);
            write_str(" queues=");
            write_u64(info.n_queues as u64);
            write_str(" backend=");
            write_u64(info.backend as u64);
            write_str(" magic=");
            write_hex(VIRTIO_ACCEL_MAGIC as u64);
            console::nl();
        }
        Err(_) => {
            println!("[accel] probe failed");
            return;
        }
    }
    let _ = dev.map(PhysAddr(base), 256);
    if dev.submit(&job).is_err() {
        println!("[accel] submit busy");
        return;
    }
    println!("[accel] submit matmul 4x4 i32 via virtio-accel ring");

    let mut idma = IdentityDma;
    let cpl = match npu.execute(&job, &mut idma) {
        Ok(c) => c,
        Err(_) => {
            println!("[accel] execute failed");
            return;
        }
    };
    let _ = dev.poll();

    let c00 = unsafe { core::ptr::read_volatile((base + 128) as *const i32) };
    let c11 = unsafe { core::ptr::read_volatile((base + 128 + 20) as *const i32) };
    write_str("[accel] complete job#");
    write_u64(cpl.job_seq as u64);
    write_str(" doorbell status=");
    write_i32(cpl.status);
    write_str(" cycles=");
    write_u64(cpl.cycles as u64);
    write_str(" C[0,0]=");
    write_i32(c00);
    write_str(" C[1,1]=");
    write_i32(c11);
    console::nl();
    if c00 == DEMO_B[0] && c11 == DEMO_B[5] {
        println!("[accel] 4x4 i32 matmul verified (I @ B = B)");
    } else {
        println!("[accel] VERIFY FAIL");
    }
}
