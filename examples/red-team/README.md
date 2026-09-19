# Red-team diligence clip

Host stdout a buyer can grep. Reuses `run_blast_demo`, `run_blast_hops_demo`,
`run_blast_nodes_demo`, `run_bank_color_demo`, `run_uncolored_compute_demo`,
`run_foreign_tenant_color_demo`, `run_qos_credits_demo`, `run_outside_slice_demo`, `run_typed_window_sid_demo`,
`run_silent_remote_demo`, `run_hbm_bw_demo`, `run_xqueue_sid_override_demo`,
`run_firewall_demo`, `run_softsfi_demo`, `run_softnoi_demo`, and `run_sva_demo`.
Not a new isolator, not a QEMU guest, not a slide. Blast-nodes needle:
`[redteam] attack=blast-nodes result=refused` (`admit_nodes` → `BlastRadius`;
hops stays `attack=blast-hops`). Bank-color needle:
`[redteam] attack=bank-color result=refused` (`admit_wave` →
`ColorError::ForeignBank`; Exchange still OK). Uncolored-compute needle:
`[redteam] attack=uncolored-compute result=refused` (`admit_wave(..., color=None)` →
`ColorError::Uncolored`; Exchange with color still OK; not ForeignBank /
bank-color). Foreign-tenant-color needle:
`[redteam] attack=foreign-tenant-color result=refused` (`admit_wave` →
`ColorError::ForeignTenant`; Exchange still OK; not ForeignBank / bank-color
or Uncolored / uncolored-compute). QoS credits needle:
`[redteam] attack=qos-credits result=refused` (`Timeline::submit` → `CreditExhausted`).
Outside-slice needle: `[redteam] attack=outside-slice result=refused`
(`admit_chiplet` → `OutsideSlice`; not hops / qos / CrossCut / bank-color).
Silent-remote needle: `[redteam] attack=silent-remote result=refused`
(`map_place` / `map_fabric` → `SilentRemoteLoad`; `MEM_FULL` never implies
`UNIFIED`; not CXL productization / BAR0 / SoftNPU).
Soft HBM BW needle: `[redteam] attack=hbm-bw result=refused`
(`SoftHbmBwMeter::charge` → `QosExceeded` vs `QosBudget.bw_mbps` on HBM
`TypedWindow`; software meter only).
XQueue SID override needle: `[redteam] attack=xqueue-sid-override result=refused`
(`stamp_queue_sid` second SID → `HalError::Busy` on pending Soft-CP XQueue;
not BAR0 / SoftNPU).
SoftSFI tensor needle: `[softsfi] tensor=refused` (`SoftOp::Tensor` → `Unmodeled`;
heap line stays separate).
SoftSFI unknown needle: `[softsfi] unknown=refused` (bad opcode / illegal width →
`Unmodeled`; tensor/heap lines stay separate; not AddImm deepen).

Typed-window-sid needle: `[redteam] attack=typed-window-sid result=refused`
(`map_window_sid` → `WrongStream`; exploration TypedWindow stub, not CXL.mem
silicon / BAR0; CrossTenant on foreign pin).

From the repo root:

```bash
make red-team
```

or `cargo run -p aether-redteam`. CI greps `[redteam] attack=… result=refused`
plus `[redteam] fabric-class admit/refuse`, `[redteam] ATOMIC_ADD accept/reject`,
`[softsfi] tensor=refused`, `[softsfi] heap=refused`, `[softsfi] unknown=refused`, and the “what this is not” closer. See
[docs/DILIGENCE.md](../../docs/DILIGENCE.md).
