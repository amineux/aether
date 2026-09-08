//! Host doorbell demo: allocate, MatMul-shaped submit, wait.
//!
//! ```text
//! cargo run -p aether-accel-client
//! ```
//!
//! Not a PJRT plugin. Same frozen `IreeHalCmd` as `aether-pjrt`.

use aether_accel_client::Doorbell;
use aether_drivers::ireecp::{IREE_HAL_CMD_SIZE, IREE_HAL_PKT_MAGIC};

fn main() {
    let mut bell = Doorbell::new().expect("IreeShapedCp probe");
    println!(
        "doorbell client on {} backend={}",
        bell.name(),
        bell.info().backend
    );

    let a = bell.allocate(16).unwrap();
    let b = bell.allocate(16).unwrap();
    let out = bell.allocate(16).unwrap();
    bell.copy_i32_from_host(a, &[1, 2, 3, 4]).unwrap();
    bell.copy_i32_from_host(b, &[5, 6, 7, 8]).unwrap();

    let ev = bell.submit_matmul(2, 2, 2, a, b, out).unwrap();
    let cmd = bell.last_cmd().expect("packed IreeHalCmd");
    let wire = cmd.to_le_bytes();
    println!(
        "frozen image magic=0x{magic:08X} bytes={len} ssid={ssid} exec=0x{exec:08X}",
        magic = cmd.magic,
        len = wire.len(),
        ssid = aether_core::iommu::StreamId::from_raw(cmd.stream_id).ssid(),
        exec = cmd.executable,
    );
    assert_eq!(cmd.magic, IREE_HAL_PKT_MAGIC);
    assert_eq!(wire.len(), IREE_HAL_CMD_SIZE);

    bell.wait(ev).unwrap();
    let mut got = [0i32; 4];
    bell.copy_i32_to_host(out, &mut got).unwrap();
    println!("matmul 2x2 -> {got:?}");
    assert_eq!(got, [19, 22, 43, 50]);
}
