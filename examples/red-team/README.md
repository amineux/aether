# Red-team diligence clip

Host stdout a buyer can grep. Reuses `run_blast_demo`, `run_blast_hops_demo`,
`run_blast_nodes_demo`, `run_bank_color_demo`, `run_uncolored_compute_demo`,
`run_qos_credits_demo`, `run_outside_slice_demo`, `run_silent_remote_demo`,
`run_firewall_demo`, `run_softsfi_demo`, `run_softnoi_demo`, and `run_sva_demo`.
Not a new isolator, not a QEMU guest, not a slide. Blast-nodes needle:
`[redteam] attack=blast-nodes result=refused` (`admit_nodes` → `BlastRadius`;
hops stays `attack=blast-hops`). Bank-color needle:
`[redteam] attack=bank-color result=refused` (`admit_wave` →
`ColorError::ForeignBank`; Exchange still OK). Uncolored-compute needle:
`[redteam] attack=uncolored-compute result=refused` (`admit_wave(..., color=None)` →
`ColorError::Uncolored`; Exchange with color still OK; not ForeignBank /
bank-color). QoS credits needle:
`[redteam] attack=qos-credits result=refused` (`Timeline::submit` → `CreditExhausted`).
Outside-slice needle: `[redteam] attack=outside-slice result=refused`
(`admit_chiplet` → `OutsideSlice`; not hops / qos / CrossCut / bank-color).
Silent-remote needle: `[redteam] attack=silent-remote result=refused`
(`map_place` / `map_fabric` → `SilentRemoteLoad`; `MEM_FULL` never implies
`UNIFIED`; not CXL productization / BAR0 / SoftNPU).
SoftSFI tensor needle: `[softsfi] tensor=refused` (`SoftOp::Tensor` → `Unmodeled`;
heap line stays separate).

From the repo root:

```bash
make red-team
```

or `cargo run -p aether-redteam`. CI greps `[redteam] attack=… result=refused`
plus `[redteam] fabric-class admit/refuse`, `[redteam] ATOMIC_ADD accept/reject`,
`[softsfi] tensor=refused`, `[softsfi] heap=refused`, and the “what this is not” closer. See
[docs/DILIGENCE.md](../../docs/DILIGENCE.md).
