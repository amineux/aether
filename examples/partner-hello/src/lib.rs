//! Partner hello: pack a frozen [`IreeHalCmd`] and submit through
//! [`aether_pjrt`] / `IreeShapedCp` on the host. No QEMU rebuild.
//!
//! This is **not** a signed vendor, a PJRT plugin, hardware SMMU, FLOPs,
//! tape-out, or an NVIDIA partnership.

#![deny(unsafe_code)]

use aether_core::accel::{AccelOp, DType};
use aether_core::iommu::StreamId;
use aether_core::space::MemorySpace;
use aether_drivers::ireecp::{
    IreeHalCmd, IREE_HAL_CMD_SIZE, IREE_HAL_COMMAND_CATEGORY_DISPATCH, IREE_HAL_PKT_MAGIC,
    IREE_REF_EXECUTABLE, IREE_SSID,
};
use aether_hal::ACCEL_BACKEND_IREE_SHAPED;
use aether_pjrt::{Client, Dispatch, Error};

/// Golden 2×2 I32 matmul: A=[[1,2],[3,4]] B=[[5,6],[7,8]] → [[19,22],[43,50]].
pub const GOLDEN_MATMUL: [i32; 4] = [19, 22, 43, 50];

/// One-screen `AccelDevice` plug-in. Printed by the binary; grepped in CI.
pub const PLUG_SCREEN: &str = "\
how to plug AccelDevice (one screen)
------------------------------------
Reserved backends — do not reuse:
  0 SoftNPU  1 virtqueue SoftNPU  2 PartnerNpuStub  3 Soft-CP  4 IreeShapedCp
Fill AccelInfo.backend with an id of your own. Do not invent a silicon
vendor ID in this tree (0xAE7E is the Aether research marker).

1. PCI / MMIO / NoC probe. AccelInfo { backend: YOUR_ID, vendor: YOUR_CHIP, … }.
2. Implement aether_hal::AccelDevice { probe, submit, poll, map }.
   Start from IreeShapedCp (this frozen IREE HAL packet) or
   SoftCommandProcessor (Aether-native CpCmd). PartnerNpuStub is a no-op.
3. map(): Memory+MAP cap walk + Soft SMMU pin (software STE→CD→S1/S2).
   IOVA is not identity. Hardware SMMU still needs partner silicon.
4. submit(): pack frozen IreeHalCmd (96-byte LE, magic 0xAE7E1EE1,
   executable 0x0001EE00, ssid=2) or your CP packet. Doorbell. Do not
   execute in submit. Completions arrive on poll / IRQ.
5. IRQ: AccelDevice::poll, Timeline::complete / retire_into.

Path B (in-kernel SoftNPU BAR, stock QEMU) stays the canonical demo.
This example does not rebuild QEMU.

Not claimed: signed vendor, PJRT plugin (GetPjRtApi), IREE HAL driver,
hardware SMMU, FLOPs, tape-out, NVIDIA partnership.
";

/// Result of one golden host submit.
#[derive(Clone, Copy, Debug)]
pub struct HelloReport {
    pub backend: u8,
    /// Aether research marker (`0xAE7E`), not a silicon vendor.
    pub research_marker: u32,
    pub cmd: IreeHalCmd,
    pub result: [i32; 4],
}

/// Allocate, pin through Soft SMMU, pack frozen `IreeHalCmd`, submit, wait.
pub fn run_golden() -> Result<HelloReport, Error> {
    let mut c = Client::iree_shaped()?;
    let info = c.info();
    if info.backend != ACCEL_BACKEND_IREE_SHAPED {
        return Err(Error::FrozenImage);
    }

    let a = c.allocate(MemorySpace::Host, 16)?;
    let b = c.allocate(MemorySpace::Host, 16)?;
    let out = c.allocate(MemorySpace::Host, 16)?;
    c.copy_i32_from_host(a, &[1, 2, 3, 4])?;
    c.copy_i32_from_host(b, &[5, 6, 7, 8])?;

    let exec = c.load_executable(AccelOp::MatMul, DType::I32)?;
    if c.executable(exec)?.isa_blob_id != IREE_REF_EXECUTABLE {
        return Err(Error::Unsupported);
    }

    let ev = c.execute(Dispatch::matmul(exec, 2, 2, 2, a, b, out))?;
    let cmd = c.last_iree_cmd().ok_or(Error::FrozenImage)?;
    check_frozen(&cmd)?;
    let cpl = c.wait(ev)?;
    if cpl.status != 0 {
        return Err(Error::JobFault(cpl.status));
    }

    let mut result = [0i32; 4];
    c.copy_i32_to_host(out, &mut result)?;
    if result != GOLDEN_MATMUL {
        return Err(Error::JobFault(-1));
    }

    Ok(HelloReport {
        backend: info.backend,
        research_marker: info.vendor,
        cmd,
        result,
    })
}

