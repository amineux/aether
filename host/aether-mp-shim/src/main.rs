//! MicroPerceptron-shaped thin consumer: memcpy / matmul / wave on frozen
//! `IreeHalCmd`. Inspiration name only. Secondary to `aether-pjrt`.
//!
//! ```text
//! cargo run -p aether-mp-shim
//! make mp-shim
//! ```
//!
//! Not a MicroPerceptron port. Not a PJRT plugin. Same 96-byte image.

use aether_core::iommu::StreamId;
use aether_drivers::ireecp::{
    IREE_HAL_CMD_SIZE, IREE_HAL_PKT_MAGIC, IREE_REF_EXECUTABLE, IREE_SSID,
};
use aether_mp_shim::{MpShim, Opcode};

fn main() {
    let mut shim = MpShim::doorbell().expect("IreeShapedCp probe");
    println!(
        "mp-shim (research sketch; MicroPerceptron inspiration only) on {} backend={}",
        shim.name(),
        shim.info().backend
    );
    println!("mp-shim secondary to aether-pjrt; not a vendor; not a full port");

    let src = shim.allocate(16).unwrap();
    let dst = shim.allocate(16).unwrap();
    shim.copy_i32_from_host(src, &[9, 8, 7, 6]).unwrap();
    assert_eq!(shim.memcpy(dst, src).unwrap(), Opcode::Memcpy);
    let mut copied = [0i32; 4];
    shim.copy_i32_to_host(dst, &mut copied).unwrap();
    println!("memcpy host-copy {copied:?} (not a v1 TRANSFER packet)");

    let a = shim.allocate(16).unwrap();
    let b = shim.allocate(16).unwrap();
    let out = shim.allocate(16).unwrap();
    shim.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
    shim.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();

    let ev = shim.submit_matmul(2, 2, 2, a, b, out).unwrap();
    let cmd = shim.last_cmd().expect("packed IreeHalCmd");
    let wire = cmd.to_le_bytes();
    println!(
        "frozen image magic=0x{magic:08X} size={len} backend={backend} executable=0x{exec:08X} ssid={ssid}",
        magic = cmd.magic,
        len = wire.len(),
        backend = shim.info().backend,
        exec = cmd.executable,
        ssid = StreamId::from_raw(cmd.stream_id).ssid(),
    );
    assert_eq!(cmd.magic, IREE_HAL_PKT_MAGIC);
    assert_eq!(wire.len(), IREE_HAL_CMD_SIZE);
    assert_eq!(cmd.executable, IREE_REF_EXECUTABLE);
    assert_eq!(StreamId::from_raw(cmd.stream_id).ssid(), IREE_SSID);

    shim.wait(ev).unwrap();
    let mut got = [0i32; 4];
    shim.copy_i32_to_host(out, &mut got).unwrap();
    println!("matmul 2x2 -> {got:?}");
    assert_eq!(got, [19, 22, 43, 50]);

    let bias = shim.allocate(8).unwrap();
    shim.copy_i32_from_host(a, &[1, 0, 0, 1]).unwrap();
    shim.copy_i32_from_host(b, &[1, 2, 3, 4]).unwrap();
    shim.copy_i32_from_host(bias, &[10, 20]).unwrap();
    let ev = shim.submit_wave(2, 2, 2, a, b, out, bias).unwrap();
    shim.wait(ev).unwrap();
    let mut wgot = [0i32; 4];
    shim.copy_i32_to_host(out, &mut wgot).unwrap();
    println!("wave 2x2+bias -> {wgot:?}");
    assert_eq!(wgot, [11, 22, 13, 24]);

    let mut bad = MpShim::doorbell().expect("IreeShapedCp probe");
    let pin = bad.allocate(256).unwrap();
    let pa = bad.buffer_guest_pa(pin).unwrap();
    let mut job = aether_core::accel::AccelJobDesc::matmul_i32(
        2,
        2,
        2,
        pa,
        aether_core::types::PhysAddr(pa.0 + 16),
        aether_core::types::PhysAddr(pa.0 + 32),
        1,
    );
    job.place = job.place.with_tile(0);
    let mut cmd = aether_drivers::ireecp::IreeHalCmd::pack(&job, bad.iommu()).unwrap();
    cmd.executable = 0xDEAD;
    match bad.submit_image(cmd, &job) {
        Err(aether_mp_shim::Error::Hal(aether_hal::HalError::Unsupported)) => {
            println!("bad executable 0xDEAD refused");
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }

    let mut soft = MpShim::soft_cp().expect("Soft-CP probe");
    let a = soft.allocate(16).unwrap();
    let b = soft.allocate(16).unwrap();
    let out = soft.allocate(16).unwrap();
    soft.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
    soft.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();
    let ev = soft.submit_matmul(2, 2, 2, a, b, out).unwrap();
    let sim = soft.last_firewall_sim().expect("Soft-CP firewall");
    assert!(sim.noted(), "SoftCmdFirewall copy-then-validate");
    soft.wait(ev).unwrap();
    println!(
        "soft-cp path firewall copy_steps={} validate_steps={} (SoftCmdFirewall still applies)",
        sim.copy_steps, sim.validate_steps
    );

    println!("path B remains canonical (stock QEMU; this shim does not rebuild QEMU)");
    println!("[mp-shim] ok");
}
