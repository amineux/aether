# Red-team diligence clip

Host stdout a buyer can grep. Reuses `run_blast_demo`, `run_firewall_demo`,
`run_softsfi_demo`, `run_softnoi_demo`, and `run_sva_demo`. Not a new
isolator, not a QEMU guest, not a slide.

From the repo root:

```bash
make red-team
```

or `cargo run -p aether-redteam`. CI greps `[redteam] attack=… result=refused`
plus the “what this is not” closer. See [docs/DILIGENCE.md](../../docs/DILIGENCE.md).