/// IreeShaped refuses any `isa_blob_id` other than [`IREE_REF_EXECUTABLE`].
pub fn refuse_bad_executable() -> Result<(), Error> {
    let mut c = Client::iree_shaped()?;
    match c.load_executable_blob(0xDEAD, AccelOp::MatMul, DType::I32) {
        Err(Error::Unsupported) => Ok(()),
        Ok(_) => Err(Error::Unsupported),
        Err(e) => Err(e),
    }
}

/// SpecForge freeze: magic 0xAE7E1EE1, 96-byte LE, ssid=2, DISPATCH,
/// executable 0x0001EE00, 2×2×2 workgroup, I32 byte spans.
pub fn check_frozen(cmd: &IreeHalCmd) -> Result<(), Error> {
    cmd.check_v1().map_err(Error::Hal)?;
    let wire = cmd.to_le_bytes();
    if cmd.magic != IREE_HAL_PKT_MAGIC || wire.len() != IREE_HAL_CMD_SIZE {
        return Err(Error::FrozenImage);
    }
    if StreamId::from_raw(cmd.stream_id).ssid() != IREE_SSID {
        return Err(Error::FrozenImage);
    }
    if cmd.executable != IREE_REF_EXECUTABLE {
        return Err(Error::Unsupported);
    }
    if cmd.command_categories != IREE_HAL_COMMAND_CATEGORY_DISPATCH {
        return Err(Error::FrozenImage);
    }
    if cmd.workgroup_count_x != 2 || cmd.workgroup_count_y != 2 || cmd.workgroup_count_z != 2 {
        return Err(Error::FrozenImage);
    }
    if cmd.binding0_length != 16 || cmd.binding1_length != 16 || cmd.binding2_length != 16 {
        return Err(Error::FrozenImage);
    }
    Ok(())
}

