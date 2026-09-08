//! Host-only Path B diligence clip for a partner leave-behind.
//!
//! Reuses the same `run_*_demo()` functions the kernel self-check
//! prints on serial. This crate is **not** a QEMU rebuild, **not** a
//! FLOP bench, **not** a tape-out checklist, and **not** a vendor
//! runtime. Soft SMMU stays software.

#![deny(unsafe_code)]

use std::fmt;

use aether_core::accel::{AccelOp, DType};
use aether_core::iommu::StreamId;
use aether_core::space::MemorySpace;
use aether_core::{run_blast_demo, run_greenctx_demo, BlastReport, GreenCtxReport};
use aether_drivers::ireecp::{IreeHalCmd, IREE_HAL_CMD_SIZE, IREE_HAL_PKT_MAGIC, IREE_SSID};
use aether_drivers::{run_firewall_demo, FirewallReport};
use aether_hal::ACCEL_BACKEND_IREE_SHAPED;
use aether_pjrt::{Client, Dispatch};

/// Needles CI / `make diligence-demo` greps. Kept in
/// [`expected.txt`](../expected.txt); tests include that file.
pub const EXPECTED_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/expected.txt");

#[derive(Debug)]
pub struct DemoError {
    pub clip: &'static str,
}

impl fmt::Display for DemoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "diligence clip failed: {}", self.clip)
    }
}

impl std::error::Error for DemoError {}

fn flag(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "FAIL"
    }
}

struct PjrtClip {
    submit_wait: bool,
    frozen: bool,
    opcode: AccelOp,
    magic: u32,
    backend: u8,
    ssid: u8,
}

fn run_pjrt_clip() -> Result<PjrtClip, DemoError> {
    let mut c = Client::iree_shaped().map_err(|_| DemoError {
        clip: "pjrt Client::iree_shaped",
    })?;
    let a = c.allocate(MemorySpace::Host, 16).map_err(|_| DemoError {
        clip: "pjrt allocate A",
    })?;
    let b = c.allocate(MemorySpace::Host, 16).map_err(|_| DemoError {
        clip: "pjrt allocate B",
    })?;
    let out = c.allocate(MemorySpace::Host, 16).map_err(|_| DemoError {
        clip: "pjrt allocate C",
    })?;
    c.copy_i32_from_host(a, &[1, 2, 3, 4])
        .map_err(|_| DemoError {
            clip: "pjrt copy A",
        })?;
    c.copy_i32_from_host(b, &[5, 6, 7, 8])
        .map_err(|_| DemoError {
            clip: "pjrt copy B",
        })?;
    let exec = c
        .load_executable(AccelOp::MatMul, DType::I32)
        .map_err(|_| DemoError {
            clip: "pjrt load_executable",
        })?;
    let ev = c
        .execute(Dispatch::matmul(exec, 2, 2, 2, a, b, out))
        .map_err(|_| DemoError {
            clip: "pjrt IreeHalCmd submit",
        })?;
    let submitted_async = !c.fence_ready(ev);
    let cmd: IreeHalCmd = c.last_iree_cmd().ok_or(DemoError {
        clip: "pjrt missing IreeHalCmd",
    })?;
    let opcode = cmd.decode_op().map_err(|_| DemoError {
        clip: "pjrt decode_op",
    })?;
    let frozen = cmd.magic == IREE_HAL_PKT_MAGIC
        && cmd.to_le_bytes().len() == IREE_HAL_CMD_SIZE
        && c.info().backend == ACCEL_BACKEND_IREE_SHAPED
        && StreamId::from_raw(cmd.stream_id).ssid() == IREE_SSID
        && opcode == AccelOp::MatMul;
    let cpl = c.wait(ev).map_err(|_| DemoError { clip: "pjrt wait" })?;
    let submit_wait = submitted_async && cpl.status == 0 && c.fence_ready(ev) && frozen;
    Ok(PjrtClip {
        submit_wait,
        frozen,
        opcode,
        magic: cmd.magic,
        backend: c.info().backend,
        ssid: StreamId::from_raw(cmd.stream_id).ssid(),
    })
}

fn op_name(op: AccelOp) -> &'static str {
    match op {
        AccelOp::Nop => "Nop",
        AccelOp::MatMul => "MatMul",
        AccelOp::Wave => "Wave",
    }
}

