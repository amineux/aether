# Aether scoreboard: measurable axes

Every number below was measured on `main` @ `3aa4223` (#230 merged, 2026-10-17, Paris; `make verify` re-checks the run-time ones) and
comes with the command that reproduces it. Run the commands from the repo
root. If a number here disagrees with what the command prints, the command
wins and this file is stale.

The structural numbers (unsafe budget, API surface, Kani harness count, ISAs
in CI) are also gated: `core/tests/scoreboard.rs` recomputes them from the
tree under `cargo test --workspace` and fails if this file drifts. Run-time
numbers (test count, refused lines, coverage) have their own gates, named in
each row.

This replaces "how much better" with axes anyone can check. The **Behind**
section is part of the scoreboard, not a footnote.

## Axes

| Axis | Aether today (measured) | Reproduce | Reference point (cited) |
| --- | --- | --- | --- |
| Named refused attack classes | **99** `result=refused` lines: 98 named single-attack lines + 1 `tenant-fuzz` line (seed `0x5AE7`, 4096 ops, 29 variants, `escapes=0 unnamed=0`). The fuzz still excludes raw `unmap` / `unmap_stream` until #161 lands | `cargo run -q --locked -p aether-redteam \| grep -c result=refused` | No public refusal catalogue was found for MIG, MPS or XDNA; MIG documents fault, memory and bandwidth isolation enforced in hardware [MIG][MPS][XDNA] |
| Isolation conformance kit | **15** attack classes, `aether-soft` backend: `refused=15 accepted=0` (`result=conformant`). The weak teaching stub fails with `accepted=10`, so the kit can fail | `cargo run -q --manifest-path examples/isolation-kit/Cargo.toml --release \| grep '^\[isolation-kit\] backend='` | XDNA partitions by column and MI300X by CPX/NPS mode, in silicon [XDNA][MI300X] |
| Tested isolation properties | 11 defended invariants (DILIGENCE §Security invariants, host-tested); 5 capability-tree properties (P-Revoke, P-Unrelated, P-Named, P-Unforge, P-Monotone) in 16 tests; two-world noninterference: `seeds=512 ops=172032 divergences=0`, **7/7** negative controls caught. Tests, not proofs | `cargo run --locked --release -q -p aether-redteam --example noninterference -- 512 16 \| grep -E 'worlds=\|caught=true'` and `grep -c '#\[test\]' core/src/caps_props.rs` | seL4 has machine-checked integrity and confidentiality proofs on verified AArch64 and RISC-V configurations [seL4-VC] |
| Bounded model checking | **18** Kani harnesses (arena 7, SMMU 4, caps 3, KV 3, sysnr 1). Not run in CI and not re-run for this file; `make kani` takes tens of minutes. Bounded checks, not a full proof | `grep -c '#\[kani::proof\]' proofs/src/*_proofs.rs` | Asterinas publishes a soundness analysis of its unsafe core plus Verus work [OSTD-sound]; seL4 [seL4-VC] |
| Error variants exercised vs allowlisted | **153** `*Error` variants in core/hal/drivers/kernel: **145** named in tests or demos, **8** allowlisted with a reason, and none of the 8 is a tenant refuse path waiting for a test: 4 kernel syscall errno (`SysError`, QEMU-only), 3 on the kill list (`CapError::GenerationMismatch`, `RelocError::BadMagic` / `Truncated`), 1 dead but kept for ABI (`OpKernelError::Cap`, refused first as `NotBound`). #230 named `BootFsError::NoRoom` and dropped it from the allowlist; #222 adds the tested `CapError::RevokeTooLarge` refusal | `cargo test -q --locked -p aether-core --test error_coverage -- --nocapture \| grep variants=` | No comparable published number found in the cited projects |
| Host tests | **662** passed, 0 failed (includes #222's exact-64, overflow-96 and one-over-65 capability-revocation regressions) | `cargo test --workspace --locked 2>&1 \| grep '^test result:' \| awk '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'` | Assurance comparison is the proofs row above, not test counts |
| Architectures booted in CI | **3** ISAs in QEMU: x86_64 (5 configs: plain, virtio-blk, PCID, no-PCID, `-smp 2`), riscv64 `virt`, aarch64 `virt`; riscv64 and aarch64 read their memory map from the FDT (`[mm] fdt mmap ok`). CI and dependency audit green on `3aa4223` (#230); Pages last deployed from Round 33 site fold. **0** physical boards | `grep -E 'run: make qemu' .github/workflows/ci.yml` and `gh run list --branch main --workflow ci --limit 1` | seL4 verified configurations name physical Arm and RISC-V platforms [seL4-VC]; Hubris ships on Oxide hardware [Hubris] |
| Unsafe budget | **248** `unsafe` keywords across **34** files: kernel 227, drivers 6 (`mmio.rs`, `softnpu.rs`), ring-3 user programs 15; 14 `SAFETY:` comments. `core/` is `#![forbid(unsafe_code)]`. Any change needs an edit to `unsafe-budget.txt` | `cargo test -q --locked -p aether-core --test unsafe_budget -- --nocapture \| grep budget_ok` | Asterinas confines unsafe to OSTD, about 14% of its codebase, with a published soundness analysis [Asterinas-ATC25][OSTD-sound] |
| Trusted (ring-0) code size | **47,805** lines: core 29,716 + kernel 8,510 + drivers 8,382 + boot 892 + hal 305 | `find core/src drivers/src kernel/src hal/src boot -type f \( -name '*.rs' -o -name '*.S' -o -name '*.s' -o -name '*.asm' -o -name '*.ld' \) -print0 \| xargs -0 cat \| wc -l` | Asterinas puts its unsafe, trusted part in OSTD, about 14% of its codebase [Asterinas-ATC25] |
| API surface | **12** syscalls (0–11; 0–8 frozen); `IreeHalCmd` v1 = **96** bytes; `AccelDevice` = **23** methods, of which **5** have no default (`probe`, `submit`, `poll`, `map`, `name`) | `grep -cE '^pub const SYS_[A-Z_]+: u64 = [0-9]+;' core/src/sysnr.rs`, `grep 'IREE_HAL_CMD_SIZE: usize' drivers/src/ireecp.rs`, `awk '/^pub trait AccelDevice/{f=1} f&&/^}/{f=0} f' hal/src/lib.rs \| grep -cE '^    fn '` | Asterinas implements 210+ Linux syscalls [Asterinas-ATC25]; Linux IOMMUFD defines about 20 ioctls [IOMMUFD-h] |
| Determinism | Red-team stdout byte-identical across 2 runs (sha256 `e8b1c817…5a02aa`, full value in the `verify` line below); diligence demo (52 lines) byte-identical; two-tenant demo log byte-identical. Gated under `cargo test` by `examples/{red-team,diligence-demo}/tests/determinism.rs` | `for i in 1 2; do cargo run -q --locked -p aether-redteam \| sha256sum; done` | No comparable published number found in the cited projects |
| Real workloads end to end | **0 real models.** `two-tenant-infer` runs a tiny 5-layer i32 MLP (Wave/Relu/MatMul/Add/Max) on SoftNPU research ops for 2 tenants; outputs match a CPU reference; attacker tenant: `attacks=9 refused=9` | `cargo run --locked -q -p aether-redteam --example two_tenant_infer \| tail -n 1` | IREE, TVM and tt-metal run real models on real targets [IREE-deploy][TVM][tt-metal] |
| Hardware isolation | **0.** Soft SMMU, software NoI and software green contexts; a real device can DMA past them | `grep -n 'Not enforced in hardware' docs/DILIGENCE.md` | MIG partitions in hardware [MIG]; MI300X CPX/NPS [MI300X]; XDNA column partitions [XDNA] |

## Behind (stated plainly)

- **No proofs.** Aether has tests, a seeded fuzz, a noninterference harness and
  18 bounded Kani harnesses that CI does not run. seL4 has machine-checked
  proofs [seL4-VC]; Asterinas has a soundness analysis for its unsafe core
  [OSTD-sound]. Aether has neither.
- **No silicon.** 0 physical boards. All 3 ISAs boot only in QEMU. SMP is a
  2-hart smoke test.
- **No real models.** The only end-to-end workload is a tiny i32 MLP on
  SoftNPU research ops. IREE, TVM and tt-metal run real models [IREE-deploy][TVM][tt-metal].
- **Software SMMU / NoI.** Stream IDs, IOVA tables, NoI admission and green
  contexts are software models. MIG, MI300X and XDNA isolate in hardware
  [MIG][MI300X][XDNA]. #161 (raw unmap not tenant-checked) is still open.
- **No customer contact.** No discovery interview has been recorded. The
  plan (what the harness offers an NPU runtime team, questions, yes and kill
  criteria) is [business/DISCOVERY.md](business/DISCOVERY.md).
- **No UCIe transport** and no measured interference on real memory.
- **No performance numbers.** This file makes no throughput, latency or
  silicon-schedule claims.
- **8 error variants** are allowlisted rather than exercised by a test: kernel
  errno, kill-listed parsers and one dead ABI wrapper. #230 named
  `BootFsError::NoRoom` and dropped boot tooling from the list. The needle list
  is closed; new coverage needs new refuse paths, not more tests.
  [business/ALLOWLIST.md](business/ALLOWLIST.md) explains each of the 8.

## Gated values

Checked by `core/tests/scoreboard.rs` (`cargo test -p aether-core --test scoreboard -- --nocapture`):

```text
scoreboard unsafe_total=248 unsafe_files=34
scoreboard syscalls=12 iree_hal_cmd_bytes=96
scoreboard accel_device_methods=23 accel_device_required=5
scoreboard kani_harnesses=18 ci_isas=3
```

Run-time values, re-measured by `make verify` (or `python3 scripts/verify.py`
without GNU make), which re-runs host tests, the diligence and red-team clips
and the error-coverage matrix, prints the red-team stdout sha256, and fails if
any value below disagrees with the run:

```text
verify refused=99 variants=153 named=145 allowlisted=8 redteam_sha256=e8b1c817ba5204114e4d4cf78c9cded655f810b9bf596f7b2e9746755c5a02aa
```

## Sources

- [seL4-VC] https://docs.sel4.systems/projects/sel4/verified-configurations.html
- [Hubris] https://hubris.oxide.computer/reference/ · https://github.com/oxidecomputer/hubris
- [Asterinas-ATC25] https://www.usenix.org/system/files/atc25-peng-yuke.pdf
- [OSTD-sound] https://asterinas.github.io/book/ostd/soundness/index.html · https://github.com/asterinas/vostd
- [IOMMUFD-h] https://github.com/torvalds/linux/blob/master/include/uapi/linux/iommufd.h
- [MIG] https://docs.nvidia.com/datacenter/tesla/mig-user-guide/latest/introduction.html
- [MPS] https://docs.nvidia.com/deploy/mps/latest/when-to-use-mps.html
- [MI300X] https://instinct.docs.amd.com/projects/amdgpu-docs/en/latest/gpu-partitioning/mi300x/overview.html
- [XDNA] https://kernel.org/doc/html/latest/accel/amdxdna/amdnpu.html
- [IREE-deploy] https://iree.dev/guides/deployment-configurations/
- [TVM] https://tvm.apache.org/docs/arch/relax_vm.html
- [tt-metal] https://github.com/tenstorrent/tt-metal
