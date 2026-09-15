# Red-team diligence clip

Host stdout a buyer can grep. Reuses `run_blast_demo`, `run_blast_hops_demo`,
`run_bank_color_demo`, `run_qos_credits_demo`, `run_outside_slice_demo`,
`run_firewall_demo`, `run_softsfi_demo`, `run_softnoi_demo`, and `run_sva_demo`.
Not a new isolator, not a QEMU guest, not a slide. Bank-color needle:
`[redteam] attack=bank-color result=refused` (`admit_wave` →
`ColorError::ForeignBank`; Exchange still OK). QoS credits needle:
`[redteam] attack=qos-credits result=refused` (`Timeline::submit` → `CreditExhausted`).
Outside-slice needle: `[redteam] attack=outside-slice result=refused`
(`admit_chiplet` → `OutsideSlice`; not hops / qos / CrossCut / bank-color).
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