/// Dump frozen packet fields (ACCEL.md offsets). Host leave-behind, not a BAR.
pub fn format_packet(cmd: &IreeHalCmd) -> String {
    let ssid = StreamId::from_raw(cmd.stream_id).ssid();
    format!(
        "\
[partner-hello] frozen IreeHalCmd (96-byte LE)
[partner-hello] 0x00 magic              = {magic:#010X}
[partner-hello] 0x04 command_categories = {cats:#06X}
[partner-hello] 0x06 binding_count      = {bc}
[partner-hello] 0x08 executable         = {exec:#010X}
[partner-hello] 0x0C function           = {func}
[partner-hello] 0x10 workgroup_count    = {wx},{wy},{wz}  (AccelJobDesc m,n,k; not tiles)
[partner-hello] 0x1C element_type       = {ety:#010X}
[partner-hello] 0x20 queue_affinity     = {qaff:#010X}
[partner-hello] 0x24 stream_id          = {sid:#010X}  (Soft SMMU ssid={ssid})
[partner-hello] 0x28 binding0_offset    = {b0o:#018X}  (IOVA A)
[partner-hello] 0x30 binding1_offset    = {b1o:#018X}
[partner-hello] 0x38 binding2_offset    = {b2o:#018X}
[partner-hello] 0x40 binding3_offset    = {b3o:#018X}
[partner-hello] 0x48 binding0_length    = {b0l}  (dtype-aware bytes)
[partner-hello] 0x4C binding1_length    = {b1l}
[partner-hello] 0x50 binding2_length    = {b2l}
[partner-hello] 0x54 binding3_length    = {b3l}
[partner-hello] 0x58 signal_payload     = {sig}
[partner-hello] magic=0xAE7E1EE1 size=96 backend=4 executable=0x0001EE00 ssid={ssid}",
        magic = cmd.magic,
        cats = cmd.command_categories,
        bc = cmd.binding_count,
        exec = cmd.executable,
        func = cmd.function,
        wx = cmd.workgroup_count_x,
        wy = cmd.workgroup_count_y,
        wz = cmd.workgroup_count_z,
        ety = cmd.element_type,
        qaff = cmd.queue_affinity,
        sid = cmd.stream_id,
        ssid = ssid,
        b0o = cmd.binding0_offset,
        b1o = cmd.binding1_offset,
        b2o = cmd.binding2_offset,
        b3o = cmd.binding3_offset,
        b0l = cmd.binding0_length,
        b1l = cmd.binding1_length,
        b2l = cmd.binding2_length,
        b3l = cmd.binding3_length,
        sig = cmd.signal_payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::iommu::SOFT_SMMU_IOVA_BASE;
    use aether_drivers::ireecp::IREE_HAL_ELEMENT_TYPE_INT_32;
    use aether_hal::{
        ACCEL_BACKEND_PARTNER_STUB, ACCEL_BACKEND_SOFTNPU, ACCEL_BACKEND_SOFT_CP,
        ACCEL_BACKEND_VIRTIO_SOFTNPU,
    };

    #[test]
    fn golden_path_packs_frozen_packet() {
        let r = run_golden().expect("golden IreeHalCmd submit");
        assert_eq!(r.backend, ACCEL_BACKEND_IREE_SHAPED);
        assert_ne!(r.backend, ACCEL_BACKEND_SOFTNPU);
        assert_ne!(r.backend, ACCEL_BACKEND_VIRTIO_SOFTNPU);
        assert_ne!(r.backend, ACCEL_BACKEND_PARTNER_STUB);
        assert_ne!(r.backend, ACCEL_BACKEND_SOFT_CP);
        assert_eq!(
            r.research_marker, 0xAE7E,
            "Aether marker; not a silicon vendor"
        );
        assert_eq!(r.result, GOLDEN_MATMUL);
        assert_eq!(r.cmd.magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(r.cmd.to_le_bytes().len(), IREE_HAL_CMD_SIZE);
        assert_eq!(r.cmd.executable, IREE_REF_EXECUTABLE);
        assert_eq!(r.cmd.element_type, IREE_HAL_ELEMENT_TYPE_INT_32);
        assert!(r.cmd.binding0_offset >= SOFT_SMMU_IOVA_BASE);
        assert_ne!(r.cmd.binding0_offset, 0);
        check_frozen(&r.cmd).unwrap();
        let dump = format_packet(&r.cmd);
        assert!(dump.contains("0x00 magic              = 0xAE7E1EE1"));
        assert!(dump.contains("0x08 executable         = 0x0001EE00"));
        assert!(dump.contains("magic=0xAE7E1EE1 size=96 backend=4 executable=0x0001EE00 ssid=2"));
    }

    #[test]
    fn bad_executable_id_is_refused() {
        refuse_bad_executable().expect("0xDEAD must be Unsupported");
    }

    #[test]
    fn plug_screen_states_non_claims() {
        assert!(PLUG_SCREEN.contains("Do not invent a silicon"));
        assert!(PLUG_SCREEN.contains("vendor ID"));
        assert!(PLUG_SCREEN.contains("Path B"));
        assert!(PLUG_SCREEN.contains("does not rebuild QEMU"));
        assert!(PLUG_SCREEN.contains("signed vendor"));
        assert!(PLUG_SCREEN.contains("PJRT plugin"));
        assert!(PLUG_SCREEN.contains("hardware SMMU"));
        assert!(PLUG_SCREEN.contains("FLOPs"));
        assert!(PLUG_SCREEN.contains("tape-out"));
        assert!(PLUG_SCREEN.contains("NVIDIA"));
    }
}