/// Scripted host narrative. Same clips as kernel serial `[blast]` /
/// `[firewall]` / `[greenctx]`, plus the PJRT `IreeHalCmd` submit+wait
/// the guest does not run.
pub fn run_diligence_demo(out: &mut dyn fmt::Write) -> Result<(), DemoError> {
    writeln!(
        out,
        "[diligence] host Path B (no QEMU rebuild; Soft SMMU is software)"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "[diligence] research prototype — not a partnership, not FLOPs, not tape-out"
    )
    .map_err(|_| DemoError { clip: "write" })?;

    let blast: BlastReport = run_blast_demo();
    writeln!(
        out,
        "[blast] tenant A=1 B=2 Memory/Activity/Cut refuse  {}",
        flag(blast.mem_ok && blast.activity_ok && blast.cut_ok)
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "[blast] SpectralCut CrossCut refuse  {}",
        flag(blast.cut_ok)
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "[blast] Soft SMMU wrong SID abort  {}",
        flag(blast.smmu_ok)
    )
    .map_err(|_| DemoError { clip: "write" })?;
    if blast.all_ok() {
        writeln!(out, "[blast] two-tenant blast radius sealed")
            .map_err(|_| DemoError { clip: "write" })?;
    } else {
        writeln!(out, "[blast] FAIL -- two-tenant refuse")
            .map_err(|_| DemoError { clip: "write" })?;
    }

    let pjrt = run_pjrt_clip()?;
    writeln!(
        out,
        "[pjrt] IreeHalCmd submit + wait  magic=0x{:08X} backend={} ssid={} opcode={}  {}",
        pjrt.magic,
        pjrt.backend,
        pjrt.ssid,
        op_name(pjrt.opcode),
        flag(pjrt.submit_wait)
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "[pjrt] research opcode {}; not FLOPs  {}",
        op_name(pjrt.opcode),
        flag(pjrt.frozen && pjrt.submit_wait)
    )
    .map_err(|_| DemoError { clip: "write" })?;

    let firewall: FirewallReport = run_firewall_demo();
    writeln!(
        out,
        "[firewall] copy-then-validate Host1x race  sneak={} hold={}  (cmd-stream integrity; not confidential GPU)  {}",
        flag(firewall.sneak_without),
        flag(firewall.hold_with),
        flag(firewall.sneak_without && firewall.hold_with)
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "[firewall] mutation-during-validate fails  {}",
        flag(firewall.sneak_without && firewall.hold_with)
    )
    .map_err(|_| DemoError { clip: "write" })?;
    if firewall.all_ok() {
        writeln!(out, "[firewall] copy-then-validate race sealed")
            .map_err(|_| DemoError { clip: "write" })?;
    } else {
        writeln!(out, "[firewall] FAIL -- SoftCmdFirewall")
            .map_err(|_| DemoError { clip: "write" })?;
    }

    let green: GreenCtxReport = run_greenctx_demo();
    writeln!(
        out,
        "[greenctx] SM/WQ pool split 70/30  part70_bw={} unpart={}  (Green Contexts / DetShare; not MIG, not FLOPs)  {}",
        green.part70_bw,
        green.unpart_bw,
        flag(green.split_ok && green.interference_ok)
    )
    .map_err(|_| DemoError { clip: "write" })?;
    if green.all_ok() {
        writeln!(out, "[greenctx] two-queue SoftGreenCtx sealed")
            .map_err(|_| DemoError { clip: "write" })?;
    } else {
        writeln!(out, "[greenctx] FAIL -- SoftGreenCtx")
            .map_err(|_| DemoError { clip: "write" })?;
    }

    writeln!(out, "[diligence] what this proves").map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  two tenants: CrossCut + wrong-SID refuse (Soft SMMU software tables)"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  PJRT-shaped Client packs frozen IreeHalCmd and waits a fence (research opcodes)"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  SoftCmdFirewall snapshot: mutation during validate does not sneak onto the queue"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  SoftGreenCtx 70/30 SM/WQ partition is measurable on a software pool"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(out, "[diligence] what this does not prove")
        .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  hardware SMMU, HW MIG, confidential GPU, or a silicon command processor"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  FLOP throughput, tape-out readiness, or a QEMU rebuild"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  a PJRT plugin, an IREE HAL driver, or an NVIDIA partnership"
    )
    .map_err(|_| DemoError { clip: "write" })?;
    writeln!(
        out,
        "  Path A -device aether-accel (stock demo stays Path B)"
    )
    .map_err(|_| DemoError { clip: "write" })?;

    let all_ok = blast.all_ok() && pjrt.submit_wait && firewall.all_ok() && green.all_ok();
    if all_ok {
        writeln!(out, "[diligence] host Path B sealed").map_err(|_| DemoError { clip: "write" })?;
        Ok(())
    } else {
        writeln!(out, "[diligence] FAIL -- host Path B")
            .map_err(|_| DemoError { clip: "write" })?;
        Err(DemoError {
            clip: "one or more clips",
        })
    }
}

/// Needles from [`expected.txt`](../expected.txt), skipping blanks and comments.
pub fn expected_needles() -> Vec<&'static str> {
    include_str!("../expected.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_diligence_demo_prints_golden_needles() {
        let mut out = String::new();
        run_diligence_demo(&mut out).expect("diligence demo");
        for needle in expected_needles() {
            assert!(
                out.contains(needle),
                "missing golden needle {needle:?}\n--- output ---\n{out}"
            );
        }
        assert!(
            !out.to_ascii_lowercase().contains("tflops"),
            "no FLOP marketing: {out}"
        );
        assert!(!out.contains("tape-out ready"), "no tape-out claim: {out}");
    }

    #[test]
    fn expected_txt_is_next_to_the_crate() {
        assert!(
            std::path::Path::new(EXPECTED_PATH).is_file(),
            "missing {EXPECTED_PATH}"
        );
    }

    #[test]
    fn expected_txt_covers_the_five_thesis_lines() {
        let needles = expected_needles().join("\n");
        assert!(needles.contains("[blast] SpectralCut CrossCut refuse"));
        assert!(needles.contains("[blast] Soft SMMU wrong SID abort"));
        assert!(needles.contains("[pjrt] IreeHalCmd submit + wait"));
        assert!(needles.contains("[firewall] mutation-during-validate fails"));
        assert!(needles.contains("[greenctx] SM/WQ pool split 70/30"));
        assert!(needles.contains("[diligence] what this proves"));
        assert!(needles.contains("[diligence] what this does not prove"));
    }
}
